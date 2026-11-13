//! The park's place in the room: a spatial anchor (`XR_FB_spatial_entity`),
//! which the headset keeps fixed to the real room.
//!
//! The pieces' poses are in the tracking (STAGE) space, and that space is
//! not the same from one session to the next: its floor is found a few
//! centimetres higher or lower, and it moves altogether when the boundary
//! is redrawn or re-centred. So the layout is saved with an anchor, made in
//! the middle of the park and stored on the headset, and with where that
//! anchor was in the tracking space when the poses were written. When the
//! anchor is found somewhere else (the next session, or after a re-centre),
//! every piece is moved the same way, and stays where it was in the room.
//!
//! A layout from before this has no anchor: its pieces stay where the
//! tracking space puts them, and an anchor is made for it then. The same if
//! the headset no longer has the anchor (its room data was cleared).
use super::{Placed, PlacedPieces};
use crate::xr::link::SkaterLink;
use crate::xr::planes::{cvt, enable_locatable};
use bevy::prelude::*;
use bevy_mod_openxr::{
    exts::OxrEnabledExtensions,
    openxr_session_running,
    poll_events::{OxrEventHandlerExt, OxrEventIn},
    resources::{OxrFrameState, OxrInstance},
    session::OxrSession,
};
use bevy_mod_openxr::spaces::OxrSpaceExt;
use bevy_mod_xr::spaces::XrPrimaryReferenceSpace;
use openxr::sys::{self, Handle as _};
use serde::{Deserialize, Serialize};
use std::ptr;

/// The anchor moved this far (m), or turned this far, since the pieces were
/// last put by it: they are put by it again. (Under it: tracking noise.)
const MOVED_M: f32 = 0.01;
const TURNED_RAD: f32 = 0.5 * std::f32::consts::PI / 180.0;
/// The first time it is found in a session, any difference counts.
const FIRST_M: f32 = 0.001;
const FIRST_RAD: f32 = 0.0005;
/// Between tries at making an anchor, s.
const RETRY_SECONDS: f32 = 10.0;

/// What the layout file says of the anchor: which one, and where it was in
/// the tracking space when the pieces' poses were written.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub(crate) struct AnchorRecord {
    /// The anchor's UUID, 32 hex digits.
    pub uuid: String,
    pub position: [f32; 3],
    /// Its turn about +Y.
    pub yaw: f32,
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum Step {
    /// Nothing asked of the headset.
    #[default]
    Idle,
    Finding(sys::AsyncRequestIdFB),
    Making(sys::AsyncRequestIdFB),
    MakingStorable(sys::AsyncRequestIdFB),
    Storing(sys::AsyncRequestIdFB),
}

#[derive(Resource, Default)]
pub(crate) struct ParkAnchor {
    /// As loaded, and as the next save writes it.
    pub record: Option<AnchorRecord>,
    step: Step,
    space: Option<sys::Space>,
    uuid: Option<[u8; 16]>,
    /// The headset has it stored: it can be found again.
    stored: bool,
    /// The saved anchor was asked for (once a session).
    sought: bool,
    /// The pieces were put by it this session.
    aligned: bool,
    wait: f32,
}

pub(super) struct AnchorPlugin;

impl Plugin for AnchorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ParkAnchor>()
            .add_oxr_event_handler(on_event)
            .add_systems(PreUpdate, (ask, follow).chain().run_if(openxr_session_running).run_if(enabled));
    }
}

fn enabled(exts: Option<Res<OxrEnabledExtensions>>) -> bool {
    exts.is_some_and(|e| e.fb_spatial_entity && e.fb_spatial_entity_query && e.fb_spatial_entity_storage)
}

fn hex(uuid: &[u8; 16]) -> String {
    uuid.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 16]> {
    let mut uuid = [0u8; 16];
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    for (i, byte) in uuid.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(uuid)
}

/// The pieces moved as the anchor did, from where the record has it to
/// `position` and `yaw`.
pub(crate) fn realign(items: &mut [Placed], from: &AnchorRecord, position: Vec3, yaw: f32) {
    let turn = yaw - from.yaw;
    let rotation = Quat::from_rotation_y(turn);
    for item in items {
        let offset = Vec3::from_array(item.position) - Vec3::from_array(from.position);
        item.position = (position + rotation * offset).to_array();
        item.yaw += turn;
    }
}

fn wrapped(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// Asks the headset for the saved anchor, or for a new one when there are
/// pieces and none to go by.
fn ask(
    instance: Res<OxrInstance>,
    session: Res<OxrSession>,
    primary: Res<XrPrimaryReferenceSpace>,
    frame: Option<Res<OxrFrameState>>,
    placed: Res<PlacedPieces>,
    time: Res<Time<Real>>,
    mut scene: ResMut<crate::xr::planes::SceneQuery>,
    mut anchor: ResMut<ParkAnchor>,
) {
    // One query at a time with the headset: the room's wait while the
    // anchor is looked for, and the anchor's until the room has been read.
    scene.bypass_change_detection().hold(matches!(anchor.step, Step::Finding(_)));
    if anchor.step != Step::Idle || anchor.space.is_some() {
        return;
    }
    if !scene.settled() {
        return;
    }
    if anchor.wait > 0.0 {
        anchor.wait -= time.delta_secs();
        return;
    }
    let saved = anchor.record.as_ref().and_then(|r| unhex(&r.uuid));
    if let (Some(uuid), false) = (saved, anchor.sought) {
        anchor.sought = true;
        let Some(fp) = instance.exts().fb_spatial_entity_query.as_ref() else { return };
        let mut uuids = [sys::UuidEXT { data: uuid }];
        let filter = sys::SpaceUuidFilterInfoFB {
            ty: sys::SpaceUuidFilterInfoFB::TYPE,
            next: ptr::null(),
            uuid_count: 1,
            uuids: uuids.as_mut_ptr(),
        };
        let info = sys::SpaceQueryInfoFB {
            ty: sys::SpaceQueryInfoFB::TYPE,
            next: ptr::null(),
            query_action: sys::SpaceQueryActionFB::LOAD,
            max_result_count: 1,
            timeout: sys::Duration::NONE,
            filter: (&filter as *const sys::SpaceUuidFilterInfoFB).cast(),
            exclude_filter: ptr::null(),
        };
        let mut request = sys::AsyncRequestIdFB::from_raw(0);
        match cvt(unsafe { (fp.query_spaces)(session.as_raw(), (&info as *const sys::SpaceQueryInfoFB).cast(), &mut request) }) {
            Ok(_) => {
                info!("PARK ANCHOR looking for {}", hex(&uuid));
                anchor.step = Step::Finding(request);
            }
            Err(e) => {
                warn!("PARK ANCHOR the saved anchor can't be looked for ({e:?}): the pieces stay by the tracking space");
                anchor.record = None;
            }
        }
        return;
    }
    if saved.is_some() && anchor.record.is_some() {
        // (looked for and not found: `on_event` has dropped the record by now)
        return;
    }
    // None to go by: one for the park as it stands, in the middle of it.
    let Some(frame) = frame else { return };
    if placed.items.is_empty() || placed.busy() {
        return;
    }
    let Some(fp) = instance.exts().fb_spatial_entity.as_ref() else { return };
    let count = placed.items.len() as f32;
    let middle = placed.items.iter().map(|i| Vec3::from_array(i.position)).sum::<Vec3>() / count;
    let lowest = placed.items.iter().map(|i| i.position[1]).fold(f32::MAX, f32::min);
    let info = sys::SpatialAnchorCreateInfoFB {
        ty: sys::SpatialAnchorCreateInfoFB::TYPE,
        next: ptr::null(),
        space: primary.as_raw_openxr_space(),
        pose_in_space: sys::Posef {
            orientation: sys::Quaternionf { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
            position: sys::Vector3f { x: middle.x, y: lowest, z: middle.z },
        },
        time: frame.predicted_display_time,
    };
    let mut request = sys::AsyncRequestIdFB::from_raw(0);
    match cvt(unsafe { (fp.create_spatial_anchor)(session.as_raw(), &info, &mut request) }) {
        Ok(_) => anchor.step = Step::Making(request),
        Err(e) => {
            warn!("PARK ANCHOR can't be made ({e:?}), trying again later");
            anchor.wait = RETRY_SECONDS;
        }
    }
}

fn store(instance: &OxrInstance, session: &OxrSession, anchor: &mut ParkAnchor) {
    let (Some(fp), Some(space)) = (instance.exts().fb_spatial_entity_storage.as_ref(), anchor.space) else { return };
    let info = sys::SpaceSaveInfoFB {
        ty: sys::SpaceSaveInfoFB::TYPE,
        next: ptr::null(),
        space,
        location: sys::SpaceStorageLocationFB::LOCAL,
        persistence_mode: sys::SpacePersistenceModeFB::INDEFINITE,
    };
    let mut request = sys::AsyncRequestIdFB::from_raw(0);
    match cvt(unsafe { (fp.save_space)(session.as_raw(), &info, &mut request) }) {
        Ok(_) => anchor.step = Step::Storing(request),
        Err(e) => {
            warn!("PARK ANCHOR can't be stored ({e:?}): the pieces stay by the tracking space");
            anchor.step = Step::Idle;
        }
    }
}

fn on_event(event: OxrEventIn, instance: Res<OxrInstance>, session: Option<Res<OxrSession>>, mut anchor: ResMut<ParkAnchor>) {
    let Some(session) = session else { return };
    match *event {
        openxr::Event::SpaceQueryResultsAvailableFB(e) if anchor.step == Step::Finding(e.request_id()) => {
            let Some(fp) = instance.exts().fb_spatial_entity_query.as_ref() else { return };
            let mut found = [sys::SpaceQueryResultFB { space: sys::Space::NULL, uuid: sys::UuidEXT { data: [0; 16] } }];
            let mut results = sys::SpaceQueryResultsFB {
                ty: sys::SpaceQueryResultsFB::TYPE,
                next: ptr::null_mut(),
                result_capacity_input: 1,
                result_count_output: 0,
                results: found.as_mut_ptr(),
            };
            match cvt(unsafe { (fp.retrieve_space_query_results)(session.as_raw(), e.request_id(), &mut results) }) {
                Ok(_) if results.result_count_output >= 1 && found[0].space != sys::Space::NULL => {
                    enable_locatable(&instance, found[0].space);
                    anchor.space = Some(found[0].space);
                    anchor.uuid = Some(found[0].uuid.data);
                    anchor.stored = true;
                }
                Ok(_) => {}
                Err(err) => warn!("PARK ANCHOR xrRetrieveSpaceQueryResultsFB: {err:?}"),
            }
        }
        openxr::Event::SpaceQueryCompleteFB(e) if anchor.step == Step::Finding(e.request_id()) => {
            anchor.step = Step::Idle;
            if anchor.space.is_some() {
                info!("PARK ANCHOR found");
            } else {
                warn!("PARK ANCHOR the saved anchor is gone ({:?}): the pieces stay by the tracking space, with a new anchor", e.result());
                anchor.record = None;
            }
        }
        openxr::Event::SpatialAnchorCreateCompleteFB(e) if anchor.step == Step::Making(e.request_id()) => {
            if let Err(err) = cvt(e.result()) {
                warn!("PARK ANCHOR wasn't made ({err:?}), trying again later");
                anchor.step = Step::Idle;
                anchor.wait = RETRY_SECONDS;
                return;
            }
            anchor.space = Some(e.space());
            anchor.uuid = Some(e.uuid().data);
            info!("PARK ANCHOR made: {}", hex(&e.uuid().data));
            enable_locatable(&instance, e.space());
            // It has to be storable before it can be stored.
            let Some(fp) = instance.exts().fb_spatial_entity.as_ref() else { return };
            let info = sys::SpaceComponentStatusSetInfoFB {
                ty: sys::SpaceComponentStatusSetInfoFB::TYPE,
                next: ptr::null(),
                component_type: sys::SpaceComponentTypeFB::STORABLE,
                enabled: sys::TRUE,
                timeout: sys::Duration::NONE,
            };
            let mut request = sys::AsyncRequestIdFB::from_raw(0);
            match unsafe { (fp.set_space_component_status)(e.space(), &info, &mut request) } {
                sys::Result::ERROR_SPACE_COMPONENT_STATUS_ALREADY_SET_FB => store(&instance, &session, &mut anchor),
                result => match cvt(result) {
                    Ok(_) => anchor.step = Step::MakingStorable(request),
                    Err(err) => {
                        warn!("PARK ANCHOR can't be made storable ({err:?}): the pieces stay by the tracking space");
                        anchor.step = Step::Idle;
                    }
                },
            }
        }
        openxr::Event::SpaceSetStatusCompleteFB(e) if anchor.step == Step::MakingStorable(e.request_id()) => {
            if let Err(err) = cvt(e.result()) {
                warn!("PARK ANCHOR wasn't made storable ({err:?}): the pieces stay by the tracking space");
                anchor.step = Step::Idle;
                return;
            }
            store(&instance, &session, &mut anchor);
        }
        openxr::Event::SpaceSaveCompleteFB(e) if anchor.step == Step::Storing(e.request_id()) => {
            anchor.step = Step::Idle;
            match cvt(e.result()) {
                Ok(_) => {
                    info!("PARK ANCHOR stored on the headset");
                    anchor.stored = true;
                }
                Err(err) => warn!("PARK ANCHOR wasn't stored ({err:?}): the pieces stay by the tracking space"),
            }
        }
        _ => {}
    }
}

/// Finds the anchor in the tracking space and keeps the pieces by it.
fn follow(
    instance: Res<OxrInstance>,
    primary: Res<XrPrimaryReferenceSpace>,
    frame: Option<Res<OxrFrameState>>,
    link: Res<SkaterLink>,
    mut placed: ResMut<PlacedPieces>,
    mut anchor: ResMut<ParkAnchor>,
) {
    let (Some(space), Some(uuid), true, Some(frame)) = (anchor.space, anchor.uuid, anchor.stored, frame) else { return };
    // (pieces are not moved under a hand or a grind)
    if placed.busy() || link.grinding {
        return;
    }
    let mut location = sys::SpaceLocation {
        ty: sys::SpaceLocation::TYPE,
        next: ptr::null_mut(),
        location_flags: sys::SpaceLocationFlags::EMPTY,
        pose: sys::Posef::IDENTITY,
    };
    let located = cvt(unsafe { (instance.fp().locate_space)(space, primary.as_raw_openxr_space(), frame.predicted_display_time, &mut location) });
    let needed = sys::SpaceLocationFlags::POSITION_VALID
        | sys::SpaceLocationFlags::ORIENTATION_VALID
        | sys::SpaceLocationFlags::POSITION_TRACKED
        | sys::SpaceLocationFlags::ORIENTATION_TRACKED;
    if located.is_err() || !location.location_flags.contains(needed) {
        return;
    }
    let (p, o) = (location.pose.position, location.pose.orientation);
    let position = Vec3::new(p.x, p.y, p.z);
    let along = Quat::from_xyzw(o.x, o.y, o.z, o.w).normalize() * Vec3::X;
    let yaw = f32::atan2(-along.z, along.x);
    if !position.is_finite() || !yaw.is_finite() {
        return;
    }
    let name = hex(&uuid);
    let now = AnchorRecord { uuid: name.clone(), position: position.to_array(), yaw };
    match anchor.record.clone().filter(|r| r.uuid == name) {
        // A new anchor: the poses as they stand are by it.
        None => {
            info!("PARK ANCHOR at {position:.3?}, turned {:.1} deg: saved with the layout", yaw.to_degrees());
            anchor.record = Some(now);
            anchor.aligned = true;
            placed.revision += 1;
        }
        Some(from) => {
            let moved = position.distance(Vec3::from_array(from.position));
            let turned = wrapped(yaw - from.yaw).abs();
            let (far, round) = if anchor.aligned { (MOVED_M, TURNED_RAD) } else { (FIRST_M, FIRST_RAD) };
            anchor.aligned = true;
            if moved < far && turned < round {
                return;
            }
            let rise = position.y - from.position[1];
            info!(
                "PARK ANCHOR moved {:.1} cm ({:+.1} cm up) and turned {:.2} deg in the tracking space: {} pieces moved with it",
                moved * 100.0,
                rise * 100.0,
                wrapped(yaw - from.yaw).to_degrees(),
                placed.items.len()
            );
            realign(&mut placed.items, &from, position, yaw);
            anchor.record = Some(now);
            placed.revision += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xr::build::pieces::{Piece, PieceKind};

    fn piece(position: [f32; 3], yaw: f32) -> Placed {
        Placed { id: 1, piece: Piece::new(PieceKind::Kicker), position, yaw, scale: 1.0, hidden: false, no_posts: 0, finish: None, tint: None, path: None }
    }

    #[test]
    fn pieces_keep_their_place_by_the_anchor() {
        let from = AnchorRecord { uuid: hex(&[7; 16]), position: [1.0, 0.03, -2.0], yaw: 0.3 };
        // A piece a metre along the anchor's X and 40 cm up, turned as the anchor is.
        let along = Quat::from_rotation_y(0.3) * Vec3::X;
        let mut items = vec![piece((Vec3::from_array(from.position) + along + Vec3::Y * 0.4).to_array(), 0.3)];
        // The tracking space's floor 3 cm lower, and the space moved and turned.
        let (position, yaw) = (Vec3::new(-0.5, 0.0, 0.7), 1.4);
        realign(&mut items, &from, position, yaw);
        let expected = position + Quat::from_rotation_y(1.4) * Vec3::X + Vec3::Y * 0.4;
        assert!(Vec3::from_array(items[0].position).distance(expected) < 1e-5, "{:?} vs {expected:?}", items[0].position);
        assert!((items[0].yaw - 1.4).abs() < 1e-6);
        // Not moved: nothing changes.
        let before = items.clone();
        let here = AnchorRecord { uuid: from.uuid.clone(), position: position.to_array(), yaw };
        realign(&mut items, &here, position, yaw);
        assert!(Vec3::from_array(items[0].position).distance(Vec3::from_array(before[0].position)) < 1e-6);
    }

    #[test]
    fn a_uuid_reads_back() {
        let uuid = [0x00, 0x1f, 0xa0, 0xff, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        assert_eq!(unhex(&hex(&uuid)), Some(uuid));
        assert_eq!(unhex("nope"), None);
        assert!((wrapped(std::f32::consts::TAU + 0.1) - 0.1).abs() < 1e-5);
    }
}
