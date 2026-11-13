//! The clean tier's sounds made from nothing: `scripts/cleanroom/synth_sounds.py`'s generators,
//! here so that what they are made from can be changed while the game runs (the sound lab,
//! `xr::ui::sound_lab`).
//!
//! Each kind of sound is modelled on what makes it (urethane wheels on a surface, a wooden deck
//! struck on the ground, a truck dragged along a rail) from noise, decaying sines and filters;
//! nothing is read from a recording or a game. Every number worth turning is a [`Param`]; a kind
//! is rendered from one value for each of its params, and its variants (pop 1 to 4) differ only
//! by their seed. Loops are built in the frequency domain, so they repeat without a seam.
use std::f32::consts::{PI, TAU};

pub(crate) const RATE: u32 = 48_000;
const RATE_F: f32 = RATE as f32;
/// A loop's length in samples (2.7 s): a power of two, for the transform.
const LOOP_N: usize = 1 << 17;
/// Loops are mastered quiet so they sit under the hits (the driver restores them).
const LOOP_RMS_DB: f32 = -24.0;
const HIT_PEAK_DB: f32 = -4.0;

/// One number a sound is made from.
pub(crate) struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub min: f32,
    pub max: f32,
    /// The slider's step; 0 for a logarithmic one (frequencies).
    pub step: f32,
    pub unit: &'static str,
}

const fn lin(key: &'static str, label: &'static str, min: f32, max: f32, step: f32, unit: &'static str) -> Param {
    Param { key, label, min, max, step, unit }
}

const fn hz(key: &'static str, label: &'static str, min: f32, max: f32) -> Param {
    Param { key, label, min, max, step: 0.0, unit: "Hz" }
}

/// One kind of sound: a loop, or a hit and its variants.
pub(crate) struct Kind {
    /// Its file name (a hit's variants are `name_1`, `name_2`...).
    pub name: &'static str,
    pub looping: bool,
    pub variants: usize,
    pub params: &'static [Param],
    pub defaults: &'static [f32],
    /// A hit's level against the loudest hits (a pop, a landing), dB.
    level_db: f32,
    make: fn(&Values, usize, &mut Rng) -> Vec<f32>,
}

impl Kind {
    /// The name of variant `variant` (from 0): `pop_1`, or the kind's own where there is one.
    pub(crate) fn file(&self, variant: usize) -> String {
        if self.looping || self.name == "chime" { self.name.to_string() } else { format!("{}_{}", self.name, variant + 1) }
    }

    /// Variant `variant` made from `values` (one for each of [`Kind::params`], in order), mastered.
    pub(crate) fn render(&self, values: &[f32], variant: usize) -> Vec<i16> {
        let file = self.file(variant);
        let mut x = (self.make)(&Values { params: self.params, values }, variant, &mut Rng::seeded(&file));
        let mean = x.iter().sum::<f32>() / x.len().max(1) as f32;
        x.iter_mut().for_each(|s| *s -= mean);
        if self.looping {
            let rms = rms(&x);
            let knee = db(-7.0);
            if rms > 0.0 {
                // (the few peaks that stick out are rounded, rather than clipped)
                x.iter_mut().for_each(|s| *s = (*s * db(LOOP_RMS_DB) / rms / knee).tanh() * knee);
            }
        } else {
            // (a hit's modes start wherever in their swing they are: brought in over a third of a millisecond, not as a step)
            for (i, s) in x.iter_mut().take(16).enumerate() {
                *s *= i as f32 / 16.0;
            }
            let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            if peak > 0.0 {
                let gain = db(HIT_PEAK_DB + self.level_db) / peak;
                x.iter_mut().for_each(|s| *s *= gain);
            }
        }
        x.iter().map(|s| if s.is_finite() { (s * 32767.0).round().clamp(-32767.0, 32767.0) as i16 } else { 0 }).collect()
    }
}

struct Values<'a> {
    params: &'a [Param],
    values: &'a [f32],
}

impl Values<'_> {
    fn v(&self, key: &str) -> f32 {
        let i = self.params.iter().position(|p| p.key == key).unwrap_or_else(|| panic!("no sound param {key}"));
        self.values[i]
    }

    fn peaks(&self) -> Vec<(f32, f32, f32)> {
        ["peak1", "peak2", "peak3"]
            .iter()
            .map(|p| (self.v(&format!("{p}_hz")), self.v(&format!("{p}_q")), self.v(&format!("{p}_gain"))))
            .filter(|&(_, _, gain)| (gain - 1.0).abs() > 1e-3)
            .collect()
    }
}

fn db(x: f32) -> f32 {
    10f32.powf(x / 20.0)
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn normalise(x: &mut [f32]) {
    let level = rms(x) + 1e-12;
    x.iter_mut().for_each(|s| *s /= level);
}

// ---------------------------------------------------------------- random numbers

/// splitmix64: seeded by a sound's name, so each is the same every time it is made.
struct Rng(u64);

impl Rng {
    fn seeded(name: &str) -> Self {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for c in name.bytes() {
            h = (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3);
        }
        Self(h)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn uniform(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    fn normal(&mut self) -> f32 {
        let (a, b) = (self.unit().max(1e-7), self.unit());
        (-2.0 * a.ln()).sqrt() * (TAU * b).cos()
    }
}

// ---------------------------------------------------------------- building blocks

/// The discrete Fourier transform of `re + i im`, in place (its length a power of two); unscaled
/// both ways, as everything made with it is brought to a level afterwards.
fn fft(re: &mut [f32], im: &mut [f32], inverse: bool) {
    let n = re.len();
    debug_assert!(n.is_power_of_two() && im.len() == n);
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let sign = if inverse { 1.0 } else { -1.0 };
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        // This stage's turns, worked out once.
        let turns: Vec<(f32, f32)> = (0..half)
            .map(|k| {
                let (s, c) = (sign * std::f64::consts::TAU * k as f64 / len as f64).sin_cos();
                (c as f32, s as f32)
            })
            .collect();
        for start in (0..n).step_by(len) {
            for (k, &(c, s)) in turns.iter().enumerate() {
                let (a, b) = (start + k, start + k + half);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                (re[b], im[b]) = (re[a] - tr, im[a] - ti);
                (re[a], im[a]) = (re[a] + tr, im[a] + ti);
            }
        }
        len <<= 1;
    }
}

/// Noise with the spectrum `shape(frequency)`, at a level of 1; periodic over its length `n` (a
/// power of two).
fn shaped_noise(rng: &mut Rng, n: usize, shape: &dyn Fn(f32) -> f32) -> Vec<f32> {
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    for k in 1..n / 2 {
        let level = shape(k as f32 * RATE_F / n as f32);
        if level <= 0.0 {
            continue;
        }
        let (a, b) = (rng.normal() * level, rng.normal() * level);
        (re[k], im[k]) = (a, b);
        (re[n - k], im[n - k]) = (a, -b);
    }
    fft(&mut re, &mut im, true);
    normalise(&mut re);
    re
}

/// A band between `lo` and `hi` Hz with gentle skirts (`slope` orders).
fn bandpass(lo: f32, hi: f32, slope: f32) -> impl Fn(f32) -> f32 {
    move |f| 1.0 / ((1.0 + (lo / f.max(1e-3)).powf(2.0 * slope)) * (1.0 + (f / hi.max(1.0)).powf(2.0 * slope))).sqrt()
}

/// Broad resonances: `gain` times more at `centre`, `q` wide.
fn bumps(peaks: &[(f32, f32, f32)], f: f32) -> f32 {
    peaks.iter().map(|&(centre, q, gain)| 1.0 + (gain - 1.0) / (1.0 + (q * (f / centre - centre / f.max(1e-3))).powi(2))).product()
}

/// A positive, periodic gain about 1 that wanders at up to `rate_hz`.
fn wobble(rng: &mut Rng, n: usize, rate_hz: f32, depth: f32, power: f32) -> Vec<f32> {
    let mut slow = shaped_noise(rng, n, &|f| if f < rate_hz && f > 0.3 { 1.0 } else { 0.0 });
    if power != 1.0 {
        slow.iter_mut().for_each(|s| *s = s.abs().powf(power));
    }
    let mean = slow.iter().sum::<f32>() / n as f32;
    let deviation = (slow.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n as f32).sqrt() + 1e-12;
    slow.iter_mut().for_each(|s| *s = (1.0 + depth * (*s - mean) / deviation).max(0.0));
    slow
}

/// Sparse clicks (grains under a wheel, a rail's pits), band-limited; periodic.
fn grit(rng: &mut Rng, n: usize, per_second: f32, lo: f32, hi: f32) -> Vec<f32> {
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    let count = (per_second * n as f32 / RATE_F) as usize;
    if count == 0 {
        return re;
    }
    for _ in 0..count {
        // (all about as loud: one tick that stands out is heard every time the loop comes round)
        let at = (rng.unit() * n as f32) as usize % n;
        re[at] = rng.uniform(0.5, 1.0) * if rng.unit() < 0.5 { -1.0 } else { 1.0 };
    }
    fft(&mut re, &mut im, false);
    let band = bandpass(lo, hi, 2.0);
    for k in 0..n {
        let level = band(k.min(n - k) as f32 * RATE_F / n as f32);
        re[k] *= level;
        im[k] *= level;
    }
    fft(&mut re, &mut im, true);
    normalise(&mut re);
    re
}

/// `len` samples of noise in a band, at a level of 1.
fn band_noise(rng: &mut Rng, len: usize, lo: f32, hi: f32) -> Vec<f32> {
    let mut x = shaped_noise(rng, len.next_power_of_two().max(256), &bandpass(lo, hi, 2.0));
    x.truncate(len);
    normalise(&mut x);
    x
}

/// A struck body's one mode: a decaying sine from `start` seconds.
fn mode(out: &mut [f32], freq: f32, decay: f32, level: f32, phase: f32, start: f32) {
    let first = (start * RATE_F).max(0.0) as usize;
    for (i, s) in out.iter_mut().enumerate().skip(first) {
        let t = i as f32 / RATE_F - start;
        *s += level * (TAU * freq * t + phase).sin() * (-t / decay.max(1e-4)).exp();
    }
}

/// A noise burst (a slap, a scuff) in a band, starting at `start` seconds.
#[allow(clippy::too_many_arguments)]
fn burst(out: &mut [f32], rng: &mut Rng, lo: f32, hi: f32, decay: f32, start: f32, attack: f32, level: f32) {
    let first = (start * RATE_F).max(0.0) as usize;
    if first >= out.len() {
        return;
    }
    let decay = decay.max(1e-4);
    // (only as much noise as is heard before it has died away)
    let live = (out.len() - first).min(((attack + 14.0 * decay) * RATE_F) as usize + 1);
    let noise = band_noise(rng, live, lo, hi);
    for (i, sample) in noise.iter().enumerate() {
        let t = i as f32 / RATE_F;
        out[first + i] += sample * (t / attack).min(1.0) * (-t / decay).exp() * level;
    }
}

const ATTACK: f32 = 0.0005;

/// `table` (frequency, decay, level) as modes, each a little off so no two hits match.
#[allow(clippy::too_many_arguments)]
fn struck(out: &mut [f32], rng: &mut Rng, table: &[(f32, f32, f32)], start: f32, spread: f32, level: f32, pitch: f32, ring: f32) {
    for &(freq, decay, amount) in table {
        let freq = freq * pitch * (1.0 + rng.uniform(-spread, spread));
        let decay = decay * ring * rng.uniform(0.8, 1.25);
        let amount = amount * level * rng.uniform(0.7, 1.3);
        mode(out, freq, decay, amount, rng.uniform(0.0, TAU), start);
    }
}

fn fade_out(x: &mut [f32], tail: f32) {
    let k = ((tail * RATE_F) as usize).min(x.len());
    let from = x.len() - k;
    for (i, s) in x[from..].iter_mut().enumerate() {
        *s *= 1.0 - i as f32 / (k.max(2) - 1) as f32;
    }
}

fn samples(seconds: f32) -> usize {
    (seconds * RATE_F) as usize
}

/// A wooden deck and its trucks: what rings when the board is knocked.
const DECK: [(f32, f32, f32); 6] =
    [(410.0, 0.045, 1.0), (760.0, 0.034, 0.8), (1180.0, 0.024, 0.6), (1750.0, 0.017, 0.45), (2600.0, 0.011, 0.3), (3900.0, 0.007, 0.2)];

// ---------------------------------------------------------------- loops

const PEAKS: [Param; 9] = [
    hz("peak1_hz", "Resonance 1", 80.0, 8000.0),
    lin("peak1_q", "Resonance 1 sharpness", 0.5, 10.0, 0.1, ""),
    lin("peak1_gain", "Resonance 1 amount (1: none)", 0.25, 4.0, 0.05, "x"),
    hz("peak2_hz", "Resonance 2", 80.0, 8000.0),
    lin("peak2_q", "Resonance 2 sharpness", 0.5, 10.0, 0.1, ""),
    lin("peak2_gain", "Resonance 2 amount (1: none)", 0.25, 4.0, 0.05, "x"),
    hz("peak3_hz", "Resonance 3", 80.0, 8000.0),
    lin("peak3_q", "Resonance 3 sharpness", 0.5, 10.0, 0.1, ""),
    lin("peak3_gain", "Resonance 3 amount (1: none)", 0.25, 4.0, 0.05, "x"),
];

macro_rules! with_peaks {
    ($($param:expr),* $(,)?) => {{
        const OWN: &[Param] = &[$($param),*];
        const ALL: [Param; OWN.len() + 9] = {
            let mut all = [const { lin("", "", 0.0, 0.0, 0.0, "") }; OWN.len() + 9];
            let mut i = 0;
            while i < OWN.len() {
                all[i] = lin(OWN[i].key, OWN[i].label, OWN[i].min, OWN[i].max, OWN[i].step, OWN[i].unit);
                i += 1;
            }
            let mut j = 0;
            while j < 9 {
                all[i + j] = lin(PEAKS[j].key, PEAKS[j].label, PEAKS[j].min, PEAKS[j].max, PEAKS[j].step, PEAKS[j].unit);
                j += 1;
            }
            all
        };
        &ALL
    }};
}

static ROLL: &[Param] = with_peaks![
    hz("low", "Surface band: from", 40.0, 1000.0),
    hz("high", "Surface band: to", 300.0, 8000.0),
    lin("rumble", "Rumble under it (45-160 Hz)", 0.0, 2.0, 0.05, ""),
    lin("hiss", "Bearing hiss (2-4.5 kHz)", 0.0, 0.3, 0.005, ""),
    lin("grain_rate", "Grains a second", 0.0, 400.0, 5.0, ""),
    hz("grain_low", "Grain band: from", 200.0, 4000.0),
    hz("grain_high", "Grain band: to", 1000.0, 12000.0),
    lin("grain", "Grain level", 0.0, 1.0, 0.01, ""),
    hz("flutter_hz", "Flutter rate", 4.0, 120.0),
    lin("flutter", "Flutter depth", 0.0, 0.6, 0.01, ""),
    lin("sway", "Slow sway depth", 0.0, 0.4, 0.01, ""),
];

/// Wheels rolling: a band of noise from the surface, the bearings' hiss, the ground's grain.
fn roll(g: &Values, _variant: usize, rng: &mut Rng) -> Vec<f32> {
    let n = LOOP_N;
    let peaks = g.peaks();
    let band = bandpass(g.v("low"), g.v("high"), 2.0);
    let surface = shaped_noise(rng, n, &|f| band(f) * bumps(&peaks, f));
    let body = shaped_noise(rng, n, &bandpass(45.0, 160.0, 2.0));
    let hiss = shaped_noise(rng, n, &bandpass(2200.0, 4500.0, 2.0));
    let grain = grit(rng, n, g.v("grain_rate"), g.v("grain_low"), g.v("grain_high"));
    let sway = wobble(rng, n, 2.5, g.v("sway"), 1.0);
    let flutter = wobble(rng, n, g.v("flutter_hz"), g.v("flutter"), 1.0);
    let (rumble, hiss_level, grain_level) = (g.v("rumble"), g.v("hiss"), g.v("grain"));
    (0..n).map(|i| (surface[i] + body[i] * rumble + hiss[i] * hiss_level + grain[i] * grain_level) * sway[i] * flutter[i]).collect()
}

static SCRAPE: &[Param] = with_peaks![
    hz("low", "Scrape band: from", 100.0, 2000.0),
    hz("high", "Scrape band: to", 800.0, 10000.0),
    hz("chatter_hz", "Stick and slip rate", 10.0, 200.0),
    lin("chatter", "Stick and slip depth", 0.0, 1.0, 0.01, ""),
    lin("body", "Body under it (120-420 Hz)", 0.0, 2.0, 0.05, ""),
    lin("grain_rate", "Grains a second", 0.0, 400.0, 5.0, ""),
    hz("grain_low", "Grain band: from", 200.0, 4000.0),
    hz("grain_high", "Grain band: to", 1000.0, 12000.0),
    lin("grain", "Grain level", 0.0, 1.0, 0.01, ""),
    lin("sway", "Slow sway depth", 0.0, 0.4, 0.01, ""),
];

/// Something dragged: band noise broken up by the stick and slip of the two surfaces.
fn scrape(g: &Values, _variant: usize, rng: &mut Rng) -> Vec<f32> {
    let n = LOOP_N;
    let peaks = g.peaks();
    let band = bandpass(g.v("low"), g.v("high"), 3.0);
    let carrier = shaped_noise(rng, n, &|f| band(f) * bumps(&peaks, f));
    let rough = wobble(rng, n, g.v("chatter_hz"), g.v("chatter"), 1.5);
    let under = shaped_noise(rng, n, &bandpass(120.0, 420.0, 2.0));
    let grain = grit(rng, n, g.v("grain_rate"), g.v("grain_low"), g.v("grain_high"));
    let sway = wobble(rng, n, 2.0, g.v("sway"), 1.0);
    let (body, grain_level) = (g.v("body"), g.v("grain"));
    (0..n).map(|i| (carrier[i] * rough[i] + under[i] * body + grain[i] * grain_level) * sway[i]).collect()
}

// ---------------------------------------------------------------- hits

static POP: &[Param] = &[
    lin("deck", "Deck ring level", 0.0, 3.0, 0.05, ""),
    lin("deck_pitch", "Deck ring pitch", 0.5, 2.0, 0.01, "x"),
    lin("deck_ring", "Deck ring length", 0.3, 3.0, 0.05, "x"),
    lin("click", "Click level", 0.0, 4.0, 0.05, ""),
    hz("click_low", "Click band: from", 500.0, 6000.0),
    hz("click_high", "Click band: to", 2000.0, 16000.0),
    lin("click_ms", "Click length", 0.5, 20.0, 0.5, "ms"),
    hz("knock_hz", "Low knock", 50.0, 300.0),
    lin("knock", "Low knock level", 0.0, 3.0, 0.05, ""),
    lin("knock_ms", "Low knock length", 5.0, 150.0, 5.0, "ms"),
];

/// The tail struck on the ground: a click, the deck's own ring, a low knock.
fn pop(g: &Values, _variant: usize, rng: &mut Rng) -> Vec<f32> {
    let mut x = vec![0.0; samples(0.26)];
    struck(&mut x, rng, &DECK, 0.0, 0.06, g.v("deck"), g.v("deck_pitch"), g.v("deck_ring"));
    burst(&mut x, rng, g.v("click_low"), g.v("click_high"), g.v("click_ms") / 1000.0, 0.0, ATTACK, g.v("click"));
    mode(&mut x, g.v("knock_hz") * rng.uniform(0.87, 1.13), g.v("knock_ms") / 1000.0, g.v("knock"), 0.0, 0.0);
    fade_out(&mut x, 0.012);
    x
}

static LAND: &[Param] = &[
    hz("thump_hz", "Thump", 40.0, 250.0),
    lin("thump_ms", "Thump length", 10.0, 250.0, 5.0, "ms"),
    lin("thump", "Thump level", 0.0, 4.0, 0.05, ""),
    lin("deck", "Deck ring level", 0.0, 3.0, 0.05, ""),
    lin("deck_pitch", "Deck ring pitch", 0.5, 2.0, 0.01, "x"),
    lin("slap", "Slap level", 0.0, 4.0, 0.05, ""),
    hz("slap_low", "Slap band: from", 150.0, 3000.0),
    hz("slap_high", "Slap band: to", 800.0, 12000.0),
    lin("slap_ms", "Slap length", 2.0, 100.0, 1.0, "ms"),
    lin("gap_ms", "Front to back wheels", 0.0, 80.0, 1.0, "ms"),
    lin("roll", "Rolling away level", 0.0, 1.5, 0.05, ""),
];

/// Four wheels coming down, the back ones a moment after the front: thumps, the deck, a slap.
fn land(g: &Values, _variant: usize, rng: &mut Rng) -> Vec<f32> {
    let mut x = vec![0.0; samples(0.42)];
    let gap = g.v("gap_ms") / 1000.0 * rng.uniform(0.6, 1.4);
    let back = rng.uniform(0.7, 1.0);
    for (start, level) in [(0.0, 1.0), (gap, back)] {
        mode(&mut x, g.v("thump_hz") * rng.uniform(0.8, 1.2), g.v("thump_ms") / 1000.0, g.v("thump") * level, 0.0, start);
        struck(&mut x, rng, &[(300.0, 0.045, 0.9), (520.0, 0.035, 0.7), (900.0, 0.022, 0.4)], start, 0.06, g.v("deck") * level, g.v("deck_pitch"), 1.0);
        burst(&mut x, rng, g.v("slap_low"), g.v("slap_high"), g.v("slap_ms") / 1000.0, start, ATTACK, g.v("slap") * level);
    }
    // the wheels rolling away
    burst(&mut x, rng, 160.0, 900.0, 0.11, gap, 0.02, g.v("roll"));
    fade_out(&mut x, 0.012);
    x
}

static CATCH: &[Param] = &[
    lin("slap", "Slap level", 0.0, 3.0, 0.05, ""),
    hz("slap_low", "Slap band: from", 100.0, 2000.0),
    hz("slap_high", "Slap band: to", 600.0, 10000.0),
    lin("slap_ms", "Slap length", 2.0, 100.0, 1.0, "ms"),
    lin("body", "Board knock level", 0.0, 3.0, 0.05, ""),
    lin("body_pitch", "Board knock pitch", 0.5, 2.0, 0.01, "x"),
    lin("scuff", "Scuff level", 0.0, 2.0, 0.05, ""),
    hz("scuff_low", "Scuff band: from", 500.0, 6000.0),
    hz("scuff_high", "Scuff band: to", 2000.0, 16000.0),
    lin("scuff_ms", "Scuff length", 2.0, 100.0, 1.0, "ms"),
];

/// Feet meeting the griptape: a soft slap and a scuff.
fn catch(g: &Values, _variant: usize, rng: &mut Rng) -> Vec<f32> {
    let mut x = vec![0.0; samples(0.2)];
    burst(&mut x, rng, g.v("slap_low"), g.v("slap_high"), g.v("slap_ms") / 1000.0, 0.0, ATTACK, g.v("slap"));
    struck(&mut x, rng, &[(240.0, 0.03, 0.7), (430.0, 0.022, 0.5)], 0.0, 0.06, g.v("body"), g.v("body_pitch"), 1.0);
    let start = rng.uniform(0.004, 0.012);
    burst(&mut x, rng, g.v("scuff_low"), g.v("scuff_high"), g.v("scuff_ms") / 1000.0, start, 0.004, g.v("scuff"));
    fade_out(&mut x, 0.012);
    x
}

static FLIP: &[Param] = &[
    lin("length", "Length", 0.5, 2.0, 0.05, "x"),
    lin("pitch", "Whoosh pitch", 0.4, 2.5, 0.02, "x"),
    lin("width", "How long each band lasts", 0.05, 0.6, 0.01, ""),
    lin("flutter", "Flutter with the spin", 0.0, 1.0, 0.02, ""),
    hz("spin_hz", "Spin rate", 6.0, 60.0),
    lin("flick", "Shoe flick level", 0.0, 2.0, 0.05, ""),
    hz("flick_low", "Flick band: from", 500.0, 6000.0),
    hz("flick_high", "Flick band: to", 2000.0, 16000.0),
];

/// The board turning over in the air: the shoe flicking off, then a whoosh that flutters with the spin.
fn flip(g: &Values, variant: usize, rng: &mut Rng) -> Vec<f32> {
    let length = [0.30, 0.26, 0.34][variant % 3] * g.v("length");
    let n = samples(length);
    let mut x = vec![0.0; n];
    let (pitch, width) = (g.v("pitch"), g.v("width"));
    // the whoosh climbs and falls: bands that come and go in turn
    for (centre, peak_at) in [(450.0f32, 0.25f32), (800.0, 0.4), (1400.0, 0.52), (2100.0, 0.6), (1100.0, 0.8)] {
        let band = band_noise(rng, n, centre * pitch * 0.75, centre * pitch * 1.35);
        let level = (700.0 / centre).powf(0.3);
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f32 / n as f32;
            *s += band[i] * (-((t - peak_at) / width).powi(2)).exp() * level;
        }
    }
    let (spin, phase, flutter) = (g.v("spin_hz") * rng.uniform(0.83, 1.17), rng.uniform(0.0, 6.0), g.v("flutter"));
    for (i, s) in x.iter_mut().enumerate() {
        *s *= 1.0 - flutter * (0.5 + 0.5 * (TAU * spin * i as f32 / RATE_F + phase).sin());
    }
    burst(&mut x, rng, g.v("flick_low"), g.v("flick_high"), 0.012, 0.0, ATTACK, g.v("flick"));
    for (i, s) in x.iter_mut().enumerate() {
        *s *= (PI * i as f32 / n as f32).sin().max(0.0).sqrt();
    }
    fade_out(&mut x, 0.03);
    x
}

static PUSH: &[Param] = &[
    hz("thump_hz", "Foot thump", 40.0, 300.0),
    lin("thump", "Foot thump level", 0.0, 3.0, 0.05, ""),
    lin("tap", "Foot tap level", 0.0, 3.0, 0.05, ""),
    lin("scrape", "Scrape level", 0.0, 3.0, 0.05, ""),
    hz("scrape_low", "Scrape band: from", 100.0, 3000.0),
    hz("scrape_high", "Scrape band: to", 800.0, 12000.0),
    lin("roughness", "Scrape roughness", 0.0, 1.0, 0.02, ""),
    hz("rough_hz", "Roughness rate", 10.0, 200.0),
    lin("scrape_len", "Scrape length", 0.05, 1.0, 0.01, ""),
];

/// A foot pushing off the ground: it lands, then scrapes back.
fn push(g: &Values, variant: usize, rng: &mut Rng) -> Vec<f32> {
    let length = [0.46, 0.36, 0.5][variant % 3];
    let n = samples(length);
    let mut x = vec![0.0; n];
    mode(&mut x, g.v("thump_hz") * rng.uniform(0.87, 1.13), 0.03, g.v("thump"), 0.0, 0.0);
    burst(&mut x, rng, 400.0, 2200.0, 0.018, 0.0, ATTACK, g.v("tap"));
    let (rate, phase) = (g.v("rough_hz") * rng.uniform(0.8, 1.2), rng.uniform(0.0, 6.0));
    let noise = band_noise(rng, n, g.v("scrape_low"), g.v("scrape_high"));
    let (level, roughness, fall) = (g.v("scrape"), g.v("roughness"), length * g.v("scrape_len"));
    for (i, s) in x.iter_mut().enumerate() {
        let t = i as f32 / RATE_F;
        let envelope = ((t - 0.03) / 0.04).clamp(0.0, 1.0) * (-(t - 0.07).max(0.0) / fall).exp();
        let rough = 1.0 - roughness * (TAU * rate * t + phase).sin().abs();
        *s += noise[i] * envelope * rough * level;
    }
    fade_out(&mut x, 0.03);
    x
}

static FOOTSTEP: &[Param] = &[
    hz("heel_hz", "Heel thump", 50.0, 300.0),
    lin("heel", "Heel thump level", 0.0, 3.0, 0.05, ""),
    lin("heel_noise", "Heel tap level", 0.0, 3.0, 0.05, ""),
    hz("noise_low", "Heel tap band: from", 100.0, 3000.0),
    hz("noise_high", "Heel tap band: to", 800.0, 12000.0),
    lin("sole", "Sole level", 0.0, 2.0, 0.05, ""),
    hz("sole_low", "Sole band: from", 200.0, 4000.0),
    hz("sole_high", "Sole band: to", 1000.0, 14000.0),
    lin("toe_ms", "Heel to sole", 5.0, 100.0, 1.0, "ms"),
];

/// A shoe on the ground: the heel, then the sole.
fn footstep(g: &Values, _variant: usize, rng: &mut Rng) -> Vec<f32> {
    let mut x = vec![0.0; samples(0.22)];
    let toe = g.v("toe_ms") / 1000.0 * rng.uniform(0.7, 1.3);
    mode(&mut x, g.v("heel_hz") * rng.uniform(0.83, 1.17), 0.035, g.v("heel"), 0.0, 0.0);
    burst(&mut x, rng, g.v("noise_low"), g.v("noise_high"), 0.016, 0.0, ATTACK, g.v("heel_noise"));
    burst(&mut x, rng, g.v("sole_low"), g.v("sole_high"), 0.02, toe, 0.003, g.v("sole"));
    mode(&mut x, rng.uniform(140.0, 180.0), 0.02, 0.875 * g.v("sole"), 0.0, toe);
    fade_out(&mut x, 0.012);
    x
}

static BAIL: &[Param] = &[
    hz("body_hz", "Body thump", 30.0, 200.0),
    lin("body", "Body thump level", 0.0, 4.0, 0.05, ""),
    lin("body_ms", "Body thump length", 10.0, 300.0, 5.0, "ms"),
    lin("thud", "Thud level", 0.0, 4.0, 0.05, ""),
    hz("thud_high", "Thud band: to", 150.0, 3000.0),
    lin("cloth", "Cloth and skin level", 0.0, 2.0, 0.05, ""),
    lin("board", "Board bounce level", 0.0, 3.0, 0.05, ""),
    lin("board_pitch", "Board bounce pitch", 0.5, 2.0, 0.01, "x"),
    lin("gap_ms", "First bounce gap", 40.0, 400.0, 5.0, "ms"),
    lin("speedup", "Each bounce's gap, of the last", 0.4, 0.95, 0.01, "x"),
];

/// A fall: the body going down, the board bouncing away and coming to rest.
fn bail(g: &Values, variant: usize, rng: &mut Rng) -> Vec<f32> {
    let length = [0.9, 0.62][variant % 2];
    let mut x = vec![0.0; samples(length)];
    mode(&mut x, g.v("body_hz") * rng.uniform(0.83, 1.17), g.v("body_ms") / 1000.0, g.v("body"), 0.0, 0.0);
    burst(&mut x, rng, 90.0, g.v("thud_high"), 0.07, 0.0, ATTACK, g.v("thud"));
    burst(&mut x, rng, 600.0, 3000.0, 0.05, 0.02, 0.01, g.v("cloth"));
    // the board: knocks coming closer together and quieter, as a thing bouncing does
    let (mut at, mut gap, mut level) = (rng.uniform(0.07, 0.12), g.v("gap_ms") / 1000.0 * rng.uniform(0.85, 1.15), g.v("board"));
    let (pitch, speedup) = (g.v("board_pitch"), g.v("speedup"));
    while at < length - 0.08 && level > 0.08 {
        struck(&mut x, rng, &DECK, at, 0.1, level, pitch, 1.0);
        burst(&mut x, rng, 1500.0, 7000.0, 0.003, at, ATTACK, level);
        at += gap;
        gap *= rng.uniform(speedup - 0.08, speedup + 0.08);
        level *= rng.uniform(0.55, 0.72);
        if gap < 0.018 {
            break;
        }
    }
    fade_out(&mut x, 0.04);
    x
}

static CHIME: &[Param] = &[
    hz("note", "First note", 300.0, 3000.0),
    lin("interval", "Second note, of the first", 1.0, 2.5, 0.005, "x"),
    lin("decay", "Ring length", 0.05, 0.8, 0.01, "s"),
    lin("second", "Second note level", 0.0, 1.5, 0.05, ""),
    lin("second_ms", "Second note after", 0.0, 300.0, 5.0, "ms"),
    lin("bright", "Brightness (upper partials)", 0.0, 4.0, 0.05, "x"),
    lin("beat", "Slow beat", 0.0, 1.0, 0.02, ""),
];

/// A score banked: two soft bell notes, the second just after the first.
fn chime(g: &Values, _variant: usize, _rng: &mut Rng) -> Vec<f32> {
    let mut x = vec![0.0; samples(0.9)];
    let (note, decay, bright, beat) = (g.v("note"), g.v("decay"), g.v("bright"), g.v("beat"));
    for (freq, start, level) in [(note, 0.0, 1.0), (note * g.v("interval"), g.v("second_ms") / 1000.0, g.v("second"))] {
        mode(&mut x, freq, decay, level, 0.0, start);
        mode(&mut x, freq * 2.76, decay * 0.4, 0.22 * level * bright, 0.0, start);
        mode(&mut x, freq * 5.4, decay / 6.0, 0.07 * level * bright, 0.0, start);
        mode(&mut x, freq * 1.003, decay, beat * level, 1.0, start); // a slow beat, as a bell has
    }
    for (i, s) in x.iter_mut().enumerate() {
        *s *= (i as f32 / RATE_F / 0.004).min(1.0);
    }
    fade_out(&mut x, 0.05);
    x
}

// ---------------------------------------------------------------- the kinds

const fn a_loop(name: &'static str, params: &'static [Param], defaults: &'static [f32], make: fn(&Values, usize, &mut Rng) -> Vec<f32>) -> Kind {
    Kind { name, looping: true, variants: 1, params, defaults, level_db: 0.0, make }
}

const fn a_hit(
    name: &'static str,
    variants: usize,
    level_db: f32,
    params: &'static [Param],
    defaults: &'static [f32],
    make: fn(&Values, usize, &mut Rng) -> Vec<f32>,
) -> Kind {
    Kind { name, looping: false, variants, params, defaults, level_db, make }
}

/// A resonance that does nothing, where a kind has fewer than three.
macro_rules! none {
    ($hz:expr) => {
        [$hz, 2.0, 1.0]
    };
}

const fn join<const A: usize, const N: usize>(own: [f32; A], peaks: [[f32; 3]; 3]) -> [f32; N] {
    let mut all = [0.0; N];
    let mut i = 0;
    while i < A {
        all[i] = own[i];
        i += 1;
    }
    let mut j = 0;
    while j < 9 {
        all[A + j] = peaks[j / 3][j % 3];
        j += 1;
    }
    all
}

// (rolls: low, high, rumble, hiss, grains a second, grain low, grain high, grain, flutter Hz, flutter, sway)
/// Smooth concrete: mostly below 1 kHz, a little grain.
static ROLL_SMOOTH: [f32; 20] =
    join([140.0, 900.0, 0.5, 0.035, 140.0, 900.0, 3000.0, 0.07, 30.0, 0.12, 0.10], [none!(300.0), none!(800.0), none!(2000.0)]);
/// Asphalt: lower, coarser, the stones ticking under the wheels.
static ROLL_ROUGH: [f32; 20] =
    join([110.0, 1500.0, 0.8, 0.035, 170.0, 700.0, 4200.0, 0.30, 55.0, 0.25, 0.10], [none!(300.0), none!(800.0), none!(2000.0)]);
/// A wooden ramp: the sheet under the wheels booms, the top goes.
static ROLL_WOOD: [f32; 20] =
    join([120.0, 620.0, 0.9, 0.035, 0.0, 900.0, 3000.0, 0.0, 11.0, 0.15, 0.10], [[170.0, 2.5, 2.6], [300.0, 2.5, 2.2], [470.0, 3.0, 1.6]]);
/// Sheet metal: brighter, a broad ring to it.
static ROLL_METAL: [f32; 20] =
    join([260.0, 2800.0, 0.4, 0.035, 40.0, 1200.0, 5000.0, 0.10, 30.0, 0.12, 0.10], [[850.0, 4.0, 2.2], [1800.0, 5.0, 2.0], [2600.0, 5.0, 1.5]]);
// (scrapes: low, high, chatter Hz, chatter, body, grains a second, grain low, grain high, grain, sway)
/// A truck along a steel rail or coping.
static GRIND_METAL: [f32; 19] =
    join([600.0, 4200.0, 90.0, 0.45, 0.3, 60.0, 1500.0, 6000.0, 0.12, 0.12], [[1250.0, 5.0, 2.4], [2500.0, 6.0, 2.0], [3700.0, 6.0, 1.5]]);
/// A truck along a concrete ledge: lower, crunchier.
static GRIND_CONCRETE: [f32; 19] =
    join([280.0, 3200.0, 45.0, 0.6, 0.45, 220.0, 500.0, 3800.0, 0.4, 0.12], [[620.0, 2.5, 2.0], [1500.0, 3.0, 1.6], none!(3000.0)]);
/// Wheels pushed sideways: urethane scrubbing, chattering as it grips and lets go.
static POWERSLIDE: [f32; 19] =
    join([380.0, 2100.0, 60.0, 0.7, 0.2, 0.0, 1000.0, 4000.0, 0.0, 0.12], [[900.0, 2.0, 1.8], none!(1500.0), none!(3000.0)]);

/// Every kind, the loops first: the order of the driver's own (`Loop::ALL`, then `Shot::ALL`).
pub(crate) static KINDS: [Kind; 15] = [
    a_loop("roll_smooth", ROLL, &ROLL_SMOOTH, roll),
    a_loop("roll_rough", ROLL, &ROLL_ROUGH, roll),
    a_loop("roll_wood", ROLL, &ROLL_WOOD, roll),
    a_loop("roll_metal", ROLL, &ROLL_METAL, roll),
    a_loop("grind_metal", SCRAPE, &GRIND_METAL, scrape),
    a_loop("grind_concrete", SCRAPE, &GRIND_CONCRETE, scrape),
    a_loop("powerslide", SCRAPE, &POWERSLIDE, scrape),
    a_hit("pop", 4, 0.0, POP, &[1.0, 1.0, 1.0, 1.6, 1800.0, 9000.0, 2.5, 110.0, 1.1, 40.0], pop),
    a_hit("flip", 3, -3.0, FLIP, &[1.0, 1.0, 0.2, 0.38, 23.0, 0.5, 2500.0, 8000.0], flip),
    a_hit("catch", 3, -4.0, CATCH, &[1.0, 280.0, 1800.0, 20.0, 1.0, 1.0, 0.35, 2200.0, 6000.0, 28.0], catch),
    a_hit("land_impact", 4, 0.0, LAND, &[86.0, 75.0, 1.6, 1.0, 1.0, 1.1, 500.0, 2600.0, 22.0, 24.0, 0.25], land),
    a_hit("push", 3, -7.0, PUSH, &[97.0, 0.9, 0.6, 0.8, 450.0, 3600.0, 0.5, 57.0, 0.3], push),
    a_hit("footstep", 4, -3.0, FOOTSTEP, &[111.0, 1.0, 0.7, 450.0, 2400.0, 0.4, 700.0, 3800.0, 31.0], footstep),
    a_hit("bail", 2, 0.0, BAIL, &[70.0, 1.8, 90.0, 1.4, 520.0, 0.35, 0.9, 1.0, 130.0, 0.70], bail),
    a_hit("chime", 1, -2.0, CHIME, &[1046.5, 1.498, 0.30, 0.8, 70.0, 1.0, 0.3], chime),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transform_goes_there_and_back() {
        let mut rng = Rng::seeded("fft");
        let original: Vec<f32> = (0..1024).map(|_| rng.normal()).collect();
        let (mut re, mut im) = (original.clone(), vec![0.0; 1024]);
        fft(&mut re, &mut im, false);
        // (a tone in bin 5 has all its energy there)
        let (mut tone, mut zero) = ((0..1024).map(|i| (TAU * 5.0 * i as f32 / 1024.0).cos()).collect::<Vec<_>>(), vec![0.0; 1024]);
        fft(&mut tone, &mut zero, false);
        assert!((tone[5] - 512.0).abs() < 0.1 && tone[6].abs() < 0.1, "{} {}", tone[5], tone[6]);
        fft(&mut re, &mut im, true);
        for (a, b) in original.iter().zip(&re) {
            assert!((a - b / 1024.0).abs() < 1e-3, "{a} {}", b / 1024.0);
        }
    }

    #[test]
    fn every_kind_has_a_default_for_each_param_within_its_range() {
        for kind in &KINDS {
            assert_eq!(kind.params.len(), kind.defaults.len(), "{}", kind.name);
            for (param, value) in kind.params.iter().zip(kind.defaults) {
                assert!((param.min..=param.max).contains(value), "{} {}: {value}", kind.name, param.key);
                assert!(param.step > 0.0 || param.min > 0.0, "{} {}: a logarithmic slider from zero", kind.name, param.key);
            }
            let mut keys: Vec<_> = kind.params.iter().map(|p| p.key).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), kind.params.len(), "{}: a key twice", kind.name);
        }
    }

    #[test]
    fn every_sound_is_made_whole() {
        for kind in &KINDS {
            for variant in 0..kind.variants {
                let x = kind.render(kind.defaults, variant);
                let name = kind.file(variant);
                let peak = x.iter().map(|s| s.unsigned_abs()).max().unwrap();
                let level = (x.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / x.len() as f64).sqrt() / 32768.0;
                assert!(x.len() > 4000, "{name}");
                assert!(peak < 32767, "{name} clips");
                if kind.looping {
                    assert_eq!(x.len(), LOOP_N);
                    assert!((0.04..0.08).contains(&level), "{name}: level {level}");
                    // It comes round without a step.
                    let seam = (i32::from(x[0]) - i32::from(x[x.len() - 1])).abs();
                    let mut steps: Vec<i32> = x.windows(2).map(|w| (i32::from(w[1]) - i32::from(w[0])).abs()).collect();
                    steps.sort_unstable();
                    assert!(seam <= 4 * steps[steps.len() * 99 / 100], "{name}: a seam of {seam}");
                    // No moment much louder than the rest: it would mark every turn of the loop.
                    let mut loud: Vec<f64> =
                        x.chunks(960).map(|c| (c.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / c.len() as f64).sqrt()).collect();
                    loud.sort_by(f64::total_cmp);
                    assert!(loud[loud.len() - 1] < 3.2 * loud[loud.len() / 2], "{name}: a moment that stands out");
                } else {
                    assert!(peak > 9000, "{name}: peak {peak}");
                    assert!(x[0].abs() < 6600 && x[x.len() - 1].abs() < 330, "{name} doesn't start or end quietly");
                }
            }
        }
    }

    #[test]
    fn a_sound_is_the_same_each_time_and_follows_its_params() {
        let kind = &KINDS[7];
        assert_eq!(kind.render(kind.defaults, 0), kind.render(kind.defaults, 0));
        assert_ne!(kind.render(kind.defaults, 0), kind.render(kind.defaults, 1), "variants differ");
        let mut duller = kind.defaults.to_vec();
        duller[3] = 0.0; // no click
        let harsh = |x: &[i16]| x.windows(2).map(|w| (f64::from(w[1]) - f64::from(w[0])).powi(2)).sum::<f64>();
        assert!(harsh(&kind.render(&duller, 0)) < 0.5 * harsh(&kind.render(kind.defaults, 0)));
    }
}
