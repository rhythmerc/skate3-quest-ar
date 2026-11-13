//! The skate pieces the player can place, as collision/render geometry in
//! piece-local game metres: +Y up, the base on y = 0, centred on the origin.
//!
//! Every quad uses the course winding (triangles 0,2,1 and 0,3,2; normal
//! `(q2 - q0) x (q1 - q0)`) facing out of the solid, because course collision
//! is one-sided. Ramps are height fields over z, extruded across x, so their
//! end caps are simple trapezoids even where the profile is concave.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::f32::consts::FRAC_PI_2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum PieceKind {
    Kicker,
    QuarterPipe,
    HalfPipe,
    Rail,
    Ledge,
    FunBox,
    Platform,
    Bank,
    Stairs,
    Spine,
    /// A tall flat wall to ride (wallrides, wallies); no grind edges.
    Wall,
    /// A rail round a quarter turn.
    CurvedRail,
    /// A pool sunk into what it is placed on: its rim is level with that
    /// surface, which is opened over it (`xr::real_world`).
    Bowl,
}

impl PieceKind {
    /// Every kind, in the palette's order: ramps, rails, boxes, walls.
    pub(crate) const ALL: [Self; 13] = [
        Self::Kicker,
        Self::Bank,
        Self::QuarterPipe,
        Self::HalfPipe,
        Self::Spine,
        Self::Bowl,
        Self::Rail,
        Self::CurvedRail,
        Self::Ledge,
        Self::FunBox,
        Self::Platform,
        Self::Stairs,
        Self::Wall,
    ];

    /// What a player would call it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Kicker => "Kicker",
            Self::QuarterPipe => "Quarter pipe",
            Self::HalfPipe => "Half pipe",
            Self::Rail => "Rail",
            Self::Ledge => "Ledge",
            Self::FunBox => "Fun box",
            Self::Platform => "Platform",
            Self::Bank => "Bank",
            Self::Stairs => "Stairs",
            Self::Spine => "Spine",
            Self::Wall => "Wall",
            Self::CurvedRail => "Curved rail",
            Self::Bowl => "Bowl",
        }
    }

    /// What a piece's `size` is of ("stretch its ...").
    pub(crate) fn size_name(self) -> &'static str {
        match self {
            Self::Kicker | Self::QuarterPipe | Self::HalfPipe | Self::Bank | Self::Stairs | Self::Spine => "height",
            Self::Rail | Self::Ledge | Self::FunBox | Self::Platform | Self::Wall => "length",
            Self::CurvedRail => "radius",
            Self::Bowl => "depth",
        }
    }

    /// The local direction a piece's `size` stretches it along.
    pub(crate) fn size_axis(self) -> Vec3 {
        match self.size_name() {
            "length" | "radius" => Vec3::X,
            _ => Vec3::Y,
        }
    }

    /// The local direction its `width` stretches it along, if it has one.
    pub(crate) fn width_axis(self) -> Option<Vec3> {
        match self {
            Self::Kicker | Self::Bank | Self::QuarterPipe | Self::Spine | Self::Stairs | Self::Bowl => Some(Vec3::X),
            Self::HalfPipe | Self::Ledge | Self::FunBox | Self::Platform => Some(Vec3::Z),
            Self::Rail | Self::CurvedRail | Self::Wall => None,
        }
    }

    /// Whether the piece goes down into the surface under it, which is then
    /// opened over its footprint.
    pub(crate) fn sunk(self) -> bool {
        self == Self::Bowl
    }

    pub(crate) fn color(self) -> Color {
        match self {
            Self::Kicker => Color::srgb(0.85, 0.55, 0.25),
            Self::QuarterPipe | Self::HalfPipe | Self::Bowl => Color::srgb(0.35, 0.6, 0.9),
            Self::Rail | Self::CurvedRail => Color::srgb(0.85, 0.85, 0.9),
            Self::Ledge => Color::srgb(0.6, 0.6, 0.62),
            Self::FunBox => Color::srgb(0.9, 0.35, 0.4),
            Self::Platform => Color::srgb(0.45, 0.75, 0.45),
            Self::Bank => Color::srgb(0.8, 0.65, 0.35),
            Self::Stairs => Color::srgb(0.7, 0.7, 0.68),
            Self::Spine => Color::srgb(0.3, 0.5, 0.85),
            Self::Wall => Color::srgb(0.75, 0.72, 0.66),
        }
    }
}

/// What a piece is made of: how it is drawn (it skates the same).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Finish {
    Wood,
    Concrete,
    Brick,
    Steel,
    /// The flat colour of its kind ([`PieceKind::color`]).
    Paint,
}

impl Finish {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Wood => "wood",
            Self::Concrete => "concrete",
            Self::Brick => "brick",
            Self::Steel => "steel",
            Self::Paint => "painted",
        }
    }
}

impl PieceKind {
    /// The finishes a piece of this kind can have, the one it gets unless
    /// another is chosen first.
    pub(crate) fn finishes(self) -> &'static [Finish] {
        use Finish::*;
        match self {
            Self::Kicker | Self::Bank | Self::FunBox => &[Wood, Concrete, Paint],
            Self::QuarterPipe | Self::HalfPipe | Self::Spine => &[Wood, Concrete, Paint],
            Self::Rail | Self::CurvedRail => &[Steel, Paint],
            Self::Bowl => &[Concrete, Wood, Paint],
            Self::Ledge | Self::Stairs => &[Concrete, Brick, Paint],
            Self::Platform => &[Concrete, Wood, Brick, Paint],
            Self::Wall => &[Brick, Concrete, Wood, Paint],
        }
    }

    /// `finish` if this kind can have it, else its default.
    pub(crate) fn finish(self, finish: Option<Finish>) -> Finish {
        let all = self.finishes();
        finish.filter(|f| all.contains(f)).unwrap_or(all[0])
    }
}

/// Resize range for the piece's main dimension.
pub(crate) const MIN_SIZE: f32 = 0.5;
pub(crate) const MAX_SIZE: f32 = 2.5;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Piece {
    pub kind: PieceKind,
    /// Multiplier on the kind's main dimension (ramp height, rail length...).
    pub size: f32,
    /// Multiplier on its width (a ramp's across, a box's depth), for the
    /// kinds that have one ([`PieceKind::width_axis`]); absent in layouts
    /// from before it.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub width: f32,
    /// How far its base is above the surface it stands on (its own metres),
    /// when it was joined to another piece higher up: it then stands on a
    /// solid base down to that surface (a rail on longer posts). Below 0 for
    /// a box joined lower than its own height: it is that much shorter.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lift: f32,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// Where another piece can be joined to a piece: a level edge of it, in its
/// own metres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Join {
    /// The edge's middle.
    pub at: Vec3,
    /// Level, out of the piece across the edge.
    pub out: Vec3,
    /// The edge's length (0: a rail's end, which joins at any angle).
    pub width: f32,
    /// Which of the piece's dimensions the edge's length follows, and that
    /// length at 1.
    pub follows: Option<(Dimension, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dimension {
    Size,
    Width,
}

fn one() -> f32 {
    1.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

/// A piece's colour, when it isn't its finish's own: steps round the hues
/// and from dark to light.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Tint {
    pub hue: u8,
    pub light: u8,
}

impl Tint {
    pub(crate) const HUES: u8 = 24;
    pub(crate) const LIGHTS: u8 = 7;

    /// Where colouring a finish starts from: about its own colour.
    pub(crate) fn of(finish: Finish) -> Self {
        match finish {
            Finish::Wood => Self { hue: 2, light: 3 },
            Finish::Brick => Self { hue: 1, light: 3 },
            Finish::Concrete | Finish::Steel => Self { hue: 14, light: 4 },
            Finish::Paint => Self { hue: 14, light: 3 },
        }
    }

    pub(crate) fn stepped(self, hue: i32, light: i32) -> Self {
        Self {
            hue: (self.hue as i32 + hue).rem_euclid(Self::HUES as i32) as u8,
            light: (self.light as i32 + light).clamp(0, Self::LIGHTS as i32 - 1) as u8,
        }
    }

    pub(crate) fn color(self) -> Color {
        Color::hsl(self.hue as f32 * 360.0 / Self::HUES as f32, 0.55, 0.2 + 0.1 * self.light as f32)
    }
}

#[derive(Debug, Default)]
pub(crate) struct PieceGeometry {
    /// Solid faces.
    pub quads: Vec<[Vec3; 4]>,
    /// Faces of grind tubes: convex rail collision, see `ground`.
    pub rail_quads: Vec<[Vec3; 4]>,
    /// Grindable polylines (a tube's top centre, a ledge or coping edge).
    pub rails: Vec<Vec<Vec3>>,
    /// Off-board ledge edges (grab/climb/respawn queries).
    pub edges: Vec<(Vec3, Vec3)>,
    /// Texture coordinates of the `quads` that bring their own (by index):
    /// surfaces curved two ways, which [`Self::uvs`] can't unroll.
    pub given_uvs: Vec<(usize, [Vec2; 4])>,
    /// The `quads` (by index) that are ridden but not drawn: where a sunk
    /// piece's rectangle is still the surface it is sunk into.
    pub unseen: Vec<usize>,
}

impl PieceGeometry {
    pub(crate) fn all_quads(&self) -> impl Iterator<Item = &[Vec3; 4]> {
        self.quads.iter().chain(&self.rail_quads)
    }
}

impl PieceGeometry {
    /// Its least and greatest corners.
    pub(crate) fn bounds(&self) -> (Vec3, Vec3) {
        self.all_quads().flatten().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), &p| (lo.min(p), hi.max(p)))
    }

    /// The piece's footprint: least and greatest local (x, z).
    pub(crate) fn footprint(&self) -> (Vec2, Vec2) {
        self.all_quads().flatten().fold((Vec2::splat(f32::MAX), Vec2::splat(f32::MIN)), |(lo, hi), p| {
            let p = Vec2::new(p.x, p.z);
            (lo.min(p), hi.max(p))
        })
    }

    /// Texture coordinates for [`Self::all_quads`], in metres: faces across
    /// the piece (ramp surfaces, the tops and long sides of boxes) run along
    /// x and along the piece's profile, so a texture follows a transition
    /// round its curve without a seam; end caps are (z, y).
    pub(crate) fn uvs(&self) -> Vec<[Vec2; 4]> {
        // Distance along the profile to each (y, z) reached so far.
        let mut along: std::collections::HashMap<(i32, i32), f32> = Default::default();
        let key = |p: Vec3| ((p.y * 1000.0).round() as i32, (p.z * 1000.0).round() as i32);
        self.all_quads()
            .enumerate()
            .map(|(index, q)| {
                if let Some((_, uv)) = self.given_uvs.iter().find(|(i, _)| *i == index) {
                    return *uv;
                }
                // (both triangles: an end cap can come to a point)
                let n = (quad_normal(q) + (q[3] - q[0]).cross(q[2] - q[0])).normalize_or_zero();
                if n.x.abs() > 0.7 {
                    return q.map(|p| Vec2::new(p.z, p.y));
                }
                // Not a face across the piece (its corners aren't two pairs): flat on.
                let mut keys: Vec<_> = q.iter().map(|&p| key(p)).collect();
                keys.sort_unstable();
                keys.dedup();
                if keys.len() > 2 {
                    return q.map(|p| if n.y.abs() > 0.7 { Vec2::new(p.x, p.z) } else { Vec2::new(p.x, p.y) });
                }
                // The face's two profile points, the one nearer -z (or the floor) first.
                let mut ends = [q[0], q[0]];
                for p in &q[1..] {
                    if key(*p) != key(ends[0]) {
                        ends[1] = *p;
                    }
                }
                if (ends[1].z, ends[1].y) < (ends[0].z, ends[0].y) {
                    ends.swap(0, 1);
                }
                let length = Vec2::new(ends[1].y - ends[0].y, ends[1].z - ends[0].z).length();
                let start = match (along.get(&key(ends[0])), along.get(&key(ends[1]))) {
                    (Some(&a), Some(&b)) if ((b - a).abs() - length).abs() < 1e-3 => return q.map(|p| {
                        Vec2::new(p.x, if key(p) == key(ends[0]) { a } else { b })
                    }),
                    (Some(&a), _) => a,
                    (None, Some(&b)) => b - length,
                    (None, None) => 0.0,
                };
                along.entry(key(ends[0])).or_insert(start);
                along.entry(key(ends[1])).or_insert(start + length);
                q.map(|p| Vec2::new(p.x, if key(p) == key(ends[0]) { start } else { start + length }))
            })
            .collect()
    }
}

impl Piece {
    pub(crate) fn new(kind: PieceKind) -> Self {
        Self { kind, size: 1.0, width: 1.0, lift: 0.0 }
    }

    pub(crate) fn sized(kind: PieceKind, size: f32) -> Self {
        Self { kind, size, width: 1.0, lift: 0.0 }
    }

    pub(crate) fn geometry(&self) -> PieceGeometry {
        let s = self.size.clamp(MIN_SIZE, MAX_SIZE);
        let w = self.width.clamp(MIN_SIZE, MAX_SIZE);
        let lift = self.lift;
        let mut g = PieceGeometry::default();
        match self.kind {
            PieceKind::Kicker => {
                let (length, height) = (2.5 * s, 0.8 * s);
                let profile: Vec<Vec2> = (0..=16)
                    .map(|i| {
                        let t = i as f32 / 16.0;
                        Vec2::new(-length * 0.5 + length * t, height * (1.0 - t) * (1.0 - t))
                    })
                    .collect();
                extrude(&mut g, &profile, 2.0 * w);
                g.edges.push((Vec3::new(-w, height, -length * 0.5), Vec3::new(w, height, -length * 0.5)));
            }
            PieceKind::QuarterPipe => {
                let (radius, deck, width) = (2.0 * s, 1.0, 4.0 * w);
                let z0 = -(radius + deck) * 0.5;
                let mut profile = vec![Vec2::new(z0, radius)];
                profile.extend(transition(z0 + deck, radius, 1.0));
                extrude(&mut g, &profile, width);
                coping(&mut g, z0 + deck, radius, width);
            }
            PieceKind::HalfPipe => {
                // (its width is the flat between its two transitions, which keep their shape)
                let (radius, deck, flat, width) = (2.0 * s, 1.0, 3.0 * w, 5.0);
                let half = deck + radius + flat * 0.5;
                let mut profile = vec![Vec2::new(-half, radius)];
                profile.extend(transition(-half + deck, radius, 1.0));
                profile.extend(transition(half - deck, radius, -1.0).into_iter().rev());
                profile.push(Vec2::new(half, radius));
                extrude(&mut g, &profile, width);
                coping(&mut g, -half + deck, radius, width);
                coping(&mut g, half - deck, radius, width);
            }
            PieceKind::Rail => {
                let (length, top, tube) = (6.0 * s, 0.5, 0.08);
                let x = length * 0.5;
                let mut tube_faces = Vec::new();
                box_quads(&mut tube_faces, Vec3::new(-x, top - tube, -tube * 0.5), Vec3::new(x, top, tube * 0.5));
                g.rail_quads = tube_faces;
                for px in [-x + 0.6, x - 0.6] {
                    box_quads(&mut g.quads, Vec3::new(px - 0.04, -lift.max(0.0), -0.04), Vec3::new(px + 0.04, top - tube, 0.04));
                }
                g.rails.push(vec![Vec3::new(-x, top, 0.0), Vec3::new(x, top, 0.0)]);
            }
            PieceKind::Ledge | PieceKind::FunBox | PieceKind::Platform | PieceKind::Wall => {
                let size = self.box_size().expect("a box");
                let grind = match self.kind {
                    PieceKind::Ledge => [true, false, true, false],
                    PieceKind::Wall => [false; 4],
                    _ => [true; 4],
                };
                // (its base goes down to the surface, or starts above its own)
                grind_box(&mut g, size, (-lift).min(size.y - MIN_BOX_M), grind);
            }
            PieceKind::Bank => {
                // A flat slope up to a short deck, run in from +z like a kicker.
                let (length, height, deck, width) = (4.0 * s, 1.0 * s, 1.0, 3.0 * w);
                let z0 = -length * 0.5;
                extrude(&mut g, &[Vec2::new(z0, height), Vec2::new(z0 + deck, height), Vec2::new(-z0, 0.0)], width);
                g.edges.push((Vec3::new(-width * 0.5, height, z0), Vec3::new(width * 0.5, height, z0)));
            }
            PieceKind::Stairs => {
                // A top deck, then steps down towards +z, with a handrail
                // alongside (outside +x) from the top step to the bottom.
                let (steps, rise, run, deck, width) = (5, 0.17 * s, 0.35 * s, 1.5, 3.0 * w);
                let height = rise * steps as f32;
                let z0 = -(deck + run * steps as f32) * 0.5;
                let mut profile = vec![Vec2::new(z0, height), Vec2::new(z0 + deck, height)];
                for i in 1..=steps {
                    let z = z0 + deck + run * i as f32;
                    profile.push(Vec2::new(z - run, height - rise * i as f32));
                    profile.push(Vec2::new(z, height - rise * i as f32));
                }
                extrude(&mut g, &profile, width);
                let (rail_x, above, tube) = (width * 0.5 + 0.35, 0.8, 0.08);
                let top = Vec3::new(rail_x, height + above, z0 + deck);
                let bottom = Vec3::new(rail_x, above, z0 + deck + run * steps as f32);
                tube_quads(&mut g.rail_quads, top, bottom, tube);
                for p in [top, bottom] {
                    box_quads(&mut g.quads, Vec3::new(p.x - 0.04, 0.0, p.z - 0.04), Vec3::new(p.x + 0.04, p.y - tube, p.z + 0.04));
                }
                g.rails.push(vec![top, bottom]);
                g.edges.push((Vec3::new(-width * 0.5, height, z0 + deck), Vec3::new(width * 0.5, height, z0 + deck)));
            }
            PieceKind::CurvedRail => {
                // A quarter turn about a corner, the piece centred on the turn's box.
                let (radius, top, tube, segments) = (3.0 * s, 0.5, 0.08, 10);
                let point = |i: usize| {
                    let a = FRAC_PI_2 * i as f32 / segments as f32;
                    Vec3::new(radius * (a.cos() - 0.5), top, radius * (a.sin() - 0.5))
                };
                let points: Vec<Vec3> = (0..=segments).map(point).collect();
                for pair in points.windows(2) {
                    level_tube_quads(&mut g.rail_quads, pair[0], pair[1], tube);
                }
                for p in [points[1], points[segments / 2], points[segments - 1]] {
                    box_quads(&mut g.quads, Vec3::new(p.x - 0.04, -lift.max(0.0), p.z - 0.04), Vec3::new(p.x + 0.04, top - tube, p.z + 0.04));
                }
                g.rails.push(points);
            }
            PieceKind::Bowl => bowl(&mut g, 1.5 * s, Vec2::new(1.2 * w, 1.2)),
            PieceKind::Spine => {
                // Two quarter pipes back to back, sharing one coping.
                let (radius, width) = (1.6 * s, 4.0 * w);
                let mut profile: Vec<Vec2> = transition(0.0, radius, -1.0).into_iter().rev().collect();
                profile.extend(transition(0.0, radius, 1.0).into_iter().skip(1));
                extrude(&mut g, &profile, width);
                coping(&mut g, 0.0, radius, width);
            }
        }
        // A ramp joined higher up stands on a solid block down to the surface.
        if lift > 0.0 && self.box_size().is_none() && !self.kind.on_posts() && !self.kind.sunk() {
            let (lo, hi) = g.footprint();
            let mut block = Vec::new();
            box_quads(&mut block, Vec3::new(lo.x, -lift, lo.y), Vec3::new(hi.x, 0.0, hi.y));
            // (not its top: that is inside the piece)
            g.quads.extend(block.into_iter().filter(|q| quad_normal(q).normalize().y < 0.9));
        }
        g
    }

    /// A box kind's size at this piece's dimensions (x, its full height, z).
    fn box_size(&self) -> Option<Vec3> {
        let s = self.size.clamp(MIN_SIZE, MAX_SIZE);
        let w = self.width.clamp(MIN_SIZE, MAX_SIZE);
        match self.kind {
            PieceKind::Ledge => Some(Vec3::new(5.0 * s, 0.5, 0.8 * w)),
            PieceKind::FunBox => Some(Vec3::new(3.0 * s, 0.5, 2.0 * w)),
            PieceKind::Platform => Some(Vec3::new(4.0 * s, 1.2, 4.0 * w)),
            PieceKind::Wall => Some(Vec3::new(6.0 * s, 2.6, 0.4)),
            _ => None,
        }
    }

    /// The least `lift` the piece can have: a box can be shorter than its
    /// own height (down to [`MIN_BOX_M`]), nothing else can go into a surface.
    pub(crate) fn least_lift(&self) -> f32 {
        self.box_size().map_or(0.0, |size| MIN_BOX_M - size.y)
    }

    /// Where other pieces join this one (`lift` aside: from its own base).
    pub(crate) fn joins(&self) -> Vec<Join> {
        use Dimension::*;
        let s = self.size.clamp(MIN_SIZE, MAX_SIZE);
        let w = self.width.clamp(MIN_SIZE, MAX_SIZE);
        // An edge across x at (y, z), facing along z.
        let across = |y: f32, z: f32, facing: f32, width: f32, per: f32| Join {
            at: Vec3::new(0.0, y, z),
            out: Vec3::Z * facing,
            width,
            follows: (per > 0.0).then_some((Width, per)),
        };
        if let Some(size) = self.box_size() {
            let h = size * 0.5;
            let (per_x, per_z) = (size.x / s, size.z / w);
            let wide = self.kind.width_axis().is_some();
            return vec![
                Join { at: Vec3::new(0.0, size.y, -h.z), out: Vec3::NEG_Z, width: size.x, follows: Some((Size, per_x)) },
                Join { at: Vec3::new(0.0, size.y, h.z), out: Vec3::Z, width: size.x, follows: Some((Size, per_x)) },
                Join { at: Vec3::new(-h.x, size.y, 0.0), out: Vec3::NEG_X, width: size.z, follows: wide.then_some((Width, per_z)) },
                Join { at: Vec3::new(h.x, size.y, 0.0), out: Vec3::X, width: size.z, follows: wide.then_some((Width, per_z)) },
            ];
        }
        match self.kind {
            PieceKind::Kicker => {
                let (length, height) = (2.5 * s, 0.8 * s);
                vec![across(height, -length * 0.5, -1.0, 2.0 * w, 2.0), across(0.0, length * 0.5, 1.0, 2.0 * w, 2.0)]
            }
            PieceKind::Bank => {
                let (length, height) = (4.0 * s, 1.0 * s);
                vec![across(height, -length * 0.5, -1.0, 3.0 * w, 3.0), across(0.0, length * 0.5, 1.0, 3.0 * w, 3.0)]
            }
            PieceKind::QuarterPipe => {
                let (radius, deck) = (2.0 * s, 1.0);
                let z0 = -(radius + deck) * 0.5;
                vec![across(radius, z0, -1.0, 4.0 * w, 4.0), across(0.0, -z0, 1.0, 4.0 * w, 4.0)]
            }
            PieceKind::HalfPipe => {
                let (radius, deck, flat) = (2.0 * s, 1.0, 3.0 * w);
                let half = deck + radius + flat * 0.5;
                vec![across(radius, -half, -1.0, 5.0, 0.0), across(radius, half, 1.0, 5.0, 0.0)]
            }
            PieceKind::Stairs => {
                let (steps, rise, run, deck) = (5.0, 0.17 * s, 0.35 * s, 1.5);
                let z0 = -(deck + run * steps) * 0.5;
                vec![across(rise * steps, z0, -1.0, 3.0 * w, 3.0), across(0.0, -z0, 1.0, 3.0 * w, 3.0)]
            }
            PieceKind::Spine => {
                let radius = 1.6 * s;
                vec![across(0.0, -radius, -1.0, 4.0 * w, 4.0), across(0.0, radius, 1.0, 4.0 * w, 4.0)]
            }
            PieceKind::Rail => {
                let x = 3.0 * s;
                [-1.0, 1.0].map(|end| Join { at: Vec3::new(end * x, 0.5, 0.0), out: Vec3::X * end, width: 0.0, follows: None }).to_vec()
            }
            PieceKind::CurvedRail => {
                let radius = 3.0 * s;
                vec![
                    Join { at: Vec3::new(radius * 0.5, 0.5, -radius * 0.5), out: Vec3::NEG_Z, width: 0.0, follows: None },
                    Join { at: Vec3::new(-radius * 0.5, 0.5, radius * 0.5), out: Vec3::NEG_X, width: 0.0, follows: None },
                ]
            }
            _ => Vec::new(),
        }
    }
}

/// The least height of a box joined lower than its own, m.
const MIN_BOX_M: f32 = 0.1;

impl PieceKind {
    /// Stands on posts, which reach down when it is joined higher up.
    pub(crate) fn on_posts(self) -> bool {
        matches!(self, Self::Rail | Self::CurvedRail)
    }
}

/// Concave circular transition from a lip at (`lip_z`, `radius`) down to the
/// floor, heading towards +z (`dir` 1) or -z (`dir` -1). Includes the lip.
fn transition(lip_z: f32, radius: f32, dir: f32) -> Vec<Vec2> {
    // Circle centred level with the lip, one radius out over the floor.
    (0..=16)
        .map(|i| {
            let a = FRAC_PI_2 * i as f32 / 16.0;
            Vec2::new(lip_z + dir * radius * (1.0 - a.cos()), radius * (1.0 - a.sin()))
        })
        .collect()
}

fn coping(g: &mut PieceGeometry, z: f32, height: f32, width: f32) {
    let (a, b) = (Vec3::new(-width * 0.5, height, z), Vec3::new(width * 0.5, height, z));
    g.rails.push(vec![a, b]);
    g.edges.push((a, b));
}

/// A box whose top edges (in order -z, +x, +z, -x) are optionally grindable.
fn grind_box(g: &mut PieceGeometry, size: Vec3, base: f32, grind: [bool; 4]) {
    let h = size * 0.5;
    box_quads(&mut g.quads, Vec3::new(-h.x, base, -h.z), Vec3::new(h.x, size.y, h.z));
    let c = [
        Vec3::new(-h.x, size.y, -h.z),
        Vec3::new(h.x, size.y, -h.z),
        Vec3::new(h.x, size.y, h.z),
        Vec3::new(-h.x, size.y, h.z),
    ];
    for i in 0..4 {
        let (a, b) = (c[i], c[(i + 1) % 4]);
        g.edges.push((a, b));
        if grind[i] {
            g.rails.push(vec![a, b]);
        }
    }
}

/// Extrude a height field `y(z)` (points in increasing z) across x in
/// [-width/2, width/2], solid down to y = 0. No bottom face: pieces sit on a
/// surface.
fn extrude(g: &mut PieceGeometry, profile: &[Vec2], width: f32) {
    let x = width * 0.5;
    let at = |x: f32, p: Vec2| Vec3::new(x, p.y, p.x);
    // Top surface, facing up (or towards the open side where vertical).
    for pair in profile.windows(2) {
        let (p, q) = (pair[0], pair[1]);
        let edge = q - p;
        let outward = Vec3::new(0.0, edge.x, -edge.y);
        push(&mut g.quads, [at(-x, p), at(x, p), at(x, q), at(-x, q)], outward);
        // End caps: the trapezoid under this segment, on both sides.
        for (side, n) in [(-x, Vec3::NEG_X), (x, Vec3::X)] {
            let cap = [
                Vec3::new(side, 0.0, p.x),
                Vec3::new(side, 0.0, q.x),
                at(side, q),
                at(side, p),
            ];
            push(&mut g.quads, cap, n);
        }
    }
    // Back walls where the profile starts or ends above the base.
    let (first, last) = (profile[0], profile[profile.len() - 1]);
    push(&mut g.quads, [at(-x, first), at(x, first), at(x, first.with_y(0.0)), at(-x, first.with_y(0.0))], Vec3::NEG_Z);
    push(&mut g.quads, [at(-x, last), at(x, last), at(x, last.with_y(0.0)), at(-x, last.with_y(0.0))], Vec3::Z);
}

/// A pool below y = 0: a flat floor (`floor` its half size) `radius` down,
/// a quarter-circle transition all the way round it up to a rim level with
/// y = 0, and that rim squared off to the piece's rectangle, so the surface
/// it is sunk into can be opened along straight lines.
fn bowl(g: &mut PieceGeometry, radius: f32, floor: Vec2) {
    const CORNER: usize = 6; // segments round each corner (even)
    const RINGS: usize = 8; // steps up the transition
    // Round the floor's edge: at each corner, the directions out of it.
    let corners = [Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0), Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0)];
    let around: Vec<(Vec2, Vec2)> = corners
        .iter()
        .enumerate()
        .flat_map(|(i, &c)| {
            (0..=CORNER).map(move |j| {
                let a = FRAC_PI_2 * (i as f32 + j as f32 / CORNER as f32);
                (c * floor, Vec2::new(a.cos(), a.sin()))
            })
        })
        .collect();
    let count = around.len();
    let ring = |k: usize, j: usize| {
        let a = FRAC_PI_2 * k as f32 / RINGS as f32;
        let (corner, out) = around[j % count];
        let at = corner + out * radius * a.sin();
        Vec3::new(at.x, -radius * a.cos(), at.y)
    };
    // Texture: round the bowl at half its height, and up the transition.
    let mut along = vec![0.0f32; count + 1];
    for j in 0..count {
        along[j + 1] = along[j] + ring(RINGS / 2, j).distance(ring(RINGS / 2, j + 1));
    }
    let up = |k: usize| radius * FRAC_PI_2 * k as f32 / RINGS as f32;
    let above = Vec3::new(0.0, radius, 0.0);
    let face = |g: &mut PieceGeometry, quad: [Vec3; 4], uv: [Vec2; 4]| {
        let centre = (quad[0] + quad[1] + quad[2] + quad[3]) * 0.25;
        push_with_uv(g, quad, uv, above - centre);
    };
    for k in 0..RINGS {
        let mut j = 0;
        while j < count {
            // On the floor's corners the first ring is one point: fan out of it.
            let on_corner = k == 0 && (j % (CORNER + 1)) < CORNER;
            if on_corner {
                let uv = |jj: usize, kk: usize| Vec2::new(along[jj], up(kk));
                face(g, [ring(0, j), ring(1, j), ring(1, j + 1), ring(1, j + 2)], [uv(j + 1, 0), uv(j, 1), uv(j + 1, 1), uv(j + 2, 1)]);
                j += 2;
                continue;
            }
            let uv = |jj: usize, kk: usize| Vec2::new(along[jj], up(kk));
            face(g, [ring(k, j), ring(k, j + 1), ring(k + 1, j + 1), ring(k + 1, j)], [uv(j, k), uv(j + 1, k), uv(j + 1, k + 1), uv(j, k + 1)]);
            j += 1;
        }
    }
    push(&mut g.quads, [
        Vec3::new(-floor.x, -radius, -floor.y),
        Vec3::new(floor.x, -radius, -floor.y),
        Vec3::new(floor.x, -radius, floor.y),
        Vec3::new(-floor.x, -radius, floor.y),
    ], Vec3::Y);
    // The rim's corners, out to the rectangle: still the surface the bowl is in, so ridden but
    // not drawn.
    for (i, &c) in corners.iter().enumerate() {
        let square = Vec3::new(c.x * (floor.x + radius), 0.0, c.y * (floor.y + radius));
        let first = i * (CORNER + 1);
        for j in (first..first + CORNER).step_by(2) {
            let before = g.quads.len();
            push(&mut g.quads, [square, ring(RINGS, j), ring(RINGS, j + 1), ring(RINGS, j + 2)], Vec3::Y);
            g.unseen.extend(before..g.quads.len());
        }
    }
    // The rim is coping all the way round.
    let rim: Vec<Vec3> = (0..=count).map(|j| ring(RINGS, j)).collect();
    g.edges.extend(rim.windows(2).filter(|w| w[0].distance(w[1]) > 1e-4).map(|w| (w[0], w[1])));
    g.rails.push(rim);
}

/// [`push`] for a solid face that brings its texture coordinates.
fn push_with_uv(g: &mut PieceGeometry, quad: [Vec3; 4], uv: [Vec2; 4], outward: Vec3) {
    let before = g.quads.len();
    push(&mut g.quads, quad, outward);
    if g.quads.len() > before {
        let flipped = g.quads[before][1] != quad[1];
        g.given_uvs.push((before, if flipped { [uv[0], uv[3], uv[2], uv[1]] } else { uv }));
    }
}

/// A square grind tube whose top centre line runs level from `a` to `b` (any
/// direction over the ground), `size` thick.
fn level_tube_quads(out: &mut Vec<[Vec3; 4]>, a: Vec3, b: Vec3, size: f32) {
    let along = (b - a) * 0.5;
    let side = along.normalize().cross(Vec3::Y).normalize() * size * 0.5;
    let centre = (a + b) * 0.5 - Vec3::Y * size * 0.5;
    let axes = [side, Vec3::Y * size * 0.5, along];
    for i in 0..3 {
        let (aj, ak) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
        for sign in [1.0, -1.0] {
            let c = centre + axes[i] * sign;
            push(out, [c - aj - ak, c + aj - ak, c + aj + ak, c - aj + ak], axes[i] * sign);
        }
    }
}

/// A square grind tube whose top centre line runs from `a` to `b` (any slope
/// in the y-z plane), `size` thick.
fn tube_quads(out: &mut Vec<[Vec3; 4]>, a: Vec3, b: Vec3, size: f32) {
    let along = (b - a) * 0.5;
    let up = along.normalize().cross(Vec3::X).normalize();
    let up = if up.y < 0.0 { -up } else { up };
    let centre = (a + b) * 0.5 - up * size * 0.5;
    let axes = [Vec3::X * size * 0.5, up * size * 0.5, along];
    for i in 0..3 {
        let (aj, ak) = (axes[(i + 1) % 3], axes[(i + 2) % 3]);
        for sign in [1.0, -1.0] {
            let c = centre + axes[i] * sign;
            push(out, [c - aj - ak, c + aj - ak, c + aj + ak, c - aj + ak], axes[i] * sign);
        }
    }
}

/// Six faces of an axis-aligned box, bottom included (rail tubes float).
fn box_quads(out: &mut Vec<[Vec3; 4]>, min: Vec3, max: Vec3) {
    let c = |x: bool, y: bool, z: bool| {
        Vec3::new(
            if x { max.x } else { min.x },
            if y { max.y } else { min.y },
            if z { max.z } else { min.z },
        )
    };
    let faces = [
        ([c(false, true, false), c(true, true, false), c(true, true, true), c(false, true, true)], Vec3::Y),
        ([c(false, false, false), c(true, false, false), c(true, false, true), c(false, false, true)], Vec3::NEG_Y),
        ([c(true, false, false), c(true, true, false), c(true, true, true), c(true, false, true)], Vec3::X),
        ([c(false, false, false), c(false, true, false), c(false, true, true), c(false, false, true)], Vec3::NEG_X),
        ([c(false, false, true), c(true, false, true), c(true, true, true), c(false, true, true)], Vec3::Z),
        ([c(false, false, false), c(true, false, false), c(true, true, false), c(false, true, false)], Vec3::NEG_Z),
    ];
    for (quad, n) in faces {
        push(out, quad, n);
    }
}

/// Course-winding normal of a quad (not normalised).
pub(crate) fn quad_normal(q: &[Vec3; 4]) -> Vec3 {
    (q[2] - q[0]).cross(q[1] - q[0])
}

/// Add `quad` facing `outward`, dropping degenerate (zero-area) quads.
fn push(out: &mut Vec<[Vec3; 4]>, quad: [Vec3; 4], outward: Vec3) {
    let n = quad_normal(&quad) + (quad[3] - quad[0]).cross(quad[2] - quad[0]);
    if n.length_squared() < 1e-10 {
        return;
    }
    out.push(if n.dot(outward) >= 0.0 {
        quad
    } else {
        [quad[0], quad[3], quad[2], quad[1]]
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centroid(g: &PieceGeometry) -> Vec3 {
        let points: Vec<Vec3> = g.all_quads().flatten().copied().collect();
        points.iter().sum::<Vec3>() / points.len() as f32
    }

    #[test]
    fn every_piece_has_upward_tops_and_no_downward_ramps() {
        for kind in PieceKind::ALL {
            for size in [MIN_SIZE, 1.0, MAX_SIZE] {
                let g = Piece::sized(kind, size).geometry();
                assert!(!g.quads.is_empty(), "{kind:?}");
                let up = g
                    .all_quads()
                    .filter(|q| quad_normal(q).normalize().y > 0.9)
                    .count();
                assert!(up > 0, "{kind:?} has something to land on");
                for q in g.all_quads() {
                    assert!(q.iter().all(|p| p.is_finite()));
                    assert!(kind.sunk() || q.iter().all(|p| p.y >= -1e-5), "{kind:?} stays above its base");
                    assert!(!kind.sunk() || q.iter().all(|p| p.y <= 1e-5), "{kind:?} stays below its rim");
                    // (the engines take faces as two triangles: a pool's fans keep both real)
                    for [a, b, c] in [[q[0], q[2], q[1]], [q[0], q[3], q[2]]] {
                        assert!(!kind.sunk() || (b - a).cross(c - a).length() > 1e-7, "{kind:?} has no collapsed triangles");
                    }
                }
            }
        }
    }

    #[test]
    fn a_lifted_piece_reaches_down_to_the_surface_and_keeps_its_top() {
        for kind in PieceKind::ALL {
            let piece = Piece { lift: 0.7, ..Piece::new(kind) };
            let (plain, lifted) = (Piece::new(kind).geometry(), piece.geometry());
            let ((lo, hi), (plain_lo, plain_hi)) = (lifted.bounds(), plain.bounds());
            assert!((hi.y - plain_hi.y).abs() < 1e-5, "{kind:?} keeps its top");
            if kind.sunk() {
                assert_eq!(lo, plain_lo, "{kind:?} is in its surface");
                continue;
            }
            assert!((lo.y + 0.7).abs() < 1e-5, "{kind:?} reaches down: {lo}");
            // A base is under the piece, not round it; a rail's is only its posts.
            assert_eq!(lifted.footprint(), plain.footprint(), "{kind:?}");
            assert_eq!(lifted.rail_quads.len(), plain.rail_quads.len());
            // Its joins are where they were: lift moves the piece, not its edges.
            assert_eq!(piece.joins(), Piece::new(kind).joins());
        }
        // A box joined lower than its own height is that much shorter, never gone.
        let low = Piece { lift: -0.8, ..Piece::new(PieceKind::Platform) }.geometry().bounds();
        assert!((low.0.y - 0.8).abs() < 1e-5 && (low.1.y - 1.2).abs() < 1e-5, "{low:?}");
        let flat = Piece { lift: -5.0, ..Piece::new(PieceKind::Platform) }.geometry().bounds();
        assert!(flat.1.y - flat.0.y > 0.05, "{flat:?}");
        assert!(Piece::new(PieceKind::Kicker).least_lift() == 0.0 && Piece::new(PieceKind::Platform).least_lift() < -1.0);
    }

    #[test]
    fn joins_are_level_edges_of_the_piece() {
        for kind in PieceKind::ALL {
            for (size, width) in [(1.0, 1.0), (0.5, 2.0), (2.5, 0.5)] {
                let piece = Piece { width, ..Piece::sized(kind, size) };
                let (lo, hi) = piece.geometry().bounds();
                for j in piece.joins() {
                    assert!((j.out.length() - 1.0).abs() < 1e-5 && j.out.y == 0.0, "{kind:?}");
                    // On the piece's outline, at a height it has, facing out of it.
                    assert!(j.at.cmpge(lo - 1e-3).all() && j.at.cmple(hi + 1e-3).all(), "{kind:?} {j:?} in {lo}..{hi}");
                    if j.width > 0.0 {
                        // (a handrail's post stands a few centimetres past the stairs' foot)
                        let outside = j.at + j.out * 0.05;
                        assert!(outside.x < lo.x || outside.x > hi.x || outside.z < lo.z || outside.z > hi.z, "{kind:?} {j:?}");
                        // The dimension it follows gives its length.
                        if let Some((dimension, per)) = j.follows {
                            let of = if dimension == Dimension::Size { size } else { width };
                            assert!((per * of - j.width).abs() < 1e-4, "{kind:?} {j:?}");
                        }
                    }
                }
            }
        }
        assert_eq!(Piece::new(PieceKind::Kicker).joins().len(), 2);
        assert_eq!(Piece::new(PieceKind::Platform).joins().len(), 4);
        assert!(Piece::new(PieceKind::Bowl).joins().is_empty());
    }

    #[test]
    fn box_faces_point_away_from_the_centre() {
        for kind in [PieceKind::Ledge, PieceKind::FunBox, PieceKind::Platform] {
            let g = Piece::new(kind).geometry();
            let c = centroid(&g);
            for q in &g.quads {
                let mid = q.iter().sum::<Vec3>() / 4.0;
                assert!(quad_normal(q).dot(mid - c) > 0.0, "{kind:?} face {q:?}");
            }
        }
    }

    #[test]
    fn ramp_surfaces_face_up_or_into_the_open() {
        let g = Piece::new(PieceKind::QuarterPipe).geometry();
        // Transition faces (not caps, not walls) have a +y or +z component.
        for q in &g.quads {
            let n = quad_normal(q).normalize();
            if n.x.abs() < 0.5 && n.y > -0.1 && q.iter().any(|p| p.y > 0.01) {
                assert!(n.y > -1e-3, "transition faces never point down: {n:?}");
            }
        }
        let kicker = Piece::new(PieceKind::Kicker).geometry();
        let ramp_up = kicker
            .quads
            .iter()
            .filter(|q| {
                let n = quad_normal(q).normalize();
                n.y > 0.1 && n.z > 0.05
            })
            .count();
        assert!(ramp_up >= 8, "kicker ramp faces lean towards the run-in");
    }

    #[test]
    fn grindable_pieces_have_rails_on_their_top() {
        for (kind, rails) in [
            (PieceKind::Rail, 1),
            (PieceKind::QuarterPipe, 1),
            (PieceKind::HalfPipe, 2),
            (PieceKind::Ledge, 2),
            (PieceKind::FunBox, 4),
            (PieceKind::Platform, 4),
            (PieceKind::Kicker, 0),
            (PieceKind::Spine, 1),
        ] {
            let g = Piece::new(kind).geometry();
            assert_eq!(g.rails.len(), rails, "{kind:?}");
            let top = g.all_quads().flatten().map(|p| p.y).fold(0.0, f32::max);
            for rail in &g.rails {
                assert!(rail.len() >= 2);
                assert!(rail.iter().all(|p| (p.y - top).abs() < 1e-4), "{kind:?} rail sits on top");
            }
        }
    }

    #[test]
    fn stair_handrail_runs_down_the_steps_above_them() {
        let g = Piece::new(PieceKind::Stairs).geometry();
        assert_eq!(g.rails.len(), 1);
        let (top, bottom) = (g.rails[0][0], g.rails[0][1]);
        assert!(top.y > bottom.y && bottom.z > top.z, "slopes down towards +z");
        let tube_top = g.rail_quads.iter().flatten().map(|p| p.y).fold(0.0, f32::max);
        assert!((tube_top - top.y).abs() < 1e-4, "the rail is the tube's top");
        let c = g.rail_quads.iter().flatten().sum::<Vec3>() / (g.rail_quads.len() * 4) as f32;
        for q in &g.rail_quads {
            let mid = q.iter().sum::<Vec3>() / 4.0;
            assert!(quad_normal(q).dot(mid - c) > 0.0, "tube faces point out");
        }
        let steps_up = g.quads.iter().filter(|q| quad_normal(q).normalize().y > 0.99).count();
        assert!(steps_up >= 6, "deck plus five treads");
    }

    #[test]
    fn spine_peaks_at_its_coping() {
        let g = Piece::new(PieceKind::Spine).geometry();
        let top = g.all_quads().flatten().map(|p| p.y).fold(0.0, f32::max);
        assert!(g.rails[0].iter().all(|p| p.z.abs() < 1e-5 && (p.y - top).abs() < 1e-4));
    }

    #[test]
    fn textures_follow_a_transition_without_a_seam_or_a_stretch() {
        for kind in PieceKind::ALL {
            let g = Piece::new(kind).geometry();
            let uvs = g.uvs();
            assert_eq!(uvs.len(), g.all_quads().count());
            for (index, (q, uv)) in g.quads.iter().zip(&uvs).enumerate() {
                // A texture metre is a metre of the face, along each of its edges (a surface
                // curved two ways, a bowl's corners, can't keep that, and brings its own).
                if g.given_uvs.iter().any(|(i, _)| *i == index) {
                    assert!(uv.iter().all(|v| v.is_finite()));
                    continue;
                }
                for i in 0..4 {
                    let (a, b) = (i, (i + 1) % 4);
                    let (face, texture) = (q[a].distance(q[b]), uv[a].distance(uv[b]));
                    assert!((face - texture).abs() < 1e-3, "{kind:?}: an edge of {face} m is {texture} m of texture");
                }
            }
        }
        // Round a quarter pipe's curve, neighbouring faces meet at the same v.
        let g = Piece::new(PieceKind::QuarterPipe).geometry();
        let uvs = g.uvs();
        let ramp: Vec<_> = g.quads.iter().zip(&uvs).filter(|(q, _)| quad_normal(q).normalize().x.abs() < 0.1).collect();
        let mut joined = 0;
        for (i, (q, uv)) in ramp.iter().enumerate() {
            for (r, rv) in &ramp[i + 1..] {
                for a in 0..4 {
                    for b in 0..4 {
                        if q[a].distance(r[b]) < 1e-4 && (uv[a].y - rv[b].y).abs() < 1e-3 {
                            joined += 1;
                        }
                    }
                }
            }
        }
        assert!(joined >= 2 * 17, "the transition's faces share their edges' texture: {joined}");
    }

    #[test]
    fn every_kind_has_a_finish_and_keeps_only_its_own() {
        for kind in PieceKind::ALL {
            assert!(!kind.finishes().is_empty());
            assert_eq!(kind.finish(None), kind.finishes()[0]);
        }
        assert_eq!(PieceKind::Rail.finish(Some(Finish::Brick)), Finish::Steel);
        assert_eq!(PieceKind::Wall.finish(Some(Finish::Concrete)), Finish::Concrete);
    }

    #[test]
    fn a_bowl_is_a_closed_pool_under_its_rim() {
        let g = Piece::new(PieceKind::Bowl).geometry();
        let (min, max) = g.footprint();
        // Every face looks into the pool or up, and the rim's rectangle is all covered at y = 0
        // or below: nothing to fall through at the corners.
        for q in &g.quads {
            let centre = (q[0] + q[1] + q[2] + q[3]) * 0.25;
            let n = (quad_normal(q) + (q[3] - q[0]).cross(q[2] - q[0])).normalize();
            assert!(n.dot(Vec3::new(0.0, 1.5, 0.0) - centre) > 0.0, "{q:?}");
        }
        let area: f32 = g
            .quads
            .iter()
            .map(|q| {
                let flat = q.map(|p| Vec3::new(p.x, 0.0, p.z));
                0.5 * ((flat[2] - flat[0]).cross(flat[1] - flat[0]).length() + (flat[3] - flat[0]).cross(flat[2] - flat[0]).length())
            })
            .sum();
        let rectangle = (max.x - min.x) * (max.y - min.y);
        assert!((area - rectangle).abs() < 0.02 * rectangle, "seen from above it fills its rectangle: {area} of {rectangle}");
        // Its corners are ridden, not drawn.
        assert_eq!(g.unseen.len(), 4 * 3);
        assert!(g.unseen.iter().all(|&i| g.quads[i].iter().all(|p| p.y.abs() < 1e-6)));
        // The coping runs all the way round, at the rim.
        let rim = &g.rails[0];
        assert!(rim[0].distance(*rim.last().unwrap()) < 1e-4 && rim.iter().all(|p| p.y.abs() < 1e-5));
    }

    #[test]
    fn a_curved_rail_turns_a_quarter_at_one_height() {
        let g = Piece::new(PieceKind::CurvedRail).geometry();
        let rail = &g.rails[0];
        let (first, last) = (rail[1] - rail[0], rail[rail.len() - 1] - rail[rail.len() - 2]);
        assert!(first.normalize().dot(last.normalize()).abs() < 0.2, "its ends are a quarter turn apart");
        assert!(rail.iter().all(|p| (p.y - rail[0].y).abs() < 1e-5));
        let (min, max) = g.footprint();
        assert!((min + max).length() < 0.2, "centred on its origin: {min} {max}");
    }

    #[test]
    fn width_stretches_only_the_kinds_that_have_one_and_only_across() {
        for kind in PieceKind::ALL {
            let extent = |piece: Piece| {
                let g = piece.geometry();
                let (lo, hi) = g.all_quads().flatten().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), &p| (lo.min(p), hi.max(p)));
                hi - lo
            };
            let (narrow, wide) = (extent(Piece::new(kind)), extent(Piece { width: 2.0, ..Piece::new(kind) }));
            match kind.width_axis() {
                Some(axis) => {
                    assert!((wide - narrow).dot(axis) > 0.3, "{kind:?} is wider");
                    assert!(((wide - narrow) * (Vec3::ONE - axis)).length() < 1e-4, "{kind:?} is only wider");
                }
                None => assert!((wide - narrow).length() < 1e-5, "{kind:?} has no width"),
            }
        }
    }

    #[test]
    fn tints_step_round_the_hues_and_stop_at_dark_and_light() {
        let t = Tint { hue: 23, light: 6 };
        assert_eq!(t.stepped(1, 1), Tint { hue: 0, light: 6 });
        assert_eq!(Tint { hue: 0, light: 0 }.stepped(-1, -1), Tint { hue: 23, light: 0 });
        let old: Piece = serde_json::from_str(r#"{"kind":"Rail","size":1.5}"#).unwrap();
        assert_eq!(old.width, 1.0, "pieces from before widths load");
        assert!(!serde_json::to_string(&Piece::new(PieceKind::Rail)).unwrap().contains("width"));
    }

    #[test]
    fn size_scales_the_main_dimension() {
        let small = Piece::sized(PieceKind::Rail, 1.0).geometry();
        let big = Piece::sized(PieceKind::Rail, 2.0).geometry();
        let len = |g: &PieceGeometry| g.rails[0][0].distance(g.rails[0][1]);
        assert!((len(&big) - 2.0 * len(&small)).abs() < 1e-4);
    }
}
