//! The room's own reverberation on the skate sounds, by Steam Audio.
//!
//! The real room (Space Setup's walls, floor, ceiling, furniture and mesh, in real metres) is
//! given to Steam Audio's ray tracer as a scene. It traces how sound from the skater reaches the
//! head by way of the room's surfaces, and what comes back is an impulse response: the early
//! reflections off the nearest surfaces and the tail the room's size and shape give it, in
//! first-order Ambisonics so that it arrives from where it would. The simulation runs on a thread
//! of its own, again whenever the skater or the head has moved far enough to change it.
//!
//! Every sound that plays through the HRTF (`binaural`) also sends itself, at the level it is
//! heard at before distance, to a [`Room`]'s bus. One never-ending source ([`ReverbPcm`]) takes
//! what the bus has gathered each frame, convolves it with the room's response and decodes that
//! through the HRTF for where the head is turned.
//!
//! Without Steam Audio, or with no room known, there is no reverberation and nothing else changes.
use crate::binaural::{AudioBuffer, AudioSettings, FRAME, Handle, Phonon, phonon};
use bevy::audio::{Decodable, Source};
use bevy::prelude::*;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const RATE: u32 = crate::clean_synth::RATE;
/// The impulse response's length, seconds: a furnished room has died away well within it.
const SECONDS: f32 = 1.0;
const IR_SAMPLES: i32 = (SECONDS * RATE as f32) as i32;
/// First-order Ambisonics: four channels.
const ORDER: i32 = 1;
const CHANNELS: usize = 4;
/// Rays traced from the head each simulation, and how many surfaces each may meet: in a room
/// a ray meets one every 2 to 3 m, so this many carry the tail to about half a second.
const RAYS: i32 = 1024;
const BOUNCES: i32 = 64;
/// The room is simulated again once the skater or the head is this far from where it last was,
/// real metres, and no sooner than this after the last time.
const SOURCE_MOVED_M: f32 = 0.5;
const HEAD_MOVED_M: f32 = 0.3;
const SIMULATE_EVERY: Duration = Duration::from_millis(500);
/// The bus: this many samples gathered ahead, and how far ahead of the reader a sound starts
/// writing (the reverb's pre-delay, 21 ms).
const BUS_SAMPLES: usize = 16_384;
const BUS_LEAD: u64 = 2 * FRAME as u64;
/// Steam Audio's generic material (plaster and furnishings between them): what it takes of each
/// band (to 800 Hz, to 8 kHz, above), and how much of what it returns is scattered.
const ABSORPTION: [f32; 3] = [0.10, 0.20, 0.30];
const SCATTERING: f32 = 0.05;
const TRANSMISSION: [f32; 3] = [0.100, 0.050, 0.030];

// ---------------------------------------------------------------- phonon.h (the part used)

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct V3 {
    x: f32,
    y: f32,
    z: f32,
}

impl From<Vec3> for V3 {
    fn from(v: Vec3) -> Self {
        Self { x: v.x, y: v.y, z: v.z }
    }
}

/// `IPLCoordinateSpace3`.
#[repr(C)]
#[derive(Clone, Copy)]
struct Space {
    right: V3,
    up: V3,
    ahead: V3,
    origin: V3,
}

impl Space {
    fn new(position: Vec3, rotation: Quat) -> Self {
        Self { right: (rotation * Vec3::X).into(), up: (rotation * Vec3::Y).into(), ahead: (rotation * Vec3::NEG_Z).into(), origin: position.into() }
    }
}

#[repr(C)]
struct SceneSettings {
    kind: i32,
    callbacks: [*mut c_void; 4],
    user_data: *mut c_void,
    embree: Handle,
    radeon_rays: Handle,
}

#[repr(C)]
struct Material {
    absorption: [f32; 3],
    scattering: f32,
    transmission: [f32; 3],
}

#[repr(C)]
struct StaticMeshSettings {
    vertices_len: i32,
    triangles_len: i32,
    materials_len: i32,
    vertices: *mut V3,
    triangles: *mut [i32; 3],
    material_indices: *mut i32,
    materials: *mut Material,
}

/// `IPL_SIMULATIONFLAGS_REFLECTIONS`.
const REFLECTIONS: i32 = 1 << 1;
/// `IPL_REFLECTIONEFFECTTYPE_CONVOLUTION`.
const CONVOLUTION: i32 = 0;

#[repr(C)]
struct SimulationSettings {
    flags: i32,
    scene_type: i32,
    reflection_type: i32,
    max_occlusion_samples: i32,
    max_rays: i32,
    diffuse_samples: i32,
    max_duration: f32,
    max_order: i32,
    max_sources: i32,
    threads: i32,
    ray_batch: i32,
    vis_samples: i32,
    sampling_rate: i32,
    frame_size: i32,
    opencl: Handle,
    radeon_rays: Handle,
    tan: Handle,
}

#[repr(C)]
struct SharedInputs {
    listener: Space,
    rays: i32,
    bounces: i32,
    duration: f32,
    order: i32,
    irradiance_min_distance: f32,
    pathing_callback: *mut c_void,
    pathing_user_data: *mut c_void,
}

#[repr(C)]
struct DistanceAttenuation {
    kind: i32,
    min_distance: f32,
    callback: *mut c_void,
    user_data: *mut c_void,
    dirty: i32,
}

#[repr(C)]
struct AirAbsorption {
    kind: i32,
    coefficients: [f32; 3],
    callback: *mut c_void,
    user_data: *mut c_void,
    dirty: i32,
}

#[repr(C)]
struct Directivity {
    dipole_weight: f32,
    dipole_power: f32,
    callback: *mut c_void,
    user_data: *mut c_void,
}

#[repr(C)]
struct BakedData {
    kind: i32,
    variation: i32,
    centre: V3,
    radius: f32,
}

/// `IPLSimulationInputs`.
#[repr(C)]
struct SourceInputs {
    flags: i32,
    direct_flags: i32,
    source: Space,
    distance_attenuation: DistanceAttenuation,
    air_absorption: AirAbsorption,
    directivity: Directivity,
    occlusion_type: i32,
    occlusion_radius: f32,
    occlusion_samples: i32,
    reverb_scale: [f32; 3],
    hybrid_transition_time: f32,
    hybrid_overlap: f32,
    baked: i32,
    baked_data: BakedData,
    pathing_probes: Handle,
    vis_radius: f32,
    vis_threshold: f32,
    vis_range: f32,
    pathing_order: i32,
    enable_validation: i32,
    find_alternate_paths: i32,
    transmission_rays: i32,
    deviation_model: *mut c_void,
}

#[repr(C)]
struct DirectParams {
    flags: i32,
    transmission_type: i32,
    distance_attenuation: f32,
    air_absorption: [f32; 3],
    directivity: f32,
    occlusion: f32,
    transmission: [f32; 3],
}

#[repr(C)]
struct ReflectionParams {
    kind: i32,
    ir: Handle,
    reverb_times: [f32; 3],
    eq: [f32; 3],
    delay: i32,
    channels: i32,
    ir_size: i32,
    tan: Handle,
    tan_slot: i32,
}

#[repr(C)]
struct PathParams {
    eq: [f32; 3],
    sh: *mut f32,
    order: i32,
    binaural: i32,
    hrtf: Handle,
    listener: Space,
    normalise_eq: i32,
}

/// `IPLSimulationOutputs`.
#[repr(C)]
struct Outputs {
    direct: DirectParams,
    reflections: ReflectionParams,
    pathing: PathParams,
}

#[repr(C)]
struct ReflectionSettings {
    kind: i32,
    ir_size: i32,
    channels: i32,
}

#[repr(C)]
struct SpeakerLayout {
    kind: i32,
    speakers_len: i32,
    speakers: *mut V3,
}

#[repr(C)]
struct DecodeSettings {
    layout: SpeakerLayout,
    hrtf: Handle,
    max_order: i32,
}

#[repr(C)]
struct DecodeParams {
    order: i32,
    hrtf: Handle,
    orientation: Space,
    binaural: i32,
}

/// `IPL_SPEAKERLAYOUTTYPE_STEREO`.
const STEREO: i32 = 1;

struct Api {
    scene_create: unsafe extern "C" fn(Handle, *mut SceneSettings, *mut Handle) -> i32,
    scene_release: unsafe extern "C" fn(*mut Handle),
    scene_commit: unsafe extern "C" fn(Handle),
    mesh_create: unsafe extern "C" fn(Handle, *mut StaticMeshSettings, *mut Handle) -> i32,
    mesh_add: unsafe extern "C" fn(Handle, Handle),
    mesh_release: unsafe extern "C" fn(*mut Handle),
    simulator_create: unsafe extern "C" fn(Handle, *mut SimulationSettings, *mut Handle) -> i32,
    simulator_set_scene: unsafe extern "C" fn(Handle, Handle),
    simulator_commit: unsafe extern "C" fn(Handle),
    simulator_shared: unsafe extern "C" fn(Handle, i32, *mut SharedInputs),
    simulator_run: unsafe extern "C" fn(Handle),
    source_create: unsafe extern "C" fn(Handle, *mut i32, *mut Handle) -> i32,
    source_add: unsafe extern "C" fn(Handle, Handle),
    source_inputs: unsafe extern "C" fn(Handle, i32, *mut SourceInputs),
    source_outputs: unsafe extern "C" fn(Handle, i32, *mut Outputs),
    reflection_create: unsafe extern "C" fn(Handle, *mut AudioSettings, *mut ReflectionSettings, *mut Handle) -> i32,
    reflection_apply: unsafe extern "C" fn(Handle, *mut ReflectionParams, *mut AudioBuffer, *mut AudioBuffer, Handle) -> i32,
    reflection_release: unsafe extern "C" fn(*mut Handle),
    decode_create: unsafe extern "C" fn(Handle, *mut AudioSettings, *mut DecodeSettings, *mut Handle) -> i32,
    decode_apply: unsafe extern "C" fn(Handle, *mut DecodeParams, *mut AudioBuffer, *mut AudioBuffer) -> i32,
    decode_release: unsafe extern "C" fn(*mut Handle),
}

impl Api {
    fn load(phonon: &Phonon) -> Result<Self, String> {
        // SAFETY: the symbols are the functions phonon.h declares, with its signatures.
        unsafe {
            Ok(Self {
                scene_create: std::mem::transmute(phonon.symbol(b"iplSceneCreate\0")?),
                scene_release: std::mem::transmute(phonon.symbol(b"iplSceneRelease\0")?),
                scene_commit: std::mem::transmute(phonon.symbol(b"iplSceneCommit\0")?),
                mesh_create: std::mem::transmute(phonon.symbol(b"iplStaticMeshCreate\0")?),
                mesh_add: std::mem::transmute(phonon.symbol(b"iplStaticMeshAdd\0")?),
                mesh_release: std::mem::transmute(phonon.symbol(b"iplStaticMeshRelease\0")?),
                simulator_create: std::mem::transmute(phonon.symbol(b"iplSimulatorCreate\0")?),
                simulator_set_scene: std::mem::transmute(phonon.symbol(b"iplSimulatorSetScene\0")?),
                simulator_commit: std::mem::transmute(phonon.symbol(b"iplSimulatorCommit\0")?),
                simulator_shared: std::mem::transmute(phonon.symbol(b"iplSimulatorSetSharedInputs\0")?),
                simulator_run: std::mem::transmute(phonon.symbol(b"iplSimulatorRunReflections\0")?),
                source_create: std::mem::transmute(phonon.symbol(b"iplSourceCreate\0")?),
                source_add: std::mem::transmute(phonon.symbol(b"iplSourceAdd\0")?),
                source_inputs: std::mem::transmute(phonon.symbol(b"iplSourceSetInputs\0")?),
                source_outputs: std::mem::transmute(phonon.symbol(b"iplSourceGetOutputs\0")?),
                reflection_create: std::mem::transmute(phonon.symbol(b"iplReflectionEffectCreate\0")?),
                reflection_apply: std::mem::transmute(phonon.symbol(b"iplReflectionEffectApply\0")?),
                reflection_release: std::mem::transmute(phonon.symbol(b"iplReflectionEffectRelease\0")?),
                decode_create: std::mem::transmute(phonon.symbol(b"iplAmbisonicsDecodeEffectCreate\0")?),
                decode_apply: std::mem::transmute(phonon.symbol(b"iplAmbisonicsDecodeEffectApply\0")?),
                decode_release: std::mem::transmute(phonon.symbol(b"iplAmbisonicsDecodeEffectRelease\0")?),
            })
        }
    }
}

// ---------------------------------------------------------------- the room

/// The room's surfaces as triangles, real metres in tracking space; each faces the way its
/// corners turn anticlockwise from.
#[derive(Default)]
pub(crate) struct Geometry {
    pub vertices: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl Geometry {
    pub(crate) fn quad(&mut self, corners: [Vec3; 4]) {
        let first = self.vertices.len() as u32;
        self.vertices.extend(corners);
        self.triangles.extend([[first, first + 1, first + 2], [first, first + 2, first + 3]]);
    }
}

/// What the game last said: where things are, what the room is, how it should sound.
struct Want {
    geometry: Option<Arc<Geometry>>,
    /// Counts the changes of `geometry` and of `damping`.
    revision: u64,
    head: (Vec3, Quat),
    source: Vec3,
    /// How much more (or less) than the generic material the room's surfaces absorb.
    damping: f32,
}

struct Ring {
    data: Vec<f32>,
    /// The count of the next sample the reverb takes.
    read: u64,
}

/// The scene a simulation was last run in.
#[derive(Default)]
struct Simulated {
    revision: Option<u64>,
    scene: Option<(usize, usize)>,
    head: Vec3,
    source: Vec3,
}

pub(crate) struct Room {
    phonon: &'static Phonon,
    api: Api,
    simulator: Handle,
    source: Handle,
    want: Mutex<Want>,
    bus: Mutex<Ring>,
    /// The reverb's level against the sounds' own (linear).
    wet: AtomicU32,
    /// A response for the room as it is has been simulated.
    ready: AtomicBool,
    /// A playback has just started: Steam Audio hands a response to the effect that plays it
    /// once, when it is simulated, so one made since then has none until the next simulation.
    again: AtomicBool,
    /// Which way the head is turned, for the decoder (right, up, ahead).
    facing: [AtomicU32; 9],
    simulated: Mutex<Simulated>,
}

// SAFETY: the simulator and source are Steam Audio objects made to be used from the simulation
// thread and the mixer at once; every other call is made under the library's lock.
unsafe impl Send for Room {}
unsafe impl Sync for Room {}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Room {
    fn new() -> Result<Arc<Self>, String> {
        let phonon = phonon().ok_or("Steam Audio is not loaded")?;
        let api = Api::load(phonon)?;
        let mut settings = SimulationSettings {
            flags: REFLECTIONS,
            scene_type: 0,
            reflection_type: CONVOLUTION,
            max_occlusion_samples: 16,
            max_rays: RAYS,
            diffuse_samples: 32,
            max_duration: SECONDS,
            max_order: ORDER,
            max_sources: 1,
            threads: 1,
            ray_batch: 1,
            vis_samples: 1,
            sampling_rate: RATE as i32,
            frame_size: FRAME as i32,
            opencl: std::ptr::null_mut(),
            radeon_rays: std::ptr::null_mut(),
            tan: std::ptr::null_mut(),
        };
        let (mut simulator, mut source): (Handle, Handle) = (std::ptr::null_mut(), std::ptr::null_mut());
        let _call = phonon.call();
        // SAFETY: the context is live and the settings are as phonon.h lays them out.
        unsafe {
            let status = (api.simulator_create)(phonon.context, &mut settings, &mut simulator);
            if status != 0 || simulator.is_null() {
                return Err(format!("iplSimulatorCreate failed ({status})"));
            }
            let mut flags = REFLECTIONS; // (IPLSourceSettings is its flags alone)
            let status = (api.source_create)(simulator, &mut flags, &mut source);
            if status != 0 || source.is_null() {
                return Err(format!("iplSourceCreate failed ({status})"));
            }
            (api.source_add)(source, simulator);
            (api.simulator_commit)(simulator);
        }
        Ok(Arc::new(Self {
            phonon,
            api,
            simulator,
            source,
            want: Mutex::new(Want { geometry: None, revision: 0, head: (Vec3::ZERO, Quat::IDENTITY), source: Vec3::ZERO, damping: 1.0 }),
            bus: Mutex::new(Ring { data: vec![0.0; BUS_SAMPLES], read: 0 }),
            wet: AtomicU32::new(1.0f32.to_bits()),
            ready: AtomicBool::new(false),
            again: AtomicBool::new(false),
            facing: [1.0f32, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, -1.0].map(|v| AtomicU32::new(v.to_bits())),
            simulated: Mutex::default(),
        }))
    }

    /// The room's surfaces (none: no room, no reverberation).
    pub(crate) fn set_geometry(&self, geometry: Option<Arc<Geometry>>) {
        let mut want = lock(&self.want);
        want.geometry = geometry.filter(|g| !g.triangles.is_empty());
        want.revision += 1;
    }

    /// Where the head is and how it is turned, in the geometry's space.
    pub(crate) fn set_head(&self, position: Vec3, rotation: Quat) {
        lock(&self.want).head = (position, rotation);
        let axes = [rotation * Vec3::X, rotation * Vec3::Y, rotation * Vec3::NEG_Z];
        for (cell, v) in self.facing.iter().zip(axes.iter().flat_map(|a| a.to_array())) {
            cell.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    /// Where the sounds come from, in the geometry's space.
    pub(crate) fn set_source(&self, position: Vec3) {
        lock(&self.want).source = position;
    }

    /// The reverb's level (1: as simulated) and how absorbent the room is against the generic
    /// material (1: as it; more is a deader room).
    pub(crate) fn set_mix(&self, wet: f32, damping: f32) {
        self.wet.store(wet.clamp(0.0, 8.0).to_bits(), Ordering::Relaxed);
        let mut want = lock(&self.want);
        if (want.damping - damping).abs() > 1e-4 {
            want.damping = damping;
            want.revision += 1;
        }
    }

    /// Whether sounds are worth sending: there is a response to put them through.
    pub(crate) fn sounding(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    /// One sound's frame onto the bus, at `level`. `at` is that sound's own place on it.
    pub(crate) fn send(&self, at: &mut Option<u64>, frame: &[f32], level: f32) {
        let mut bus = lock(&self.bus);
        let fits = |w: u64| w >= bus.read && w + frame.len() as u64 <= bus.read + BUS_SAMPLES as u64;
        let write = at.filter(|&w| fits(w)).unwrap_or(bus.read + BUS_LEAD);
        if level != 0.0 {
            for (i, sample) in frame.iter().enumerate() {
                bus.data[(write + i as u64) as usize % BUS_SAMPLES] += sample * level;
            }
        }
        *at = Some(write + frame.len() as u64);
    }

    fn take(&self, frame: &mut [f32; FRAME]) {
        let mut bus = lock(&self.bus);
        for (i, out) in frame.iter_mut().enumerate() {
            let cell = (bus.read + i as u64) as usize % BUS_SAMPLES;
            *out = std::mem::take(&mut bus.data[cell]);
        }
        bus.read += FRAME as u64;
    }

    /// Simulate the room again if it, the skater or the head has changed enough; true if it did.
    fn simulate(&self) -> bool {
        let (geometry, revision, head, source, damping) = {
            let want = lock(&self.want);
            (want.geometry.clone(), want.revision, want.head, want.source, want.damping)
        };
        let mut simulated = lock(&self.simulated);
        let api = &self.api;
        if simulated.revision != Some(revision) {
            // The scene anew: made aside, then swapped in between simulations.
            self.ready.store(false, Ordering::Relaxed);
            let made = geometry.as_ref().and_then(|geometry| match self.scene(geometry, damping) {
                Ok(scene) => Some(scene),
                Err(why) => {
                    warn!("ROOM REVERB no scene: {why}");
                    None
                }
            });
            let _call = self.phonon.call();
            // SAFETY: no simulation is running (this is the thread that runs them), and the old
            // scene is released only after the simulator has let go of it.
            unsafe {
                (api.simulator_set_scene)(self.simulator, made.map_or(std::ptr::null_mut(), |(scene, _)| scene));
                (api.simulator_commit)(self.simulator);
                if let Some((scene, mesh)) = simulated.scene.take() {
                    let (mut scene, mut mesh) = (scene as Handle, mesh as Handle);
                    (api.mesh_release)(&mut mesh);
                    (api.scene_release)(&mut scene);
                }
            }
            simulated.scene = made.map(|(scene, mesh)| (scene as usize, mesh as usize));
            simulated.revision = Some(revision);
            simulated.head = Vec3::INFINITY;
        }
        if simulated.scene.is_none() {
            return false;
        }
        let moved = simulated.head.distance(head.0) >= HEAD_MOVED_M || simulated.source.distance(source) >= SOURCE_MOVED_M;
        if !self.again.swap(false, Ordering::Relaxed) && !moved {
            return false;
        }
        let mut shared = SharedInputs {
            listener: Space::new(head.0, head.1),
            rays: RAYS,
            bounces: BOUNCES,
            duration: SECONDS,
            order: ORDER,
            irradiance_min_distance: 1.0,
            pathing_callback: std::ptr::null_mut(),
            pathing_user_data: std::ptr::null_mut(),
        };
        // SAFETY: zero is "the default" or "none" for every field of IPLSimulationInputs.
        let mut inputs: SourceInputs = unsafe { std::mem::zeroed() };
        inputs.flags = REFLECTIONS;
        inputs.source = Space::new(source, Quat::IDENTITY);
        inputs.reverb_scale = [1.0; 3];
        inputs.hybrid_transition_time = 1.0;
        inputs.hybrid_overlap = 0.25;
        // SAFETY: the simulator and source are live; the inputs are as phonon.h lays them out.
        unsafe {
            {
                let _call = self.phonon.call();
                (api.simulator_shared)(self.simulator, REFLECTIONS, &mut shared);
                (api.source_inputs)(self.source, REFLECTIONS, &mut inputs);
            }
            // (not under the lock: this is the long part, and made to run beside the mixer)
            (api.simulator_run)(self.simulator);
        }
        (simulated.head, simulated.source) = (head.0, source);
        self.ready.store(true, Ordering::Relaxed);
        true
    }

    /// A Steam Audio scene of `geometry`, and its one mesh.
    fn scene(&self, geometry: &Geometry, damping: f32) -> Result<(Handle, Handle), String> {
        let api = &self.api;
        let mut vertices: Vec<V3> = geometry.vertices.iter().map(|&v| v.into()).collect();
        let mut triangles: Vec<[i32; 3]> = geometry.triangles.iter().map(|t| t.map(|i| i as i32)).collect();
        let mut indices = vec![0i32; triangles.len()];
        let mut material = Material { absorption: ABSORPTION.map(|a| (a * damping).clamp(0.01, 0.99)), scattering: SCATTERING, transmission: TRANSMISSION };
        let mut settings = SceneSettings {
            kind: 0,
            callbacks: [std::ptr::null_mut(); 4],
            user_data: std::ptr::null_mut(),
            embree: std::ptr::null_mut(),
            radeon_rays: std::ptr::null_mut(),
        };
        let mut mesh_settings = StaticMeshSettings {
            vertices_len: vertices.len() as i32,
            triangles_len: triangles.len() as i32,
            materials_len: 1,
            vertices: vertices.as_mut_ptr(),
            triangles: triangles.as_mut_ptr(),
            material_indices: indices.as_mut_ptr(),
            materials: &mut material,
        };
        let (mut scene, mut mesh): (Handle, Handle) = (std::ptr::null_mut(), std::ptr::null_mut());
        let _call = self.phonon.call();
        // SAFETY: the context is live; the mesh's arrays outlive the call, which copies them.
        unsafe {
            let status = (api.scene_create)(self.phonon.context, &mut settings, &mut scene);
            if status != 0 || scene.is_null() {
                return Err(format!("iplSceneCreate failed ({status})"));
            }
            let status = (api.mesh_create)(scene, &mut mesh_settings, &mut mesh);
            if status != 0 || mesh.is_null() {
                (api.scene_release)(&mut scene);
                return Err(format!("iplStaticMeshCreate failed ({status})"));
            }
            (api.mesh_add)(mesh, scene);
            (api.scene_commit)(scene);
        }
        Ok((scene, mesh))
    }
}

/// The app's room, once Steam Audio is loaded; its simulations run on a thread started here.
pub(crate) fn room() -> Option<&'static Arc<Room>> {
    static ROOM: OnceLock<Option<Arc<Room>>> = OnceLock::new();
    ROOM.get_or_init(|| match Room::new() {
        Ok(room) => {
            let simulated = room.clone();
            let work = move || {
                let mut told = None;
                loop {
                    let started = std::time::Instant::now();
                    if simulated.simulate() {
                        let revision = lock(&simulated.simulated).revision;
                        if told != revision {
                            told = revision;
                            let triangles = lock(&simulated.want).geometry.as_ref().map_or(0, |g| g.triangles.len());
                            info!("ROOM REVERB the room simulated in {:.0} ms ({triangles} triangles)", started.elapsed().as_secs_f32() * 1000.0);
                        }
                    }
                    std::thread::sleep(SIMULATE_EVERY.saturating_sub(started.elapsed()).max(Duration::from_millis(50)));
                }
            };
            match std::thread::Builder::new().name("room-reverb".into()).spawn(work) {
                Ok(_) => Some(room),
                Err(e) => {
                    warn!("ROOM REVERB off: no thread for it ({e})");
                    None
                }
            }
        }
        Err(why) => {
            info!("ROOM REVERB off: {why}");
            None
        }
    })
    .as_ref()
}

/// The game world to the room's own space (real metres, tracking space), where there is one: set
/// by the headset's rig, read by the sound driver to say where the skater is in the room.
#[derive(Resource, Default)]
pub(crate) struct RoomFromWorld(pub Option<bevy::math::Affine3A>);

// ---------------------------------------------------------------- the source

/// The room's answer to everything sent to it, as a Bevy audio source that never ends.
#[derive(Asset, TypePath, Clone)]
pub(crate) struct ReverbPcm(pub Arc<Room>);

pub(crate) struct ReverbPlayback {
    room: Arc<Room>,
    /// The convolution, the Ambisonics decoder and the HRTF it decodes through.
    effects: Option<(Handle, Handle, Handle)>,
    ready: VecDeque<f32>,
}

// SAFETY: the effects are used by this one playback, on whichever thread mixes it.
unsafe impl Send for ReverbPlayback {}
unsafe impl Sync for ReverbPlayback {}

impl Drop for ReverbPlayback {
    fn drop(&mut self) {
        if let Some((mut reflection, mut decode, _)) = self.effects.take() {
            let _call = self.room.phonon.call();
            // SAFETY: made by their create functions and released once.
            unsafe {
                (self.room.api.reflection_release)(&mut reflection);
                (self.room.api.decode_release)(&mut decode);
            }
        }
    }
}

impl ReverbPlayback {
    fn new(room: Arc<Room>) -> Self {
        let (phonon, api) = (room.phonon, &room.api);
        let effects = phonon.hrtf(RATE).and_then(|hrtf| {
            let mut audio = AudioSettings { sampling_rate: RATE as i32, frame_size: FRAME as i32 };
            let mut reflection_settings = ReflectionSettings { kind: CONVOLUTION, ir_size: IR_SAMPLES, channels: CHANNELS as i32 };
            let mut decode_settings = DecodeSettings { layout: SpeakerLayout { kind: STEREO, speakers_len: 2, speakers: std::ptr::null_mut() }, hrtf, max_order: ORDER };
            let (mut reflection, mut decode): (Handle, Handle) = (std::ptr::null_mut(), std::ptr::null_mut());
            let _call = phonon.call();
            // SAFETY: the context and HRTF are live and the settings are as phonon.h lays them out.
            unsafe {
                let status = (api.reflection_create)(phonon.context, &mut audio, &mut reflection_settings, &mut reflection);
                if status != 0 || reflection.is_null() {
                    warn!("ROOM REVERB off: iplReflectionEffectCreate failed ({status})");
                    return None;
                }
                let status = (api.decode_create)(phonon.context, &mut audio, &mut decode_settings, &mut decode);
                if status != 0 || decode.is_null() {
                    warn!("ROOM REVERB off: iplAmbisonicsDecodeEffectCreate failed ({status})");
                    (api.reflection_release)(&mut reflection);
                    return None;
                }
            }
            Some((reflection, decode, hrtf))
        });
        room.again.store(true, Ordering::Relaxed);
        Self { room, effects, ready: VecDeque::with_capacity(2 * FRAME) }
    }

    /// One frame of the room's answer into `ready`.
    fn render(&mut self) {
        let mut mono = [0.0f32; FRAME];
        self.room.take(&mut mono);
        let (mut left, mut right) = ([0.0f32; FRAME], [0.0f32; FRAME]);
        if let Some((reflection, decode, hrtf)) = self.effects.filter(|_| self.room.sounding()) {
            let api = &self.room.api;
            let mut ambisonic = [[0.0f32; FRAME]; CHANNELS];
            let mut input = [mono.as_mut_ptr()];
            let mut middle = ambisonic.each_mut().map(|channel| channel.as_mut_ptr());
            let mut output = [left.as_mut_ptr(), right.as_mut_ptr()];
            let mut from = AudioBuffer { channels: 1, samples: FRAME as i32, data: input.as_mut_ptr() };
            let mut through = AudioBuffer { channels: CHANNELS as i32, samples: FRAME as i32, data: middle.as_mut_ptr() };
            let mut to = AudioBuffer { channels: 2, samples: FRAME as i32, data: output.as_mut_ptr() };
            let facing = self.room.facing.each_ref().map(|cell| f32::from_bits(cell.load(Ordering::Relaxed)));
            let axis = |i: usize| V3 { x: facing[3 * i], y: facing[3 * i + 1], z: facing[3 * i + 2] };
            let mut decode_params =
                DecodeParams { order: ORDER, hrtf, orientation: Space { right: axis(0), up: axis(1), ahead: axis(2), origin: V3::default() }, binaural: 1 };
            let _call = self.room.phonon.call();
            // SAFETY: the source and effects are live; every buffer is FRAME samples a channel;
            // zero is a valid IPLSimulationOutputs to be filled in.
            unsafe {
                let mut outputs: Outputs = std::mem::zeroed();
                (api.source_outputs)(self.room.source, REFLECTIONS, &mut outputs);
                if !outputs.reflections.ir.is_null() {
                    outputs.reflections.kind = CONVOLUTION;
                    outputs.reflections.channels = CHANNELS as i32;
                    outputs.reflections.ir_size = IR_SAMPLES;
                    (api.reflection_apply)(reflection, &mut outputs.reflections, &mut from, &mut through, std::ptr::null_mut());
                    (api.decode_apply)(decode, &mut decode_params, &mut through, &mut to);
                }
            }
        }
        let wet = f32::from_bits(self.room.wet.load(Ordering::Relaxed));
        self.ready.extend(left.iter().zip(&right).flat_map(|(l, r)| [l * wet, r * wet]));
    }
}

impl Iterator for ReverbPlayback {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.ready.is_empty() {
            self.render();
        }
        self.ready.pop_front()
    }
}

impl Source for ReverbPlayback {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        RATE
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Decodable for ReverbPcm {
    type DecoderItem = f32;
    type Decoder = ReverbPlayback;

    fn decoder(&self) -> ReverbPlayback {
        ReverbPlayback::new(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A room `size` across, its walls facing in, the floor at y = 0.
    fn shoebox(size: Vec3) -> Arc<Geometry> {
        let (x, y, z) = (size.x / 2.0, size.y, size.z / 2.0);
        let corner = |i: usize| Vec3::new(if i & 1 == 0 { -x } else { x }, if i & 2 == 0 { 0.0 } else { y }, if i & 4 == 0 { -z } else { z });
        let mut geometry = Geometry::default();
        // (each seen from inside, anticlockwise)
        for face in [[0, 4, 5, 1], [2, 3, 7, 6], [0, 2, 6, 4], [1, 5, 7, 3], [0, 1, 3, 2], [4, 6, 7, 5]] {
            geometry.quad(face.map(corner));
        }
        Arc::new(geometry)
    }

    /// The level of each 100 ms of the room's answer to a click, left and right together.
    fn answer(room: &Arc<Room>) -> Vec<f32> {
        let mut playback = ReverbPcm(room.clone()).decoder();
        room.simulate();
        let mut click = [0.0f32; FRAME];
        click[0] = 1.0;
        room.send(&mut None, &click, 1.0);
        let samples: Vec<f32> = playback.by_ref().take(2 * RATE as usize).collect();
        samples.chunks(2 * RATE as usize / 10).map(|c| c.iter().map(|s| s * s).sum::<f32>().sqrt()).collect()
    }

    #[test]
    fn the_bus_gathers_sounds_and_hands_each_sample_out_once() {
        let Ok(room) = Room::new() else {
            eprintln!("Steam Audio not found: skipped");
            return;
        };
        let (mut a, mut b) = (None, None);
        let ones = [1.0f32; FRAME];
        let mut heard = 0.0;
        for _ in 0..40 {
            room.send(&mut a, &ones, 0.5);
            room.send(&mut b, &ones, 0.25);
            let mut frame = [0.0; FRAME];
            room.take(&mut frame);
            heard += frame.iter().sum::<f32>();
        }
        // Everything sent but the lead still on the bus: 38 of the 40 frames, both sounds summed.
        assert!((heard - 38.0 * FRAME as f32 * 0.75).abs() < 1.0, "{heard}");
        // A sound that fell behind starts again ahead of the reader.
        let mut late = Some(0);
        room.send(&mut late, &ones, 1.0);
        assert_eq!(late, Some(lock(&room.bus).read + BUS_LEAD + FRAME as u64));
    }

    /// Needs libphonon.so (scripts/quest/fetch-steam-audio.sh): skipped without it.
    #[test]
    fn a_room_answers_and_a_bigger_or_barer_one_for_longer() {
        let Ok(room) = Room::new() else {
            eprintln!("Steam Audio not found: skipped");
            return;
        };
        assert!(answer(&room).iter().all(|&level| level == 0.0), "no room, no reverberation");
        room.set_geometry(Some(shoebox(Vec3::new(4.0, 2.5, 5.0))));
        room.set_head(Vec3::new(0.5, 1.6, 1.0), Quat::IDENTITY);
        room.set_source(Vec3::new(-1.0, 0.1, -1.0));
        let started = std::time::Instant::now();
        assert!(room.simulate() && room.sounding());
        println!("simulated in {:.0} ms", started.elapsed().as_secs_f32() * 1000.0);
        assert!(!room.simulate(), "nothing has moved");
        let small = answer(&room);
        println!("a 4 x 2.5 x 5 m room, each 100 ms: {small:?}");
        assert!(small[0] > 1e-3, "the room answers: {small:?}");
        assert!(small[0] > 3.0 * small[4], "and dies away: {small:?}");
        assert!(small.iter().all(|level| level.is_finite()));
        // A hall: more of it left 0.6 s on.
        room.set_geometry(Some(shoebox(Vec3::new(20.0, 8.0, 30.0))));
        assert!(room.simulate());
        let hall = answer(&room);
        assert!(hall[6] / hall[0] > 2.0 * small[6] / small[0], "small {small:?}, hall {hall:?}");
        // The same small room with soft walls: less of it.
        room.set_geometry(Some(shoebox(Vec3::new(4.0, 2.5, 5.0))));
        room.set_mix(1.0, 3.0);
        assert!(room.simulate());
        let soft = answer(&room);
        let total = |levels: &[f32]| levels.iter().map(|l| l * l).sum::<f32>();
        assert!(total(&soft) < 0.6 * total(&small), "small {small:?}, soft {soft:?}");
        room.set_geometry(None);
        assert!(!room.simulate() && !room.sounding());
    }

    #[test]
    fn the_answer_comes_from_the_side_the_sound_is_on() {
        let Ok(room) = Room::new() else {
            eprintln!("Steam Audio not found: skipped");
            return;
        };
        // A wide room, the head in the middle looking down -z, the source by the right-hand wall.
        room.set_geometry(Some(shoebox(Vec3::new(12.0, 3.0, 6.0))));
        room.set_head(Vec3::new(0.0, 1.6, 0.0), Quat::IDENTITY);
        room.set_source(Vec3::new(5.0, 1.0, 0.0));
        let side = |room: &Arc<Room>| {
            let mut playback = ReverbPcm(room.clone()).decoder();
            assert!(room.simulate(), "a new playback is given the response");
            let mut click = [0.0f32; FRAME];
            click[0] = 1.0;
            room.send(&mut None, &click, 1.0);
            // (the first 60 ms: the early reflections, off that wall)
            let samples: Vec<f32> = playback.by_ref().take(2 * (RATE as usize * 60 / 1000 + BUS_LEAD as usize)).collect();
            let power = |skip: usize| samples.iter().skip(skip).step_by(2).map(|s| s * s).sum::<f32>();
            (power(0), power(1))
        };
        let (left, right) = side(&room);
        assert!(right > 1.3 * left, "from the right: left {left}, right {right}");
        // Turned to face +z, that wall is on the left.
        room.set_head(Vec3::new(0.0, 1.6, 0.0), Quat::from_rotation_y(std::f32::consts::PI));
        let (left, right) = side(&room);
        assert!(left > 1.3 * right, "turned round: left {left}, right {right}");
    }
}
