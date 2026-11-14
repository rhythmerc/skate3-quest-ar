//! Not for players: a few seconds of what occlusion works from, saved to
//! files, so that its filtering can be worked on at a desk against the
//! real thing (the developer's row on the AR settings page starts one).
//!
//! A capture is a folder in [`super::XrFiles`]' `captures`:
//!
//! - `capture.jsonl`: a line of JSON for each thing, in the order they came.
//!   `start` (the room's surfaces), `frame` (each drawn frame: its display
//!   time, the eyes in tracking space, the clocks), `depth` (a depth image
//!   that differs from the one before it: its view, and where it is in
//!   `depth.bin`), `camera_info` (the left passthrough camera's calibration)
//!   and `camera` (a camera frame: its time, and where it is in
//!   `camera.bin`).
//! - `depth.bin`: the depth images, 16 bits a texel as the headset gave
//!   them (little endian, the image's first row first).
//! - `camera.bin`: the camera's frames: the Y plane, then U and V at half
//!   the size each way.
//!
//! The camera's frames are of the room: the files stay on the headset until
//! they are pulled off it over adb, and go nowhere else.
use bevy::prelude::*;
use bevy::render::render_resource::Buffer;
use bevy::render::renderer::RenderDevice;
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a capture runs. (The camera takes about a second of it to open.)
pub(crate) const SECONDS: u64 = 5;

/// One thing for the capture's files.
pub(crate) enum Item {
    Note(Value),
    /// A depth image (16 bits a texel) or a camera frame, with its note.
    Depth(Value, Vec<u8>),
    Camera(Value, Vec<u8>),
}

struct Running {
    tx: Sender<Item>,
    until: Instant,
    /// Set when the capture ends: the camera's thread closes.
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
struct Shared {
    running: Mutex<Option<Running>>,
    status: Mutex<String>,
}

/// The capture, in the game's world and the renderer's.
#[derive(Resource, Clone, Default)]
pub(crate) struct Capture(Arc<Shared>);

impl Capture {
    /// Starts a capture into `folder` (made here). `start` is its first note.
    pub(crate) fn start(&self, folder: PathBuf, start: Value) -> Result<(), String> {
        let mut running = self.0.running.lock().unwrap();
        if running.is_some() {
            return Err("A capture is already running.".into());
        }
        std::fs::create_dir_all(&folder).map_err(|e| format!("No capture folder: {e}"))?;
        let files = Files::create(&folder).map_err(|e| format!("No capture files: {e}"))?;
        let (tx, rx) = channel();
        let shared = self.0.clone();
        std::thread::Builder::new()
            .name("sk8-capture".into())
            .spawn(move || {
                let said = write(files, rx);
                info!("Occlusion capture: {said} ({})", folder.display());
                *shared.status.lock().unwrap() = said;
            })
            .map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let _ = tx.send(Item::Note(start));
        #[cfg(target_os = "android")]
        {
            let (tx, stop) = (tx.clone(), stop.clone());
            let _ = std::thread::Builder::new().name("sk8-capture-camera".into()).spawn(move || camera(&tx, &stop));
        }
        *self.0.status.lock().unwrap() = format!("Recording for {SECONDS} seconds...");
        *running = Some(Running { tx, until: Instant::now() + Duration::from_secs(SECONDS), stop });
        Ok(())
    }

    /// Where to send things while a capture runs.
    pub(crate) fn sender(&self) -> Option<Sender<Item>> {
        self.0.running.lock().unwrap().as_ref().map(|r| r.tx.clone())
    }

    /// Ends the capture when its time is up.
    fn tick(&self) {
        let mut running = self.0.running.lock().unwrap();
        if running.as_ref().is_some_and(|r| Instant::now() >= r.until) {
            // (said first: the files' thread says how it went once the last sender is gone)
            *self.0.status.lock().unwrap() = "Saving...".into();
            if let Some(ended) = running.take() {
                ended.stop.store(true, Ordering::Relaxed);
            }
        }
    }

    /// For the settings page (empty: no capture yet).
    pub(crate) fn status(&self) -> String {
        self.0.status.lock().unwrap().clone()
    }
}

struct Files {
    notes: std::io::BufWriter<std::fs::File>,
    depth: std::io::BufWriter<std::fs::File>,
    camera: std::io::BufWriter<std::fs::File>,
}

impl Files {
    fn create(folder: &std::path::Path) -> std::io::Result<Self> {
        let file = |name: &str| std::fs::File::create(folder.join(name)).map(std::io::BufWriter::new);
        Ok(Self { notes: file("capture.jsonl")?, depth: file("depth.bin")?, camera: file("camera.bin")? })
    }
}

/// Writes what comes until every sender is gone, and says what was saved.
fn write(mut files: Files, rx: Receiver<Item>) -> String {
    let (mut depth_at, mut camera_at) = (0u64, 0u64);
    let (mut depths, mut frames) = (0, 0);
    let mut put = |files: &mut Files| -> std::io::Result<()> {
        for item in rx.iter() {
            let (mut note, blob) = match item {
                Item::Note(note) => (note, None),
                Item::Depth(note, data) => (note, Some((false, data))),
                Item::Camera(note, data) => (note, Some((true, data))),
            };
            if let Some((camera, data)) = blob {
                let (file, at, count) =
                    if camera { (&mut files.camera, &mut camera_at, &mut frames) } else { (&mut files.depth, &mut depth_at, &mut depths) };
                note["offset"] = json!(*at);
                note["len"] = json!(data.len());
                file.write_all(&data)?;
                *at += data.len() as u64;
                *count += 1;
            }
            writeln!(files.notes, "{note}")?;
        }
        files.notes.flush()?;
        files.depth.flush()?;
        files.camera.flush()
    };
    match put(&mut files) {
        Ok(()) => format!("Saved {depths} depth images and {frames} camera frames"),
        Err(e) => format!("The capture stopped: {e}"),
    }
}

/// The steady clock and the one that also counts sleep, ns (0, 0 where
/// there are none): a camera frame's time is on one of them.
pub(crate) fn clocks() -> (i64, i64) {
    #[cfg(unix)]
    {
        #[repr(C)]
        struct Timespec {
            seconds: i64,
            nanos: i64,
        }
        unsafe extern "C" {
            fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
        }
        const MONOTONIC: i32 = 1;
        const BOOTTIME: i32 = 7;
        let read = |clock| {
            let mut t = Timespec { seconds: 0, nanos: 0 };
            if unsafe { clock_gettime(clock, &mut t) } == 0 { t.seconds * 1_000_000_000 + t.nanos } else { 0 }
        };
        (read(MONOTONIC), read(BOOTTIME))
    }
    #[cfg(not(unix))]
    (0, 0)
}

/// Each drawn frame's note while a capture runs, and its end.
pub(super) fn log_frames(
    capture: Res<Capture>,
    frame: Option<Res<bevy_mod_openxr::resources::OxrFrameState>>,
    rig: Res<super::rig::Rig>,
    eyes: Query<(&Transform, &bevy_mod_xr::camera::XrCamera)>,
) {
    capture.tick();
    let Some(tx) = capture.sender() else { return };
    let (mono, boot) = clocks();
    // (The eye cameras' own transforms are tracking space.)
    let eyes: Vec<Value> = eyes
        .iter()
        .map(|(t, eye)| json!({ "eye": eye.0, "p": t.translation.to_array(), "q": t.rotation.to_array() }))
        .collect();
    let _ = tx.send(Item::Note(json!({
        "kind": "frame",
        "xr_time": frame.map(|f| f.predicted_display_time.as_nanos()),
        "mono": mono,
        "boot": boot,
        "eyes": eyes,
        "world_from_tracking": rig.root_transform().map(|r| r.to_matrix().to_cols_array()),
    })));
}

/// The left passthrough camera's frames for as long as the capture runs.
/// (Room lighting's capture gives the camera up meanwhile.)
#[cfg(target_os = "android")]
fn camera(tx: &Sender<Item>, stop: &AtomicBool) {
    use super::room_light::camera::{BORROWED, Pick, Stream};
    let missing = |why: String| {
        warn!("Occlusion capture: no camera: {why}");
        let _ = tx.send(Item::Note(json!({ "kind": "camera_missing", "why": why })));
    };
    if !crate::camera_access() {
        return missing("the cameras aren't allowed (turn room lighting on once)".into());
    }
    struct Lent;
    impl Drop for Lent {
        fn drop(&mut self) {
            BORROWED.store(false, Ordering::Relaxed);
        }
    }
    BORROWED.store(true, Ordering::Relaxed);
    let _lent = Lent;
    // (room lighting's thread lets go within its next look, under a second)
    std::thread::sleep(Duration::from_millis(800));
    let mut opened = Err(String::new());
    for _ in 0..4 {
        opened = Stream::open(Pick::UpTo(1280));
        if opened.is_ok() || stop.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    let mut stream = match opened {
        Ok(stream) => stream,
        Err(why) => return missing(why),
    };
    let mut info = stream.calibration.clone();
    info["kind"] = json!("camera_info");
    let _ = tx.send(Item::Note(info));
    while !stop.load(Ordering::Relaxed) {
        let Some(frame) = stream.next_packed() else {
            std::thread::sleep(Duration::from_millis(3));
            continue;
        };
        let (mono, boot) = clocks();
        let note = json!({
            "kind": "camera",
            "timestamp": frame.timestamp_ns,
            "mono": mono,
            "boot": boot,
            "exposure_ns": frame.exposure_ns,
            "iso": frame.iso,
            "width": frame.width,
            "height": frame.height,
        });
        if tx.send(Item::Camera(note, frame.data)).is_err() {
            break;
        }
    }
}

/// A depth image on its way back from the GPU.
struct Pending {
    buffer: Buffer,
    /// 0: not asked for. 1: asked. 2: mapped. 3: failed.
    state: Arc<AtomicU8>,
    note: Value,
    layer: usize,
    width: usize,
    height: usize,
    row_bytes: usize,
}

/// The depth images being read back (the renderer's world).
#[derive(Resource, Default)]
pub(super) struct Readbacks {
    pending: Mutex<Vec<Pending>>,
    /// Each eye's last image, hashed: only one that differs is saved.
    last: [u64; 2],
}

impl Readbacks {
    /// The bytes a row of `width` texels takes in a read-back buffer.
    pub(super) fn row_bytes(width: u32) -> u32 {
        (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
    }

    /// A buffer the eye's depth image (32-bit floats) was just copied to.
    pub(super) fn add(&self, buffer: Buffer, note: Value, layer: usize, width: u32, height: u32) {
        self.pending.lock().unwrap().push(Pending {
            buffer,
            state: Arc::new(AtomicU8::new(0)),
            note,
            layer,
            width: width as usize,
            height: height as usize,
            row_bytes: Self::row_bytes(width) as usize,
        });
    }
}

/// The depth images copied out this frame and before are mapped, and those
/// that have come back are sent to the capture's files.
pub(super) fn collect(capture: Res<Capture>, device: Res<RenderDevice>, mut readbacks: ResMut<Readbacks>) {
    let Readbacks { pending, last } = &mut *readbacks;
    let pending = pending.get_mut().unwrap();
    if pending.is_empty() {
        return;
    }
    for p in pending.iter() {
        if p.state.compare_exchange(0, 1, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            let state = p.state.clone();
            p.buffer.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                state.store(if result.is_ok() { 2 } else { 3 }, Ordering::Relaxed);
            });
        }
    }
    let _ = device.poll(wgpu::PollType::Poll);
    let tx = capture.sender();
    pending.retain_mut(|p| match p.state.load(Ordering::Relaxed) {
        2 => {
            let data = texels(&p.buffer.slice(..).get_mapped_range(), p.width, p.height, p.row_bytes);
            p.buffer.unmap();
            let hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                data.hash(&mut hasher);
                hasher.finish()
            };
            if let (Some(tx), true) = (&tx, last[p.layer.min(1)] != hash) {
                last[p.layer.min(1)] = hash;
                let mut note = std::mem::take(&mut p.note);
                note["width"] = json!(p.width);
                note["height"] = json!(p.height);
                let _ = tx.send(Item::Depth(note, data));
            }
            false
        }
        3 => false,
        // (still on its way; with no capture any more it is dropped)
        _ => tx.is_some(),
    });
}

/// A read-back image's rows of 32-bit floats (0-1) as 16 bits a texel, the
/// headset's own values.
fn texels(mapped: &[u8], width: usize, height: usize, row_bytes: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(width * height * 2);
    for row in mapped.chunks(row_bytes).take(height) {
        for texel in row.chunks_exact(4).take(width) {
            let d = f32::from_le_bytes([texel[0], texel[1], texel[2], texel[3]]);
            out.extend_from_slice(&((d.clamp(0.0, 1.0) * 65535.0).round() as u16).to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_is_written_where_its_notes_say() {
        let folder = std::env::temp_dir().join(format!("sk8-capture-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let (tx, rx) = channel();
        tx.send(Item::Note(json!({ "kind": "start" }))).unwrap();
        tx.send(Item::Depth(json!({ "kind": "depth", "layer": 0 }), vec![1, 2, 3, 4])).unwrap();
        tx.send(Item::Camera(json!({ "kind": "camera" }), vec![9; 6])).unwrap();
        tx.send(Item::Depth(json!({ "kind": "depth", "layer": 1 }), vec![5, 6])).unwrap();
        drop(tx);
        let said = write(Files::create(&folder).unwrap(), rx);
        assert_eq!(said, "Saved 2 depth images and 1 camera frames");
        let notes: Vec<Value> = std::fs::read_to_string(folder.join("capture.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(notes.len(), 4);
        assert_eq!(notes[0]["kind"], "start");
        assert_eq!((notes[1]["offset"].as_u64(), notes[1]["len"].as_u64()), (Some(0), Some(4)));
        assert_eq!((notes[3]["offset"].as_u64(), notes[3]["len"].as_u64()), (Some(4), Some(2)), "the second image follows the first");
        assert_eq!(notes[2]["offset"].as_u64(), Some(0), "the camera's frames are their own file");
        assert_eq!(std::fs::read(folder.join("depth.bin")).unwrap(), [1, 2, 3, 4, 5, 6]);
        assert_eq!(std::fs::read(folder.join("camera.bin")).unwrap(), [9; 6]);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_read_back_image_is_the_headsets_values_without_the_rows_padding() {
        // Two texels a row in rows of twelve bytes (four of padding), two rows.
        let mut mapped = Vec::new();
        for row in [[0.0f32, 1.0], [0.5, 32768.0 / 65535.0]] {
            for d in row {
                mapped.extend_from_slice(&d.to_le_bytes());
            }
            mapped.extend_from_slice(&[0xAA; 4]);
        }
        let out = texels(&mapped, 2, 2, 12);
        let values: Vec<u16> = out.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(values, [0, 65535, 32768, 32768]);
    }

    #[test]
    fn a_capture_runs_once_at_a_time_and_says_how_it_went() {
        let folder = std::env::temp_dir().join(format!("sk8-capture-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let capture = Capture::default();
        assert!(capture.sender().is_none());
        capture.start(folder.clone(), json!({ "kind": "start" })).unwrap();
        assert!(capture.start(folder.clone(), json!({})).is_err(), "not a second one over the first");
        assert!(capture.sender().is_some());
        // Its time is up: it ends, and the files are finished.
        capture.0.running.lock().unwrap().as_mut().unwrap().until = Instant::now();
        capture.tick();
        assert!(capture.sender().is_none());
        for _ in 0..200 {
            if capture.status().starts_with("Saved") {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(capture.status(), "Saved 0 depth images and 0 camera frames");
        assert!(std::fs::read_to_string(folder.join("capture.jsonl")).unwrap().contains("start"));
        let _ = std::fs::remove_dir_all(&folder);
    }
}
