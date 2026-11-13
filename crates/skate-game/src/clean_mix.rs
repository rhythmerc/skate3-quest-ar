//! A level and a four-band parametric EQ (a low shelf, two peaks, a high shelf) for each kind of
//! clean sound, and the tuning file they are kept in with the generator's values.
//!
//! A [`Strip`] is one kind's level and EQ, shared by everything of that kind that is playing;
//! each playback runs its own [`Filter`] over it. A [`Buffer`] is one sound's samples, which can
//! be swapped for others while it plays (a loop made again with new values). Both are set by the
//! game and read where sound is mixed.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) const BANDS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Band {
    /// Hz.
    pub freq: f32,
    /// dB; 0 leaves the sound as it is.
    pub gain: f32,
    pub q: f32,
}

pub(crate) const FLAT: [Band; BANDS] = [
    Band { freq: 120.0, gain: 0.0, q: 0.71 },
    Band { freq: 500.0, gain: 0.0, q: 1.0 },
    Band { freq: 2500.0, gain: 0.0, q: 1.0 },
    Band { freq: 7000.0, gain: 0.0, q: 0.71 },
];

pub(crate) const BAND_NAMES: [&str; BANDS] = ["Low shelf", "Peak 1", "Peak 2", "High shelf"];

fn flat() -> [Band; BANDS] {
    FLAT
}

/// One kind of sound as tuned.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tune {
    /// Made by `clean_synth` (else the recording).
    #[serde(default)]
    pub synth: bool,
    #[serde(default)]
    pub level_db: f32,
    #[serde(default = "flat")]
    pub eq: [Band; BANDS],
    /// The generator's values that are not its defaults, by key.
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
}

impl Default for Tune {
    fn default() -> Self {
        Self { synth: false, level_db: 0.0, eq: FLAT, params: BTreeMap::new() }
    }
}

/// `sound-tuning.json`: every kind that is not as it comes, by name.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tuning {
    #[serde(default)]
    pub sounds: BTreeMap<String, Tune>,
    /// The room's reverberation (`reverb`): its level against the sounds' own (1: as simulated).
    #[serde(default = "one")]
    pub reverb: f32,
    /// How absorbent the room's surfaces are taken to be, against a generic room's (more: deader).
    #[serde(default = "one")]
    pub damping: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for Tuning {
    fn default() -> Self {
        Self { sounds: BTreeMap::new(), reverb: 1.0, damping: 1.0 }
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

/// RBJ's cookbook: the first band a low shelf, the last a high shelf, peaks between.
fn biquad(index: usize, band: &Band, rate: f32) -> Biquad {
    let f = band.freq.clamp(10.0, rate * 0.45);
    let q = band.q.max(0.1);
    let a = 10f32.powf(band.gain / 40.0);
    let w0 = std::f32::consts::TAU * f / rate;
    let (cw, alpha) = (w0.cos(), w0.sin() / (2.0 * q));
    let (b0, b1, b2, a0, a1, a2);
    if index != 0 && index != BANDS - 1 {
        b0 = 1.0 + alpha * a;
        b1 = -2.0 * cw;
        b2 = 1.0 - alpha * a;
        a0 = 1.0 + alpha / a;
        a1 = -2.0 * cw;
        a2 = 1.0 - alpha / a;
    } else {
        let sa = 2.0 * a.sqrt() * alpha;
        let s = if index == 0 { 1.0 } else { -1.0 }; // the high shelf: the low's, mirrored
        b0 = a * ((a + 1.0) - s * (a - 1.0) * cw + sa);
        b1 = s * 2.0 * a * ((a - 1.0) - s * (a + 1.0) * cw);
        b2 = a * ((a + 1.0) - s * (a - 1.0) * cw - sa);
        a0 = (a + 1.0) + s * (a - 1.0) * cw + sa;
        a1 = -s * 2.0 * ((a - 1.0) + s * (a + 1.0) * cw);
        a2 = (a + 1.0) + s * (a - 1.0) * cw - sa;
    }
    Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
}

/// One kind's level (linear; 0 while another is soloed) and EQ.
pub(crate) struct Strip {
    set: Mutex<(f32, [Band; BANDS])>,
    version: AtomicU32,
}

impl Default for Strip {
    fn default() -> Self {
        Self { set: Mutex::new((1.0, FLAT)), version: AtomicU32::new(0) }
    }
}

impl Strip {
    pub(crate) fn set(&self, level: f32, bands: [Band; BANDS]) {
        let mut set = self.set.lock().unwrap_or_else(|p| p.into_inner());
        if *set != (level, bands) {
            *set = (level, bands);
            self.version.fetch_add(1, Ordering::Release);
        }
    }
}

/// One sound's samples, as they are now.
pub(crate) struct Buffer {
    samples: Mutex<Arc<Vec<i16>>>,
    version: AtomicU32,
}

impl Buffer {
    pub(crate) fn new(samples: Arc<Vec<i16>>) -> Arc<Self> {
        Arc::new(Self { samples: Mutex::new(samples), version: AtomicU32::new(0) })
    }

    pub(crate) fn set(&self, samples: Arc<Vec<i16>>) {
        *self.samples.lock().unwrap_or_else(|p| p.into_inner()) = samples;
        self.version.fetch_add(1, Ordering::Release);
    }

    pub(crate) fn get(&self) -> Arc<Vec<i16>> {
        self.samples.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// The samples if they are not the ones `seen` was last given for.
    pub(crate) fn newer(&self, seen: &mut u32) -> Option<Arc<Vec<i16>>> {
        let version = self.version.load(Ordering::Acquire);
        (version != *seen).then(|| {
            *seen = version;
            self.get()
        })
    }
}

/// One playback's pass through its kind's strip.
pub(crate) struct Filter {
    strip: Arc<Strip>,
    seen: Option<u32>,
    rate: f32,
    stages: [Biquad; BANDS],
    flat: [bool; BANDS],
    z: [[f32; 2]; BANDS],
    /// The level as the last block ended (it ramps over a block).
    level: f32,
    target: f32,
}

impl Filter {
    pub(crate) fn new(strip: Arc<Strip>, rate: u32) -> Self {
        let pass = Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };
        Self { strip, seen: None, rate: rate as f32, stages: [pass; BANDS], flat: [true; BANDS], z: [[0.0; 2]; BANDS], level: 1.0, target: 1.0 }
    }

    /// A block of this playback's sound through the EQ and level, in place.
    pub(crate) fn run(&mut self, block: &mut [f32]) {
        let version = self.strip.version.load(Ordering::Acquire);
        if self.seen != Some(version) {
            let (level, bands) = *self.strip.set.lock().unwrap_or_else(|p| p.into_inner());
            for (i, band) in bands.iter().enumerate() {
                self.stages[i] = biquad(i, band, self.rate);
                self.flat[i] = band.gain == 0.0;
            }
            if self.seen.is_none() {
                self.level = level;
            }
            self.target = level;
            self.seen = Some(version);
        }
        for (i, stage) in self.stages.iter().enumerate() {
            let z = &mut self.z[i];
            if self.flat[i] {
                *z = [0.0; 2]; // a flat band passes as is, and starts clean when it's turned
                continue;
            }
            for sample in block.iter_mut() {
                // transposed direct form II
                let x = *sample;
                let y = stage.b0 * x + z[0];
                z[0] = stage.b1 * x - stage.a1 * y + z[1];
                z[1] = stage.b2 * x - stage.a2 * y;
                *sample = y;
            }
            for state in z.iter_mut() {
                if state.abs() < 1e-20 {
                    *state = 0.0; // no denormals in a fade's tail
                }
            }
        }
        let (from, to) = (self.level, self.target);
        if from != 1.0 || to != 1.0 {
            let n = block.len().max(1) as f32;
            for (i, sample) in block.iter_mut().enumerate() {
                *sample *= from + (to - from) * (i as f32 / n);
            }
        }
        self.level = to;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn level_at(freq: f32, strip: &Arc<Strip>) -> f32 {
        let mut filter = Filter::new(strip.clone(), 48_000);
        let mut power = 0.0f64;
        for block in 0..40 {
            let mut x: Vec<f32> = (0..512).map(|i| (std::f32::consts::TAU * freq * (block * 512 + i) as f32 / 48_000.0).sin()).collect();
            filter.run(&mut x);
            if block >= 20 {
                power += x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
        }
        // (against a sine's own power, a half)
        (10.0 * (power / (20.0 * 512.0) / 0.5).max(1e-30).log10()) as f32
    }

    #[test]
    fn a_flat_strip_changes_nothing() {
        let mut filter = Filter::new(Arc::new(Strip::default()), 48_000);
        let original: Vec<f32> = (0..512).map(|i| (i as f32 * 0.37).sin()).collect();
        let mut x = original.clone();
        filter.run(&mut x);
        assert_eq!(x, original);
    }

    #[test]
    fn each_band_does_its_own_part() {
        let strip = Arc::new(Strip::default());
        let mut bands = FLAT;
        bands[1] = Band { freq: 1000.0, gain: 9.0, q: 2.0 };
        strip.set(1.0, bands);
        assert!((level_at(1000.0, &strip) - 9.0).abs() < 0.3, "{}", level_at(1000.0, &strip));
        assert!(level_at(100.0, &strip).abs() < 0.5 && level_at(8000.0, &strip).abs() < 0.5);
        let mut bands = FLAT;
        bands[0] = Band { freq: 200.0, gain: -12.0, q: 0.71 };
        bands[3] = Band { freq: 5000.0, gain: 6.0, q: 0.71 };
        strip.set(1.0, bands);
        assert!((level_at(40.0, &strip) + 12.0).abs() < 0.6, "{}", level_at(40.0, &strip));
        assert!(level_at(1000.0, &strip).abs() < 1.0, "{}", level_at(1000.0, &strip));
        assert!((level_at(15000.0, &strip) - 6.0).abs() < 0.6, "{}", level_at(15000.0, &strip));
        // The level, and a soloed neighbour's silence.
        strip.set(0.5, FLAT);
        assert!((level_at(1000.0, &strip) + 6.02).abs() < 0.1);
        strip.set(0.0, FLAT);
        assert!(level_at(1000.0, &strip) < -100.0);
    }

    #[test]
    fn a_tuning_file_goes_round_and_a_partial_one_loads() {
        let tuning: Tuning = serde_json::from_str(r#"{"sounds":{"pop":{"synth":true,"params":{"click":0.5}},"chime":{"level_db":-3}}}"#).unwrap();
        assert!(tuning.sounds["pop"].synth && tuning.sounds["pop"].eq == FLAT);
        assert_eq!(tuning.sounds["chime"].level_db, -3.0);
        let again: Tuning = serde_json::from_str(&serde_json::to_string(&tuning).unwrap()).unwrap();
        assert_eq!(again, tuning);
        assert_eq!(serde_json::from_str::<Tuning>("{}").unwrap(), Tuning::default());
    }
}
