//! How a ramp is dressed when it is drawn: built of plywood sheets, each with
//! its own run of grain and its own tone, with the seams between them, the
//! sheets' edges showing down its sides, a steel plate where it meets the
//! ground and a coping tube on its lip.
//!
//! None of it is ridden: the piece skates as its plain shape
//! (`PieceGeometry::quads`). The sheets are that shape cut along the seams
//! ([`sheets`]: the piece's mesh, in wood); the rest is a second mesh over it
//! ([`details`]), coloured by its corners and drawn without a photograph.
use super::pieces::{PieceGeometry, Ramp, quad_normal};
use bevy::prelude::*;

/// A sheet of plywood is no wider than this across a ramp, m.
const SHEET_M: f32 = 2.6;
/// A seam between two sheets: how wide it is drawn, and how far off the surface.
const SEAM_M: f32 = 0.012;
const LIFT_M: f32 = 0.003;
/// How much of a sheet's edge shows down a ramp's side.
const EDGE_M: f32 = 0.05;
/// A coping tube's radius, and how far its top stands over the deck.
const COPING_M: f32 = 0.032;
const COPING_PROUD_M: f32 = 0.012;
/// A hollow ramp's frame: how deep and how thick its timbers are.
const TIMBER_M: f32 = 0.12;
const TIMBER_THICK_M: f32 = 0.05;
/// The plate at a ramp's foot: its length up the ramp at most, and as a share of the ramp.
const PLATE_M: f32 = 0.3;
const PLATE_SHARE: f32 = 0.14;

/// A mesh's corners, three to a triangle.
#[derive(Default)]
pub(super) struct Dressed {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Linear; over a photograph, they multiply it.
    pub colors: Vec<[f32; 4]>,
}

#[derive(Clone, Copy)]
struct Corner {
    at: Vec3,
    normal: Vec3,
    uv: Vec2,
}

impl Dressed {
    /// A flat or curved face of any number of corners (convex), wound as the pieces' quads are.
    fn face(&mut self, corners: &[Corner], color: [f32; 4]) {
        for i in 1..corners.len().saturating_sub(1) {
            for c in [corners[0], corners[i + 1], corners[i]] {
                self.positions.push(c.at.to_array());
                self.normals.push(c.normal.to_array());
                self.uvs.push(c.uv.to_array());
                self.colors.push(color);
            }
        }
    }

    /// A flat face, facing `normal`.
    fn quad(&mut self, quad: [Vec3; 4], normal: Vec3, color: [f32; 4]) {
        let n = quad_normal(&quad) + (quad[3] - quad[0]).cross(quad[2] - quad[0]);
        if n.length_squared() < 1e-12 {
            return;
        }
        let quad = if n.dot(normal) >= 0.0 { quad } else { [quad[0], quad[3], quad[2], quad[1]] };
        self.face(&quad.map(|at| Corner { at, normal, uv: Vec2::ZERO }), color);
    }
}

fn linear(r: f32, g: f32, b: f32) -> [f32; 4] {
    Color::srgb(r, g, b).to_linear().to_f32_array()
}

/// A number from 0 to 1 that is always the same for the same sheet.
fn chance(sheet: (u32, u32, u32), salt: u32) -> f32 {
    let mut h = sheet.0.wrapping_mul(0x9e37_79b9) ^ sheet.1.wrapping_mul(0x85eb_ca6b) ^ sheet.2.wrapping_mul(0xc2b2_ae35) ^ salt.wrapping_mul(0x27d4_eb2f);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

/// The part of a face on each side of a level cut (`axis` 0: x, 2: z) at `value`: below, above.
fn cut(face: &[Corner], axis: usize, value: f32) -> (Vec<Corner>, Vec<Corner>) {
    let (mut below, mut above) = (Vec::new(), Vec::new());
    for i in 0..face.len() {
        let (a, b) = (face[i], face[(i + 1) % face.len()]);
        let (da, db) = (a.at[axis] - value, b.at[axis] - value);
        if da <= 0.0 {
            below.push(a);
        }
        if da >= 0.0 {
            above.push(a);
        }
        if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
            let t = da / (da - db);
            let at = Corner { at: a.at.lerp(b.at, t), normal: a.normal.lerp(b.normal, t).normalize_or(a.normal), uv: a.uv.lerp(b.uv, t) };
            below.push(at);
            above.push(at);
        }
    }
    (below, above)
}

/// A ramp's ends that stand up off the ground, where a wall closes it or a
/// frame holds it open: where along it (z), which way is out (z), how high.
fn backs(ramp: &Ramp) -> Vec<(f32, f32, f32)> {
    let (Some(first), Some(last)) = (ramp.profile.first(), ramp.profile.last()) else { return Vec::new() };
    [(first, -1.0), (last, 1.0)].into_iter().filter(|(end, _)| end.y > 0.05).map(|(end, out)| (end.x, out, end.y)).collect()
}

/// A timber: a box, its grain along its longest side, in pine.
fn timber(out: &mut Dressed, min: Vec3, max: Vec3, sheet: (u32, u32, u32)) {
    let size = max - min;
    if size.min_element() <= 1e-4 {
        return;
    }
    let long = if size.x >= size.y && size.x >= size.z { 0 } else if size.y >= size.z { 1 } else { 2 };
    let tone = 0.8 + 0.2 * chance(sheet, 3);
    let color = [tone * 1.04, tone * 0.93, tone * 0.72, 1.0];
    let shift = Vec2::new(chance(sheet, 1), chance(sheet, 2)) * 7.0;
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for (side, normal) in [(min[axis], -1.0), (max[axis], 1.0)] {
            let corner = |a: f32, b: f32| {
                let mut at = Vec3::ZERO;
                at[axis] = side;
                at[u] = a;
                at[v] = b;
                let mut n = Vec3::ZERO;
                n[axis] = normal;
                // (along the grain, then across it)
                let across = if u == long { b } else { a };
                Corner { at, normal: n, uv: Vec2::new(at[long], across + side) + shift }
            };
            let face = [corner(min[u], min[v]), corner(max[u], min[v]), corner(max[u], max[v]), corner(min[u], max[v])];
            let wound = quad_normal(&face.map(|c| c.at));
            let face = if wound.dot(face[0].normal) >= 0.0 { face } else { [face[0], face[3], face[2], face[1]] };
            out.face(&face, color);
        }
    }
}

/// Whether a face has any size left.
fn solid(face: &[Corner]) -> bool {
    face.len() >= 3 && (1..face.len() - 1).any(|i| (face[i].at - face[0].at).cross(face[i + 1].at - face[0].at).length() > 1e-7)
}

/// Where a ramp `width` across is cut into sheets no wider than [`SHEET_M`] (x).
fn across(width: f32) -> Vec<f32> {
    let n = (width / SHEET_M).ceil().max(1.0) as usize;
    (1..n).map(|i| -width * 0.5 + width * i as f32 / n as f32).collect()
}

/// Where one sheet ends and the next begins along a ramp (z): its joints and its lips.
fn along(ramp: &Ramp) -> Vec<f32> {
    let mut cuts: Vec<f32> = ramp.joints.iter().copied().chain(ramp.copings.iter().map(|c| c.0.x)).collect();
    cuts.sort_by(f32::total_cmp);
    cuts
}

/// A ramp's own mesh, in wood: its faces cut along the seams between its
/// sheets, each sheet with its own stretch of the photograph and its own
/// tone. None: not a ramp (it is drawn as it is ridden).
pub(super) fn sheets(g: &PieceGeometry) -> Option<Dressed> {
    let ramp = g.ramp.as_ref()?;
    let (across, along) = (across(ramp.width), along(ramp));
    let (normals, uvs) = (g.normals(), g.uvs());
    let mut out = Dressed::default();
    for (index, q) in g.all_quads().enumerate().filter(|(index, _)| !g.unseen.contains(index)) {
        let facing = (quad_normal(q) + (q[3] - q[0]).cross(q[2] - q[0])).normalize_or_zero();
        let face: Vec<Corner> = (0..4).map(|i| Corner { at: q[i], normal: normals[index][i], uv: uvs[index][i] }).collect();
        // (a side is one board from end to end of each stretch; what is ridden, and the walls
        // behind it, are sheets across)
        let side = facing.x.abs() > 0.7;
        // A hollow ramp has no wall behind its decks (the block a raised one stands on stays).
        if ramp.hollow && !side && facing.y <= 0.02 && q.iter().all(|p| p.y > -1e-4) {
            continue;
        }
        let mut parts = vec![face];
        for (axis, cuts) in [(0, &across), (2, &along)] {
            if side && axis == 0 {
                continue;
            }
            for &value in cuts {
                parts = parts.into_iter().flat_map(|part| { let (a, b) = cut(&part, axis, value); [a, b] }).filter(|part| solid(part)).collect();
            }
        }
        for mut part in parts {
            let middle = part.iter().map(|c| c.at).sum::<Vec3>() / part.len() as f32;
            let column = across.iter().filter(|&&x| x < middle.x).count() as u32;
            let stretch = along.iter().filter(|&&z| z < middle.z).count() as u32;
            let kind = if side { 1 + (facing.x > 0.0) as u32 } else if facing.y > 0.02 { 0 } else { 3 };
            let sheet = (kind, if side { 0 } else { column }, stretch);
            // Its own stretch of the grain, and a tone a little its own.
            let shift = Vec2::new(chance(sheet, 1), chance(sheet, 2)) * 7.0;
            let tone = 0.74 + 0.3 * chance(sheet, 3);
            let warm = 0.95 + 0.1 * chance(sheet, 4);
            for corner in &mut part {
                corner.uv += shift;
            }
            out.face(&part, [tone * warm, tone, tone / warm, 1.0]);
        }
    }
    // A hollow ramp's frame, seen through its open backs: a timber under the deck, one on the
    // ground, one between on a tall ramp, and a stud up the middle behind them.
    if ramp.hollow {
        let x = ramp.width * 0.5 - 0.02;
        for (end, (z, out_z, height)) in backs(ramp).into_iter().enumerate() {
            let end = end as u32;
            let slab = |from: f32, to: f32| (z - out_z * from, z - out_z * to);
            let rail = |out: &mut Dressed, low: f32, high: f32, index: u32| {
                let (a, b) = slab(0.0, TIMBER_THICK_M);
                timber(out, Vec3::new(-x, low, a.min(b)), Vec3::new(x, high, a.max(b)), (4, end, index));
            };
            rail(&mut out, height - 0.004 - TIMBER_M, height - 0.004, 0);
            rail(&mut out, 0.0, TIMBER_M.min(height * 0.3), 1);
            if height > 1.2 {
                rail(&mut out, (height - TIMBER_M) * 0.5, (height + TIMBER_M) * 0.5, 2);
            }
            let (a, b) = slab(TIMBER_THICK_M, TIMBER_THICK_M * 2.0);
            timber(&mut out, Vec3::new(-TIMBER_M * 0.5, 0.0, a.min(b)), Vec3::new(TIMBER_M * 0.5, height - 0.004, a.max(b)), (4, end, 3));
        }
    }
    Some(out)
}

/// What is drawn over a ramp and not ridden: its coping tubes, and, when it
/// is `wooden`, the seams between its sheets, their edges down its sides and
/// the steel plate at its foot. None: not a ramp, or nothing to add.
pub(super) fn details(g: &PieceGeometry, wooden: bool) -> Option<Dressed> {
    let ramp = g.ramp.as_ref()?;
    let mut out = Dressed::default();
    let x = ramp.width * 0.5;
    let steel = linear(0.66, 0.68, 0.71);
    let rubber = linear(0.07, 0.07, 0.075);
    let seam = linear(0.2, 0.14, 0.09);
    let edge = linear(0.4, 0.28, 0.16);
    let plate = linear(0.16, 0.16, 0.175);

    // A tube on each lip, round, with its ends closed.
    const ROUND: usize = 10;
    for &(lip, black) in &ramp.copings {
        let color = if black { rubber } else { steel };
        let centre = Vec2::new(lip.x, lip.y + COPING_PROUD_M - COPING_M);
        let spoke = |i: usize| {
            let a = std::f32::consts::TAU * i as f32 / ROUND as f32;
            Vec3::new(0.0, a.sin(), a.cos())
        };
        let on = |side: f32, i: usize| Vec3::new(side, centre.y, centre.x) + spoke(i) * COPING_M;
        for i in 0..ROUND {
            let (n0, n1) = (spoke(i), spoke(i + 1));
            let corners = [(-x, i, n0), (x, i, n0), (x, i + 1, n1), (-x, i + 1, n1)].map(|(side, i, normal)| Corner { at: on(side, i), normal, uv: Vec2::ZERO });
            let facing = quad_normal(&corners.map(|c| c.at));
            let corners = if facing.dot(n0 + n1) >= 0.0 { corners } else { [corners[0], corners[3], corners[2], corners[1]] };
            out.face(&corners, color);
        }
        for side in [-x, x] {
            let normal = Vec3::X * side.signum();
            let mut cap: Vec<Corner> = (0..ROUND).map(|i| Corner { at: on(side, i), normal, uv: Vec2::ZERO }).collect();
            if quad_normal(&[cap[0].at, cap[1].at, cap[2].at, cap[3].at]).dot(normal) < 0.0 {
                cap.reverse();
            }
            out.face(&cap, color);
        }
    }

    if wooden {
        let profile = &ramp.profile;
        let at = |x: f32, p: Vec2| Vec3::new(x, p.y, p.x);
        // The way each stretch of the profile faces (up, or into the open), and each point's.
        let facing = |p: Vec2, q: Vec2| Vec2::new(-(q.y - p.y), q.x - p.x).normalize_or(Vec2::Y);
        let stretches: Vec<Vec2> = profile.windows(2).map(|w| facing(w[0], w[1])).collect();
        let points: Vec<Vec2> = (0..profile.len())
            .map(|i| (stretches[i.saturating_sub(1)] + stretches[i.min(stretches.len() - 1)]).normalize_or(Vec2::Y))
            .collect();
        let out_of = |n: Vec2| Vec3::new(0.0, n.y, n.x);

        // Seams: between the sheets side by side, all the way along; and across, at each joint.
        for &cut in &across(ramp.width) {
            for (w, n) in profile.windows(2).zip(&stretches) {
                let up = out_of(*n) * LIFT_M;
                let (a, b) = (at(cut - SEAM_M * 0.5, w[0]) + up, at(cut + SEAM_M * 0.5, w[0]) + up);
                let (c, d) = (at(cut + SEAM_M * 0.5, w[1]) + up, at(cut - SEAM_M * 0.5, w[1]) + up);
                out.quad([a, b, c, d], out_of(*n), seam);
            }
        }
        for &joint in &ramp.joints {
            let Some(i) = profile.windows(2).position(|w| w[0].x <= joint && joint <= w[1].x && w[1].x > w[0].x) else { continue };
            let (p, q) = (profile[i], profile[i + 1]);
            let here = p.lerp(q, (joint - p.x) / (q.x - p.x));
            let run = (q - p).normalize_or(Vec2::X) * SEAM_M * 0.5;
            let up = out_of(stretches[i]) * LIFT_M;
            out.quad([at(-x, here - run) + up, at(x, here - run) + up, at(x, here + run) + up, at(-x, here + run) + up], out_of(stretches[i]), seam);
        }

        // The sheets' edges, round each side: under the ridden surface, along the ground, and up
        // each end that stands off it.
        for side in [-x, x] {
            let normal = Vec3::X * side.signum();
            let off = side + side.signum() * LIFT_M;
            let under = |i: usize| {
                let p = profile[i] - points[i] * EDGE_M;
                Vec2::new(p.x, p.y.max(0.0))
            };
            let over = |i: usize| Vec2::new(profile[i].x, profile[i].y.min(EDGE_M));
            for i in 0..profile.len() - 1 {
                out.quad([at(off, profile[i]), at(off, profile[i + 1]), at(off, under(i + 1)), at(off, under(i))], normal, edge);
                let (a, b) = (profile[i].with_y(0.0), profile[i + 1].with_y(0.0));
                out.quad([at(off, a), at(off, b), at(off, over(i + 1)), at(off, over(i))], normal, edge);
            }
            for (z, out_z, height) in backs(ramp) {
                let inner = z - out_z * EDGE_M;
                out.quad([Vec3::new(off, 0.0, z), Vec3::new(off, height, z), Vec3::new(off, height, inner), Vec3::new(off, 0.0, inner)], normal, edge);
            }
        }
        // And round the wall that closes each such end (a hollow ramp has none), with the seams
        // between its sheets.
        for (z, out_z, height) in backs(ramp).into_iter().filter(|_| !ramp.hollow) {
            let normal = Vec3::Z * out_z;
            let off = z + out_z * LIFT_M;
            let mut band = |x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]| {
                out.quad([Vec3::new(x0, y0, off), Vec3::new(x1, y0, off), Vec3::new(x1, y1, off), Vec3::new(x0, y1, off)], normal, color);
            };
            band(-x, 0.0, x, EDGE_M.min(height * 0.5), edge);
            band(-x, height - EDGE_M.min(height * 0.5), x, height, edge);
            band(-x, 0.0, -x + EDGE_M, height, edge);
            band(x - EDGE_M, 0.0, x, height, edge);
            for &cut in &across(ramp.width) {
                band(cut - SEAM_M * 0.5, 0.0, cut + SEAM_M * 0.5, height, seam);
            }
        }

        // A steel plate over each foot: where the ramp comes down to the ground at its end.
        let length: f32 = profile.windows(2).map(|w| w[0].distance(w[1])).sum();
        let reach = PLATE_M.min(length * PLATE_SHARE);
        let last = profile.len() - 1;
        for (foot, step) in [(0usize, 1isize), (last, -1)] {
            if profile[foot].y > 0.01 {
                continue;
            }
            let (mut left, mut i) = (reach, foot as isize);
            while left > 1e-4 && (i + step) >= 0 && (i + step) <= last as isize {
                let (p, q) = (profile[i as usize], profile[(i + step) as usize]);
                let stretch = p.distance(q);
                let q = if stretch > left { p.lerp(q, left / stretch) } else { q };
                let n = stretches[i.min(i + step) as usize];
                let up = out_of(n) * (LIFT_M * 1.5);
                out.quad([at(-x, p) + up, at(x, p) + up, at(x, q) + up, at(-x, q) + up], out_of(n), plate);
                left -= stretch;
                i += step;
            }
        }
    }
    (!out.positions.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::super::pieces::{Piece, PieceKind};
    use super::*;

    fn area(mesh: &Dressed) -> f32 {
        mesh.positions.chunks_exact(3).map(|t| (Vec3::from_array(t[1]) - Vec3::from_array(t[0])).cross(Vec3::from_array(t[2]) - Vec3::from_array(t[0])).length() * 0.5).sum()
    }

    #[test]
    fn a_ramp_in_sheets_is_the_ramp_it_was() {
        for kind in [PieceKind::Kicker, PieceKind::QuarterPipe, PieceKind::HalfPipe, PieceKind::Spine] {
            let g = Piece::new(kind).geometry();
            let mesh = sheets(&g).expect("a ramp");
            // Cut along its seams it covers what it did, and faces the way it did.
            let whole: f32 = g.all_quads().map(|q| (quad_normal(q).length() + (q[3] - q[0]).cross(q[2] - q[0]).length()) * 0.5).sum();
            // (a hollow one has its back walls out and a frame in)
            if !g.ramp.as_ref().unwrap().hollow {
                assert!((area(&mesh) - whole).abs() < whole * 1e-3, "{kind:?}: {} of {whole}", area(&mesh));
            }
            for (t, n) in mesh.positions.chunks_exact(3).zip(mesh.normals.chunks_exact(3)) {
                let [a, b, c] = [t[0], t[1], t[2]].map(Vec3::from_array);
                let wound = (b - a).cross(c - a);
                // (an end cap that comes to a point leaves a triangle of no size)
                assert!(wound.length() < 1e-6 || wound.dot(Vec3::from_array(n[0])) > 0.0, "{kind:?}: a face wound away from its normal");
            }
            assert_eq!(mesh.positions.len(), mesh.colors.len());
            assert_eq!(mesh.positions.len(), mesh.uvs.len());
        }
        assert!(sheets(&Piece::new(PieceKind::Ledge).geometry()).is_none());
    }

    #[test]
    fn a_half_pipes_flat_is_four_sheets() {
        let g = Piece::new(PieceKind::HalfPipe).geometry();
        let ramp = g.ramp.as_ref().unwrap();
        assert_eq!(across(ramp.width), vec![0.0]);
        // The flat (y = 0, between the transitions): its faces fall in four quarters, each its own tone.
        let mesh = sheets(&g).unwrap();
        let mut tones: Vec<(bool, bool, [u32; 3])> = Vec::new();
        for (t, c) in mesh.positions.chunks_exact(3).zip(mesh.colors.chunks_exact(3)) {
            let corners = [t[0], t[1], t[2]].map(Vec3::from_array);
            let middle = (corners[0] + corners[1] + corners[2]) / 3.0;
            if corners.iter().all(|p| p.y.abs() < 1e-5) && middle.z.abs() < 1.49 {
                let facing = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
                if facing.y > 0.0 {
                    tones.push((middle.x > 0.0, middle.z > 0.0, [c[0][0].to_bits(), c[0][1].to_bits(), c[0][2].to_bits()]));
                }
            }
        }
        tones.sort();
        tones.dedup();
        assert_eq!(tones.len(), 4, "{tones:?}");
        let mut only: Vec<[u32; 3]> = tones.iter().map(|t| t.2).collect();
        only.dedup();
        assert_eq!(only.len(), 4, "each quarter is its own sheet");
    }

    #[test]
    fn a_half_pipe_is_open_behind_its_decks_on_a_frame() {
        let g = Piece::new(PieceKind::HalfPipe).geometry();
        let ramp = g.ramp.as_ref().unwrap();
        assert!(ramp.hollow && backs(ramp).len() == 2);
        let end = ramp.profile[0].x;
        // Nothing of it fills the plane of its back but the frame's timbers: far less than the wall did.
        let mesh = sheets(&g).unwrap();
        let on_the_back: f32 = mesh
            .positions
            .chunks_exact(3)
            .filter(|t| t.iter().all(|p| (p[2] - end).abs() < 1e-4))
            .map(|t| (Vec3::from_array(t[1]) - Vec3::from_array(t[0])).cross(Vec3::from_array(t[2]) - Vec3::from_array(t[0])).length() * 0.5)
            .sum();
        let wall = ramp.width * ramp.profile[0].y;
        assert!(on_the_back > 0.5 && on_the_back < wall * 0.25, "{on_the_back} of {wall}");
        // It is ridden as the solid it was: the wall is still there to run into.
        assert!(g.quads.iter().any(|q| q.iter().all(|p| (p.z - end).abs() < 1e-4)));
        // A quarter pipe is closed.
        let quarter = Piece::new(PieceKind::QuarterPipe).geometry();
        assert!(!quarter.ramp.as_ref().unwrap().hollow);
    }

    #[test]
    fn the_details_are_what_each_ramp_has() {
        let count = |kind, wooden| details(&Piece::new(kind).geometry(), wooden).map_or(0, |d| d.positions.len() / 3);
        // In concrete: only the coping, where there is one.
        assert_eq!(count(PieceKind::Kicker, false), 0);
        assert!(count(PieceKind::QuarterPipe, false) > 0);
        assert_eq!(count(PieceKind::HalfPipe, false), 2 * count(PieceKind::QuarterPipe, false));
        // In wood: more of everything.
        for kind in [PieceKind::Kicker, PieceKind::QuarterPipe, PieceKind::HalfPipe, PieceKind::Spine] {
            assert!(count(kind, true) > count(kind, false) + 20, "{kind:?}");
        }
        // The quarter pipe's lip is black, the half pipe's steel; nothing of either is under the ground.
        let quarter = details(&Piece::new(PieceKind::QuarterPipe).geometry(), false).unwrap();
        let half = details(&Piece::new(PieceKind::HalfPipe).geometry(), false).unwrap();
        assert!(quarter.colors[0][0] < 0.05 && half.colors[0][0] > 0.3);
        let wooden = details(&Piece::new(PieceKind::HalfPipe).geometry(), true).unwrap();
        assert!(wooden.positions.iter().all(|p| p[1] > -1e-4));
        assert!(details(&Piece::new(PieceKind::Ledge).geometry(), true).is_none());
    }
}
