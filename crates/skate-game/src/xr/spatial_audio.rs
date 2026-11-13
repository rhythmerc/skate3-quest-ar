//! Skate sounds in 3D: the headset is the listener and the skater the emitter
//! (`skate_audio::clean` plays every sound at the skater when
//! [`SpatialSkateAudio`] is present).
//!
//! Bevy's spatial audio pans between two ears and falls off with the inverse
//! square of distance, clamped to full volume within one unit. Positions are
//! game-world metres, which are `k` times real metres (see [`rig`]), so the
//! spatial scale is set to `1 / (k * REFERENCE_M)`: one unit is
//! [`REFERENCE_M`] of real room, whatever size the world is scaled to. The
//! listener sits between the eye cameras, so its ear gap is also real metres.
//!
//! [`rig`]: super::rig
use super::planes::{RoomMesh, SceneRevision, SceneShapes, Shape};
use super::rig::Rig;
use crate::reverb::{Geometry, RoomFromWorld};
use crate::skate_audio::clean::SpatialSkateAudio;
use bevy::audio::{DefaultSpatialScale, SpatialListener, SpatialScale};
use bevy::prelude::*;
use bevy_mod_xr::camera::XrCamera;

/// Real distance from the head inside which the skater is at full volume, m (a skater across the
/// room still heard well).
/// Twice as far is a quarter of the level.
const REFERENCE_M: f32 = 3.0;
/// Real distance between the ears, m.
const EAR_GAP_M: f32 = 0.18;

#[derive(Component)]
struct HeadListener;

pub(crate) struct SpatialAudioPlugin;

impl Plugin for SpatialAudioPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(SpatialSkateAudio)
            .add_systems(Startup, |mut commands: Commands| {
                commands.spawn((HeadListener, SpatialListener::new(EAR_GAP_M), Transform::default()));
            })
            .add_systems(Update, (follow_head, room_surfaces));
    }
}

/// Put the listener at the head (between the eyes) in the game world, and keep
/// the spatial scale in real metres.
fn follow_head(
    rig: Res<Rig>,
    eyes: Query<&Transform, (With<XrCamera>, Without<HeadListener>)>,
    mut listener: Query<&mut Transform, With<HeadListener>>,
    mut scale: ResMut<DefaultSpatialScale>,
    mut room_from_world: ResMut<RoomFromWorld>,
) {
    let (Some(root), Ok(mut listener)) = (rig.root_transform(), listener.single_mut()) else {
        return;
    };
    room_from_world.0 = Some(root.compute_affine().inverse());
    let mut count = 0.0;
    let mut position = Vec3::ZERO;
    let mut rotation = None;
    for eye in &eyes {
        position += eye.translation;
        rotation.get_or_insert(eye.rotation);
        count += 1.0;
    }
    let Some(rotation) = rotation else { return };
    // Eye cameras live under the tracking root (tracking space); the root maps
    // tracking space into the game world, scale included, so the ear gap
    // scales with the world like the room does.
    let head = Transform::from_translation(position / count).with_rotation(rotation);
    // (the room's reverberation is worked out in the room's own space, real metres)
    if let Some(room) = crate::reverb::room() {
        room.set_head(head.translation, head.rotation);
    }
    let target = root.mul_transform(head);
    if *listener != target {
        *listener = target;
    }
    let units = SpatialScale::new(1.0 / (rig.scale * REFERENCE_M));
    if scale.0.0 != units.0 {
        scale.0 = units;
    }
}

/// The real room's surfaces to its reverberation, whenever the room scan's entities change: every
/// Scene plane and box, and the room mesh.
fn room_surfaces(shapes: Res<SceneShapes>, mesh: Res<RoomMesh>, revision: Res<SceneRevision>, mut given: Local<Option<(u64, usize, bool)>>) {
    let Some(room) = crate::reverb::room() else { return };
    let key = (revision.0, shapes.0.len(), mesh.0.is_some());
    if *given == Some(key) {
        return;
    }
    *given = Some(key);
    let geometry = room_geometry(&shapes.0, mesh.0.as_ref().map(|m| (m.pose, &*m.vertices, &*m.indices)));
    info!("ROOM REVERB the room: {} surfaces, {} triangles", shapes.0.len(), geometry.triangles.len());
    room.set_geometry(Some(std::sync::Arc::new(geometry)));
}

fn room_geometry(shapes: &[Shape], mesh: Option<(Transform, &[Vec3], &[u32])>) -> Geometry {
    let mut geometry = Geometry::default();
    for shape in shapes {
        match *shape {
            // (a plane faces along its +Y: corners anticlockwise seen from there)
            Shape::Plane { pose, half, .. } => {
                let (u, w) = (pose.rotation * Vec3::X * half.x, pose.rotation * Vec3::Z * half.y);
                geometry.quad([pose.translation - u - w, pose.translation - u + w, pose.translation + u + w, pose.translation + u - w]);
            }
            Shape::Box { pose, half } => {
                for (axis, a, b) in [(Vec3::X, Vec3::Y, Vec3::Z), (Vec3::Y, Vec3::Z, Vec3::X), (Vec3::Z, Vec3::X, Vec3::Y)] {
                    for sign in [1.0, -1.0] {
                        // (each face seen from outside: a, b and the outward normal are right-handed)
                        let centre = pose.translation + pose.rotation * (axis * half * sign);
                        let (u, w) = (pose.rotation * (a * half) * sign, pose.rotation * (b * half));
                        geometry.quad([centre - u - w, centre + u - w, centre + u + w, centre - u + w]);
                    }
                }
            }
        }
    }
    if let Some((pose, vertices, indices)) = mesh {
        let first = geometry.vertices.len() as u32;
        geometry.vertices.extend(vertices.iter().map(|&v| pose.transform_point(v)));
        geometry.triangles.extend(indices.chunks_exact(3).map(|t| [first + t[0], first + t[1], first + t[2]]));
    }
    geometry
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normal(geometry: &Geometry, triangle: usize) -> Vec3 {
        let [a, b, c] = geometry.triangles[triangle].map(|i| geometry.vertices[i as usize]);
        (b - a).cross(c - a).normalize()
    }

    #[test]
    fn the_rooms_surfaces_face_the_way_they_are() {
        let floor = Shape::Plane { pose: Transform::IDENTITY, half: Vec2::new(2.0, 3.0), floor: true };
        let table = Shape::Box { pose: Transform::from_xyz(1.0, 0.4, 0.0), half: Vec3::new(0.5, 0.4, 0.3) };
        let vertices = [Vec3::ZERO, Vec3::X, Vec3::Z];
        let geometry = room_geometry(&[floor, table], Some((Transform::from_xyz(0.0, 2.0, 0.0), &vertices, &[0, 1, 2])));
        assert_eq!(geometry.triangles.len(), 2 + 12 + 1);
        assert!(normal(&geometry, 0).distance(Vec3::Y) < 1e-4 && normal(&geometry, 1).distance(Vec3::Y) < 1e-4);
        // Each of the box's faces looks away from its middle.
        for triangle in 2..14 {
            let [a, b, c] = geometry.triangles[triangle].map(|i| geometry.vertices[i as usize]);
            assert!(normal(&geometry, triangle).dot((a + b + c) / 3.0 - Vec3::new(1.0, 0.4, 0.0)) > 0.0, "face {triangle}");
        }
        assert_eq!(geometry.vertices[geometry.triangles[14][1] as usize], Vec3::new(1.0, 2.0, 0.0));
    }
}
