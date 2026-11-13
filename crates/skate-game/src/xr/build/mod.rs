//! Build mode: put skate pieces on the real floor, tables and furniture.
//!
//! Hold the left menu button for a second (without chording it) to enter or
//! leave. While building the pad is neutral, and the legend beside each
//! controller ([`legend`]) says what its sticks and buttons do at that moment.
//!
//! There are two things to be doing, and the controls never mean one of them
//! by where the pointer happens to be:
//!
//! - **Placing** (nothing selected): the sticks change the piece about to be
//!   dropped, shown as a ghost where the right controller points. Left stick
//!   left/right chooses its kind (the wrist palette), up/down what it is made
//!   of; right stick turns it and scales it; A or the trigger drops it. The
//!   right stick's click holds it in the hand instead of on a surface (free
//!   placement), and again puts it back.
//! - **Editing** (a piece selected: point at it and press the right grip):
//!   the same sticks change that piece, which is outlined. Hold the grip to
//!   move it, B deletes it, the right stick's click copies it, A or the
//!   trigger is done.
//!
//! Holding the left grip brings up the second set for either: left stick
//! up/down its length or height and left/right its width (arrows on the
//! piece show which it has), right stick its colour, A hides it.
//!
//! Left trigger: undo. Left stick click: a starter park. Y: rescan the room.
//! Hold X: clear every piece.
//!
//! Pieces line up with table edges and with each other ([`snap`]). They live
//! in tracking (STAGE) space, so they stay on the same real table, and are
//! saved between launches ([`layout`]). Their collision and grind rails are
//! built by `xr::real_world`.
mod layout;
mod legend;
pub(crate) mod pieces;
mod snap;

use self::pieces::{Finish, MAX_SIZE, MIN_SIZE, Piece, PieceGeometry, PieceKind, Tint};
use super::actions::{Hand, LatestTouch, TouchRead};
use super::planes::{SceneCapture, SceneShapes, Shape};
use super::rig::{Rig, heading};
use super::link::{ArWorld, TeleportSkater};
use super::spawn::ray_triangle;
use crate::input::ExternalPad;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_mod_openxr::openxr_session_running;
use serde::{Deserialize, Serialize};
use crate::xr::touch::XboxState;

const HOLD_TO_TOGGLE_SECONDS: f32 = 1.0;
/// Left stick deflection that makes a menu hold a D-pad chord instead.
const CHORD_STICK: f32 = 0.3;
const PRESS: f32 = 0.7;
const RELEASE: f32 = 0.4;
const FLICK: f32 = 0.6;
/// Turn per second at full stick deflection, radians.
const TURN_RATE: f32 = 2.2;
/// Size change per second at full stick deflection (multiplicative).
const RESIZE_RATE: f32 = 0.8;
/// Piece sizes are kept on this grid, so each kind has a bounded set of meshes.
const SIZE_STEP: f32 = 0.05;
/// World scale change per second at full stick deflection (multiplicative).
const SCALE_RATE: f32 = 1.0;
/// Park scale: every placed piece is this many times its authored size.
/// Wide, because "skater size" changes it the other way to keep every piece
/// its real size while the skater grows or shrinks (world scale 1..60).
pub(crate) const MIN_PARK_SCALE: f32 = 0.05;
pub(crate) const MAX_PARK_SCALE: f32 = 60.0;
/// A grip counts as held for the park-scale chord past this pull.
const PARK_CHORD_GRIP: f32 = 0.6;
/// One piece's own uniform scale, on top of the park scale.
pub(crate) const MIN_PIECE_SCALE: f32 = 0.25;
pub(crate) const MAX_PIECE_SCALE: f32 = 4.0;
/// Game metres per real metre: 1 is life size, 60 is a tiny toy.
pub(crate) const MIN_SCALE: f32 = 1.0;
pub(crate) const MAX_SCALE: f32 = 60.0;
const STICK_DEADZONE: f32 = 0.25;
/// The right grip held this long on the selected piece picks it up.
const MOVE_HOLD_SECONDS: f32 = 0.25;
/// Free placement holds the piece this far in front of the controller, m.
const FREE_REACH_M: f32 = 0.3;
/// Colour steps per second at full stick deflection.
const TINT_RATE: f32 = 7.0;
const UNDO_DEPTH: usize = 40;
const CLEAR_HOLD_SECONDS: f32 = 1.0;
/// Game-world units (metres at skater scale).
const MAX_RANGE: f32 = 2000.0;
const MISS_LENGTH: f32 = 40.0;
/// Pieces go on ground-like faces, not walls.
const MIN_UP: f32 = 0.6;
/// Wrist palette: real-world size of the largest icon, and spacing, metres.
const ICON_SIZE_M: f32 = 0.024;
const ICON_SPACING_M: f32 = 0.03;
/// The palette shows the chosen piece and this many either side of it, the
/// row sliding under it as the choice moves (and round, past its ends).
const PALETTE_REACH: f32 = 3.0;
/// How quickly the row slides to a new choice, per second.
const PALETTE_SLIDE: f32 = 12.0;
/// The chosen piece's finishes stack this far apart above it, metres.
const FINISH_SPACING_M: f32 = 0.036;
/// The most finishes a kind has (`PieceKind::finishes`).
const MAX_FINISHES: usize = 4;

#[derive(Resource, Default)]
pub(crate) struct BuildMode {
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Placed {
    pub id: u64,
    pub piece: Piece,
    /// Tracking-space (STAGE) position of the piece's base centre.
    pub position: [f32; 3],
    /// Turn about +Y in tracking space.
    pub yaw: f32,
    /// This piece's uniform scale ([`MIN_PIECE_SCALE`]..[`MAX_PIECE_SCALE`]);
    /// absent in layouts from before 0.0.3.
    #[serde(default = "unit_scale")]
    pub scale: f32,
    /// Not drawn (outside build mode), still skated.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// What it is made of; absent (layouts from before finishes, and pieces
    /// left as they come): its kind's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<Finish>,
    /// Its colour, when not its finish's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<Tint>,
}

fn unit_scale() -> f32 {
    1.0
}

impl Placed {
    /// Pose of the piece in the game world; `park_scale` and its own scale
    /// grow it about its base centre, which stays on the same real spot.
    pub(crate) fn world_transform(&self, root: &Transform, park_scale: f32) -> Transform {
        Transform {
            translation: root.transform_point(Vec3::from_array(self.position)),
            rotation: root.rotation * Quat::from_rotation_y(self.yaw),
            scale: Vec3::splat(park_scale * self.scale),
        }
    }

    pub(crate) fn finish(&self) -> Finish {
        self.piece.kind.finish(self.finish)
    }

    /// Seen from above in tracking space, for [`snap::join`]: `metres` is
    /// real metres per metre of the piece (the park's scale over the rig's).
    fn footprint(&self, metres: f32) -> snap::Footprint {
        let (min, max) = self.piece.geometry().footprint();
        let k = metres * self.scale;
        snap::Footprint { position: Vec3::from_array(self.position), yaw: self.yaw, min: min * k, max: max * k }
    }

    /// Where other pieces join it, in tracking space ([`snap::meet`]).
    fn edges(&self, metres: f32) -> Vec<snap::Edge> {
        let k = metres * self.scale;
        let (at, turn) = (Vec3::from_array(self.position), Quat::from_rotation_y(self.yaw));
        self.piece.joins().iter().map(|j| snap::Edge { at: at + turn * (j.at * k), out: turn * j.out, width: j.width * k }).collect()
    }

    pub(crate) fn name(&self) -> String {
        format!("{:?} {}", self.piece.kind, self.id)
    }
}

/// Everything the player has placed. `revision` changes on every committed
/// edit (collision rebuild and save key off it); `dragging` is the piece
/// following the controller, `resizing` the one being resized and
/// `rescaling` the world scale being changed: collision waits until they end.
#[derive(Resource)]
pub(crate) struct PlacedPieces {
    pub items: Vec<Placed>,
    /// Size of every piece relative to its authored size (the skater keeps
    /// the world scale), [`MIN_PARK_SCALE`]..[`MAX_PARK_SCALE`].
    pub park_scale: f32,
    pub revision: u64,
    pub dragging: Option<u64>,
    pub resizing: Option<u64>,
    pub rescaling: bool,
    /// The piece last pointed at or placed in build mode (the menu edits it).
    pub selected: Option<u64>,
    /// The piece the build ray is on right now.
    pub pointed: Option<u64>,
    /// Draw invisible pieces as ghosts outside build mode too.
    pub show_hidden: bool,
    /// Earlier layouts, newest last, for undo.
    history: Vec<Vec<Placed>>,
}

impl Default for PlacedPieces {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            park_scale: 1.0,
            revision: 0,
            dragging: None,
            resizing: None,
            rescaling: false,
            selected: None,
            pointed: None,
            show_hidden: false,
            history: Vec::new(),
        }
    }
}

impl PlacedPieces {
    pub(crate) fn busy(&self) -> bool {
        self.dragging.is_some() || self.resizing.is_some() || self.rescaling
    }

    /// Remember the current layout before an edit.
    pub(crate) fn checkpoint(&mut self) {
        if self.history.len() == UNDO_DEPTH {
            self.history.remove(0);
        }
        self.history.push(self.items.clone());
    }

    pub(crate) fn undo(&mut self) -> bool {
        let Some(items) = self.history.pop() else {
            return false;
        };
        self.items = items;
        self.dragging = None;
        self.resizing = None;
        self.revision += 1;
        true
    }

    fn add(&mut self, piece: Piece, position: Vec3, yaw: f32, scale: f32, finish: Option<Finish>, tint: Option<Tint>) -> u64 {
        self.checkpoint();
        let id = self.insert(piece, position, yaw, scale);
        if let Some(item) = self.items.last_mut() {
            item.finish = finish;
            item.tint = tint;
        }
        id
    }

    fn insert(&mut self, piece: Piece, position: Vec3, yaw: f32, scale: f32) -> u64 {
        let id = self.items.iter().map(|p| p.id).max().map_or(1, |m| m + 1);
        self.items.push(Placed {
            id,
            piece,
            position: position.to_array(),
            yaw,
            scale,
            hidden: false,
            finish: None,
            tint: None,
        });
        self.revision += 1;
        id
    }

    pub(crate) fn get_mut(&mut self, id: u64) -> Option<&mut Placed> {
        self.items.iter_mut().find(|p| p.id == id)
    }

    /// Every piece removed (undoable).
    pub(crate) fn clear(&mut self) {
        self.checkpoint();
        self.items.clear();
        self.dragging = None;
        self.resizing = None;
        self.selected = None;
        self.revision += 1;
    }

    pub(crate) fn remove(&mut self, id: u64) -> Option<Placed> {
        let at = self.items.iter().position(|p| p.id == id)?;
        self.checkpoint();
        self.revision += 1;
        Some(self.items.remove(at))
    }
}

#[derive(Resource)]
struct BuildState {
    /// The kind of the next piece (an index into `PieceKind::ALL`).
    selected: usize,
    /// The next piece of each kind: its size, width, finish (an index into
    /// its kind's) and colour.
    sizes: [f32; PieceKind::ALL.len()],
    widths: [f32; PieceKind::ALL.len()],
    finishes: [usize; PieceKind::ALL.len()],
    tints: [Option<Tint>; PieceKind::ALL.len()],
    yaw: f32,
    /// `yaw`, lined up with a table edge or a neighbour.
    placed_yaw: f32,
    /// World-space ray (origin, end, lands on a ground-like face).
    ray: Option<(Vec3, Vec3, bool)>,
    /// Where a piece would go (tracking position, world point).
    ghost: Option<(Vec3, Vec3)>,
    /// Free placement: pieces go where the right hand holds them, not on
    /// the surface it points at.
    free: bool,
    /// The piece in hand is joined to another's edge: how far that leaves
    /// its base above the surface (`Piece::lift`), and the edge (its middle,
    /// the direction along it and its length, tracking space).
    lift: f32,
    join: Option<(Vec3, Vec3, f32)>,
    /// The player has set the next piece's dimensions: a join leaves them.
    own_dimensions: bool,
    menu_hold: Option<f32>,
    toggled: bool,
    clear_hold: f32,
    /// Where the skater's deck was in the room when a rescale began.
    rescale_from: Option<(Vec3, Vec3)>,
    /// The rescale in progress is the park's, not the world's.
    park_rescale: bool,
    /// The park was resized: once collision is rebuilt, drop the skater onto
    /// whatever is now under this real (tracking) spot.
    drop_after_rebuild: Option<(Vec3, Vec3, u64)>,
    /// Unsnapped size (or scale, or width) of the selected piece while a
    /// stick changes it.
    resize_size: f32,
    /// What the edit in progress on the selected piece changes.
    edit: Edit,
    /// Uniform scale of the piece to drop.
    scale: f32,
    /// How long the right grip has been held since it was pressed.
    grip_hold: Option<f32>,
    /// Parts of a colour step the stick has run up (hue, lightness).
    tint_owed: Vec2,
    previous: Buttons,
}

impl Default for BuildState {
    fn default() -> Self {
        Self {
            selected: 0,
            sizes: [1.0; PieceKind::ALL.len()],
            widths: [1.0; PieceKind::ALL.len()],
            finishes: [0; PieceKind::ALL.len()],
            tints: [None; PieceKind::ALL.len()],
            yaw: 0.0,
            placed_yaw: 0.0,
            ray: None,
            ghost: None,
            free: false,
            lift: 0.0,
            join: None,
            own_dimensions: false,
            menu_hold: None,
            toggled: false,
            clear_hold: 0.0,
            rescale_from: None,
            park_rescale: false,
            drop_after_rebuild: None,
            resize_size: 1.0,
            edit: Edit::Turn,
            scale: 1.0,
            grip_hold: None,
            tint_owed: Vec2::ZERO,
            previous: Buttons::default(),
        }
    }
}

/// What a held stick changes.
#[derive(Clone, Copy, PartialEq)]
enum Edit {
    Turn,
    Scale,
    Stretch,
    Widen,
    Tint,
}

impl BuildState {
    /// The finish and colour of the next piece to drop.
    fn finish(&self) -> Finish {
        let all = PieceKind::ALL[self.selected].finishes();
        all[self.finishes[self.selected].min(all.len() - 1)]
    }

    fn tint(&self) -> Option<Tint> {
        self.tints[self.selected]
    }

    fn piece(&self) -> Piece {
        Piece {
            kind: PieceKind::ALL[self.selected],
            size: snap_size(self.sizes[self.selected]),
            width: snap_size(self.widths[self.selected]),
            lift: self.lift,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Buttons {
    trigger: bool,
    grip: bool,
    a: bool,
    b: bool,
    y: bool,
    undo: bool,
    park: bool,
    click: bool,
    left_flick: i8,
    finish_flick: i8,
}

pub(crate) struct BuildPlugin;

impl Plugin for BuildPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "occluder.wgsl");
        app.add_plugins(MaterialPlugin::<Occluder>::default())
            .init_resource::<BuildMode>()
            .init_resource::<PlacedPieces>()
            .init_resource::<BuildState>()
            .init_resource::<PieceAssets>()
            .add_systems(Startup, (layout::load, spawn_visuals))
            .add_systems(
                PreUpdate,
                (toggle_build, build_controls)
                    .chain()
                    .after(TouchRead)
                    .before(crate::input::poll_controllers)
                    .run_if(openxr_session_running),
            )
            .add_plugins(legend::LegendPlugin)
            .add_systems(Update, (sync_pieces, show_build, show_edit, describe, layout::save, drop_after_park_scale));
    }
}

fn toggle_build(
    touch: Res<LatestTouch>,
    time: Res<Time<Real>>,
    mut mode: ResMut<BuildMode>,
    mut state: ResMut<BuildState>,
    mut placed: ResMut<PlacedPieces>,
) {
    let touch = &touch.0;
    // Menu + grips is the SK8 menu chord, not a build-mode hold.
    let chording = touch.x
        || touch.left_grip > 0.5
        || touch.right_grip > 0.5
        || touch.left_stick[0].abs() > CHORD_STICK
        || touch.left_stick[1].abs() > CHORD_STICK;
    state.menu_hold = match (state.menu_hold, touch.menu && !chording) {
        (_, false) => {
            state.toggled = false;
            None
        }
        (None, true) => Some(0.0),
        (Some(t), true) => Some(t + time.delta_secs()),
    };
    if state.menu_hold.is_some_and(|t| t >= HOLD_TO_TOGGLE_SECONDS) && !state.toggled {
        state.toggled = true;
        mode.active = !mode.active;
        if !mode.active {
            placed.selected = None;
            if placed.dragging.take().is_some() | placed.resizing.take().is_some() {
                placed.revision += 1;
            }
        }
        info!("BUILD mode: {}", mode.active);
    }
}

#[allow(clippy::too_many_arguments)]
fn build_controls(
    touch: Res<LatestTouch>,
    time: Res<Time<Real>>,
    mode: Res<BuildMode>,
    rig: Res<Rig>,
    hands: Query<(&Hand, &Transform)>,
    world: Res<ArWorld>,
    mut teleport: MessageWriter<TeleportSkater>,
    shapes: Res<SceneShapes>,
    mut pad: ResMut<ExternalPad>,
    mut state: ResMut<BuildState>,
    mut placed: ResMut<PlacedPieces>,
    mut capture: ResMut<SceneCapture>,
) {
    if !mode.active {
        state.ray = None;
        state.ghost = None;
        placed.pointed = None;
        if placed.rescaling {
            finish_rescale(&rig, &mut teleport, &mut state, &mut placed);
        }
        return;
    }
    pad.set(XboxState {
        buttons: 0,
        triggers: [0, 0],
        left: [0, 0],
        right: [0, 0],
    });
    let touch = &touch.0;
    let dt = time.delta_secs();
    let flick = |x: f32, was: i8| match x {
        x if x > FLICK => 1,
        x if x < -FLICK => -1,
        x if x.abs() < FLICK * 0.5 => 0,
        _ => was,
    };
    // The left grip brings up the second set of actions.
    let more = touch.left_grip > PRESS;
    let previous = state.previous;
    let now = Buttons {
        trigger: if previous.trigger { touch.right_trigger > RELEASE } else { touch.right_trigger > PRESS },
        grip: if previous.grip { touch.right_grip > RELEASE } else { touch.right_grip > PRESS },
        a: touch.a,
        b: touch.b,
        y: touch.y,
        undo: if previous.undo { touch.left_trigger > RELEASE } else { touch.left_trigger > PRESS },
        park: touch.left_stick_click,
        click: touch.right_stick_click,
        left_flick: if more { 0 } else { flick(touch.left_stick[0], previous.left_flick) },
        finish_flick: if more { 0 } else { flick(touch.left_stick[1], previous.finish_flick) },
    };
    state.previous = now;
    let pressed = |now: bool, was: bool| now && !was;
    let finish_edit = |placed: &mut PlacedPieces| {
        if let Some(id) = placed.resizing.take() {
            placed.revision += 1;
            info!("BUILD changed piece {id}");
        }
    };

    // Choosing another kind is also being done with the selected piece.
    if now.left_flick != 0 && now.left_flick != previous.left_flick {
        let n = PieceKind::ALL.len() as i32;
        state.selected = (state.selected as i32 + now.left_flick as i32).rem_euclid(n) as usize;
        state.own_dimensions = false;
        finish_edit(&mut placed);
        placed.selected = None;
        info!("BUILD selected {:?}", PieceKind::ALL[state.selected]);
    }
    if pressed(now.y, previous.y) {
        capture.requested = true;
        info!("BUILD rescan requested");
    }
    if pressed(now.undo, previous.undo) {
        if placed.undo() {
            info!("BUILD undo ({} pieces)", placed.items.len());
        }
        return;
    }
    if touch.x {
        state.clear_hold += dt;
        if state.clear_hold >= CLEAR_HOLD_SECONDS && !placed.items.is_empty() {
            placed.clear();
            info!("BUILD cleared");
        }
    } else {
        state.clear_hold = 0.0;
    }
    if placed.selected.is_some_and(|id| !placed.items.iter().any(|p| p.id == id)) {
        placed.selected = None;
    }

    // (the skater's size and the whole park's are the menu's: Build a park; the sticks here only
    // ever change one piece)
    if placed.rescaling {
        finish_rescale(&rig, &mut teleport, &mut state, &mut placed);
    }

    let Some(root) = rig.root_transform() else {
        return;
    };
    let Some((_, controller)) = hands.iter().find(|(hand, _)| matches!(hand, Hand::Right)) else {
        return;
    };
    let origin = root.transform_point(controller.translation);
    let direction = (root.rotation * (controller.rotation * Vec3::NEG_Z)).normalize();
    if state.ghost.is_none() {
        // Face pieces away from the player until they are turned.
        state.yaw = heading(controller.rotation);
    }
    let hit = cast(&world, &placed, &root, origin, direction);
    let to_tracking = root.compute_affine().inverse();
    placed.pointed = hit.and_then(|h| h.piece);
    let valid = hit.filter(|h| h.normal.y >= MIN_UP);
    // Where a piece goes: on the surface pointed at, or (free placement) in the hand.
    state.ghost = if state.free {
        let held = controller.transform_point(Vec3::NEG_Z * FREE_REACH_M);
        Some((held, root.transform_point(held)))
    } else {
        valid.map(|h| (to_tracking.transform_point3(h.point), h.point))
    };
    state.ray = match hit {
        // (in the hand there is nothing to point at but a piece to select)
        _ if state.free && placed.pointed.is_none() => None,
        Some(h) => Some((origin, h.point, valid.is_some() || state.free)),
        None => Some((origin, origin + direction * MISS_LENGTH, false)),
    };

    // What the sticks change: the selected piece, or else the next one. Right stick: turn
    // (left/right) and scale as a whole (up/down). With the left grip: left stick its length or
    // height (up/down) and its width (left/right), right stick its colour.
    let [rx, ry] = touch.right_stick;
    let [lx, ly] = touch.left_stick;
    let pushed = |x: f32, y: f32| x.abs().max(y.abs()) > STICK_DEADZONE;
    let edit = match (more, pushed(lx, ly), pushed(rx, ry)) {
        (true, true, _) => Some(if ly.abs() > lx.abs() { Edit::Stretch } else { Edit::Widen }),
        (true, false, true) => Some(Edit::Tint),
        (false, _, true) => Some(if ry.abs() > rx.abs() { Edit::Scale } else { Edit::Turn }),
        _ => None,
    };
    // The turn eases in from the deadzone, so a light push is a fine adjustment.
    let push = ((rx.abs() - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)).clamp(0.0, 1.0);
    let turn = -rx.signum() * push * push * TURN_RATE * dt;
    let factor = match edit {
        Some(Edit::Scale) => ry,
        Some(Edit::Stretch) => ly,
        Some(Edit::Widen) => lx,
        _ => 0.0,
    };
    let factor = (factor * RESIZE_RATE * dt).exp();
    // Whole colour steps the stick has run up: hue left/right, lighter up.
    let steps = if edit == Some(Edit::Tint) {
        state.tint_owed += if rx.abs() > ry.abs() { Vec2::new(rx, 0.0) } else { Vec2::new(0.0, ry) } * TINT_RATE * dt;
        let whole = state.tint_owed.trunc();
        state.tint_owed -= whole;
        (whole.x as i32, whole.y as i32)
    } else {
        state.tint_owed = Vec2::ZERO;
        (0, 0)
    };
    let flicked = (now.finish_flick != 0 && now.finish_flick != previous.finish_flick).then_some(now.finish_flick as i32);
    let reset_tint = more && pressed(now.click, previous.click);
    match placed.selected.filter(|_| placed.dragging.is_none()) {
        Some(id) => {
            let kind = placed.items.iter().find(|p| p.id == id).map(|p| p.piece.kind);
            let edit = edit.filter(|&e| e != Edit::Widen || kind.is_some_and(|k| k.width_axis().is_some()));
            if let Some(edit) = edit {
                // Collision and the save wait until the stick is let go.
                let started = placed.resizing.is_none();
                if started {
                    placed.checkpoint();
                    placed.resizing = Some(id);
                }
                if started || state.edit != edit {
                    let item = placed.items.iter().find(|p| p.id == id);
                    state.resize_size = item.map_or(1.0, |p| match edit {
                        Edit::Scale => p.scale,
                        Edit::Widen => p.piece.width,
                        _ => p.piece.size,
                    });
                }
                state.edit = edit;
                let (lo, hi) = if edit == Edit::Scale { (MIN_PIECE_SCALE, MAX_PIECE_SCALE) } else { (MIN_SIZE, MAX_SIZE) };
                if !matches!(edit, Edit::Turn | Edit::Tint) {
                    state.resize_size = (state.resize_size * factor).clamp(lo, hi);
                }
                let value = state.resize_size;
                if let Some(item) = placed.get_mut(id) {
                    match edit {
                        Edit::Turn => item.yaw += turn,
                        Edit::Scale => {
                            // (its base stays on the surface it reaches down to)
                            let scale = snap_scale(value);
                            item.piece.lift *= item.scale / scale;
                            item.scale = scale;
                        }
                        Edit::Stretch => item.piece.size = snap_size(value),
                        Edit::Widen => item.piece.width = snap_size(value),
                        Edit::Tint if steps != (0, 0) => {
                            item.tint = Some(item.tint.unwrap_or(Tint::of(item.finish())).stepped(steps.0, steps.1));
                        }
                        Edit::Tint => {}
                    }
                }
            } else {
                finish_edit(&mut placed);
            }
            if flicked.is_some() || reset_tint {
                placed.checkpoint();
                if let Some(item) = placed.get_mut(id) {
                    if let Some(step) = flicked {
                        let all = item.piece.kind.finishes();
                        let at = all.iter().position(|&f| f == item.finish()).unwrap_or(0) as i32;
                        item.finish = Some(all[(at + step).rem_euclid(all.len() as i32) as usize]);
                    } else {
                        item.tint = None;
                    }
                    info!("BUILD piece {id} is now {:?} {:?}", item.finish(), item.tint);
                }
                placed.revision += 1;
            }
        }
        None => {
            finish_edit(&mut placed);
            let selected = state.selected;
            let kind = PieceKind::ALL[selected];
            match edit {
                Some(Edit::Turn) => state.yaw += turn,
                // (a piece being moved keeps its shape: only its turn follows the stick)
                _ if placed.dragging.is_some() => {}
                Some(Edit::Scale) => state.scale = (state.scale * factor).clamp(MIN_PIECE_SCALE, MAX_PIECE_SCALE),
                Some(Edit::Stretch) => {
                    state.sizes[selected] = (state.sizes[selected] * factor).clamp(MIN_SIZE, MAX_SIZE);
                    state.own_dimensions = true;
                }
                Some(Edit::Widen) if kind.width_axis().is_some() => {
                    state.widths[selected] = (state.widths[selected] * factor).clamp(MIN_SIZE, MAX_SIZE);
                    state.own_dimensions = true;
                }
                Some(Edit::Tint) if steps != (0, 0) => {
                    state.tints[selected] = Some(state.tints[selected].unwrap_or(Tint::of(state.finish())).stepped(steps.0, steps.1));
                }
                _ => {}
            }
            if let Some(step) = flicked {
                let n = kind.finishes().len() as i32;
                state.finishes[selected] = (state.finishes[selected] as i32 + step).rem_euclid(n) as usize;
                info!("BUILD next {kind:?} in {:?}", state.finish());
            }
            if reset_tint {
                state.tints[selected] = None;
            }
        }
    }

    state.placed_yaw = match state.ghost {
        Some((position, _)) => snap::snap_yaw(
            shapes.0.iter().filter_map(|s| match *s {
                Shape::Box { pose, half } => Some((pose.rotation, pose.translation, half)),
                Shape::Plane { .. } => None,
            }),
            position,
            state.yaw,
        ),
        None => state.yaw,
    };
    // Beside another piece of its kind, it sits flush against it; at an edge
    // of any other, edge to edge at that edge's height.
    state.lift = 0.0;
    state.join = None;
    if let Some((position, _)) = state.ghost.filter(|_| !state.free) {
        let metres = placed.park_scale / root.scale.x;
        let moving = match placed.dragging {
            Some(id) => placed.items.iter().find(|p| p.id == id).cloned(),
            None => Some(Placed {
                id: 0,
                piece: state.piece(),
                position: [0.0; 3],
                yaw: 0.0,
                scale: snap_scale(state.scale),
                hidden: false,
                finish: None,
                tint: None,
            }),
        };
        if let Some(mut moving) = moving {
            moving.piece.lift = 0.0;
            moving.position = position.to_array();
            moving.yaw = state.placed_yaw;
            let neighbours: Vec<_> = placed
                .items
                .iter()
                .filter(|p| p.piece.kind == moving.piece.kind && Some(p.id) != placed.dragging)
                .map(|p| p.footprint(metres))
                .collect();
            if let Some((place, yaw)) = snap::join(neighbours.iter().copied(), moving.footprint(metres)) {
                state.ghost = Some((place, root.transform_point(place)));
                state.placed_yaw = yaw;
            } else if let Some(place) =
                Some(()).filter(|_| moving.piece.kind == PieceKind::Rail).and_then(|_| snap::join_ends(neighbours, moving.footprint(metres)))
            {
                // Rails also meet end to end at any angle.
                state.ghost = Some((place, root.transform_point(place)));
            } else {
                let theirs: Vec<snap::Edge> = placed
                    .items
                    .iter()
                    .filter(|p| Some(p.id) != placed.dragging)
                    .flat_map(|p| p.edges(metres))
                    .collect();
                let reach = snap::JOIN_REACH * {
                    let f = moving.footprint(metres);
                    (f.max - f.min).max_element()
                };
                let k = metres * moving.scale;
                let own = |piece: Piece| -> Vec<snap::Edge> {
                    piece.joins().iter().map(|j| snap::Edge { at: j.at * k, out: j.out, width: j.width * k }).collect()
                };
                let mut met = snap::meet(theirs.iter().copied(), position, state.placed_yaw, &own(moving.piece), reach);
                // The next piece takes the width of the edge it joins, unless the player set its own.
                if let (Some(found), None, false) = (met, placed.dragging, state.own_dimensions) {
                    if let Some((dimension, per)) = moving.piece.joins()[found.own].follows {
                        let fitted = found.edge.width / (per * k);
                        if (MIN_SIZE..=MAX_SIZE).contains(&fitted) {
                            let fitted = snap_size(fitted);
                            let selected = state.selected;
                            match dimension {
                                pieces::Dimension::Size => {
                                    state.sizes[selected] = fitted;
                                    moving.piece.size = fitted;
                                }
                                pieces::Dimension::Width => {
                                    state.widths[selected] = fitted;
                                    moving.piece.width = fitted;
                                }
                            }
                            met = snap::meet(theirs.iter().copied(), position, state.placed_yaw, &own(moving.piece), reach).or(met);
                        }
                    }
                }
                // How far above (or into) what is under it that leaves its base.
                let joined = met.and_then(|met| {
                    let above = root.transform_point(met.place.with_y(met.edge.at.y - 0.01));
                    let under = cast(&world, &placed, &root, above, root.rotation * Vec3::NEG_Y)?;
                    let lift = (met.place.y - to_tracking.transform_point3(under.point).y) / k;
                    let lift = if lift.abs() < 0.02 { 0.0 } else { lift };
                    (lift >= moving.piece.least_lift()).then_some((met, lift))
                });
                if let Some((met, lift)) = joined {
                    state.ghost = Some((met.place, root.transform_point(met.place)));
                    state.placed_yaw = met.yaw;
                    state.lift = lift;
                    let along = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * met.edge.out;
                    state.join = Some((met.edge.at, along, met.edge.width));
                }
            }
        }
    }

    // The right grip: on a piece, select it; held, move the selected piece.
    if pressed(now.grip, previous.grip) {
        if let Some(id) = placed.pointed.filter(|&id| Some(id) != placed.selected) {
            finish_edit(&mut placed);
            placed.selected = Some(id);
            info!("BUILD selected piece {id}");
        }
        state.grip_hold = placed.selected.map(|_| 0.0);
    }
    if !now.grip {
        state.grip_hold = None;
    }
    if let (Some(held), Some(id)) = (state.grip_hold, placed.selected) {
        state.grip_hold = Some(held + dt);
        if held + dt >= MOVE_HOLD_SECONDS && placed.dragging.is_none() {
            finish_edit(&mut placed);
            placed.checkpoint();
            placed.dragging = Some(id);
            if let Some(item) = placed.items.iter().find(|p| p.id == id) {
                state.yaw = item.yaw;
            }
            info!("BUILD picked up piece {id}");
        }
    }
    if let Some(id) = placed.dragging {
        if let (Some((position, _)), Some(item)) = (state.ghost, placed.items.iter_mut().find(|p| p.id == id)) {
            item.position = position.to_array();
            item.yaw = state.placed_yaw;
            item.piece.lift = state.lift;
        }
        if !now.grip {
            placed.dragging = None;
            placed.revision += 1;
            info!("BUILD moved piece {id}");
        }
        return;
    }
    if placed.resizing.is_some() {
        return;
    }
    let confirm = !more && (pressed(now.trigger, previous.trigger) || pressed(now.a, previous.a));
    let click = !more && pressed(now.click, previous.click);
    // B removes the piece the pointer is on, selected or not; pointing at
    // none, the selected one.
    if pressed(now.b, previous.b) {
        if let Some(id) = placed.pointed.or(placed.selected) {
            if let Some(removed) = placed.remove(id) {
                info!("BUILD deleted {:?} (piece {})", removed.piece.kind, removed.id);
            }
            if placed.selected == Some(id) {
                placed.selected = None;
            }
            placed.pointed = None;
        }
        return;
    }
    if let Some(id) = placed.selected {
        // Editing.
        if more && pressed(now.a, previous.a) {
            placed.checkpoint();
            if let Some(item) = placed.get_mut(id) {
                item.hidden = !item.hidden;
                info!("BUILD piece {id} hidden={}", item.hidden);
            }
            placed.revision += 1;
        } else if click {
            // A copy: the next piece to drop is this one over again.
            if let Some(item) = placed.items.iter().find(|p| p.id == id).cloned() {
                let selected = PieceKind::ALL.iter().position(|&k| k == item.piece.kind).unwrap_or(0);
                state.selected = selected;
                state.sizes[selected] = item.piece.size;
                state.widths[selected] = item.piece.width;
                state.scale = item.scale;
                state.yaw = item.yaw;
                state.finishes[selected] = item.piece.kind.finishes().iter().position(|&f| f == item.finish()).unwrap_or(0);
                state.tints[selected] = item.tint;
                info!("BUILD copied piece {} ({:?})", item.id, item.piece.kind);
            }
            placed.selected = None;
        } else if confirm {
            placed.selected = None;
        }
        return;
    }
    // Placing.
    if click {
        state.free = !state.free;
        info!("BUILD free placement: {}", state.free);
        return;
    }
    if pressed(now.park, previous.park) {
        if let Some((position, _)) = state.ghost {
            placed.checkpoint();
            let yaw = state.placed_yaw;
            for (kind, size, offset, turn) in STARTER_PARK {
                // Offsets are game metres in the park's frame; positions are
                // tracking metres.
                let at = position + Quat::from_rotation_y(yaw) * Vec3::from_array(offset) * placed.park_scale / rig.scale;
                placed.insert(Piece::sized(kind, size), at, yaw + turn, 1.0);
            }
            info!("BUILD starter park at {position:?} ({} pieces)", placed.items.len());
        }
        return;
    }
    if confirm {
        if let Some((position, _)) = state.ghost {
            let piece = state.piece();
            let yaw = state.placed_yaw;
            let scale = snap_scale(state.scale);
            let finish = Some(state.finish()).filter(|&f| f != piece.kind.finishes()[0]);
            let id = placed.add(piece, position, yaw, scale, finish, state.tint());
            info!("BUILD placed {:?} size {:.2} scale {scale:.2} as piece {id} at {position:?}", piece.kind, piece.size);
        }
    }
}

/// The legend: what each control does now ([`build_controls`] is what does it).
fn describe(
    mode: Res<BuildMode>,
    state: Res<BuildState>,
    placed: Res<PlacedPieces>,
    touch: Res<LatestTouch>,
    mut legend: ResMut<legend::Legend>,
) {
    use legend::Row;
    let mut next = legend::Legend::default();
    if mode.active {
        let chosen = PieceKind::ALL[state.selected];
        let editing = placed.selected.and_then(|id| placed.items.iter().find(|p| p.id == id));
        // What the sticks change: the selected piece, or the next one.
        let (kind, finish, tinted, its) = match editing {
            Some(item) => (item.piece.kind, item.finish(), item.tint.is_some(), "its"),
            None => (chosen, state.finish(), state.tint().is_some(), "its"),
        };
        let lower = |k: PieceKind| k.name().to_lowercase();
        let pointed = placed.pointed.and_then(|id| placed.items.iter().find(|p| p.id == id));
        if placed.dragging.is_some() {
            next.right = vec![Row::new("Stick ↔", "turn it"), Row::new("Grip", "let go to put it down")];
        } else if touch.0.left_grip > PRESS {
            // The second set, while the left grip is held.
            next.left = vec![Row::new("Stick ↕", format!("{its} {}", kind.size_name()))];
            if kind.width_axis().is_some() {
                next.left.push(Row::new("Stick ↔", format!("{its} width")));
            }
            next.left.push(Row::new("Grip", "let go for the first set"));
            next.right = vec![Row::new("Stick ↔", "colour: hue"), Row::new("Stick ↕", "colour: lighter / darker")];
            if tinted {
                next.right.push(Row::new("Stick click", format!("{}'s own colour", finish.name())));
            }
            if let Some(item) = editing {
                next.right.push(Row::new("A", if item.hidden { "show it again" } else { "hide it (still skated)" }));
            }
        } else if let Some(item) = editing {
            next.left = vec![
                Row::new("Stick ↕", format!("its material: {}", finish.name())),
                Row::new("Grip (hold)", "more: stretch, colour, hide"),
                Row::new("Trigger", "undo"),
                Row::new("Stick ↔", "done; choose a new piece"),
            ];
            next.right = vec![
                Row::new("Stick ↔", format!("turn this {}", lower(item.piece.kind))),
                Row::new("Stick ↕", "make it bigger / smaller"),
                Row::new("Grip (hold)", "move it"),
                Row::new("Stick click", "copy it"),
                Row::new("B", match pointed.filter(|p| p.id != item.id) {
                    Some(other) => format!("remove that {}", lower(other.piece.kind)),
                    None => "remove it".to_string(),
                }),
                Row::new("A / Trigger", "done"),
            ];
        } else {
            next.left = vec![
                Row::new("Stick ↔", format!("piece: {}", chosen.name())),
                Row::new("Stick ↕", format!("material: {}", finish.name())),
                Row::new("Grip (hold)", "more: stretch, colour"),
                Row::new("Trigger", "undo"),
                Row::new("Stick click", "drop a starter park"),
                Row::new("X (hold)", "clear every piece"),
                Row::new("Menu (hold)", "leave build mode"),
            ];
            next.right = vec![
                Row::new("Stick ↔", "turn it"),
                Row::new("Stick ↕", "make it bigger / smaller"),
                Row::new(
                    "A / Trigger",
                    match (state.free, state.ghost.is_some()) {
                        (true, _) => "put it here",
                        (false, true) if state.join.is_some() => "join it on there",
                        (false, true) => "put it there",
                        (false, false) => "point at a surface",
                    },
                ),
            ];
            if let Some(item) = pointed {
                next.right.push(Row::new("Grip", format!("select this {}", lower(item.piece.kind))));
                next.right.push(Row::new("B", format!("remove this {}", lower(item.piece.kind))));
            }
            next.right.push(Row::new("Stick click", if state.free { "back onto surfaces" } else { "hold it in the hand" }));
            next.right.push(Row::new("Y", "scan the room again"));
        }
    }
    if *legend != next {
        *legend = next;
    }
}

/// A small line to skate away from the player: kicker and rail side by side,
/// a fun box, a ledge and a quarter pipe to turn on. (kind, size, offset in
/// game metres with -z away from the player, extra turn).
const STARTER_PARK: [(PieceKind, f32, [f32; 3], f32); 5] = [
    (PieceKind::Kicker, 0.8, [-4.0, 0.0, -5.0], 0.0),
    (PieceKind::Rail, 0.8, [4.0, 0.0, -6.0], std::f32::consts::FRAC_PI_2),
    (PieceKind::FunBox, 1.0, [0.0, 0.0, -12.0], 0.0),
    (PieceKind::Ledge, 0.8, [6.0, 0.0, -14.0], std::f32::consts::FRAC_PI_2),
    (PieceKind::QuarterPipe, 1.0, [0.0, 0.0, -20.0], 0.0),
];

fn snap_size(size: f32) -> f32 {
    ((size / SIZE_STEP).round() * SIZE_STEP).clamp(MIN_SIZE, MAX_SIZE)
}

pub(crate) fn snap_scale(scale: f32) -> f32 {
    ((scale / SIZE_STEP).round() * SIZE_STEP).clamp(MIN_PIECE_SCALE, MAX_PIECE_SCALE)
}

/// The scale stick was let go: put the skater back on the same real spot at
/// the new scale, and rebuild collision (and save) for it. After a park
/// rescale the skater is dropped onto whatever is under it once the new
/// collision exists ([`drop_after_park_scale`]).
fn finish_rescale(
    rig: &Rig,
    teleport: &mut MessageWriter<TeleportSkater>,
    state: &mut BuildState,
    placed: &mut PlacedPieces,
) {
    placed.rescaling = false;
    placed.revision += 1;
    let park = std::mem::take(&mut state.park_rescale);
    let Some((deck, forward)) = state.rescale_from.take() else {
        return;
    };
    if park {
        info!("BUILD park scale {:.2}", placed.park_scale);
        state.drop_after_rebuild = Some((deck, forward, placed.revision));
        return;
    }
    info!("BUILD world scale {:.2} (game metres per real metre)", rig.scale);
    let Some(root) = rig.root_transform() else {
        return;
    };
    teleport.write(TeleportSkater {
        point: root.transform_point(deck),
        forward: root.rotation * forward,
    });
}

/// Height above the skater's spot to search down from after a park rescale,
/// game metres.
const DROP_SEARCH: f32 = 200.0;

fn drop_after_park_scale(
    rig: Res<Rig>,
    world: Res<ArWorld>,
    placed: Res<PlacedPieces>,
    mut state: ResMut<BuildState>,
    mut teleport: MessageWriter<TeleportSkater>,
    mut seen: Local<u64>,
) {
    // Wait for the rebuild that follows the rescale's revision.
    let Some((deck, forward, revision)) = state.drop_after_rebuild else {
        *seen = world.revision;
        return;
    };
    if placed.revision != revision || world.revision == *seen {
        return;
    }
    state.drop_after_rebuild = None;
    let Some(root) = rig.root_transform() else {
        return;
    };
    let point = root.transform_point(deck);
    let from = point + Vec3::Y * DROP_SEARCH;
    let top = world
        .all_triangles()
        .filter_map(|[a, b, c]| ray_triangle(from, Vec3::NEG_Y, a, b, c))
        .min_by(f32::total_cmp)
        .map_or(point, |t| from - Vec3::Y * t);
    teleport.write(TeleportSkater {
        point: top,
        forward: root.rotation * forward,
    });
}

#[derive(Clone, Copy)]
struct Hit {
    point: Vec3,
    normal: Vec3,
    piece: Option<u64>,
}

/// Nearest face along the ray: the real room (collision tag 0) or a placed
/// piece. The piece being dragged is ignored so it can't land on itself.
fn cast(
    world: &ArWorld,
    placed: &PlacedPieces,
    root: &Transform,
    origin: Vec3,
    direction: Vec3,
) -> Option<Hit> {
    let mut best: Option<(f32, Hit)> = None;
    let mut consider = |t: Option<f32>, normal: Vec3, piece: Option<u64>| {
        if let Some(t) = t.filter(|&t| t <= MAX_RANGE) {
            if best.is_none_or(|(b, _)| t < b) {
                let normal = normal.normalize_or_zero();
                let normal = if normal.dot(direction) > 0.0 { -normal } else { normal };
                best = Some((t, Hit { point: origin + direction * t, normal, piece }));
            }
        }
    };
    for [a, b, c] in world.room() {
        consider(ray_triangle(origin, direction, a, b, c), (b - a).cross(c - a), None);
    }
    for item in placed.items.iter().filter(|p| Some(p.id) != placed.dragging) {
        let at = item.world_transform(root, placed.park_scale);
        for q in item.piece.geometry().all_quads() {
            let [a, b, c, d] = q.map(|p| at.transform_point(p));
            let normal = (c - a).cross(b - a);
            consider(ray_triangle(origin, direction, a, b, c), normal, Some(item.id));
            consider(ray_triangle(origin, direction, a, c, d), normal, Some(item.id));
        }
    }
    best.map(|(_, hit)| hit)
}

/// Meshes and materials per piece shape, shared by placed pieces, the ghost
/// and the palette.
#[derive(Resource, Default)]
struct PieceAssets {
    meshes: HashMap<(PieceKind, u32, u32, u32), Handle<Mesh>>,
    solid: HashMap<(PieceKind, Finish, Option<Tint>), Handle<StandardMaterial>>,
    /// A finish's photograph, and (true) that photograph without its own
    /// colour, to be given another.
    textures: HashMap<(Finish, bool), Handle<Image>>,
    occluder: Option<Handle<Occluder>>,
    /// The ghost in free placement.
    ghost_free: Option<Handle<StandardMaterial>>,
    ghost: Option<Handle<StandardMaterial>>,
    /// Invisible pieces, when shown.
    hidden: Option<Handle<StandardMaterial>>,
}

impl PieceAssets {
    fn mesh(&mut self, piece: Piece, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        self.meshes
            .entry((piece.kind, piece.size.to_bits(), piece.width.to_bits(), piece.lift.to_bits()))
            .or_insert_with(|| meshes.add(piece_mesh(&piece.geometry())))
            .clone()
    }

    fn solid(
        &mut self,
        kind: PieceKind,
        finish: Finish,
        tint: Option<Tint>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
    ) -> Handle<StandardMaterial> {
        if let Some(material) = self.solid.get(&(kind, finish, tint)) {
            return material.clone();
        }
        let look = finish.look();
        let texture = look.texture.map(|(bytes, _)| {
            self.textures
                .entry((finish, tint.is_some()))
                .or_insert_with(|| images.add(tiling_texture(bytes, tint.is_some())))
                .clone()
        });
        // A colour of the player's goes over the photograph's light and dark
        // (kept at `NEUTRAL_LEVEL`, which the colour makes up for).
        let color = match tint {
            Some(tint) if texture.is_some() => (tint.color().to_linear() * (1.0 / NEUTRAL_LEVEL)).into(),
            Some(tint) => tint.color(),
            None => look.color.unwrap_or(kind.color()),
        };
        let material = materials.add(StandardMaterial {
            base_color: color,
            base_color_texture: texture,
            // Texture coordinates are metres ([`PieceGeometry::uvs`]).
            uv_transform: bevy::math::Affine2::from_scale(Vec2::splat(1.0 / look.texture.map_or(1.0, |(_, metres)| metres))),
            perceptual_roughness: look.roughness,
            metallic: look.metallic,
            // (a sunk piece is only its inside: its faces seen from behind, through the surface
            // it is in, are not there)
            double_sided: !kind.sunk(),
            cull_mode: kind.sunk().then_some(bevy::render::render_resource::Face::Back),
            ..default()
        });
        self.solid.insert((kind, finish, tint), material.clone());
        material
    }
}

/// How a finish is drawn.
struct Look {
    /// A photograph of the surface (a JPEG, see `park-textures/CREDITS.json`)
    /// and the metres one copy of it covers.
    texture: Option<(&'static [u8], f32)>,
    /// Its colour; none: the piece's own (`PieceKind::color`).
    color: Option<Color>,
    roughness: f32,
    metallic: f32,
}

macro_rules! park_texture {
    ($name:literal) => {
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/park-textures/", $name, ".jpg")) as &[u8]
    };
}

impl Finish {
    fn look(self) -> Look {
        let photo = |bytes, metres, roughness| Look {
            texture: Some((bytes, metres)),
            color: Some(Color::WHITE),
            roughness,
            metallic: 0.0,
        };
        match self {
            Finish::Wood => photo(park_texture!("wood"), 2.4, 0.6),
            Finish::Concrete => photo(park_texture!("concrete"), 3.0, 0.9),
            // (its bricks bigger than life: at a miniature's size real ones are specks)
            Finish::Brick => photo(park_texture!("brick"), 2.0, 0.9),
            // (no reflections to show in a room, so only a little metal in it)
            Finish::Steel => Look { texture: None, color: Some(Color::srgb(0.62, 0.64, 0.67)), roughness: 0.35, metallic: 0.25 },
            Finish::Paint => Look { texture: None, color: None, roughness: 0.85, metallic: 0.0 },
        }
    }
}

/// A photograph as a repeating texture with its smaller copies (mipmaps):
/// the park is a miniature, a brick a fraction of a pixel at arm's length,
/// and without them it shimmers.
/// The mean (linear) level of a photograph with its colour taken out.
const NEUTRAL_LEVEL: f32 = 0.5;

fn tiling_texture(jpeg: &[u8], neutral: bool) -> Image {
    use bevy::image::{CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor, ImageType};
    let image = Image::from_buffer(
        jpeg,
        ImageType::Extension("jpg"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        bevy::asset::RenderAssetUsages::default(),
    );
    let mut image = match image.and_then(|i| Ok(i.convert(bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb))) {
        Ok(Some(image)) => image,
        _ => {
            warn!("BUILD a park texture couldn't be read");
            return Image::default();
        }
    };
    let (mut width, mut height) = (image.width() as usize, image.height() as usize);
    let mut level = image.data.clone().unwrap_or_default();
    if neutral {
        // Its light and dark only, about `NEUTRAL_LEVEL`.
        let linear = |v: u8| (v as f32 / 255.0).powf(2.2);
        let lights: Vec<f32> = level.chunks_exact(4).map(|p| 0.2126 * linear(p[0]) + 0.7152 * linear(p[1]) + 0.0722 * linear(p[2])).collect();
        let mean = (lights.iter().sum::<f32>() / lights.len().max(1) as f32).max(1e-4);
        for (pixel, light) in level.chunks_exact_mut(4).zip(lights) {
            let v = ((light / mean * NEUTRAL_LEVEL).clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round() as u8;
            pixel[..3].fill(v);
        }
    }
    let mut all = level.clone();
    let mut levels = 1;
    while width > 1 && height > 1 && width % 2 == 0 && height % 2 == 0 {
        level = halve(&level, width, height);
        (width, height) = (width / 2, height / 2);
        all.extend_from_slice(&level);
        levels += 1;
    }
    image.data = Some(all);
    image.texture_descriptor.mip_level_count = levels;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

/// An RGBA8 image at half its size: each pixel the mean of four.
fn halve(pixels: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height);
    for y in (0..height).step_by(2) {
        for x in (0..width).step_by(2) {
            for c in 0..4 {
                let at = |x: usize, y: usize| pixels[(y * width + x) * 4 + c] as u32;
                out.push(((at(x, y) + at(x + 1, y) + at(x, y + 1) + at(x + 1, y + 1) + 2) / 4) as u8);
            }
        }
    }
    out
}

fn piece_mesh(geometry: &PieceGeometry) -> Mesh {
    let seen = |index: &usize| !geometry.unseen.contains(index);
    let positions: Vec<[f32; 3]> = geometry
        .all_quads()
        .enumerate()
        .filter(|(i, _)| seen(i))
        .flat_map(|(_, q)| [0, 2, 1, 0, 3, 2].map(|i| q[i].to_array()))
        .collect();
    let uvs: Vec<[f32; 2]> = geometry
        .uvs()
        .iter()
        .enumerate()
        .filter(|(i, _)| seen(i))
        .flat_map(|(_, uv)| [0, 2, 1, 0, 3, 2].map(|i| uv[i].to_array()))
        .collect();
    let mut mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.compute_flat_normals();
    mesh
}

/// A sunk piece's outside, drawn as nothing but depth (see `occluder.wgsl`): the piece's own
/// mesh, only the faces that look away.
#[derive(Asset, TypePath, bevy::render::render_resource::AsBindGroup, Clone, Default)]
struct Occluder {
    #[uniform(0)]
    _unused: Vec4,
}

impl Material for Occluder {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://skate_game/xr/build/occluder.wgsl".into()
    }

    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = Some(bevy::render::render_resource::Face::Front);
        Ok(())
    }
}

/// A placed piece's entity: its id, shape, whether it is drawn as a ghost,
/// and its finish and colour.
#[derive(Component)]
struct PlacedVisual(u64, Piece, bool, Finish, Option<Tint>);
/// The piece about to be dropped: its shape, and whether it is in the hand.
#[derive(Component)]
struct GhostVisual(Option<(Piece, bool)>);
/// What marks the piece being edited or pointed at.
#[derive(Component)]
struct EditMark(Mark, Option<Piece>);

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    /// A shell round the selected piece.
    Selected,
    /// A fainter one round the piece the pointer is on.
    Pointed,
    /// A stretch arrow (0: its size, 1: its width): its shaft and two heads.
    Shaft(usize),
    Head(usize, f32),
    /// A bar along the edge the piece in hand is joined to.
    Join,
}
#[derive(Component)]
struct BuildRay;
#[derive(Component)]
struct BuildMarker;
/// A piece in the wrist palette, and what it is drawn as.
#[derive(Component)]
struct PaletteIcon {
    slot: Slot,
    shown: Option<(PieceKind, Finish, Option<Tint>)>,
}

#[derive(Clone, Copy)]
enum Slot {
    /// The row: one of each kind, in the finish its next piece will have.
    Kind(usize),
    /// The stack above the chosen kind: that kind in each of its finishes.
    Finish(usize),
}

fn spawn_visuals(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut assets: ResMut<PieceAssets>,
) {
    let unlit = |color: Color| StandardMaterial {
        base_color: color,
        unlit: true,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    assets.ghost = Some(materials.add(StandardMaterial {
        alpha_mode: AlphaMode::Blend,
        ..unlit(Color::srgba(0.3, 1.0, 0.5, 0.45))
    }));
    assets.hidden = Some(materials.add(StandardMaterial {
        alpha_mode: AlphaMode::Blend,
        ..unlit(Color::srgba(0.75, 0.85, 1.0, 0.22))
    }));
    assets.ghost_free = Some(materials.add(StandardMaterial {
        alpha_mode: AlphaMode::Blend,
        ..unlit(Color::srgba(0.35, 0.65, 1.0, 0.5))
    }));
    let shell = |color: Color| StandardMaterial { alpha_mode: AlphaMode::Blend, ..unlit(color) };
    commands.spawn((
        EditMark(Mark::Selected, None),
        MeshMaterial3d(materials.add(shell(Color::srgba(1.0, 0.85, 0.2, 0.4)))),
        Transform::default(),
        Visibility::Hidden,
    ));
    commands.spawn((
        EditMark(Mark::Pointed, None),
        MeshMaterial3d(materials.add(shell(Color::srgba(1.0, 1.0, 1.0, 0.2)))),
        Transform::default(),
        Visibility::Hidden,
    ));
    let arrow = materials.add(unlit(Color::srgb(1.0, 0.85, 0.2)));
    let (shaft, head) = (meshes.add(Cuboid::new(1.0, 1.0, 1.0)), meshes.add(Cone { radius: 1.0, height: 2.0 }));
    for axis in 0..2 {
        commands.spawn((EditMark(Mark::Shaft(axis), None), Mesh3d(shaft.clone()), MeshMaterial3d(arrow.clone()), Transform::default(), Visibility::Hidden));
        for end in [1.0, -1.0] {
            commands.spawn((EditMark(Mark::Head(axis, end), None), Mesh3d(head.clone()), MeshMaterial3d(arrow.clone()), Transform::default(), Visibility::Hidden));
        }
    }
    commands.spawn((
        EditMark(Mark::Join, None),
        Mesh3d(shaft.clone()),
        MeshMaterial3d(materials.add(unlit(Color::srgb(0.3, 1.0, 0.9)))),
        Transform::default(),
        Visibility::Hidden,
    ));
    commands.spawn((
        BuildRay,
        Mesh3d(meshes.add(Cylinder::new(0.04, 1.0))),
        MeshMaterial3d(materials.add(unlit(Color::srgb(1.0, 0.8, 0.2)))),
        Transform::default(),
        Visibility::Hidden,
    ));
    commands.spawn((
        BuildMarker,
        Mesh3d(meshes.add(Annulus::new(0.5, 0.8))),
        MeshMaterial3d(materials.add(unlit(Color::srgb(1.0, 0.35, 0.3)))),
        Transform::default(),
        Visibility::Hidden,
    ));
    commands.spawn((GhostVisual(None), Transform::default(), Visibility::Hidden));
    let slots = (0..PieceKind::ALL.len()).map(Slot::Kind).chain((0..MAX_FINISHES).map(Slot::Finish));
    for slot in slots {
        commands.spawn((PaletteIcon { slot, shown: None }, Transform::default(), Visibility::Hidden));
    }
    // (every kind's own finish, made now: the first piece dropped shouldn't wait on a texture)
    for kind in PieceKind::ALL {
        assets.solid(kind, kind.finishes()[0], None, &mut materials, &mut images);
    }
}

/// One entity per placed piece, kept at its world pose. Invisible pieces are
/// ghosts in build mode (or with "show invisible pieces") and not drawn
/// otherwise.
#[allow(clippy::too_many_arguments)]
fn sync_pieces(
    mut commands: Commands,
    placed: Res<PlacedPieces>,
    mode: Res<BuildMode>,
    rig: Res<Rig>,
    mut assets: ResMut<PieceAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut occluders: ResMut<Assets<Occluder>>,
    mut visuals: Query<(Entity, &PlacedVisual, &mut Transform, &mut Visibility)>,
) {
    let Some(root) = rig.root_transform() else {
        return;
    };
    let see_hidden = mode.active || placed.show_hidden;
    let mut shown = Vec::with_capacity(placed.items.len());
    for (entity, visual, mut transform, mut visibility) in &mut visuals {
        match placed.items.iter().find(|p| p.id == visual.0) {
            Some(item) if item.piece == visual.1 && item.hidden == visual.2 && item.finish() == visual.3 && item.tint == visual.4 => {
                let target = item.world_transform(&root, placed.park_scale);
                if *transform != target {
                    *transform = target;
                }
                visibility.set_if_neq(if item.hidden && !see_hidden { Visibility::Hidden } else { Visibility::Inherited });
                shown.push(item.id);
            }
            _ => commands.entity(entity).despawn(),
        }
    }
    for item in placed.items.iter().filter(|p| !shown.contains(&p.id)) {
        let material = if item.hidden {
            assets.hidden.clone().unwrap_or_default()
        } else {
            assets.solid(item.piece.kind, item.finish(), item.tint, &mut materials, &mut images)
        };
        let visual = commands
            .spawn((
                PlacedVisual(item.id, item.piece, item.hidden, item.finish(), item.tint),
                Mesh3d(assets.mesh(item.piece, &mut meshes)),
                MeshMaterial3d(material),
                item.world_transform(&root, placed.park_scale),
                if item.hidden && !see_hidden { Visibility::Hidden } else { Visibility::Inherited },
            ))
            .id();
        // Sunk into a surface: from the side that surface hides it (its own outside does, drawn
        // into depth only).
        if item.piece.kind.sunk() && !item.hidden {
            let occluder = assets.occluder.get_or_insert_with(|| occluders.add(Occluder::default())).clone();
            commands.spawn((Mesh3d(assets.mesh(item.piece, &mut meshes)), MeshMaterial3d(occluder), Transform::default(), ChildOf(visual)));
        }
    }
}

#[allow(clippy::type_complexity)]
fn show_build(
    mode: Res<BuildMode>,
    state: Res<BuildState>,
    placed: Res<PlacedPieces>,
    rig: Res<Rig>,
    hands: Query<(&Hand, &Transform), Without<PaletteIcon>>,
    mut assets: ResMut<PieceAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut commands: Commands,
    mut ray: Query<(&mut Transform, &mut Visibility), (With<BuildRay>, Without<BuildMarker>, Without<GhostVisual>, Without<PaletteIcon>, Without<Hand>)>,
    mut marker: Query<(&mut Transform, &mut Visibility), (With<BuildMarker>, Without<BuildRay>, Without<GhostVisual>, Without<PaletteIcon>, Without<Hand>)>,
    mut ghost: Query<(Entity, &mut GhostVisual, &mut Transform, &mut Visibility), (Without<BuildRay>, Without<BuildMarker>, Without<PaletteIcon>, Without<Hand>)>,
    mut icons: Query<(Entity, &mut PaletteIcon, &mut Transform, &mut Visibility), (Without<BuildRay>, Without<BuildMarker>, Without<GhostVisual>, Without<Hand>)>,
    time: Res<Time<Real>>,
    mut slid: Local<f32>,
) {
    let (Ok((mut ray_tf, mut ray_vis)), Ok((mut marker_tf, mut marker_vis))) =
        (ray.single_mut(), marker.single_mut())
    else {
        return;
    };
    let Ok((ghost_entity, mut ghost_piece, mut ghost_tf, mut ghost_vis)) = ghost.single_mut() else {
        return;
    };
    let root = rig.root_transform();
    // Ray.
    match state.ray.filter(|_| mode.active) {
        Some((start, end, _)) => {
            let length = start.distance(end).max(1e-3);
            *ray_tf = Transform {
                translation: (start + end) * 0.5,
                rotation: Quat::from_rotation_arc(Vec3::Y, (end - start) / length),
                scale: Vec3::new(1.0, length, 1.0),
            };
            *ray_vis = Visibility::Visible;
        }
        None => *ray_vis = Visibility::Hidden,
    }
    // A ring on the piece the ray is on: grip moves it, B deletes it.
    match placed.pointed.and(state.ray).filter(|_| mode.active && placed.dragging.is_none()) {
        Some((_, end, _)) => {
            *marker_tf = Transform::from_translation(end + Vec3::Y * 0.05)
                .with_rotation(Quat::from_rotation_arc(Vec3::Z, Vec3::Y));
            *marker_vis = Visibility::Visible;
        }
        None => *marker_vis = Visibility::Hidden,
    }
    // Ghost of the piece to drop.
    let piece = state.piece();
    // (also over a placed piece: a piece can go on top of another; not while one is being edited)
    match state.ghost.zip(root).filter(|_| mode.active && placed.dragging.is_none() && placed.selected.is_none()) {
        Some(((_, point), root)) => {
            if ghost_piece.0 != Some((piece, state.free)) {
                ghost_piece.0 = Some((piece, state.free));
                let material = if state.free { assets.ghost_free.clone() } else { assets.ghost.clone() };
                commands.entity(ghost_entity).insert((
                    Mesh3d(assets.mesh(piece, &mut meshes)),
                    MeshMaterial3d(material.unwrap_or_default()),
                ));
            }
            *ghost_tf = Transform {
                translation: point,
                rotation: root.rotation * Quat::from_rotation_y(state.placed_yaw),
                scale: Vec3::splat(placed.park_scale * snap_scale(state.scale)),
            };
            *ghost_vis = Visibility::Visible;
        }
        None => *ghost_vis = Visibility::Hidden,
    }
    // Wrist palette above the left controller: a row of the kinds, and above
    // the chosen one a stack of what it can be made of.
    let left = hands.iter().find(|(hand, _)| matches!(hand, Hand::Left)).map(|(_, t)| *t);
    let chosen = PieceKind::ALL[state.selected];
    // Where the row has slid to (a place in `PieceKind::ALL`, between kinds
    // while it moves), going round the short way.
    let count = PieceKind::ALL.len() as f32;
    let round = |d: f32| (d + count * 0.5).rem_euclid(count) - count * 0.5;
    *slid += round(state.selected as f32 - *slid) * (1.0 - (-PALETTE_SLIDE * time.delta_secs()).exp());
    *slid = slid.rem_euclid(count);
    for (entity, mut icon, mut transform, mut visibility) in &mut icons {
        let (Some(root), Some(left), true) = (root, left, mode.active) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        // (kind, finish, places along the row from the chosen one, height
        // above the controller, raised)
        let (kind, finish, tint, along, height, raised) = match icon.slot {
            // The chosen kind's place in the row is the foot of its stack.
            Slot::Kind(i) if i == state.selected => {
                *visibility = Visibility::Hidden;
                continue;
            }
            Slot::Kind(i) => {
                let kind = PieceKind::ALL[i];
                let all = kind.finishes();
                let along = round(i as f32 - *slid);
                if along.abs() > PALETTE_REACH + 0.5 {
                    *visibility = Visibility::Hidden;
                    continue;
                }
                (kind, all[state.finishes[i].min(all.len() - 1)], state.tints[i], along, 0.06, false)
            }
            Slot::Finish(j) => {
                let Some(&finish) = chosen.finishes().get(j) else {
                    *visibility = Visibility::Hidden;
                    continue;
                };
                let raised = finish == state.finish();
                (chosen, finish, state.tint().filter(|_| raised), round(state.selected as f32 - *slid), 0.075 + j as f32 * FINISH_SPACING_M, raised)
            }
        };
        if icon.shown != Some((kind, finish, tint)) {
            icon.shown = Some((kind, finish, tint));
            commands.entity(entity).insert((
                Mesh3d(assets.mesh(Piece::new(kind), &mut meshes)),
                MeshMaterial3d(assets.solid(kind, finish, tint, &mut materials, &mut images)),
            ));
        }
        let offset = Vec3::new(along * ICON_SPACING_M, height, -0.02);
        // Smaller toward the row's ends, where it comes in and goes out.
        let fade = (1.0 - (along.abs() / (PALETTE_REACH + 0.5)).powi(2) * 0.6).clamp(0.0, 1.0);
        let scale = ICON_SIZE_M / icon_extent(kind) * if raised { 1.4 } else { fade };
        *transform = Transform {
            translation: root.transform_point(left.transform_point(offset)),
            rotation: root.rotation * left.rotation,
            scale: Vec3::splat(root.scale.x * scale),
        };
        *visibility = Visibility::Visible;
    }
}

/// What marks the piece being worked on: a shell round the selected piece and
/// a fainter one round the piece pointed at, and, while the left grip is
/// held, an arrow along each way the piece stretches (the selected piece, or
/// the one about to be dropped).
#[allow(clippy::too_many_arguments)]
fn show_edit(
    mode: Res<BuildMode>,
    state: Res<BuildState>,
    placed: Res<PlacedPieces>,
    rig: Res<Rig>,
    touch: Res<LatestTouch>,
    mut assets: ResMut<PieceAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
    mut marks: Query<(Entity, &mut EditMark, &mut Transform, &mut Visibility)>,
) {
    let root = rig.root_transform().filter(|_| mode.active);
    let find = |id: Option<u64>| id.and_then(|id| placed.items.iter().find(|p| p.id == id));
    let selected = find(placed.selected);
    let pointed = find(placed.pointed.filter(|&id| Some(id) != placed.selected && placed.dragging.is_none()));
    // What the arrows are on: its shape and where it is in the world.
    let stretched = root.filter(|_| touch.0.left_grip > PRESS && placed.dragging.is_none()).and_then(|root| match selected {
        Some(item) => Some((item.piece, item.world_transform(&root, placed.park_scale))),
        None => state.ghost.map(|(_, point)| {
            (state.piece(), Transform {
                translation: point,
                rotation: root.rotation * Quat::from_rotation_y(state.placed_yaw),
                scale: Vec3::splat(placed.park_scale * snap_scale(state.scale)),
            })
        }),
    });
    for (entity, mut mark, mut transform, mut visibility) in &mut marks {
        let shown = match (mark.0, root) {
            (Mark::Selected | Mark::Pointed, Some(root)) => {
                let item = if mark.0 == Mark::Selected { selected } else { pointed };
                item.map(|item| {
                    if mark.1 != Some(item.piece) {
                        mark.1 = Some(item.piece);
                        commands.entity(entity).insert(Mesh3d(assets.mesh(item.piece, &mut meshes)));
                    }
                    // A little bigger than the piece, about its middle.
                    let at = item.world_transform(&root, placed.park_scale);
                    let (lo, hi) = item.piece.geometry().bounds();
                    let middle = (lo + hi) * 0.5;
                    Transform { translation: at.transform_point(middle - middle * 1.04), scale: at.scale * 1.04, ..at }
                })
            }
            (Mark::Join, Some(root)) => state.join.filter(|_| placed.selected.is_none() || placed.dragging.is_some()).map(|(at, along, width)| Transform {
                translation: root.transform_point(at),
                rotation: root.rotation * Quat::from_rotation_arc(Vec3::X, along),
                scale: root.scale.x * Vec3::new(width.max(0.06), 0.012, 0.012),
            }),
            (Mark::Shaft(axis) | Mark::Head(axis, _), _) => stretched.and_then(|(piece, at)| {
                let along = if axis == 0 { Some(piece.kind.size_axis()) } else { piece.kind.width_axis() }?;
                let (lo, hi) = piece.geometry().bounds();
                // Over the piece's top, the width's arrow above the size's.
                let middle = ((lo + hi) * 0.5).with_y(hi.y + 0.35 + 0.3 * axis as f32);
                let half = ((hi - lo).dot(along) * 0.5).max(0.6);
                let thick = 0.06;
                Some(match mark.0 {
                    Mark::Shaft(_) => Transform {
                        translation: at.transform_point(middle),
                        rotation: at.rotation * Quat::from_rotation_arc(Vec3::X, along),
                        scale: at.scale * Vec3::new(2.0 * half, thick, thick),
                    },
                    Mark::Head(_, end) => Transform {
                        translation: at.transform_point(middle + along * end * half),
                        rotation: at.rotation * Quat::from_rotation_arc(Vec3::Y, along * end),
                        scale: at.scale * (thick * 2.2),
                    },
                    _ => unreachable!(),
                })
            }),
            _ => None,
        };
        match shown {
            Some(target) => {
                if *transform != target {
                    *transform = target;
                }
                visibility.set_if_neq(Visibility::Visible);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

/// Largest dimension of a default-size piece, game metres.
fn icon_extent(kind: PieceKind) -> f32 {
    let geometry = Piece::new(kind).geometry();
    let (min, max) = geometry
        .all_quads()
        .flatten()
        .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    (max - min).max_element().max(0.1)
}
