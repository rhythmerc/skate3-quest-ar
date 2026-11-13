//! The clean sounds heard through Steam Audio's HRTF: where a sound is in the
//! room (left, right, in front, behind, above, below) by what the head and
//! ears do to it, instead of a pan between two speakers.
//!
//! Steam Audio (Valve, Apache-2.0) is not built in: `libphonon.so` is loaded
//! when the app starts, from the APK on the headset
//! (`scripts/quest/fetch-steam-audio.sh` puts it there) or wherever
//! `SK8_STEAM_AUDIO` names. Without it every sound plays as it did before,
//! through Bevy's own panning.
//!
//! [`CleanPcm`] is the sound driver's source either way. With a [`Voice`] it
//! comes out in stereo: the mono sound, read at the voice's pitch, at the
//! emitter's level for its distance, and put through a binaural effect of its
//! own toward the emitter's direction from the head.
use crate::clean_mix::{Buffer, Filter, Strip};
use bevy::audio::{Decodable, Source};
use bevy::prelude::*;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Samples a binaural effect takes at a time (about 11 ms at 48 kHz: how late
/// a turn of the head is heard).
pub(crate) const FRAME: usize = 512;
/// The version of the interface these declarations are of (4.8.1).
const VERSION: u32 = (4 << 16) | (8 << 8) | 1;

// ---------------------------------------------------------------- phonon.h (the part used)

pub(crate) type Handle = *mut c_void;

#[repr(C)]
struct ContextSettings {
    version: u32,
    log: Option<unsafe extern "C" fn()>,
    allocate: Option<unsafe extern "C" fn()>,
    free: Option<unsafe extern "C" fn()>,
    simd_level: i32,
    flags: i32,
}

#[repr(C)]
pub(crate) struct AudioSettings {
    pub sampling_rate: i32,
    pub frame_size: i32,
}

#[repr(C)]
struct HrtfSettings {
    kind: i32,
    sofa_file_name: *const std::ffi::c_char,
    sofa_data: *const u8,
    sofa_data_size: i32,
    volume: f32,
    norm_type: i32,
}

#[repr(C)]
struct BinauralEffectSettings {
    hrtf: Handle,
}

#[repr(C)]
struct BinauralEffectParams {
    direction: [f32; 3],
    interpolation: i32,
    spatial_blend: f32,
    hrtf: Handle,
    peak_delays: *mut f32,
}

#[repr(C)]
pub(crate) struct AudioBuffer {
    pub channels: i32,
    pub samples: i32,
    pub data: *mut *mut f32,
}

/// `IPL_HRTFINTERPOLATION_BILINEAR`: between the measured directions, so a
/// moving sound doesn't step from one to the next.
const BILINEAR: i32 = 1;
/// `IPL_SIMDLEVEL_AVX2` (it uses the best the machine has up to this; NEON on the headset).
const SIMD: i32 = 3;

pub(crate) struct Phonon {
    // (kept loaded for as long as its functions are)
    library: libloading::Library,
    pub(crate) context: Handle,
    effect_create: unsafe extern "C" fn(Handle, *mut AudioSettings, *mut BinauralEffectSettings, *mut Handle) -> i32,
    effect_release: unsafe extern "C" fn(*mut Handle),
    effect_apply: unsafe extern "C" fn(Handle, *mut BinauralEffectParams, *mut AudioBuffer, *mut AudioBuffer) -> i32,
    /// The built-in HRTF at each sampling rate asked for so far.
    hrtfs: std::sync::Mutex<Vec<(u32, usize)>>,
    hrtf_create: unsafe extern "C" fn(Handle, *mut AudioSettings, *mut HrtfSettings, *mut Handle) -> i32,
    /// Held for every call into the library. Effects are made where a sound
    /// is started and used and released where sound is mixed, and Steam
    /// Audio did not stand those happening at once: a sound came out wrong,
    /// and on the Quest the mixer aborted on a mutex already destroyed.
    calls: std::sync::Mutex<()>,
}

// SAFETY: the context and HRTFs are Steam Audio objects made to be shared between threads; each
// effect is used by the one playback that owns it.
unsafe impl Send for Phonon {}
unsafe impl Sync for Phonon {}

impl Phonon {
    /// A function of the library by name (`name` ends in a NUL), for the parts of it used elsewhere.
    pub(crate) fn symbol(&self, name: &[u8]) -> Result<*const c_void, String> {
        // SAFETY: only looked up here; the caller gives it its type.
        unsafe { self.library.get::<*const c_void>(name) }.map(|s| *s).map_err(|e| e.to_string())
    }

    pub(crate) fn call(&self) -> std::sync::MutexGuard<'_, ()> {
        self.calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn load() -> Result<Self, String> {
        let mut places: Vec<std::path::PathBuf> = std::env::var_os("SK8_STEAM_AUDIO").map(Into::into).into_iter().collect();
        places.push("libphonon.so".into());
        if let Some(beside) = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("libphonon.so"))) {
            places.push(beside);
        }
        // (a development checkout, after scripts/quest/fetch-steam-audio.sh)
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        places.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/steam-audio/lib/linux-x64/libphonon.so"));
        let mut errors = Vec::new();
        let library = places
            .iter()
            // SAFETY: Steam Audio's library runs no code of note on load.
            .find_map(|place| unsafe { libloading::Library::new(place) }.map_err(|e| errors.push(format!("{}: {e}", place.display()))).ok())
            .ok_or_else(|| errors.join("; "))?;
        // SAFETY: the symbols are the functions phonon.h declares, with its signatures.
        unsafe {
            let symbol = |name: &[u8]| library.get::<*const c_void>(name).map(|s| *s).map_err(|e| e.to_string());
            let context_create: unsafe extern "C" fn(*mut ContextSettings, *mut Handle) -> i32 =
                std::mem::transmute(symbol(b"iplContextCreate\0")?);
            let mut settings = ContextSettings { version: VERSION, log: None, allocate: None, free: None, simd_level: SIMD, flags: 0 };
            let mut context: Handle = std::ptr::null_mut();
            let status = context_create(&mut settings, &mut context);
            if status != 0 || context.is_null() {
                return Err(format!("iplContextCreate failed ({status})"));
            }
            Ok(Self {
                context,
                effect_create: std::mem::transmute(symbol(b"iplBinauralEffectCreate\0")?),
                effect_release: std::mem::transmute(symbol(b"iplBinauralEffectRelease\0")?),
                effect_apply: std::mem::transmute(symbol(b"iplBinauralEffectApply\0")?),
                hrtf_create: std::mem::transmute(symbol(b"iplHRTFCreate\0")?),
                hrtfs: Default::default(),
                calls: Default::default(),
                library,
            })
        }
    }

    pub(crate) fn hrtf(&self, rate: u32) -> Option<Handle> {
        let mut hrtfs = self.hrtfs.lock().ok()?;
        if let Some((_, hrtf)) = hrtfs.iter().find(|(r, _)| *r == rate) {
            return Some(*hrtf as Handle);
        }
        let mut audio = AudioSettings { sampling_rate: rate as i32, frame_size: FRAME as i32 };
        let mut settings =
            HrtfSettings { kind: 0, sofa_file_name: std::ptr::null(), sofa_data: std::ptr::null(), sofa_data_size: 0, volume: 1.0, norm_type: 0 };
        let mut hrtf: Handle = std::ptr::null_mut();
        // SAFETY: the context is live and the settings are as phonon.h lays them out.
        let status = {
            let _call = self.call();
            unsafe { (self.hrtf_create)(self.context, &mut audio, &mut settings, &mut hrtf) }
        };
        if status != 0 || hrtf.is_null() {
            warn!("Steam Audio: no HRTF at {rate} Hz ({status})");
            return None;
        }
        hrtfs.push((rate, hrtf as usize));
        Some(hrtf)
    }
}

pub(crate) fn phonon() -> Option<&'static Phonon> {
    static PHONON: OnceLock<Option<Phonon>> = OnceLock::new();
    PHONON
        .get_or_init(|| match Phonon::load() {
            Ok(phonon) => {
                info!("Steam Audio loaded: sounds are heard through its HRTF");
                Some(phonon)
            }
            Err(why) => {
                info!("Steam Audio not loaded, sounds are panned instead ({why})");
                None
            }
        })
        .as_ref()
}

/// Whether sounds can be put through the HRTF here.
pub(crate) fn available() -> bool {
    phonon().is_some()
}

// ---------------------------------------------------------------- what the game sets

fn load(cell: &AtomicU32) -> f32 {
    f32::from_bits(cell.load(Ordering::Relaxed))
}

/// Where the sounds come from, as the head has it: shared by every voice (it
/// is all the skater), written by the game each frame.
pub(crate) struct Emitter {
    /// Toward the source in the listener's own frame: +x right, +y up, -z ahead.
    direction: [AtomicU32; 3],
    /// Its level for its distance.
    gain: AtomicU32,
}

impl Default for Emitter {
    fn default() -> Self {
        Self { direction: [0.0f32, 0.0, -1.0].map(|v| AtomicU32::new(v.to_bits())), gain: AtomicU32::new(1.0f32.to_bits()) }
    }
}

impl Emitter {
    pub(crate) fn set(&self, direction: Vec3, gain: f32) {
        let direction = direction.try_normalize().unwrap_or(Vec3::NEG_Z);
        for (cell, v) in self.direction.iter().zip(direction.to_array()) {
            cell.store(v.to_bits(), Ordering::Relaxed);
        }
        self.gain.store(gain.clamp(0.0, 4.0).to_bits(), Ordering::Relaxed);
    }
}

/// One sound's place in the room and its pitch.
pub(crate) struct Voice {
    pub emitter: Arc<Emitter>,
    /// Playback rate (1: as recorded). The HRTF comes after it, so a pitched
    /// loop keeps its direction.
    pitch: AtomicU32,
    /// How loud the game is playing it (what its sink's volume is set to), which the sound
    /// itself doesn't otherwise know: the level it is sent to the room's reverb at.
    level: AtomicU32,
}

impl Voice {
    pub(crate) fn new(emitter: Arc<Emitter>) -> Arc<Self> {
        Arc::new(Self { emitter, pitch: AtomicU32::new(1.0f32.to_bits()), level: AtomicU32::new(0.0f32.to_bits()) })
    }

    pub(crate) fn set_level(&self, level: f32) {
        self.level.store(level.clamp(0.0, 8.0).to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn set_pitch(&self, pitch: f32) {
        self.pitch.store(pitch.clamp(0.25, 4.0).to_bits(), Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- the source

/// Mono 16-bit PCM as a Bevy audio source: as it is, or (with a voice, and
/// Steam Audio loaded) in stereo through the HRTF. A live one (`clean_mix`)
/// goes through its kind's level and EQ first, and a live loop takes up new
/// samples as it plays.
#[derive(Asset, TypePath, Clone)]
pub(crate) struct CleanPcm {
    buffer: Arc<Buffer>,
    strip: Option<Arc<Strip>>,
    rate: u32,
    looping: bool,
    voice: Option<Arc<Voice>>,
}

impl CleanPcm {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn new(samples: Arc<Vec<i16>>, rate: u32, looping: bool, voice: Option<Arc<Voice>>) -> Self {
        Self { buffer: Buffer::new(samples), strip: None, rate, looping, voice }
    }

    pub(crate) fn live(buffer: Arc<Buffer>, strip: Arc<Strip>, rate: u32, looping: bool, voice: Option<Arc<Voice>>) -> Self {
        Self { buffer, strip: Some(strip), rate, looping, voice }
    }
}

struct Effect {
    phonon: &'static Phonon,
    effect: Handle,
    hrtf: Handle,
}

// SAFETY: an effect is used by the one playback that owns it, on whichever thread mixes it.
unsafe impl Send for Effect {}
unsafe impl Sync for Effect {}

impl Drop for Effect {
    fn drop(&mut self) {
        // SAFETY: made by iplBinauralEffectCreate and released once.
        let _call = self.phonon.call();
        unsafe { (self.phonon.effect_release)(&mut self.effect) };
    }
}

pub(crate) struct CleanPlayback {
    samples: Arc<Vec<i16>>,
    /// Where a loop's samples come from, and which of them it has.
    buffer: Arc<Buffer>,
    seen: u32,
    filter: Option<Filter>,
    rate: u32,
    looping: bool,
    /// Where in `samples` the next one read is.
    at: f64,
    spatial: Option<(Effect, Arc<Voice>)>,
    /// The emitter's level as the last frame ended (levels ramp over a frame).
    gain: f32,
    /// Rendered sound (stereo through the HRTF, else mono), waiting to be handed out.
    ready: VecDeque<f32>,
    /// Frames of silence still to go through the effect after the sound ends (the HRTF's tail).
    tail: u32,
    /// The room it reverberates in, its place on that room's bus, and (a hit) the level it was
    /// started at.
    room: Option<Arc<crate::reverb::Room>>,
    sent: Option<u64>,
    started_at: f32,
}

impl CleanPlayback {
    fn sample(&self, index: usize) -> f32 {
        let n = self.samples.len();
        let s = if self.looping && n > 0 { self.samples[index % n] } else { self.samples.get(index).copied().unwrap_or(0) };
        f32::from(s) / 32768.0
    }

    fn ended(&self) -> bool {
        !self.looping && self.at >= self.samples.len() as f64
    }

    /// One frame into `ready`; false once the sound (and through the HRTF, its tail) is done.
    fn render(&mut self) -> bool {
        if self.looping
            && let Some(samples) = self.buffer.newer(&mut self.seen)
        {
            self.samples = samples;
        }
        let mut count = FRAME;
        if self.ended() {
            if self.spatial.is_none() || self.tail == 0 {
                return false;
            }
            self.tail -= 1;
        } else if self.spatial.is_none() && !self.looping {
            count = FRAME.min(self.samples.len() - self.at as usize);
        }
        let (pitch, target) = match &self.spatial {
            Some((_, voice)) => (f64::from(load(&voice.pitch)), load(&voice.emitter.gain)),
            // As it is: at the sink's own speed and panning.
            None => (1.0, 1.0),
        };
        let mut mono = [0.0f32; FRAME];
        let mut at = self.at;
        for out in mono[..count].iter_mut() {
            let (whole, part) = (at.floor(), at.fract() as f32);
            let (a, b) = (self.sample(whole as usize), self.sample(whole as usize + 1));
            *out = a + (b - a) * part;
            at += pitch;
        }
        if self.looping && !self.samples.is_empty() {
            at %= self.samples.len() as f64;
        }
        if let Some(filter) = &mut self.filter {
            filter.run(&mut mono[..count]);
        }
        // To the room, as loud as the game plays it and before distance takes its part (the
        // room's response has the way from the skater to the head in it).
        if let (Some(room), Some((_, voice))) = (&self.room, &self.spatial) {
            let level = if self.looping { load(&voice.level) } else { self.started_at };
            if room.sounding() {
                room.send(&mut self.sent, &mono, level);
            }
        }
        for (i, out) in mono[..count].iter_mut().enumerate() {
            *out *= self.gain + (target - self.gain) * (i as f32 / FRAME as f32);
        }
        (self.at, self.gain) = (at, target);
        let Some((effect, voice)) = &self.spatial else {
            self.ready.extend(&mono[..count]);
            return true;
        };
        let (mut left, mut right) = ([0.0f32; FRAME], [0.0f32; FRAME]);
        let mut input = [mono.as_mut_ptr()];
        let mut output = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut from = AudioBuffer { channels: 1, samples: FRAME as i32, data: input.as_mut_ptr() };
        let mut to = AudioBuffer { channels: 2, samples: FRAME as i32, data: output.as_mut_ptr() };
        let mut params = BinauralEffectParams {
            direction: voice.emitter.direction.each_ref().map(load),
            interpolation: BILINEAR,
            spatial_blend: 1.0,
            hrtf: effect.hrtf,
            peak_delays: std::ptr::null_mut(),
        };
        // SAFETY: the effect and HRTF are live, and both buffers are FRAME samples a channel.
        {
            let _call = effect.phonon.call();
            unsafe { (effect.phonon.effect_apply)(effect.effect, &mut params, &mut from, &mut to) };
        }
        self.ready.extend(left.iter().zip(&right).flat_map(|(l, r)| [*l, *r]));
        true
    }
}

impl Iterator for CleanPlayback {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.ready.is_empty() && !self.render() {
            return None;
        }
        self.ready.pop_front()
    }
}

impl Source for CleanPlayback {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> u16 {
        if self.spatial.is_some() { 2 } else { 1 }
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        (!self.looping && self.rate != 0).then(|| Duration::from_secs_f64(self.samples.len() as f64 / f64::from(self.rate)))
    }
}

impl Decodable for CleanPcm {
    type DecoderItem = f32;
    type Decoder = CleanPlayback;

    fn decoder(&self) -> CleanPlayback {
        let spatial = self.voice.as_ref().and_then(|voice| {
            let phonon = phonon()?;
            let hrtf = phonon.hrtf(self.rate)?;
            let mut audio = AudioSettings { sampling_rate: self.rate as i32, frame_size: FRAME as i32 };
            let mut settings = BinauralEffectSettings { hrtf };
            let mut effect: Handle = std::ptr::null_mut();
            // SAFETY: the context and HRTF are live and the settings are as phonon.h lays them out.
            let status = {
                let _call = phonon.call();
                unsafe { (phonon.effect_create)(phonon.context, &mut audio, &mut settings, &mut effect) }
            };
            (status == 0 && !effect.is_null()).then(|| (Effect { phonon, effect, hrtf }, voice.clone()))
        });
        let mut seen = 0;
        let samples = self.buffer.newer(&mut seen).unwrap_or_else(|| self.buffer.get());
        CleanPlayback {
            samples,
            buffer: self.buffer.clone(),
            seen,
            filter: self.strip.clone().map(|strip| Filter::new(strip, self.rate)),
            rate: self.rate,
            looping: self.looping,
            at: 0.0,
            gain: spatial.as_ref().map_or(1.0, |(_, voice)| load(&voice.emitter.gain)),
            room: spatial.as_ref().and_then(|_| crate::reverb::room().cloned()),
            sent: None,
            started_at: spatial.as_ref().map_or(0.0, |(_, voice)| load(&voice.level)),
            spatial,
            ready: VecDeque::with_capacity(2 * FRAME),
            tail: 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(seconds: f32) -> Arc<Vec<i16>> {
        // (noise: every band the ears use)
        let mut state = 0x2545_F491u32;
        Arc::new(
            (0..(seconds * 48_000.0) as usize)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    ((state >> 16) as i16) / 4
                })
                .collect(),
        )
    }

    fn levels(direction: Vec3, gain: f32) -> (f32, f32, usize) {
        let emitter = Arc::new(Emitter::default());
        emitter.set(direction, gain);
        let playback = CleanPcm::new(tone(0.5), 48_000, false, Some(Voice::new(emitter))).decoder();
        assert_eq!(playback.channels(), 2);
        let samples: Vec<f32> = playback.collect();
        let power = |skip: usize| samples.iter().skip(skip).step_by(2).map(|s| s * s).sum::<f32>().sqrt();
        (power(0), power(1), samples.len() / 2)
    }

    #[test]
    fn without_a_voice_the_sound_is_as_it_was() {
        let samples = tone(0.1);
        let mut once = CleanPcm::new(samples.clone(), 48_000, false, None).decoder();
        assert_eq!((once.channels(), once.sample_rate()), (1, 48_000));
        let out: Vec<f32> = once.by_ref().collect();
        assert_eq!(out.len(), samples.len());
        assert_eq!(out[100], f32::from(samples[100]) / 32768.0);
        let looped: Vec<f32> = CleanPcm::new(samples.clone(), 48_000, true, None).decoder().take(samples.len() + 5).collect();
        assert_eq!(looped[samples.len() + 3], looped[3], "a loop comes round");
    }

    /// Needs libphonon.so (scripts/quest/fetch-steam-audio.sh): skipped without it.
    #[test]
    fn a_sound_to_one_side_is_louder_in_that_ear() {
        if !available() {
            eprintln!("Steam Audio not found: skipped");
            return;
        }
        let (left, right, frames) = levels(Vec3::X, 1.0);
        assert!(right > 1.6 * left, "to the right: left {left}, right {right}");
        assert!(frames >= 24_000, "the whole sound and its tail: {frames}");
        let (left, right, _) = levels(Vec3::NEG_X, 1.0);
        assert!(left > 1.6 * right, "to the left: left {left}, right {right}");
        let (left, right, _) = levels(Vec3::NEG_Z, 1.0);
        assert!((left / right - 1.0).abs() < 0.25, "straight ahead: left {left}, right {right}");
        let (near, _, _) = levels(Vec3::NEG_Z, 1.0);
        let (far, _, _) = levels(Vec3::NEG_Z, 0.25);
        assert!((far / near - 0.25).abs() < 0.05, "a quarter of the level: {far} of {near}");
    }

    #[test]
    fn a_pitched_voice_plays_faster_and_keeps_its_direction() {
        if !available() {
            eprintln!("Steam Audio not found: skipped");
            return;
        }
        let emitter = Arc::new(Emitter::default());
        emitter.set(Vec3::X, 1.0);
        let voice = Voice::new(emitter);
        voice.set_pitch(2.0);
        let samples: Vec<f32> = CleanPcm::new(tone(0.5), 48_000, false, Some(voice)).decoder().collect();
        let frames = samples.len() / 2;
        assert!((12_000..14_500).contains(&frames), "half as long at twice the pitch: {frames}");
        let power = |skip: usize| samples.iter().skip(skip).step_by(2).map(|s| s * s).sum::<f32>();
        assert!(power(1) > 2.0 * power(0));
    }
}
