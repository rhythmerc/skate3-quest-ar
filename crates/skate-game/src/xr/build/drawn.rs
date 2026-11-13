//! Drawn pieces: a rail, a deck (ramp or platform), a pad and a bowl made
//! from points the player places, instead of a fixed shape stretched.
//!
//! A [`Path`] is those points (nodes) in the piece's own metres, from its
//! origin (the first node, on its surface). An open path is a centre line
//! (rail, deck); a closed one an outline (pad, bowl). It runs smoothly
//! through its nodes unless a node is a corner, and the stretch between two
//! nodes can be pulled aside by its one bend handle. Heights go straight
//! from node to node (or eased, for a deck that is a transition).
//!
//! Outlines are filled from their middle, so a pad or bowl should be a
//! shape whose middle sees all of its edge (no hooks or hourglasses).
use super::{MAX_SIZE, MIN_SIZE, Piece, PieceGeometry, PieceKind, box_quads, push, swept_tube};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::f32::consts::FRAC_PI_2;

/// The most nodes a drawn piece has.
pub(crate) const MAX_NODES: usize = 16;
/// A drawn rail's top above its nodes, and its thickness (as the ready-made rail's).
pub(crate) const RAIL_TOP: f32 = 0.5;
const TUBE: f32 = 0.08;
/// A rail's end posts stand this far in from its ends (less on a short rail).
const END_POST_INSET: f32 = 0.6;
/// A deck's width at width 1.
const DECK_WIDTH: f32 = 2.0;
/// A bowl's depth at size 1.
const BOWL_DEPTH: f32 = 1.2;
/// A side this far above its surface is a ledge: grindable, climbable.
const LEDGE: f32 = 0.25;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Node {
    /// From the piece's origin; y is the piece's height here.
    pub at: [f32; 3],
    /// The height of the surface under it.
    #[serde(default)]
    pub base: f32,
    /// The path turns sharply here.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub corner: bool,
    /// How far the middle of the stretch to the next node is pulled aside (x, z).
    #[serde(default)]
    pub bend: [f32; 2],
}

impl Node {
    pub(crate) fn new(at: Vec3, base: f32) -> Self {
        Self { at: at.to_array(), base, corner: false, bend: [0.0; 2] }
    }

    fn p(&self) -> Vec3 {
        Vec3::from_array(self.at)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Path {
    pub nodes: Vec<Node>,
    /// Heights ease in and out between nodes (a transition) instead of
    /// going straight (a bank).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub eased: bool,
}

/// A point along a path: where, and the surface's height under it.
#[derive(Clone, Copy, Debug)]
struct Sample {
    p: Vec3,
    base: f32,
}

impl PieceKind {
    /// Made from points the player places.
    pub(crate) fn drawn(self) -> bool {
        matches!(self, Self::RailPath | Self::Deck | Self::Pad | Self::Pool)
    }

    /// Its points are an outline, not a line.
    pub(crate) fn closed(self) -> bool {
        matches!(self, Self::Pad | Self::Pool)
    }

    /// The fewest nodes it can have.
    pub(crate) fn least_nodes(self) -> usize {
        if self.closed() { 3 } else { 2 }
    }
}

impl Path {
    /// The stretches of the path: one from each node to the next (the last
    /// back to the first when closed).
    pub(crate) fn stretches(&self, closed: bool) -> usize {
        match self.nodes.len() {
            0 | 1 => 0,
            n if closed => n,
            n => n - 1,
        }
    }

    fn node(&self, i: isize, closed: bool) -> Option<&Node> {
        let n = self.nodes.len() as isize;
        if closed { self.nodes.get(i.rem_euclid(n) as usize) } else { usize::try_from(i).ok().and_then(|i| self.nodes.get(i)) }
    }

    /// The direction (over the ground, unit) the path leaves node `i` in
    /// (`out`) or arrives at it in.
    fn heading(&self, i: usize, out: bool, closed: bool) -> Vec2 {
        let i = i as isize;
        let at = |j: isize| self.node(j, closed).map(|n| n.p().xz());
        let here = at(i).unwrap_or_default();
        let (before, after) = (at(i - 1), at(i + 1));
        let corner = self.node(i, closed).is_some_and(|n| n.corner);
        let through = match (before, after) {
            (Some(b), Some(a)) if !corner => a - b,
            (_, Some(a)) if out => a - here,
            (Some(b), _) if !out => here - b,
            (Some(b), None) => here - b,
            (None, Some(a)) => a - here,
            _ => Vec2::X,
        };
        through.normalize_or(Vec2::X)
    }

    /// Where the path is `t` (0-1) of the way along stretch `i`; `bent`:
    /// with its bend.
    fn along(&self, i: usize, t: f32, closed: bool, bent: bool) -> Sample {
        let (a, b) = (self.nodes[i], *self.node(i as isize + 1, closed).expect("a stretch has an end"));
        let (p0, p1) = (a.p().xz(), b.p().xz());
        let chord = p0.distance(p1);
        let (m0, m1) = (self.heading(i, true, closed) * chord, self.heading((i + 1) % self.nodes.len(), false, closed) * chord);
        let (t2, t3) = (t * t, t * t * t);
        let mut at = p0 * (2.0 * t3 - 3.0 * t2 + 1.0) + m0 * (t3 - 2.0 * t2 + t) + p1 * (-2.0 * t3 + 3.0 * t2) + m1 * (t3 - t2);
        if bent {
            // All of it at the middle, none (and no turn) at the nodes.
            at += Vec2::from_array(a.bend) * 16.0 * t2 * (1.0 - t) * (1.0 - t);
        }
        let rise = if self.eased { t2 * (3.0 - 2.0 * t) } else { t };
        Sample { p: Vec3::new(at.x, a.at[1] + (b.at[1] - a.at[1]) * rise, at.y), base: a.base + (b.base - a.base) * t }
    }

    /// The middle of stretch `i`: where its bend handle is.
    pub(crate) fn middle(&self, i: usize, closed: bool) -> Vec3 {
        self.along(i, 0.5, closed, true).p
    }

    /// The same without its bend: a bend is measured from here.
    pub(crate) fn straight_middle(&self, i: usize, closed: bool) -> Vec3 {
        self.along(i, 0.5, closed, false).p
    }

    /// The path as points close enough together to draw and ride (an even
    /// number of steps a stretch). A closed path doesn't repeat its first.
    fn line(&self, closed: bool) -> Vec<Sample> {
        let mut out = Vec::new();
        for i in 0..self.stretches(closed) {
            let (a, b) = (self.nodes[i], *self.node(i as isize + 1, closed).expect("a stretch has an end"));
            let straight = a.bend == [0.0; 2] && (a.corner || self.node(i as isize - 1, closed).is_none()) && (b.corner || self.node(i as isize + 2, closed).is_none());
            let steps = if straight && !self.eased { 2 } else { 2 * ((a.p().distance(b.p()) / 0.6).ceil() as usize).clamp(3, 8) };
            for step in 0..steps {
                out.push(self.along(i, step as f32 / steps as f32, closed, true));
            }
        }
        if !closed {
            if let Some(last) = self.nodes.last() {
                out.push(Sample { p: last.p(), base: last.base });
            }
        }
        out
    }

    /// Changes whenever the shape would.
    pub(crate) fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for n in &self.nodes {
            for v in n.at.iter().chain([&n.base]).chain(&n.bend) {
                v.to_bits().hash(&mut h);
            }
            n.corner.hash(&mut h);
        }
        self.eased.hash(&mut h);
        h.finish()
    }

    /// The shape a kind's icon shows (and a drawn piece without a path).
    pub(crate) fn sample_of(kind: PieceKind) -> Self {
        let node = |x: f32, y: f32, z: f32| Node::new(Vec3::new(x, y, z), 0.0);
        let nodes = match kind {
            PieceKind::RailPath => vec![node(-2.5, 0.0, 0.8), node(-0.8, 0.0, -0.6), node(0.8, 0.3, 0.6), node(2.5, 0.3, -0.8)],
            PieceKind::Deck => vec![node(-2.5, 0.0, 0.8), node(0.0, 0.8, -0.4), node(2.5, 0.8, 0.4)],
            PieceKind::Pad => vec![node(-1.8, 0.5, -1.0), node(1.6, 0.5, -1.4), node(2.2, 0.5, 0.9), node(-0.4, 0.5, 1.6)],
            _ => vec![node(-2.0, 0.0, -1.2), node(1.8, 0.0, -1.6), node(2.4, 0.0, 1.0), node(-0.6, 0.0, 1.8), node(-2.6, 0.0, 0.6)],
        };
        Self { nodes, eased: false }
    }

    /// The piece: what is ridden and drawn.
    pub(crate) fn geometry(&self, piece: Piece) -> PieceGeometry {
        let mut g = PieceGeometry::default();
        let closed = piece.kind.closed();
        if self.nodes.len() < piece.kind.least_nodes() {
            return g;
        }
        let line = self.line(closed);
        match piece.kind {
            PieceKind::RailPath => rail(&mut g, self, &line),
            PieceKind::Deck => deck(&mut g, &line, DECK_WIDTH * piece.width.clamp(MIN_SIZE, MAX_SIZE)),
            PieceKind::Pad => pad(&mut g, &line),
            _ => bowl(&mut g, &line, BOWL_DEPTH * piece.size.clamp(MIN_SIZE, MAX_SIZE)),
        }
        g
    }

    /// A deck's two ends, for other pieces to join: each edge's middle, the
    /// direction out of the deck, and its width.
    pub(crate) fn ends(&self, piece: Piece) -> Vec<(Vec3, Vec3, f32)> {
        if piece.kind != PieceKind::Deck || self.nodes.len() < 2 {
            return Vec::new();
        }
        let last = self.nodes.len() - 1;
        let level = |v: Vec2| Vec3::new(v.x, 0.0, v.y);
        let width = DECK_WIDTH * piece.width;
        vec![(self.nodes[0].p(), -level(self.heading(0, true, false)), width), (self.nodes[last].p(), level(self.heading(last, false, false)), width)]
    }
}

fn rail(g: &mut PieceGeometry, path: &Path, line: &[Sample]) {
    let top: Vec<Vec3> = line.iter().map(|s| s.p + Vec3::Y * RAIL_TOP).collect();
    swept_tube(&mut g.rail_quads, &top, TUBE);
    // A post under every node, and under the middle of a long stretch. The
    // two at the ends stand in from them, as the straight rail's do: under
    // the very end, a post stands out past the tube's end face, and the
    // board coming off the rail trips on it.
    let length: f32 = line.windows(2).map(|w| w[0].p.distance(w[1].p)).sum();
    let inset = END_POST_INSET.min(length * 0.2);
    let from_end = |mut samples: Box<dyn Iterator<Item = &Sample> + '_>| {
        let mut left = inset;
        let mut at = *samples.next().expect("a rail has points");
        for next in samples {
            let step = at.p.distance(next.p);
            if step >= left && step > 1e-6 {
                let t = left / step;
                return Sample { p: at.p.lerp(next.p, t), base: at.base + (next.base - at.base) * t };
            }
            left -= step;
            at = *next;
        }
        at
    };
    let last = path.nodes.len() - 1;
    let mut posts: Vec<Sample> = path.nodes[1..last].iter().map(|n| Sample { p: n.p(), base: n.base }).collect();
    posts.insert(0, from_end(Box::new(line.iter())));
    posts.push(from_end(Box::new(line.iter().rev())));
    for i in 0..path.stretches(false) {
        if path.nodes[i].p().distance(path.nodes[i + 1].p()) > 3.5 {
            posts.push(path.along(i, 0.5, false, true));
        }
    }
    for post in posts {
        let (p, top) = (post.p, post.p.y + RAIL_TOP - TUBE);
        if top > post.base {
            super::post(g, Vec3::new(p.x - 0.04, post.base, p.z - 0.04), Vec3::new(p.x + 0.04, top, p.z + 0.04));
        }
    }
    g.rails.push(top);
}

fn deck(g: &mut PieceGeometry, line: &[Sample], width: f32) {
    let n = line.len();
    // Each point's direction over the ground, and its two edges.
    let heading = |i: usize| {
        let (a, b) = (line[i.saturating_sub(1)].p, line[(i + 1).min(n - 1)].p);
        (b - a).with_y(0.0).normalize_or(Vec3::X)
    };
    let mut sides: [Vec<Vec3>; 2] = [Vec::with_capacity(n), Vec::with_capacity(n)];
    for i in 0..n {
        let forward = heading(i);
        let left = Vec3::Y.cross(forward);
        for (side, sign) in [(0, 1.0), (1, -1.0)] {
            let mut at = line[i].p + left * sign * width * 0.5;
            // Inside a turn tighter than the deck is wide, the edge would run backwards: it waits.
            if let Some(&before) = sides[side].last() {
                if (at - before).with_y(0.0).dot(forward) <= 0.0 {
                    at = before.with_y(at.y);
                }
            }
            sides[side].push(at);
        }
    }
    let down = |p: Vec3, i: usize| p.with_y(line[i].base.min(p.y));
    for i in 0..n - 1 {
        let (l0, l1, r0, r1) = (sides[0][i], sides[0][i + 1], sides[1][i], sides[1][i + 1]);
        push(&mut g.quads, [l0, r0, r1, l1], Vec3::Y);
        let forward = heading(i);
        let left = Vec3::Y.cross(forward);
        push(&mut g.quads, [l0, l1, down(l1, i + 1), down(l0, i)], left);
        push(&mut g.quads, [r0, r1, down(r1, i + 1), down(r0, i)], -left);
    }
    for (i, out) in [(0, -heading(0)), (n - 1, heading(n - 1))] {
        let (l, r) = (sides[0][i], sides[1][i]);
        push(&mut g.quads, [l, r, down(r, i), down(l, i)], out);
        if line[i].p.y - line[i].base > LEDGE {
            g.edges.push((l, r));
        }
    }
    // Its sides are ledges where they stand clear of the surface.
    for side in &sides {
        let mut run: Vec<Vec3> = Vec::new();
        for i in 0..=n {
            if i < n && line[i].p.y - line[i].base > LEDGE {
                run.push(side[i]);
                continue;
            }
            run.dedup_by(|a, b| a.distance(*b) < 1e-4);
            if run.len() > 1 {
                g.edges.extend(run.windows(2).map(|w| (w[0], w[1])));
                g.rails.push(std::mem::take(&mut run));
            }
            run.clear();
        }
    }
}

/// The middle of an outline, and the outline with an even number of points.
fn outline(line: &[Sample]) -> (Vec3, Vec<Vec3>) {
    let mut points: Vec<Vec3> = line.iter().map(|s| s.p).collect();
    if points.len() % 2 == 1 {
        let (a, b) = (points[points.len() - 1], points[0]);
        points.push((a + b) * 0.5);
    }
    let middle = points.iter().sum::<Vec3>() / points.len() as f32;
    (middle, points)
}

/// A level face from its middle out to a ring, as quads of two real triangles.
fn fan(out: &mut Vec<[Vec3; 4]>, middle: Vec3, ring: &[Vec3]) {
    let n = ring.len();
    for j in (0..n).step_by(2) {
        push(out, [middle, ring[j], ring[(j + 1) % n], ring[(j + 2) % n]], Vec3::Y);
    }
}

/// A level, convex face with up to five corners (some of which may
/// coincide), as quads of two real triangles each.
fn level(out: &mut Vec<[Vec3; 4]>, corners: &[Vec3]) {
    let mut points: Vec<Vec3> = Vec::with_capacity(corners.len());
    for &p in corners {
        if points.iter().all(|q| q.distance(p) > 1e-4) {
            points.push(p);
        }
    }
    // A triangle is a quad with a corner in the middle of one side.
    let triangle = |out: &mut Vec<[Vec3; 4]>, a: Vec3, b: Vec3, c: Vec3| {
        if (b - a).cross(c - a).length() > 1e-6 {
            push(out, [a, b, (b + c) * 0.5, c], Vec3::Y);
        }
    };
    match points[..] {
        [a, b, c] => triangle(out, a, b, c),
        [a, b, c, d] => {
            // (either half may be a sliver: then the other is all of it)
            let (first, second) = ((c - a).cross(b - a).length(), (d - a).cross(c - a).length());
            if first > 1e-6 && second > 1e-6 {
                push(out, [a, b, c, d], Vec3::Y);
            } else {
                triangle(out, a, b, c);
                triangle(out, a, c, d);
            }
        }
        [a, b, c, d, e] => {
            level(out, &[a, b, c, d]);
            triangle(out, a, d, e);
        }
        _ => {}
    }
}

fn pad(g: &mut PieceGeometry, line: &[Sample]) {
    let (middle, ring) = outline(line);
    let top = ring[0].y;
    let base = line.iter().map(|s| s.base).fold(f32::MAX, f32::min).min(top);
    let ring: Vec<Vec3> = ring.iter().map(|p| p.with_y(top)).collect();
    let middle = middle.with_y(top);
    fan(&mut g.quads, middle, &ring);
    let n = ring.len();
    for j in 0..n {
        let (a, b) = (ring[j], ring[(j + 1) % n]);
        push(&mut g.quads, [a, b, b.with_y(base), a.with_y(base)], (a + b) * 0.5 - middle);
        g.edges.push((a, b));
    }
    if top - base > LEDGE {
        g.rails.push(ring.iter().copied().chain([ring[0]]).collect());
    }
}

/// A pool below y = 0 inside the outline: a quarter-circle transition from
/// the lip down to a flat floor, and (ridden, not drawn) the surface it is
/// sunk into out to the outline's rectangle, which is what that surface is
/// opened along.
fn bowl(g: &mut PieceGeometry, line: &[Sample], depth: f32) {
    const RINGS: usize = 6;
    let (middle, lip) = outline(line);
    let (middle, lip): (Vec3, Vec<Vec3>) = (middle.with_y(0.0), lip.iter().map(|p| p.with_y(0.0)).collect());
    let n = lip.len();
    let reach = lip.iter().map(|p| p.distance(middle)).fold(f32::MAX, f32::min);
    let radius = depth.min(reach * 0.6);
    let ring = |k: usize, j: usize| {
        let a = FRAC_PI_2 * k as f32 / RINGS as f32;
        let out = lip[j % n] - middle;
        (middle + out * (1.0 - radius * (1.0 - a.sin()) / out.length())).with_y(-radius * a.cos())
    };
    let above = middle + Vec3::Y * radius;
    for k in 0..RINGS {
        for j in 0..n {
            let quad = [ring(k, j), ring(k, j + 1), ring(k + 1, j + 1), ring(k + 1, j)];
            push(&mut g.quads, quad, above - (quad[0] + quad[2]) * 0.5);
        }
    }
    let floor: Vec<Vec3> = (0..n).map(|j| ring(0, j)).collect();
    fan(&mut g.quads, middle.with_y(-radius), &floor);
    // Out to the rectangle.
    let (lo, hi) = lip.iter().fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), &p| (lo.min(p), hi.max(p)));
    // Where the line from the middle through `p` leaves the rectangle, and by which side.
    let leave = |p: Vec3| {
        let d = p - middle;
        let tx = if d.x > 0.0 { (hi.x - middle.x) / d.x } else if d.x < 0.0 { (lo.x - middle.x) / d.x } else { f32::MAX };
        let tz = if d.z > 0.0 { (hi.z - middle.z) / d.z } else if d.z < 0.0 { (lo.z - middle.z) / d.z } else { f32::MAX };
        let side = if tx <= tz { if d.x > 0.0 { 0 } else { 2 } } else if d.z > 0.0 { 1 } else { 3 };
        (middle + d * tx.min(tz).max(1.0), side)
    };
    let before = g.quads.len();
    for j in 0..n {
        let (a, b) = (lip[j], lip[(j + 1) % n]);
        let ((ra, side_a), (rb, side_b)) = (leave(a), leave(b));
        if side_a == side_b {
            level(&mut g.quads, &[a, b, rb, ra]);
        } else {
            // Round the rectangle's corner between them.
            let x = if side_a == 0 || side_b == 0 { hi.x } else { lo.x };
            let z = if side_a == 1 || side_b == 1 { hi.z } else { lo.z };
            level(&mut g.quads, &[a, b, rb, Vec3::new(x, 0.0, z), ra]);
        }
    }
    g.unseen.extend(before..g.quads.len());
    g.edges.extend((0..n).map(|j| (lip[j], lip[(j + 1) % n])));
    g.rails.push(lip.iter().copied().chain([lip[0]]).collect());
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::quad_normal;

    fn node(x: f32, y: f32, z: f32) -> Node {
        Node::new(Vec3::new(x, y, z), 0.0)
    }

    fn real_triangles(g: &PieceGeometry, what: &str) {
        for q in g.all_quads() {
            assert!(q.iter().all(|p| p.is_finite()), "{what}");
            for [a, b, c] in [[q[0], q[2], q[1]], [q[0], q[3], q[2]]] {
                assert!((b - a).cross(c - a).length() > 1e-7, "{what}: a collapsed triangle in {q:?}");
            }
        }
    }

    #[test]
    fn every_drawn_kind_makes_a_piece_to_ride() {
        for kind in [PieceKind::RailPath, PieceKind::Deck, PieceKind::Pad, PieceKind::Pool] {
            let g = Path::sample_of(kind).geometry(Piece::new(kind));
            assert!(!g.quads.is_empty() && !g.rails.is_empty(), "{kind:?}");
            assert!(g.all_quads().any(|q| quad_normal(q).normalize().y > 0.9), "{kind:?} has something to land on");
            if kind.closed() {
                real_triangles(&g, &format!("{kind:?}"));
            }
            // Too few points: nothing yet.
            let mut short = Path::sample_of(kind);
            short.nodes.truncate(kind.least_nodes() - 1);
            assert!(short.geometry(Piece::new(kind)).quads.is_empty());
        }
    }

    #[test]
    fn a_path_runs_through_its_nodes_and_its_bend_handle() {
        let mut path = Path { nodes: vec![node(0.0, 0.0, 0.0), node(3.0, 0.5, 0.0), node(3.0, 0.5, 4.0)], eased: false };
        for (i, n) in path.nodes.iter().enumerate().take(2) {
            assert!(path.along(i, 0.0, false, true).p.distance(n.p()) < 1e-5);
            assert!(path.along(i, 1.0, false, true).p.distance(path.nodes[i + 1].p()) < 1e-5);
        }
        // Heights go straight; eased, they leave and arrive level.
        assert!((path.along(0, 0.25, false, true).p.y - 0.125).abs() < 1e-5);
        path.eased = true;
        assert!(path.along(0, 0.25, false, true).p.y < 0.1);
        // The middle goes where the bend handle is put.
        let target = path.straight_middle(1, false) + Vec3::new(0.7, 0.0, -0.2);
        path.nodes[1].bend = [0.7, -0.2];
        assert!(path.middle(1, false).distance(target) < 1e-5);
        assert!(path.along(1, 0.0, false, true).p.distance(path.nodes[1].p()) < 1e-5, "the nodes stay");
        // A corner: the path leaves it straight for the next node.
        path.nodes[1].corner = true;
        path.nodes[1].bend = [0.0; 2];
        assert!((path.heading(1, true, false) - Vec2::Y).length() < 1e-5 && (path.heading(1, false, false) - Vec2::X).length() < 1e-5);
        assert_ne!(path.key(), Path { eased: false, ..path.clone() }.key());
    }

    #[test]
    fn a_rail_is_a_tube_over_its_nodes_on_posts_down_to_the_surface() {
        let mut path = Path { nodes: vec![node(0.0, 0.0, 0.0), node(3.0, 0.0, 0.0), node(6.0, 1.0, 1.0)], eased: false };
        path.nodes[2].base = 0.4;
        let g = path.geometry(Piece::new(PieceKind::RailPath));
        let line = &g.rails[0];
        assert!(line[0].distance(Vec3::new(0.0, RAIL_TOP, 0.0)) < 1e-5 && line[line.len() - 1].distance(Vec3::new(6.0, 1.0 + RAIL_TOP, 1.0)) < 1e-5);
        assert!(line.windows(2).all(|w| w[0].distance(w[1]) > 1e-3 && w[0].distance(w[1]) < 1.2));
        // Three posts; the last from the surface it stands on.
        assert_eq!(g.quads.len(), 18);
        let last_post_foot = g.quads.iter().flatten().filter(|p| p.x > 5.0).map(|p| p.y).fold(f32::MAX, f32::min);
        assert!((last_post_foot - 0.4).abs() < 0.1, "{last_post_foot}");
        // The end posts stand in from the ends: nothing of them past the tube's end faces, or near them.
        assert!(g.quads.iter().flatten().all(|p| p.x > 0.5 && p.x < 5.6), "a post at the rail's end");
    }

    #[test]
    fn a_rail_has_no_faces_across_it_but_its_two_ends() {
        // Straight, level: nothing faces along it between x = 0 and 6.
        let straight = Path { nodes: vec![node(0.0, 0.0, 0.0), node(6.0, 0.0, 0.0)], eased: false };
        let g = straight.geometry(Piece::new(PieceKind::RailPath));
        assert_eq!(g.rail_quads.len(), 6, "one box");
        // Bent and rising: still only the two ends face along the rail.
        let mut bent = Path { nodes: vec![node(0.0, 0.0, 0.0), node(3.0, 0.0, 0.0), node(6.0, 1.0, 2.0), node(9.0, 1.0, 2.0)], eased: false };
        bent.nodes[1].bend = [0.3, 0.4];
        let g = bent.geometry(Piece::new(PieceKind::RailPath));
        let line = &g.rails[0];
        let mut across = 0;
        for q in &g.rail_quads {
            let normal = (quad_normal(q) + (q[3] - q[0]).cross(q[2] - q[0])).normalize();
            let centre = (q[0] + q[1] + q[2] + q[3]) * 0.25;
            // The rail's direction where this face is.
            let nearest = (0..line.len() - 1).min_by(|&a, &b| ((line[a] + line[a + 1]) * 0.5).distance(centre).total_cmp(&((line[b] + line[b + 1]) * 0.5).distance(centre))).unwrap();
            let along = (line[nearest + 1] - line[nearest]).normalize();
            if normal.dot(along).abs() > 0.7 {
                across += 1;
                assert!(centre.x < 0.05 || centre.x > 8.95, "a face across the rail at {centre}");
            }
        }
        assert_eq!(across, 2);
        // The top is one unbroken strip, 8 cm wide, under the grind line.
        let tops: Vec<&[Vec3; 4]> = g.rail_quads.iter().filter(|q| quad_normal(q).normalize().y > 0.7).collect();
        assert!(tops.len() >= 3);
        for q in &tops {
            for p in q.iter() {
                let d = line.windows(2).map(|w| { let ab = w[1] - w[0]; (w[0] + ab * ((*p - w[0]).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)).distance(*p) }).fold(f32::MAX, f32::min);
                assert!((d - TUBE * 0.5).abs() < 0.012, "{d}");
            }
        }
    }

    #[test]
    fn a_deck_is_as_wide_as_asked_solid_to_the_surface_and_faces_out() {
        let path = Path { nodes: vec![node(0.0, 0.0, 0.0), node(4.0, 1.0, 0.0), node(8.0, 1.0, 2.0)], eased: false };
        let piece = Piece { width: 1.5, ..Piece::new(PieceKind::Deck) };
        let g = path.geometry(piece);
        let (lo, hi) = g.bounds();
        assert!(lo.y.abs() < 1e-5 && (hi.y - 1.0).abs() < 1e-5, "{lo} {hi}");
        // At its start (heading +x) it is 3 m across z.
        let start: Vec<f32> = g.quads.iter().flatten().filter(|p| p.x.abs() < 0.2).map(|p| p.z).collect();
        let across = start.iter().fold(f32::MIN, |a, &b| a.max(b)) - start.iter().fold(f32::MAX, |a, &b| a.min(b));
        assert!((across - 3.0).abs() < 0.05, "{across}");
        // Nothing faces down (it has no underside), and its top does face up.
        let normals: Vec<Vec3> = g.quads.iter().map(|q| (quad_normal(q) + (q[3] - q[0]).cross(q[2] - q[0])).normalize()).collect();
        assert!(normals.iter().all(|n| n.y > -1e-3), "a face looks down");
        assert!(normals.iter().filter(|n| n.y > 0.7).count() >= 10);
        // The raised part's sides are ledges; the ramp's foot isn't.
        assert_eq!(g.rails.len(), 2);
        assert!(g.rails.iter().flatten().all(|p| p.y > LEDGE));
        // Its ends are joins: out of the deck, level.
        let ends = path.ends(piece);
        assert!(ends[0].1.distance(Vec3::NEG_X) < 1e-4 && ends[1].1.x > 0.5 && (ends[0].2 - 3.0).abs() < 1e-5);
        // A turn tighter than its width doesn't fold its inside edge back.
        let tight = Path { nodes: vec![node(0.0, 0.5, 0.0), node(1.0, 0.5, 0.0), node(1.0, 0.5, 1.0)], eased: false };
        let g = tight.geometry(Piece { width: 2.5, ..Piece::new(PieceKind::Deck) });
        assert!(g.all_quads().flatten().all(|p| p.is_finite()));
    }

    #[test]
    fn a_pad_is_level_and_a_bowl_is_sunk_within_its_outline() {
        let pad = Path::sample_of(PieceKind::Pad).geometry(Piece::new(PieceKind::Pad));
        let (lo, hi) = pad.bounds();
        assert!(lo.y.abs() < 1e-5 && (hi.y - 0.5).abs() < 1e-5);
        assert_eq!(pad.rails.len(), 1, "its edge is a ledge all round");
        let path = Path::sample_of(PieceKind::Pool);
        let bowl = path.geometry(Piece::new(PieceKind::Pool));
        let (lo, hi) = bowl.bounds();
        assert!(hi.y.abs() < 1e-5 && lo.y < -0.5, "{lo} {hi}");
        // What is drawn stays inside the outline's box; what isn't fills that box at the rim.
        assert!(!bowl.unseen.is_empty());
        for &i in &bowl.unseen {
            assert!(bowl.quads[i].iter().all(|p| p.y.abs() < 1e-5));
        }
        let area: f32 = bowl
            .quads
            .iter()
            .enumerate()
            .filter(|(i, q)| bowl.unseen.contains(i) || q.iter().all(|p| p.y.abs() < 1e-5))
            .map(|(_, q)| ((q[2] - q[0]).cross(q[1] - q[0]).length() + (q[3] - q[0]).cross(q[2] - q[0]).length()) * 0.5)
            .sum();
        // The rim's filler and the hole it surrounds make up the rectangle: the filler alone is less.
        assert!(area > 0.0 && area < (hi.x - lo.x) * (hi.z - lo.z));
    }
}
