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
mod anchor;
mod dress;
mod layout;
mod legend;
pub(crate) mod pieces;
mod snap;

use self::pieces::{drawn, Finish, MAX_SIZE, MIN_SIZE, Piece, PieceGeometry, PieceKind, Tint};
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
/// A handle's dot: its radius, real metres.
const HANDLE_DOT_M: f32 = 0.012;
/// How near the pointer's ray a drawn piece's handle is taken, real metres.
const HANDLE_REACH_M: f32 = 0.05;
/// How near an edge (or an outline's first point) a point being drawn takes it, real metres.
const DRAW_SNAP_M: f32 = 0.08;
/// How fast the stick raises a point, real metres a second.
const RISE_RATE: f32 = 0.25;
/// A pad's height until the stick changes it, its own metres.
const PAD_HEIGHT_M: f32 = 0.5;

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
    /// Its posts (by bit) that would stand over a gap and are left out
    /// (`no_posts_over_gaps`); not saved.
    #[serde(skip)]
    pub no_posts: u32,
    /// How much further down than made its two end posts reach, to stand on
    /// what is under them (its own metres; `no_posts_over_gaps`); not saved.
    #[serde(skip)]
    pub ends_down: [f32; 2],
    /// What it is made of; absent (layouts from before finishes, and pieces
    /// left as they come): its kind's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<Finish>,
    /// Its colour, when not its finish's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<Tint>,
    /// A drawn piece's points (`drawn`), from `position` in its own metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<drawn::Path>,
}

/// A handle of a drawn piece: one of its points, or the bend of the stretch
/// after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grab {
    Node(usize),
    Bend(usize),
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
        let (min, max) = self.geometry().footprint();
        let k = metres * self.scale;
        snap::Footprint { position: Vec3::from_array(self.position), yaw: self.yaw, min: min * k, max: max * k }
    }

    /// Where other pieces join it, in tracking space ([`snap::meet`]).
    fn edges(&self, metres: f32) -> Vec<snap::Edge> {
        let k = metres * self.scale;
        let (at, turn) = (Vec3::from_array(self.position), Quat::from_rotation_y(self.yaw));
        let ends = self.path.iter().flat_map(|path| path.ends(self.piece)).map(|(at, out, width)| pieces::Join { at, out, width, follows: None });
        self.piece
            .joins()
            .into_iter()
            .chain(ends)
            .map(|j| snap::Edge { at: at + turn * (j.at * k), out: turn * j.out, width: j.width * k })
            .collect()
    }

    /// What is ridden and drawn: its path's shape if it was drawn, else its kind's.
    pub(crate) fn geometry(&self) -> PieceGeometry {
        self.whole_geometry().with_ends_down(self.ends_down).without_posts(self.no_posts)
    }

    /// Its posts are not as its kind's come: its mesh is its own.
    fn own_posts(&self) -> bool {
        self.no_posts != 0 || self.ends_down != [0.0; 2]
    }

    /// With every post, the ones over a gap too.
    fn whole_geometry(&self) -> PieceGeometry {
        match &self.path {
            Some(path) => path.geometry(self.piece),
            None => self.piece.geometry(),
        }
    }

    /// Changes whenever its shape would.
    fn shape_key(&self) -> u64 {
        let ends = u64::from(self.ends_down[0].to_bits()) << 32 | u64::from(self.ends_down[1].to_bits());
        self.path.as_ref().map_or(0, drawn::Path::key) ^ u64::from(self.no_posts).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ ends.wrapping_mul(0xc2b2_ae3d_27d4_eb4f)
    }

    /// A tracking-space point in the piece's own metres, and back.
    fn to_local(&self, metres: f32, point: Vec3) -> Vec3 {
        Quat::from_rotation_y(-self.yaw) * (point - Vec3::from_array(self.position)) / (metres * self.scale)
    }

    fn from_local(&self, metres: f32, point: Vec3) -> Vec3 {
        Vec3::from_array(self.position) + Quat::from_rotation_y(self.yaw) * (point * metres * self.scale)
    }

    /// A drawn piece's handles, in tracking space.
    fn handles(&self, metres: f32) -> Vec<(Grab, Vec3)> {
        let Some(path) = &self.path else { return Vec::new() };
        let closed = self.piece.kind.closed();
        let up = if self.piece.kind == PieceKind::RailPath { Vec3::Y * drawn::RAIL_TOP } else { Vec3::ZERO };
        let nodes = path.nodes.iter().enumerate().map(|(i, n)| (Grab::Node(i), Vec3::from_array(n.at)));
        let bends = (0..path.stretches(closed)).map(|i| (Grab::Bend(i), path.middle(i, closed)));
        nodes.chain(bends).map(|(h, p)| (h, self.from_local(metres, p + up))).collect()
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
    /// A drawn piece's handle is being moved.
    pub shaping: bool,
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
            shaping: false,
            selected: None,
            pointed: None,
            show_hidden: false,
            history: Vec::new(),
        }
    }
}

impl PlacedPieces {
    pub(crate) fn busy(&self) -> bool {
        self.dragging.is_some() || self.resizing.is_some() || self.rescaling || self.shaping
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
            no_posts: 0,
ends_down: [0.0; 2],
            finish: None,
            tint: None,
            path: None,
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
    /// The points placed so far of the piece being drawn (tracking space),
    /// the one the pointer would add, and whether the pointer is back on
    /// the first (an outline closes there).
    draft: Vec<drawn::Node>,
    pending: Option<drawn::Node>,
    closing: bool,
    /// The height (tracking y) the next point takes; none: its surface's.
    level: Option<f32>,
    /// The next drawn piece has no base down to its surface (`drawn::Path::floating`).
    floating: bool,
    /// How far the stick raised the point being placed or moved this frame, m.
    rise: f32,
    /// The handle of the selected drawn piece the pointer is on, and the one held.
    handle: Option<Grab>,
    handle_drag: Option<Grab>,
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
    /// The piece being moved: from where the pointer was on the ground when
    /// it was picked up to the piece's own place (tracking space, level),
    /// and its turn then. It keeps that far from the pointer, so it doesn't
    /// jump to it.
    carry: Option<(Vec3, f32)>,
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
            draft: Vec::new(),
            pending: None,
            closing: false,
            level: None,
            floating: false,
            rise: 0.0,
            handle: None,
            handle_drag: None,
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
            carry: None,
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
            .add_plugins(anchor::AnchorPlugin)
            .add_systems(Update, no_posts_over_gaps.before(sync_pieces))
            .add_systems(Update, (sync_pieces, show_build, show_edit, show_drawn, describe, layout::save, drop_after_park_scale));
    }
}

/// A post is left out when what is under it is this far below its foot (the
/// piece's own metres), or nothing is.
const POST_GAP: f32 = 0.2;

/// A rail's posts are left out where they would stand over a gap: which
/// ones is worked out again whenever the layout or the room changes.
fn no_posts_over_gaps(rig: Res<Rig>, world: Res<ArWorld>, mut placed: ResMut<PlacedPieces>, mut seen: Local<Option<(u64, u64)>>) {
    // (before there is a room to stand on, every post stays)
    if placed.busy() || world.revision == 0 || *seen == Some((placed.revision, world.revision)) {
        return;
    }
    let Some(root) = rig.root_transform() else { return };
    let posts = posts_over_gaps(&world, &placed, &root);
    if placed.items.iter().zip(&posts).any(|(item, (mask, down))| item.no_posts != *mask || item.ends_down != *down) {
        let placed = &mut *placed;
        for (item, (mask, down)) in placed.items.iter_mut().zip(posts) {
            item.no_posts = mask;
            item.ends_down = down;
        }
        // (the collision and the pieces' meshes are made again)
        placed.revision += 1;
    }
    *seen = Some((placed.revision, world.revision));
}

/// For each piece, its posts (by bit) with nothing to stand on: under the
/// post's foot the room and the other pieces are well below it (the inside
/// of a bowl, the drop between two tables or two platforms) or not there at
/// all. A rail always keeps the posts at its two ends: those reach down
/// instead to whatever is under them, by the second of the pair (its own
/// metres).
fn posts_over_gaps(world: &ArWorld, placed: &PlacedPieces, root: &Transform) -> Vec<(u32, [f32; 2])> {
    let metres = placed.park_scale / root.scale.x;
    let to_tracking = root.compute_affine().inverse();
    let down = root.rotation * Vec3::NEG_Y;
    placed
        .items
        .iter()
        .map(|item| {
            if !item.piece.kind.on_posts() {
                return (0, [0.0; 2]);
            }
            let whole = item.whole_geometry();
            let ends = whole.end_posts();
            let k = metres * item.scale;
            let (mut mask, mut ends_down) = (0u32, [0.0; 2]);
            for (index, (foot, top)) in whole.post_spans().into_iter().enumerate().take(32) {
                let (foot, top) = (item.from_local(metres, foot), item.from_local(metres, top));
                let under = cast_past(world, placed, root, root.transform_point(top), down, Some(item.id)).map(|hit| to_tracking.transform_point3(hit.point).y);
                if ends.contains(&Some(index)) {
                    // (to the millimetre, so that it settles)
                    let reach = under.map_or(0.0, |y| ((foot.y - y) / k * 1000.0).round() / 1000.0).max(0.0);
                    for end in ends.iter().zip(&mut ends_down).filter(|(end, _)| **end == Some(index)).map(|(_, down)| down) {
                        *end = reach;
                    }
                } else if under.is_none_or(|y| y < foot.y - POST_GAP * k) {
                    mask |= 1 << index;
                }
            }
            (mask, ends_down)
        })
        .collect()
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
        state.draft.clear();
        state.pending = None;
        state.handle = None;
        if state.handle_drag.take().is_some() {
            placed.shaping = false;
            placed.revision += 1;
        }
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
    state.pending = None;
    state.closing = false;
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
        state.draft.clear();
        state.level = None;
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
    if placed.dragging.is_none() {
        state.carry = None;
    }
    state.ghost = if state.free {
        let held = controller.transform_point(Vec3::NEG_Z * FREE_REACH_M);
        Some((held, root.transform_point(held)))
    } else {
        valid.map(|h| {
            // (a piece being moved: as far from the pointer as when it was picked up, turned as it has been since)
            let carried = state.carry.map_or(Vec3::ZERO, |(offset, yaw)| Quat::from_rotation_y(state.yaw - yaw) * offset);
            let at = to_tracking.transform_point3(h.point) + carried;
            (at, root.transform_point(at))
        })
    };
    state.ray = match hit {
        // (in the hand there is nothing to point at but a piece to select)
        _ if state.free && placed.pointed.is_none() => None,
        Some(h) => Some((origin, h.point, valid.is_some() || state.free)),
        None => Some((origin, origin + direction * MISS_LENGTH, false)),
    };

    // The handle of the selected drawn piece the pointer is on (the one held stays).
    let metres = placed.park_scale / root.scale.x;
    state.handle = state.handle_drag.or_else(|| {
        let item = placed.selected.and_then(|id| placed.items.iter().find(|p| p.id == id))?;
        item.handles(metres)
            .into_iter()
            .filter_map(|(handle, at)| {
                let at = root.transform_point(at) - origin;
                let along = at.dot(direction);
                let off = (at - direction * along).length();
                (along > 0.0 && off < HANDLE_REACH_M * root.scale.x).then_some((off, handle))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, handle)| handle)
    });

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
    // (a held handle takes the stick: it raises the point)
    let edit = edit.filter(|_| state.handle_drag.is_none());
    state.rise = if !more && ry.abs() > STICK_DEADZONE && ry.abs() > rx.abs() { ry * RISE_RATE * dt } else { 0.0 };
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
            let edit = edit
                .filter(|&e| e != Edit::Widen || kind.is_some_and(|k| k.width_axis().is_some()))
                .filter(|&e| e != Edit::Stretch || kind.is_some_and(|k| k.sized()));
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
                // (a drawn piece is turned and sized by its points; the stick raises the next)
                Some(Edit::Turn | Edit::Scale) if kind.drawn() => {}
                Some(Edit::Stretch) if !kind.sized() => {}
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
    // In the hand (free placement) it joins what the hand is near, at that
    // height, and stays in the air there.
    let held = state.free;
    if let Some((position, _)) = state.ghost {
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
                no_posts: 0,
ends_down: [0.0; 2],
                finish: None,
                tint: None,
                path: None,
            }),
        };
        if let Some(mut moving) = moving.filter(|m| !m.piece.kind.drawn()) {
            moving.piece.lift = 0.0;
            moving.position = position.to_array();
            moving.yaw = state.placed_yaw;
            let neighbours: Vec<_> = placed
                .items
                .iter()
                .filter(|p| p.piece.kind == moving.piece.kind && Some(p.id) != placed.dragging)
                .map(|p| p.footprint(metres))
                .collect();
            if let Some((place, yaw)) = snap::join(neighbours.iter().copied(), moving.footprint(metres), held) {
                state.ghost = Some((place, root.transform_point(place)));
                state.placed_yaw = yaw;
            } else if let Some(place) =
                Some(()).filter(|_| moving.piece.kind == PieceKind::Rail).and_then(|_| snap::join_ends(neighbours, moving.footprint(metres), held))
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
                let mut met = snap::meet(theirs.iter().copied(), position, state.placed_yaw, &own(moving.piece), reach, held);
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
                            met = snap::meet(theirs.iter().copied(), position, state.placed_yaw, &own(moving.piece), reach, held).or(met);
                        }
                    }
                }
                // How far above (or into) what is under it that leaves its base.
                let joined = met.and_then(|met| {
                    if held {
                        return Some((met, 0.0));
                    }
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
    // On a handle of the selected drawn piece, the grip holds that instead.
    if pressed(now.grip, previous.grip) && state.handle.is_some() && placed.dragging.is_none() {
        finish_edit(&mut placed);
        placed.checkpoint();
        placed.shaping = true;
        state.handle_drag = state.handle;
    } else if pressed(now.grip, previous.grip) {
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
    if let (Some(handle), Some(id)) = (state.handle_drag, placed.selected) {
        if now.grip {
            let to = cast_past(&world, &placed, &root, origin, direction, Some(id))
                .filter(|h| h.normal.y >= MIN_UP)
                .map(|h| to_tracking.transform_point3(h.point));
            let rise = state.rise;
            if let Some(item) = placed.get_mut(id) {
                let k = metres * item.scale;
                let to = to.map(|p| item.to_local(metres, p));
                shape(item, handle, to, rise / k);
            }
        } else {
            state.handle_drag = None;
            placed.shaping = false;
            placed.revision += 1;
            info!("BUILD shaped piece {id}");
        }
        return;
    }
    if let (Some(held), Some(id)) = (state.grip_hold, placed.selected) {
        state.grip_hold = Some(held + dt);
        if held + dt >= MOVE_HOLD_SECONDS && placed.dragging.is_none() {
            finish_edit(&mut placed);
            placed.checkpoint();
            placed.dragging = Some(id);
            if let Some(item) = placed.items.iter().find(|p| p.id == id) {
                state.yaw = item.yaw;
                // Where the pointer is on the ground with the piece out of its way.
                let under = Some(()).filter(|_| !state.free).and_then(|_| cast(&world, &placed, &root, origin, direction)).filter(|h| h.normal.y >= MIN_UP);
                state.carry = under.map(|h| ((Vec3::from_array(item.position) - to_tracking.transform_point3(h.point)).with_y(0.0), item.yaw));
                // (this frame too: it stays where it is)
                state.ghost = state.ghost.filter(|_| state.carry.is_none()).or(Some((Vec3::from_array(item.position), root.transform_point(Vec3::from_array(item.position)))));
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
    let drawing = placed.selected.is_none() && PieceKind::ALL[state.selected].drawn();
    // A drawn piece's handles: B takes a point out (or a bend), the stick's
    // click makes a point a corner, the trigger on a bend puts a point there.
    if let (Some(handle), Some(id)) = (state.handle, placed.selected) {
        let (b, click, trigger) = (pressed(now.b, previous.b), click, !more && pressed(now.trigger, previous.trigger));
        if b || click || trigger {
            placed.checkpoint();
            let changed = placed.get_mut(id).is_some_and(|item| reshape(item, handle, b, click, trigger));
            if !changed {
                placed.history.pop();
            }
            if changed {
                placed.revision += 1;
                state.handle = None;
                return;
            }
            if b {
                return;
            }
        }
    }
    // A drawn deck or pad: with its base down to the surface under it, or without (floating).
    if more && pressed(now.trigger, previous.trigger) {
        match placed.selected {
            Some(id) => {
                if placed.items.iter().any(|p| p.id == id && p.path.is_some() && p.piece.kind.floats()) {
                    placed.checkpoint();
                    if let Some(path) = placed.get_mut(id).and_then(|item| item.path.as_mut()) {
                        path.floating = !path.floating;
                        info!("BUILD piece {id} floating={}", path.floating);
                    }
                    placed.revision += 1;
                }
            }
            None if PieceKind::ALL[state.selected].floats() => {
                state.floating = !state.floating;
                info!("BUILD next drawn piece floating={}", state.floating);
            }
            None => {}
        }
        return;
    }
    // A deck's slope: straight, or eased into a transition.
    if more && pressed(now.b, previous.b) {
        if let Some(id) = placed.selected.filter(|&id| placed.items.iter().any(|p| p.id == id && p.piece.kind == PieceKind::Deck)) {
            placed.checkpoint();
            if let Some(path) = placed.get_mut(id).and_then(|item| item.path.as_mut()) {
                path.eased = !path.eased;
            }
            placed.revision += 1;
        }
        return;
    }
    // Drawing: B takes back the last point.
    if drawing && !state.draft.is_empty() && pressed(now.b, previous.b) {
        state.draft.pop();
        state.level = state.draft.last().map(|n| n.at[1]);
        return;
    }
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
            // A copy: the next piece to drop is this one over again (a drawn one is drawn again).
            if let Some(item) = placed.items.iter().find(|p| p.id == id && p.path.is_none()).cloned() {
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
    // Drawing: the trigger puts a point where the pointer is, A finishes the piece.
    state.pending = None;
    state.closing = false;
    if drawing {
        let kind = PieceKind::ALL[state.selected];
        let trigger = !more && pressed(now.trigger, previous.trigger);
        let finish = !more && pressed(now.a, previous.a);
        if let Some(at) = valid.map(|h| to_tracking.transform_point3(h.point)) {
            let first = state.draft.first().copied();
            let mut node = drawn::Node::new(at, at.y);
            match kind {
                // A bowl's lip is level with the surface its first point is on.
                PieceKind::Pool => {
                    let lip = first.map_or(at.y, |f| f.base);
                    node.at[1] = lip;
                    node.base = lip;
                }
                // A pad is level: the stick raises all of it.
                PieceKind::Pad => {
                    let floor = first.map_or(at.y, |f| f.base);
                    let top = (state.level.unwrap_or(floor + PAD_HEIGHT_M * metres) + state.rise).max(floor + 0.02);
                    state.level = Some(top);
                    for n in &mut state.draft {
                        n.at[1] = top;
                    }
                    node.at[1] = top;
                }
                // A rail or deck keeps the last point's height until the stick changes it.
                _ => {
                    let y = (state.level.unwrap_or(at.y) + state.rise).max(at.y);
                    if state.rise != 0.0 {
                        state.level = Some(y);
                    }
                    node.at[1] = y;
                    // On another piece's edge (a rail's end for a rail), it starts or ends there.
                    let rail = kind == PieceKind::RailPath;
                    let edge = placed
                        .items
                        .iter()
                        .flat_map(|p| p.edges(metres))
                        .filter(|e| (e.width == 0.0) == rail)
                        .map(|e| (e.at.xz().distance(at.xz()), e))
                        .filter(|(d, _)| *d < DRAW_SNAP_M)
                        .min_by(|a, b| a.0.total_cmp(&b.0));
                    if let Some((_, edge)) = edge {
                        let y = edge.at.y - if rail { drawn::RAIL_TOP * metres } else { 0.0 };
                        node.at = [edge.at.x, y, edge.at.z];
                        node.base = node.base.min(y);
                        state.join = Some((edge.at, Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * edge.out, edge.width));
                    }
                }
            }
            state.closing = kind.closed()
                && state.draft.len() >= kind.least_nodes()
                && first.is_some_and(|f| Vec3::from_array(f.at).xz().distance(at.xz()) < DRAW_SNAP_M);
            if !state.closing && state.draft.len() < drawn::MAX_NODES {
                state.pending = Some(node);
            }
        }
        if trigger && !state.closing {
            if let Some(node) = state.pending.take() {
                if kind != PieceKind::Pad && kind != PieceKind::Pool {
                    state.level = Some(node.at[1]);
                }
                state.draft.push(node);
                info!("BUILD drew point {} of a {}", state.draft.len(), kind.noun());
            }
        } else if (finish || (trigger && state.closing)) && state.draft.len() >= kind.least_nodes() {
            let first = state.draft[0];
            let origin = Vec3::new(first.at[0], first.base, first.at[2]);
            let mut path = draft_path(&state.draft, None, origin, metres);
            path.floating = state.floating && kind.floats();
            let piece = state.piece();
            let finish = Some(state.finish()).filter(|&f| f != kind.finishes()[0]);
            let id = placed.add(piece, origin, 0.0, 1.0, finish, state.tint());
            if let Some(item) = placed.get_mut(id) {
                item.path = Some(path);
            }
            info!("BUILD drew a {} of {} points as piece {id}", kind.noun(), state.draft.len());
            state.draft.clear();
            state.level = None;
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
        let lower = |k: PieceKind| k.noun();
        let pointed = placed.pointed.and_then(|id| placed.items.iter().find(|p| p.id == id));
        if placed.dragging.is_some() {
            next.right = vec![Row::new("Stick ↔", "turn it"), Row::new("Grip", "let go to put it down")];
        } else if touch.0.left_grip > PRESS {
            // The second set, while the left grip is held.
            if kind.sized() {
                next.left.push(Row::new("Stick ↕", format!("{its} {}", kind.size_name())));
            }
            if kind.width_axis().is_some() {
                next.left.push(Row::new("Stick ↔", format!("{its} width")));
            }
            next.left.push(Row::new("Grip", "let go for the first set"));
            next.right = vec![Row::new("Stick ↔", "colour: hue"), Row::new("Stick ↕", "colour: lighter / darker")];
            if tinted {
                next.right.push(Row::new("Stick click", format!("{}'s own colour", finish.name())));
            }
            let base = |floating: bool| if floating { "base: down to the surface" } else { "base: none (it floats)" };
            match editing {
                Some(item) if item.piece.kind.floats() => {
                    if let Some(path) = &item.path {
                        next.right.push(Row::new("Trigger", base(path.floating)));
                    }
                }
                None if chosen.floats() => next.right.push(Row::new("Trigger", base(state.floating))),
                _ => {}
            }
            if let Some(item) = editing {
                next.right.push(Row::new("A", if item.hidden { "show it again" } else { "hide it (still skated)" }));
                if let (PieceKind::Deck, Some(path)) = (item.piece.kind, &item.path) {
                    next.right.push(Row::new("B", if path.eased { "slope: straight" } else { "slope: curved" }));
                }
            }
        } else if let Some(item) = editing {
            next.left = vec![
                Row::new("Stick ↕", format!("its material: {}", finish.name())),
                Row::new("Grip (hold)", "more: stretch, colour, hide"),
                Row::new("Trigger", "undo"),
                Row::new("Stick ↔", "done; choose a new piece"),
            ];
            next.right = match (state.handle_drag.or(state.handle), &item.path) {
                (Some(Grab::Node(i)), Some(path)) if state.handle_drag.is_some() => {
                    let mut rows = vec![Row::new("Grip", "let go to leave it there")];
                    match item.piece.kind {
                        PieceKind::Pool => {}
                        PieceKind::Pad => rows.push(Row::new("Stick ↕", "raise / lower the pad")),
                        _ => rows.push(Row::new("Stick ↕", "raise / lower this point")),
                    }
                    let _ = (i, path);
                    rows
                }
                (Some(Grab::Bend(_)), Some(_)) if state.handle_drag.is_some() => vec![Row::new("Grip", "let go to leave it there")],
                (Some(Grab::Node(i)), Some(path)) => {
                    let mut rows = vec![
                        Row::new("Grip (hold)", "move this point"),
                        Row::new("Stick click", if path.nodes.get(i).is_some_and(|n| n.corner) { "make it smooth" } else { "make it a corner" }),
                    ];
                    if path.nodes.len() > item.piece.kind.least_nodes() {
                        rows.push(Row::new("B", "take this point out"));
                    }
                    rows.push(Row::new("A", "done"));
                    rows
                }
                (Some(Grab::Bend(i)), Some(path)) => {
                    let mut rows = vec![Row::new("Grip (hold)", "bend this stretch")];
                    if path.nodes.len() < drawn::MAX_NODES {
                        rows.push(Row::new("Trigger", "add a point here"));
                    }
                    if path.nodes.get(i).is_some_and(|n| n.bend != [0.0; 2]) {
                        rows.push(Row::new("B", "straighten it"));
                    }
                    rows.push(Row::new("A", "done"));
                    rows
                }
                (_, Some(_)) => vec![
                    Row::new("Pointer", "on a dot: move or bend it"),
                    Row::new("Stick ↔", format!("turn this {}", lower(item.piece.kind))),
                    Row::new("Stick ↕", "make it bigger / smaller"),
                    Row::new("Grip (hold)", "move it"),
                    Row::new("B", match pointed.filter(|p| p.id != item.id) {
                        Some(other) => format!("remove that {}", lower(other.piece.kind)),
                        None => "remove it".to_string(),
                    }),
                    Row::new("A / Trigger", "done"),
                ],
                _ => vec![
                Row::new("Stick ↔", format!("turn this {}", lower(item.piece.kind))),
                Row::new("Stick ↕", "make it bigger / smaller"),
                Row::new("Grip (hold)", "move it"),
                Row::new("Stick click", "copy it"),
                Row::new("B", match pointed.filter(|p| p.id != item.id) {
                    Some(other) => format!("remove that {}", lower(other.piece.kind)),
                    None => "remove it".to_string(),
                }),
                Row::new("A / Trigger", "done"),
                ],
            };
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
            if chosen.drawn() {
                let enough = state.draft.len() >= chosen.least_nodes();
                next.right = vec![Row::new(
                    "Trigger",
                    match (state.closing, state.pending.is_some(), state.draft.is_empty()) {
                        (true, ..) => "close the outline",
                        (_, true, true) => "put its first point here",
                        (_, true, false) => "put the next point here",
                        _ if state.draft.len() >= drawn::MAX_NODES => "that's every point: A to finish",
                        _ => "point at a surface",
                    },
                )];
                match chosen {
                    PieceKind::Pool => {}
                    PieceKind::Pad => next.right.push(Row::new("Stick ↕", "raise / lower the pad")),
                    _ => next.right.push(Row::new("Stick ↕", "raise / lower the next point")),
                }
                if enough {
                    next.right.push(Row::new("A", format!("finish the {}", lower(chosen))));
                }
                if !state.draft.is_empty() {
                    next.right.push(Row::new("B", "take back the last point"));
                } else if let Some(item) = pointed {
                    next.right.push(Row::new("Grip", format!("select this {}", lower(item.piece.kind))));
                    next.right.push(Row::new("B", format!("remove this {}", lower(item.piece.kind))));
                }
            } else {
            next.right = vec![
                Row::new("Stick ↔", "turn it"),
                Row::new("Stick ↕", "make it bigger / smaller"),
                Row::new(
                    "A / Trigger",
                    match (state.free, state.ghost.is_some()) {
                        (true, _) if state.join.is_some() => "join it on there",
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

/// The points being drawn (tracking space), with the one the pointer would
/// add, as a path from `origin` in the piece's own metres.
fn draft_path(draft: &[drawn::Node], pending: Option<drawn::Node>, origin: Vec3, metres: f32) -> drawn::Path {
    let nodes = draft
        .iter()
        .chain(pending.iter())
        .map(|n| drawn::Node::new((Vec3::from_array(n.at) - origin) / metres, (n.base - origin.y) / metres))
        .collect();
    drawn::Path { nodes, ..default() }
}

/// Moves a handle of a drawn piece: a point to `to` (the piece's own
/// metres; none: the pointer is on nothing) and up by `rise`, or a bend so
/// its stretch's middle is at `to`.
fn shape(item: &mut Placed, handle: Grab, to: Option<Vec3>, rise: f32) {
    let kind = item.piece.kind;
    let Some(path) = item.path.as_mut() else { return };
    match handle {
        Grab::Node(i) => {
            let Some(node) = path.nodes.get_mut(i) else { return };
            if let Some(to) = to {
                node.at[0] = to.x;
                node.at[2] = to.z;
                if kind != PieceKind::Pool {
                    node.base = to.y;
                }
            }
            match kind {
                // (level with the surface its first point is on)
                PieceKind::Pool => {}
                PieceKind::Pad => {
                    let floor = path.nodes.iter().map(|n| n.base).fold(f32::MAX, f32::min);
                    let top = (path.nodes[i].at[1] + rise).max(floor + 0.02);
                    for n in &mut path.nodes {
                        n.at[1] = top;
                    }
                }
                _ => node.at[1] = (node.at[1] + rise).max(node.base),
            }
        }
        Grab::Bend(i) => {
            if let (Some(to), true) = (to, i < path.stretches(kind.closed())) {
                let straight = path.straight_middle(i, kind.closed());
                path.nodes[i].bend = [to.x - straight.x, to.z - straight.z];
            }
        }
    }
}

/// A button on a handle: B takes a point out (or straightens a bend), the
/// stick's click turns a point between smooth and a corner, the trigger on
/// a bend puts a point there. Whether anything changed.
fn reshape(item: &mut Placed, handle: Grab, b: bool, click: bool, trigger: bool) -> bool {
    let kind = item.piece.kind;
    let Some(path) = item.path.as_mut() else { return false };
    match handle {
        Grab::Node(i) if b && path.nodes.len() > kind.least_nodes() && i < path.nodes.len() => {
            path.nodes.remove(i);
        }
        Grab::Node(i) if click && i < path.nodes.len() => path.nodes[i].corner = !path.nodes[i].corner,
        Grab::Bend(i) if b && i < path.nodes.len() && path.nodes[i].bend != [0.0; 2] => path.nodes[i].bend = [0.0; 2],
        Grab::Bend(i) if trigger && i < path.stretches(kind.closed()) && path.nodes.len() < drawn::MAX_NODES => {
            let (a, next) = (path.nodes[i], path.nodes[(i + 1) % path.nodes.len()]);
            let middle = path.middle(i, kind.closed());
            path.nodes[i].bend = [0.0; 2];
            path.nodes.insert(i + 1, drawn::Node::new(middle, (a.base + next.base) * 0.5));
        }
        _ => return false,
    }
    true
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
fn cast(world: &ArWorld, placed: &PlacedPieces, root: &Transform, origin: Vec3, direction: Vec3) -> Option<Hit> {
    cast_past(world, placed, root, origin, direction, None)
}

/// [`cast`], through the piece `past` too.
fn cast_past(
    world: &ArWorld,
    placed: &PlacedPieces,
    root: &Transform,
    origin: Vec3,
    direction: Vec3,
    past: Option<u64>,
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
    // A bowl being moved (or seen past) has left the surface open where it was until the
    // collision is made again: the pointer lands on that surface still.
    for (_, [a, b, c, d]) in world.holes.iter().filter(|(id, _)| Some(*id) == placed.dragging || Some(*id) == past) {
        consider(ray_triangle(origin, direction, *a, *b, *c), (*b - *a).cross(*c - *a), None);
        consider(ray_triangle(origin, direction, *a, *c, *d), (*c - *a).cross(*d - *a), None);
    }
    for item in placed.items.iter().filter(|p| Some(p.id) != placed.dragging && Some(p.id) != past) {
        let at = item.world_transform(root, placed.park_scale);
        for q in item.geometry().all_quads() {
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
    /// (the last: in wood, for the kinds that are then built of sheets)
    meshes: HashMap<(PieceKind, u32, u32, u32, bool), Handle<Mesh>>,
    /// What is drawn over the pieces and not ridden (`dress::details`).
    detail: Option<Handle<StandardMaterial>>,
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
    fn mesh(&mut self, piece: Piece, finish: Finish, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
        let wooden = finish == Finish::Wood && piece.kind.plywood();
        self.meshes
            .entry((piece.kind, piece.size.to_bits(), piece.width.to_bits(), piece.lift.to_bits(), wooden))
            .or_insert_with(|| meshes.add(piece_mesh(&piece.geometry(), finish)))
            .clone()
    }

    /// The material of everything drawn over a piece: coloured by its corners.
    fn detail(&mut self, materials: &mut Assets<StandardMaterial>) -> Handle<StandardMaterial> {
        self.detail
            .get_or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::WHITE,
                    perceptual_roughness: 0.45,
                    metallic: 0.15,
                    // (it lies a few millimetres over the piece, which in a miniature is nothing: it wins)
                    depth_bias: 8.0,
                    ..default()
                })
            })
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
        // A ramp in wood is plywood: the photograph's grain, in birch's colour.
        let plywood = finish == Finish::Wood && kind.plywood() && tint.is_none();
        let neutral = tint.is_some() || plywood;
        let texture = look.texture.map(|(bytes, _)| {
            self.textures
                .entry((finish, neutral))
                .or_insert_with(|| images.add(tiling_texture(bytes, neutral)))
                .clone()
        });
        // A colour of the player's goes over the photograph's light and dark
        // (kept at `NEUTRAL_LEVEL`, which the colour makes up for).
        let color = match tint {
            Some(tint) if texture.is_some() => (tint.color().to_linear() * (1.0 / NEUTRAL_LEVEL)).into(),
            Some(tint) => tint.color(),
            None if plywood => (PLYWOOD.to_linear() * (1.0 / NEUTRAL_LEVEL)).into(),
            None => look.color.unwrap_or(kind.color()),
        };
        let material = materials.add(StandardMaterial {
            base_color: color,
            base_color_texture: texture,
            // Texture coordinates are metres ([`PieceGeometry::uvs`]).
            // (a sheet of plywood's grain is finer than a plank's)
            uv_transform: bevy::math::Affine2::from_scale(Vec2::splat(1.0 / look.texture.map_or(1.0, |(_, metres)| if plywood { metres * 0.6 } else { metres }))),
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

/// Birch plywood, as a ramp is built of.
const PLYWOOD: Color = Color::srgb(0.8, 0.69, 0.52);

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

/// A mesh from corners, three to a triangle.
fn dressed_mesh(dressed: dress::Dressed, textured: bool) -> Mesh {
    let mesh = Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, dressed.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, dressed.normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, dressed.colors);
    if textured { mesh.with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, dressed.uvs) } else { mesh }
}

/// What is drawn over a piece and not ridden (`dress::details`), if it has any.
fn detail_mesh(geometry: &PieceGeometry, kind: PieceKind, finish: Finish) -> Option<Mesh> {
    dress::details(geometry, finish == Finish::Wood && kind.plywood()).map(|dressed| dressed_mesh(dressed, false))
}

/// A piece's mesh. In wood, a ramp is in sheets (`dress::sheets`).
fn piece_mesh(geometry: &PieceGeometry, finish: Finish) -> Mesh {
    if let Some(sheets) = Some(()).filter(|_| finish == Finish::Wood).and_then(|_| dress::sheets(geometry)) {
        return dressed_mesh(sheets, true);
    }
    let seen = |index: &usize| !geometry.unseen.contains(index);
    let positions: Vec<[f32; 3]> = geometry
        .all_quads()
        .enumerate()
        .filter(|(i, _)| seen(i))
        .flat_map(|(_, q)| [0, 2, 1, 0, 3, 2].map(|i| q[i].to_array()))
        .collect();
    let normals: Vec<[f32; 3]> = geometry
        .normals()
        .iter()
        .enumerate()
        .filter(|(i, _)| seen(i))
        .flat_map(|(_, n)| [0, 2, 1, 0, 3, 2].map(|i| n[i].to_array()))
        .collect();
    let uvs: Vec<[f32; 2]> = geometry
        .uvs()
        .iter()
        .enumerate()
        .filter(|(i, _)| seen(i))
        .flat_map(|(_, uv)| [0, 2, 1, 0, 3, 2].map(|i| uv[i].to_array()))
        .collect();
    let mesh = Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        bevy::asset::RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
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
struct PlacedVisual(u64, Piece, bool, Finish, Option<Tint>, u64);
/// The piece about to be dropped: its shape, and whether it is in the hand.
#[derive(Component)]
struct GhostVisual(Option<(Piece, bool)>);
/// What marks the piece being edited or pointed at.
#[derive(Component)]
struct EditMark(Mark, Option<(Piece, u64)>);

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
    commands.spawn((DraftVisual(None), MeshMaterial3d(assets.ghost.clone().unwrap_or_default()), Transform::default(), Visibility::Hidden));
    let dot = meshes.add(Sphere::new(1.0));
    let dots = [Color::WHITE, Color::srgb(1.0, 0.55, 0.15), Color::srgb(0.3, 1.0, 0.9), Color::srgb(1.0, 0.9, 0.2), Color::srgba(0.3, 1.0, 0.5, 0.6)]
        .map(|c| materials.add(StandardMaterial { alpha_mode: if c.alpha() < 1.0 { AlphaMode::Blend } else { AlphaMode::Opaque }, ..unlit(c) }));
    commands.insert_resource(HandleDots(dots));
    for i in 0..2 * drawn::MAX_NODES + 1 {
        commands.spawn((HandleDot(i, None), Mesh3d(dot.clone()), Transform::default(), Visibility::Hidden));
    }
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
            Some(item)
                if item.piece == visual.1
                    && item.hidden == visual.2
                    && item.finish() == visual.3
                    && item.tint == visual.4
                    && item.shape_key() == visual.5 =>
            {
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
        // (a drawn piece's mesh is its own)
        let mesh = match (&item.path, item.own_posts()) {
            (None, false) => assets.mesh(item.piece, item.finish(), &mut meshes),
            // (so is a rail's with posts left out)
            _ => meshes.add(piece_mesh(&item.geometry(), item.finish())),
        };
        let visual = commands
            .spawn((
                PlacedVisual(item.id, item.piece, item.hidden, item.finish(), item.tint, item.shape_key()),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material),
                item.world_transform(&root, placed.park_scale),
                if item.hidden && !see_hidden { Visibility::Hidden } else { Visibility::Inherited },
            ))
            .id();
        // Sunk into a surface: from the side that surface hides it (its own outside does, drawn
        // into depth only).
        // What is drawn over it: a ramp's coping, and in wood its seams, edges and plate.
        if let Some(detail) = Some(()).filter(|_| !item.hidden && item.path.is_none()).and_then(|_| detail_mesh(&item.geometry(), item.piece.kind, item.finish())) {
            commands.spawn((Mesh3d(meshes.add(detail)), MeshMaterial3d(assets.detail(&mut materials)), Transform::default(), ChildOf(visual)));
        }
        if item.piece.kind.sunk() && !item.hidden {
            let occluder = assets.occluder.get_or_insert_with(|| occluders.add(Occluder::default())).clone();
            commands.spawn((Mesh3d(mesh), MeshMaterial3d(occluder), Transform::default(), ChildOf(visual)));
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
    match state.ghost.zip(root).filter(|_| mode.active && placed.dragging.is_none() && placed.selected.is_none() && !piece.kind.drawn()) {
        Some(((_, point), root)) => {
            if ghost_piece.0 != Some((piece, state.free)) {
                ghost_piece.0 = Some((piece, state.free));
                let material = if state.free { assets.ghost_free.clone() } else { assets.ghost.clone() };
                commands.entity(ghost_entity).insert((
                    Mesh3d(assets.mesh(piece, Finish::Paint, &mut meshes)),
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
                Mesh3d(assets.mesh(Piece::new(kind), finish, &mut meshes)),
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

/// The piece being drawn, as far as it goes.
#[derive(Component)]
struct DraftVisual(Option<u64>);
/// One of the dots on a drawn piece's points and bends (its place in the
/// pool, and which of [`HandleDots`] it wears).
#[derive(Component)]
struct HandleDot(usize, Option<usize>);
/// The dots' materials: a point, a corner, a bend, the one pointed at, the
/// point about to be placed.
#[derive(Resource)]
struct HandleDots([Handle<StandardMaterial>; 5]);

/// A drawn piece's dots (the selected piece's handles, or the points placed
/// so far of the one being drawn) and the preview of the one being drawn.
#[allow(clippy::type_complexity)]
fn show_drawn(
    mode: Res<BuildMode>,
    state: Res<BuildState>,
    placed: Res<PlacedPieces>,
    rig: Res<Rig>,
    dots: Option<Res<HandleDots>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
    mut draft: Query<(Entity, &mut DraftVisual, &mut Transform, &mut Visibility), Without<HandleDot>>,
    mut pool: Query<(Entity, &mut HandleDot, &mut Transform, &mut Visibility), Without<DraftVisual>>,
) {
    let (Some(dots), Ok((entity, mut visual, mut transform, mut visibility))) = (dots, draft.single_mut()) else { return };
    let root = rig.root_transform().filter(|_| mode.active);
    let kind = PieceKind::ALL[state.selected];
    let drawing = placed.selected.is_none() && kind.drawn();
    // (where in tracking space, which material, how big)
    let mut shown: Vec<(Vec3, usize, f32)> = Vec::new();
    let mut preview = None;
    if let Some(root) = root {
        let metres = placed.park_scale / root.scale.x;
        if drawing {
            let up = if kind == PieceKind::RailPath { Vec3::Y * drawn::RAIL_TOP * metres } else { Vec3::ZERO };
            shown.extend(state.draft.iter().enumerate().map(|(i, n)| {
                (Vec3::from_array(n.at) + up, if i == 0 && state.closing { 3 } else { 0 }, 1.0)
            }));
            shown.extend(state.pending.iter().map(|n| (Vec3::from_array(n.at) + up, 4, 1.0)));
            if let Some(first) = state.draft.first() {
                let origin = Vec3::new(first.at[0], first.base, first.at[2]);
                let mut path = draft_path(&state.draft, state.pending, origin, metres);
                path.floating = state.floating && kind.floats();
                // (a bowl is shown by its points alone: filled in as it is drawn, its preview
                // jumps about with every point)
                if path.nodes.len() >= kind.least_nodes() && kind != PieceKind::Pool {
                    preview = Some((origin, path));
                }
            }
        } else if let Some(item) = placed.selected.and_then(|id| placed.items.iter().find(|p| p.id == id)) {
            shown.extend(item.handles(metres).into_iter().map(|(handle, at)| {
                let corner = matches!(handle, Grab::Node(i) if item.path.as_ref().is_some_and(|p| p.nodes[i].corner));
                let (material, size) = match handle {
                    _ if state.handle == Some(handle) => (3, 1.5),
                    Grab::Node(_) if corner => (1, 1.0),
                    Grab::Node(_) => (0, 1.0),
                    Grab::Bend(_) => (2, 0.7),
                };
                (at, material, size)
            }));
        }
        match preview {
            Some((origin, path)) => {
                let key = path.key() ^ state.selected as u64 ^ (state.piece().width.to_bits() as u64) << 32;
                if visual.0 != Some(key) {
                    visual.0 = Some(key);
                    commands.entity(entity).insert(Mesh3d(meshes.add(piece_mesh(&path.geometry(state.piece()), Finish::Paint))));
                }
                *transform = Transform { translation: root.transform_point(origin), rotation: root.rotation, scale: Vec3::splat(placed.park_scale) };
                visibility.set_if_neq(Visibility::Visible);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    } else {
        visibility.set_if_neq(Visibility::Hidden);
    }
    for (entity, mut dot, mut transform, mut visibility) in &mut pool {
        let (Some(&(at, material, size)), Some(root)) = (shown.get(dot.0), root) else {
            visibility.set_if_neq(Visibility::Hidden);
            continue;
        };
        if dot.1 != Some(material) {
            dot.1 = Some(material);
            commands.entity(entity).insert(MeshMaterial3d(dots.0[material].clone()));
        }
        *transform = Transform::from_translation(root.transform_point(at)).with_scale(Vec3::splat(HANDLE_DOT_M * size * root.scale.x));
        visibility.set_if_neq(Visibility::Visible);
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
                // (a drawn piece being reshaped shows its handles, not a shell over them)
                let item = if mark.0 == Mark::Selected { selected.filter(|item| item.path.is_none()) } else { pointed };
                item.map(|item| {
                    if mark.1 != Some((item.piece, item.shape_key())) {
                        mark.1 = Some((item.piece, item.shape_key()));
                        let mesh = match (&item.path, item.own_posts()) {
                            (None, false) => assets.mesh(item.piece, Finish::Paint, &mut meshes),
                            _ => meshes.add(piece_mesh(&item.geometry(), Finish::Paint)),
                        };
                        commands.entity(entity).insert(Mesh3d(mesh));
                    }
                    // A little bigger than the piece, about its middle.
                    let at = item.world_transform(&root, placed.park_scale);
                    let (lo, hi) = item.geometry().bounds();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xr::touch::TouchState;

    #[test]
    fn a_bowl_being_moved_can_be_put_back_where_it_was() {
        // The floor is open over the bowl (piece 7): nothing of it is left there to point at.
        let hole = [Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 0.0, -2.0), Vec3::new(2.0, 0.0, 2.0), Vec3::new(-2.0, 0.0, 2.0)];
        let world = ArWorld { holes: vec![(7, hole)], ..default() };
        let mut placed = PlacedPieces::default();
        let (from, down) = (Vec3::new(0.5, 3.0, -0.5), Vec3::NEG_Y);
        assert!(cast(&world, &placed, &Transform::IDENTITY, from, down).is_none(), "the hole, while the bowl is in it");
        // Picked up, the pointer lands on the surface it was sunk into, facing up.
        placed.dragging = Some(7);
        let hit = cast(&world, &placed, &Transform::IDENTITY, from, down).expect("the surface where the bowl was");
        assert!(hit.point.distance(Vec3::new(0.5, 0.0, -0.5)) < 1e-4 && hit.normal.y > 0.99 && hit.piece.is_none());
        // Another bowl's hole stays a hole.
        placed.dragging = Some(8);
        assert!(cast(&world, &placed, &Transform::IDENTITY, from, down).is_none());
    }

    /// The wooden ramps as they are drawn, saved as a picture to look at:
    /// `SK8_PIECE_SHOT=/path/ramps.png cargo test ... a_picture_of_the_wooden_ramps -- --ignored`
    /// (`SK8_PIECE_VIEW=x,y,z` moves the camera; `SK8_PIECE_KINDS=HalfPipe,...` chooses the pieces).
    #[test]
    #[ignore = "needs a GPU; writes a picture"]
    fn a_picture_of_the_wooden_ramps() {
        use bevy::camera::RenderTarget;
        use bevy::render::render_resource::TextureFormat;
        use bevy::render::view::screenshot::{Screenshot, save_to_disk};
        let Some(path) = std::env::var_os("SK8_PIECE_SHOT") else { return };
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .build()
                .disable::<bevy::winit::WinitPlugin>()
                .set(WindowPlugin { primary_window: None, exit_condition: bevy::window::ExitCondition::DontExit, ..default() }),
        );
        app.init_resource::<PieceAssets>();
        let image = Image::new_target_texture(1600, 1000, TextureFormat::Rgba8UnormSrgb, None);
        let image = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        let target = image.clone();
        app.add_systems(Startup, move |mut commands: Commands, mut assets: ResMut<PieceAssets>, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>, mut images: ResMut<Assets<Image>>| {
            let view: Vec<f32> = std::env::var("SK8_PIECE_VIEW").ok().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or_default();
            let eye = if view.len() == 3 { Vec3::new(view[0], view[1], view[2]) } else { Vec3::new(9.0, 6.5, 11.0) };
            commands.spawn((
                Camera3d::default(),
                Camera { clear_color: ClearColorConfig::Custom(Color::srgb(0.8, 0.8, 0.82)), ..default() },
                RenderTarget::Image(target.clone().into()),
                AmbientLight { brightness: 500.0, ..default() },
                Transform::from_translation(eye).looking_at(Vec3::new(0.0, 0.6, 0.0), Vec3::Y),
            ));
            commands.spawn((DirectionalLight { illuminance: 9000.0, shadows_enabled: true, ..default() }, Transform::from_xyz(4.0, 9.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y)));
            commands.spawn((Mesh3d(meshes.add(Plane3d::default().mesh().size(60.0, 60.0))), MeshMaterial3d(materials.add(Color::srgb(0.75, 0.75, 0.76))), Transform::from_xyz(0.0, -0.002, 0.0)));
            let wanted = std::env::var("SK8_PIECE_KINDS").unwrap_or_else(|_| "HalfPipe,QuarterPipe,Kicker".into());
            let kinds: Vec<PieceKind> = PieceKind::ALL.iter().copied().filter(|k| wanted.split(',').any(|w| w == format!("{k:?}"))).collect();
            let finish = if std::env::var_os("SK8_PIECE_CONCRETE").is_some() { Finish::Concrete } else { Finish::Wood };
            let mut x = -(kinds.len() as f32 - 1.0) * 3.2;
            for kind in kinds {
                let piece = Piece::new(kind);
                let at = Transform::from_xyz(x, 0.0, 0.0).with_rotation(Quat::from_rotation_y(0.5));
                x += 6.4;
                let visual = commands.spawn((Mesh3d(assets.mesh(piece, finish, &mut meshes)), MeshMaterial3d(assets.solid(kind, finish, None, &mut materials, &mut images)), at)).id();
                if let Some(detail) = detail_mesh(&piece.geometry(), kind, finish) {
                    commands.spawn((Mesh3d(meshes.add(detail)), MeshMaterial3d(assets.detail(&mut materials)), Transform::default(), ChildOf(visual)));
                }
            }
        });
        app.finish();
        app.cleanup();
        for _ in 0..40 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.world_mut().spawn(Screenshot::image(image)).observe(save_to_disk(std::path::PathBuf::from(path)));
        for _ in 0..40 {
            app.update();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Build mode run by hand: a 40 m floor, the right controller 1.5 m over its middle.
    struct Bench {
        world: World,
        schedule: Schedule,
    }

    impl Bench {
        fn new(kind: PieceKind) -> Self {
            let mut world = World::new();
            let floor = [Vec3::new(-20.0, 0.0, -20.0), Vec3::new(20.0, 0.0, -20.0), Vec3::new(20.0, 0.0, 20.0), Vec3::new(-20.0, 0.0, 20.0)];
            world.insert_resource(ArWorld { revision: 1, room_quads: vec![floor], ..default() });
            world.insert_resource(Rig { scale: 1.0, world_origin: Vec3::ZERO, anchor: Some((Vec3::ZERO, 0.0)) });
            world.insert_resource(BuildMode { active: true });
            world.insert_resource(BuildState { selected: PieceKind::ALL.iter().position(|&k| k == kind).unwrap(), ..default() });
            world.insert_resource(Time::<Real>::default());
            world.init_resource::<LatestTouch>();
            world.init_resource::<PlacedPieces>();
            world.init_resource::<SceneShapes>();
            world.init_resource::<SceneCapture>();
            world.init_resource::<ExternalPad>();
            world.init_resource::<Messages<TeleportSkater>>();
            world.spawn((Hand::Right, Transform::from_xyz(0.0, 1.5, 0.0)));
            let mut schedule = Schedule::default();
            schedule.add_systems(build_controls);
            Self { world, schedule }
        }

        /// Point the controller at `at`.
        fn aim(&mut self, at: Vec3) {
            let mut hands = self.world.query::<(&Hand, &mut Transform)>();
            for (_, mut hand) in hands.iter_mut(&mut self.world) {
                *hand = Transform::from_xyz(0.0, 1.5, 0.0).looking_at(at, Vec3::Y);
            }
        }

        /// `frames` frames (72 a second) with the controls as `touch` leaves them.
        fn hold(&mut self, frames: usize, touch: impl Fn(&mut TouchState)) {
            for _ in 0..frames {
                let mut state = TouchState::default();
                touch(&mut state);
                self.world.resource_mut::<LatestTouch>().0 = state;
                self.world.resource_mut::<Time<Real>>().advance_by(std::time::Duration::from_secs_f32(1.0 / 72.0));
                self.schedule.run(&mut self.world);
            }
        }

        fn tap(&mut self, touch: impl Fn(&mut TouchState)) {
            self.hold(2, touch);
            self.hold(2, |_| {});
        }

        fn state(&self) -> &BuildState {
            self.world.resource::<BuildState>()
        }

        fn placed(&self) -> &PlacedPieces {
            self.world.resource::<PlacedPieces>()
        }
    }

    #[test]
    fn the_first_point_of_a_ramp_is_raised_with_the_stick_before_it_is_put_down() {
        let mut bench = Bench::new(PieceKind::Deck);
        bench.aim(Vec3::new(0.0, 0.0, -3.0));
        bench.hold(72, |t| t.right_stick = [0.0, 1.0]);
        let pending = bench.state().pending.expect("a point to put down");
        assert!((pending.at[1] - RISE_RATE).abs() < 0.01 && pending.base.abs() < 1e-4, "{pending:?}");
        bench.tap(|t| t.right_trigger = 1.0);
        assert!((bench.state().draft[0].at[1] - RISE_RATE).abs() < 0.01);
        // The next starts at that height; finished, the ramp begins raised over its surface.
        bench.aim(Vec3::new(0.0, 0.0, -7.0));
        bench.hold(2, |_| {});
        bench.tap(|t| t.right_trigger = 1.0);
        bench.tap(|t| t.a = true);
        let item = &bench.placed().items[0];
        let nodes = &item.path.as_ref().unwrap().nodes;
        assert!(item.position[1].abs() < 1e-4 && (nodes[0].at[1] - RISE_RATE).abs() < 0.01 && (nodes[1].at[1] - RISE_RATE).abs() < 0.01, "{item:?}");
    }

    #[test]
    fn a_drawn_ramp_picked_up_stays_under_the_pointer_where_it_was_taken() {
        let mut bench = Bench::new(PieceKind::Deck);
        for (z, stick) in [(-3.0, 0.0), (-7.0, 1.0)] {
            bench.aim(Vec3::new(0.0, 0.0, z));
            bench.hold(72, |t| t.right_stick = [0.0, stick]);
            bench.tap(|t| t.right_trigger = 1.0);
        }
        bench.tap(|t| t.a = true);
        let before = bench.placed().items[0].clone();
        // Pointing at its top a metre along and off its middle line (clear of its dots), the grip held.
        let on = |x: f32, z: f32| Vec3::new(x, 0.0, z);
        bench.aim(Vec3::new(0.7, 0.06, -4.0));
        bench.hold(2, |_| {});
        assert_eq!((bench.placed().pointed, bench.state().handle), (Some(1), None));
        bench.hold(36, |t| t.right_grip = 1.0);
        assert_eq!(bench.placed().dragging, Some(1));
        assert!(Vec3::from_array(bench.placed().items[0].position).distance(Vec3::from_array(before.position)) < 1e-3, "it jumped when picked up: {:?}", bench.placed().items[0].position);
        // Where the pointer goes on the floor, it goes as far; back again, it is back.
        let start = bench.state().ghost.unwrap().0 - Vec3::from_array(before.position);
        let _ = start;
        let from = {
            // (the pointer's place on the floor behind the ramp, which it is held by)
            let hand = Vec3::new(0.0, 1.5, 0.0);
            let aim = (Vec3::new(0.7, 0.06, -4.0) - hand).normalize();
            hand + aim * (1.5 / -aim.y)
        };
        for to in [on(2.0, -5.0), on(3.0, -2.0), on(-4.0, -6.0), from, from + Vec3::X * 0.05, from] {
            bench.aim(to);
            bench.hold(3, |t| t.right_grip = 1.0);
            let moved = Vec3::from_array(bench.placed().items[0].position) - Vec3::from_array(before.position);
            assert!(moved.distance(to - from) < 2e-3, "pointer moved {}, the ramp {moved}", to - from);
        }
        bench.hold(3, |_| {});
        let after = &bench.placed().items[0];
        assert_eq!(bench.placed().dragging, None);
        assert!(Vec3::from_array(after.position).distance(Vec3::from_array(before.position)) < 2e-3);
        assert_eq!((&after.path, after.yaw, after.piece), (&before.path, before.yaw, before.piece), "its shape is as it was");
    }

    #[test]
    fn a_rail_has_no_posts_over_a_gap_but_the_ones_at_its_ends() {
        let item = |id, kind, position: [f32; 3]| Placed {
            id,
            piece: Piece::new(kind),
            position,
            yaw: 0.0,
            scale: 1.0,
            hidden: false,
            no_posts: 0,
            ends_down: [0.0; 2],
            finish: None,
            tint: None,
            path: None,
        };
        // A floor 40 m square, open over the bowl as the collision makes it (its hole is the bowl's own to fill).
        let (lo, hi) = Piece::new(PieceKind::Bowl).geometry().footprint();
        let strip = |x0: f32, z0: f32, x1: f32, z1: f32| [Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z0), Vec3::new(x1, 0.0, z1), Vec3::new(x0, 0.0, z1)];
        let floor = vec![strip(-20.0, -20.0, 20.0, lo.y), strip(-20.0, hi.y, 20.0, 40.0), strip(-20.0, lo.y, lo.x, hi.y), strip(hi.x, lo.y, 20.0, hi.y)];
        let world = ArWorld { revision: 1, room_quads: floor, ..default() };
        // A rail (its two posts 2.4 m either side of its middle) with one end over a bowl, one well
        // clear, and one put up 2 m in the air in free placement.
        let bowl = item(1, PieceKind::Bowl, [0.0, 0.0, 0.0]);
        let over = item(2, PieceKind::Rail, [2.4, 0.0, 0.0]);
        let clear = item(3, PieceKind::Rail, [0.0, 0.0, 9.0]);
        let high = item(4, PieceKind::Rail, [0.0, 2.0, -9.0]);
        let mut placed = PlacedPieces { items: vec![bowl, over, clear, high], ..default() };
        let posts = posts_over_gaps(&world, &placed, &Transform::IDENTITY);
        // Both of a straight rail's posts are at its ends: they stay, and reach down to what is under them.
        assert!(posts.iter().all(|(mask, _)| *mask == 0), "{posts:?}");
        assert_eq!(posts[2].1, [0.0; 2]);
        assert!(posts[1].1[0] > 0.5 && posts[1].1[1] == 0.0, "the end over the bowl stands on the bowl's floor: {:?}", posts[1].1);
        assert!(posts[3].1.iter().all(|down| (down - 2.0).abs() < 1e-3), "{:?}", posts[3].1);
        let made = placed.items[3].geometry().bounds().0.y;
        placed.items[3].ends_down = posts[3].1;
        let g = placed.items[3].geometry();
        assert!((g.bounds().0.y - (made - 2.0)).abs() < 1e-3 && g.quads.len() == placed.items[2].geometry().quads.len(), "its posts are longer, not more");
        assert_eq!(g.rail_quads.len(), 6, "the rail itself is as it was");
        assert_ne!(placed.items[3].shape_key(), placed.items[2].shape_key(), "and its mesh is made again");
        assert!(placed.items[3].own_posts() && !placed.items[2].own_posts());
        // A drawn rail from one pad to another, its points on their tops: the posts between them,
        // over the drop, go; the two at its ends, over the pads, stay as they are.
        let pad = |id, x: f32| {
            let node = |dx: f32, dz: f32| drawn::Node { corner: true, ..drawn::Node::new(Vec3::new(dx, 1.0, dz), 0.0) };
            Placed { path: Some(drawn::Path { nodes: vec![node(-1.5, -1.5), node(1.5, -1.5), node(1.5, 1.5), node(-1.5, 1.5)], ..default() }), ..item(id, PieceKind::Pad, [x, 0.0, 30.0]) }
        };
        let node = |x: f32| drawn::Node::new(Vec3::new(x, 1.0, 0.0), 1.0);
        let rail = Placed { path: Some(drawn::Path { nodes: vec![node(-0.5), node(4.0), node(8.5)], ..default() }), ..item(7, PieceKind::RailPath, [0.0, 0.0, 30.0]) };
        let mut placed = PlacedPieces { items: vec![pad(5, 0.0), pad(6, 8.0), rail], ..default() };
        let spans = placed.items[2].whole_geometry().post_spans();
        assert_eq!(spans.len(), 5, "its ends, its middle node, and the middle of each long stretch: {spans:?}");
        let (mask, down) = posts_over_gaps(&world, &placed, &Transform::IDENTITY)[2];
        for (index, (foot, _)) in spans.iter().enumerate() {
            let on_a_pad = foot.x < 1.5 || foot.x > 6.5;
            assert_eq!(mask & (1 << index) == 0, on_a_pad, "the post at x = {}", foot.x);
        }
        assert_eq!(down, [0.0; 2]);
        // With the second pad gone, that end's post still stands: down to the floor, a metre below.
        placed.items.remove(1);
        let (mask, down) = posts_over_gaps(&world, &placed, &Transform::IDENTITY)[1];
        let ends = placed.items[1].whole_geometry().end_posts();
        assert!(ends.iter().all(|end| end.is_some_and(|post| mask & (1 << post) == 0)) && mask.count_ones() == 3, "{mask:b} {ends:?}");
        assert!(down[0] == 0.0 && (down[1] - 1.0).abs() < 1e-3, "{down:?}");
    }

}
