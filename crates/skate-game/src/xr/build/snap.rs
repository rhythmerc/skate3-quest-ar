//! Line pieces up with what they are dropped on or beside.
//!
//! With the furniture: when a piece's yaw
//! is within [`SNAP_ANGLE`] of a table's edge direction, it takes that
//! direction exactly, so a rail or ledge runs straight along the table.
//!
//! With each other ([`join`]): a piece brought beside another of its kind,
//! turned about the same way, takes its place flush against it, so two half
//! pipes make one long one and walls run on in a line.
use bevy::prelude::*;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// How far off a neighbour's direction (or its reverse) a piece may be turned
/// and still join it.
pub(crate) const JOIN_ANGLE: f32 = 25.0 * PI / 180.0;
/// How near its place beside a neighbour a piece must be to take it, as a
/// share of the piece's longest side.
const JOIN_REACH: f32 = 0.25;
/// Pieces join only on the same surface: their bases this near in height, m.
const JOIN_HEIGHT_M: f32 = 0.05;

/// A placed piece seen from above, in tracking space: where its origin is,
/// its turn, and its least and greatest local (x, z) in real metres.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Footprint {
    pub position: Vec3,
    pub yaw: f32,
    pub min: Vec2,
    pub max: Vec2,
}

/// Where a piece (`own`, at the pointed position and turn) goes when it is
/// near enough a place flush against one of `neighbours`: its position and
/// turn there. The nearest such place wins.
pub(crate) fn join(neighbours: impl IntoIterator<Item = Footprint>, own: Footprint) -> Option<(Vec3, f32)> {
    let reach = JOIN_REACH * (own.max - own.min).max_element();
    let mut best: Option<(f32, Vec3, f32)> = None;
    for n in neighbours {
        if (n.position.y - own.position.y).abs() > JOIN_HEIGHT_M {
            continue;
        }
        for (turned, min, max) in [(0.0, own.min, own.max), (PI, -own.max, -own.min)] {
            let yaw = n.yaw + turned;
            if angle_between(yaw, own.yaw).abs() > JOIN_ANGLE {
                continue;
            }
            // Against each side of the neighbour, origins in line.
            let offsets = [
                Vec2::new(n.max.x - min.x, 0.0),
                Vec2::new(n.min.x - max.x, 0.0),
                Vec2::new(0.0, n.max.y - min.y),
                Vec2::new(0.0, n.min.y - max.y),
            ];
            for offset in offsets {
                let place = n.position + Quat::from_rotation_y(n.yaw) * Vec3::new(offset.x, 0.0, offset.y);
                let distance = place.xz().distance(own.position.xz());
                if distance <= reach && best.is_none_or(|(d, ..)| distance < d) {
                    best = Some((distance, place, own.yaw + angle_between(yaw, own.yaw)));
                }
            }
        }
    }
    best.map(|(_, place, yaw)| (place, yaw))
}

pub(crate) const SNAP_ANGLE: f32 = 10.0 * PI / 180.0;
/// How far outside a top face a point may be and still count as on it, m.
const MARGIN_M: f32 = 0.05;

/// A box-shaped surface in tracking space: rotation, centre, half extents.
pub(crate) type SurfaceBox = (Quat, Vec3, Vec3);

/// Heading (yaw about +Y, the convention of `rig::heading`) of a direction.
fn heading_of(direction: Vec3) -> f32 {
    (-direction.x).atan2(-direction.z)
}

/// `a - b` wrapped into (-π, π].
fn angle_between(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(TAU);
    if d > PI { d - TAU } else { d }
}

/// `yaw`, or the nearest edge direction of the box whose top `point` (tracking
/// space) is on, if that is within [`SNAP_ANGLE`].
pub(crate) fn snap_yaw(boxes: impl IntoIterator<Item = SurfaceBox>, point: Vec3, yaw: f32) -> f32 {
    for (rotation, centre, half) in boxes {
        let local = rotation.inverse() * (point - centre);
        if (local.abs() - half).max_element() > MARGIN_M {
            continue;
        }
        // A horizontal box axis: whichever of X/Z (in the box frame) is
        // least vertical. The other edges are this one plus quarter turns.
        let edge = [Vec3::X, Vec3::Y, Vec3::Z]
            .into_iter()
            .map(|a| rotation * a)
            .min_by(|a, b| a.y.abs().total_cmp(&b.y.abs()))
            .expect("three axes");
        let base = heading_of(edge.with_y(0.0));
        let nearest = (0..4)
            .map(|k| base + k as f32 * FRAC_PI_2)
            .min_by(|a, b| angle_between(*a, yaw).abs().total_cmp(&angle_between(*b, yaw).abs()))
            .expect("four edges");
        if angle_between(nearest, yaw).abs() <= SNAP_ANGLE {
            return yaw + angle_between(nearest, yaw);
        }
        return yaw;
    }
    yaw
}

/// How near a neighbour's end a rail's end must be to meet it, as a share of
/// the rail's length.
const END_REACH: f32 = 0.15;

/// The two ends of a piece's long (x) axis, in tracking space.
fn ends(f: &Footprint) -> [Vec3; 2] {
    let z = (f.min.y + f.max.y) * 0.5;
    [f.min.x, f.max.x].map(|x| f.position + Quat::from_rotation_y(f.yaw) * Vec3::new(x, 0.0, z))
}

/// End to end at any angle, for rails: where `own` goes (its turn kept) so
/// that one of its ends meets the nearest end of one of `neighbours`, when
/// they are already close. A run of rails can then bend.
pub(crate) fn join_ends(neighbours: impl IntoIterator<Item = Footprint>, own: Footprint) -> Option<Vec3> {
    let reach = END_REACH * (own.max.x - own.min.x);
    let mut best: Option<(f32, Vec3)> = None;
    for n in neighbours {
        if (n.position.y - own.position.y).abs() > JOIN_HEIGHT_M {
            continue;
        }
        for theirs in ends(&n) {
            for mine in ends(&own) {
                let distance = theirs.xz().distance(mine.xz());
                if distance <= reach && best.is_none_or(|(d, _)| distance < d) {
                    best = Some((distance, own.position + (theirs - mine).with_y(n.position.y - own.position.y)));
                }
            }
        }
    }
    best.map(|(_, place)| place)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(yaw: f32) -> SurfaceBox {
        (Quat::from_rotation_y(yaw), Vec3::new(1.0, 0.375, -1.0), Vec3::new(0.6, 0.375, 0.4))
    }

    #[test]
    fn close_yaws_line_up_with_the_table() {
        let top = Vec3::new(1.1, 0.75, -1.05);
        for table_yaw in [0.0, 0.3, -1.2] {
            for k in 0..4 {
                let edge = table_yaw + k as f32 * FRAC_PI_2;
                let snapped = snap_yaw([table(table_yaw)], top_of(table_yaw, top), edge + 0.1);
                assert!(angle_between(snapped, edge).abs() < 1e-4, "{table_yaw} {k}: {snapped}");
            }
        }
    }

    #[test]
    fn far_yaws_and_the_floor_are_left_alone() {
        let top = Vec3::new(1.1, 0.75, -1.05);
        assert_eq!(snap_yaw([table(0.0)], top, 0.5), 0.5, "30° off an edge stays");
        assert_eq!(snap_yaw([table(0.0)], Vec3::new(3.0, 0.0, 3.0), 0.05), 0.05, "not on the table");
    }

    fn pipe(position: Vec3, yaw: f32) -> Footprint {
        Footprint { position, yaw, min: Vec2::new(-0.25, -0.45), max: Vec2::new(0.25, 0.45) }
    }

    #[test]
    fn a_copy_brought_beside_its_twin_sits_flush_against_it() {
        for yaw in [0.0, 0.7, -2.0] {
            let placed = pipe(Vec3::new(1.0, 0.0, -2.0), yaw);
            let right = Quat::from_rotation_y(yaw) * Vec3::X;
            // Half a width out to its right, a little off and a little turned.
            let near = placed.position + right * 0.53 + Vec3::new(0.02, 0.0, -0.03);
            let (place, turn) = join([placed], pipe(near, yaw + 0.2)).expect("near enough");
            assert!(place.distance(placed.position + right * 0.5) < 1e-4, "{yaw}: {place}");
            assert!(angle_between(turn, yaw).abs() < 1e-4);
            // Turned right round, it still joins, and stays turned round.
            let (place, turn) = join([placed], pipe(near, yaw + PI - 0.1)).expect("near enough");
            assert!(place.distance(placed.position + right * 0.5) < 1e-4);
            assert!(angle_between(turn, yaw + PI).abs() < 1e-4);
            // End to end.
            let ahead = Quat::from_rotation_y(yaw) * Vec3::Z;
            let (place, _) = join([placed], pipe(placed.position + ahead * 0.95, yaw)).expect("near enough");
            assert!(place.distance(placed.position + ahead * 0.9) < 1e-4);
        }
    }

    #[test]
    fn far_turned_or_raised_copies_are_left_alone() {
        let placed = pipe(Vec3::ZERO, 0.0);
        assert!(join([placed], pipe(Vec3::new(1.2, 0.0, 0.0), 0.0)).is_none(), "too far");
        assert!(join([placed], pipe(Vec3::new(0.5, 0.0, 0.0), 0.8)).is_none(), "turned 45 degrees");
        assert!(join([placed], pipe(Vec3::new(0.5, 0.4, 0.0), 0.0)).is_none(), "on a table above it");
        assert!(join([], pipe(Vec3::ZERO, 0.0)).is_none());
    }

    #[test]
    fn the_nearest_place_wins() {
        let (a, b) = (pipe(Vec3::ZERO, 0.0), pipe(Vec3::new(1.2, 0.0, 0.0), 0.0));
        let (place, _) = join([a, b], pipe(Vec3::new(0.62, 0.0, 0.0), 0.0)).expect("between them");
        assert!(place.distance(Vec3::new(0.7, 0.0, 0.0)) < 1e-4, "{place}");
    }

    #[test]
    fn rails_meet_end_to_end_at_any_angle() {
        let rail = |position: Vec3, yaw: f32| Footprint { position, yaw, min: Vec2::new(-0.25, -0.01), max: Vec2::new(0.25, 0.01) };
        let placed = rail(Vec3::new(1.0, 0.0, -1.0), 0.3);
        let end = ends(&placed)[1];
        for turn in [0.9, -1.2, 2.5] {
            // Brought so its near end is a couple of centimetres off the placed rail's.
            let own_yaw = placed.yaw + turn;
            let start = Quat::from_rotation_y(own_yaw) * Vec3::new(-0.25, 0.0, 0.0);
            let own = rail(end - start + Vec3::new(0.02, 0.0, 0.015), own_yaw);
            let place = join_ends([placed], own).expect("near enough");
            assert!((place + start).distance(end) < 1e-4, "{turn}: {place}");
        }
        assert!(join_ends([placed], rail(end + Vec3::new(0.6, 0.0, 0.0), 1.0)).is_none(), "too far");
        assert!(join_ends([placed], rail(end + Vec3::Y * 0.3, 1.0)).is_none(), "on another surface");
    }

    fn top_of(table_yaw: f32, point: Vec3) -> Vec3 {
        let (rotation, centre, _) = table(table_yaw);
        let local = Quat::from_rotation_y(0.0).inverse() * (point - Vec3::new(1.0, 0.375, -1.0));
        centre + rotation * local
    }
}
