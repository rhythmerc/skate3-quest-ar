//! The sound lab: a developer's panel for tuning the clean sounds by ear, in the headset.
//!
//! Pick a kind of sound, hold it playing (a loop stays on, a hit repeats), solo it, and turn its
//! level, its four-band EQ and, where it is a synthesised one, every value the generator
//! (`clean_synth`) makes it from. The EQ is a graph of what it does to the sound, each band a
//! handle on it to drag with the laser: across for where it sits, up and down for how much. Each change is heard as it is made and saved to
//! `sound-tuning.json` beside the AR settings; `scripts/quest/pull-sound-tuning.sh` brings that
//! file into the source tree, where the next build starts from it.
//!
//! Not for players: the start page links here only with `"dev": true` in `ar-settings.json` (or
//! `SK8_DEV` set).
use super::{Act, GRAPH_COLUMNS, GRAPH_SIZE, Graph, GraphHandle, Outcome, PageId, UiPage, UiProvider, Widget};
use crate::clean_mix::{BAND_NAMES, BANDS, Band, FLAT, response_db};
use crate::clean_synth::{KINDS, Param, RATE};
use crate::skate_audio::clean::SoundLab;
use crate::xr::ar_settings::ArSettings;
use bevy::prelude::*;

const LAB: PageId = "sound_lab";
const EQ: PageId = "sound_lab_eq";
const MADE_OF: PageId = "sound_lab_made_of";

/// A slider's id for each generator value.
const VALUE_IDS: [&str; 24] = [
    "v0", "v1", "v2", "v3", "v4", "v5", "v6", "v7", "v8", "v9", "v10", "v11", "v12", "v13", "v14", "v15", "v16", "v17", "v18", "v19", "v20",
    "v21", "v22", "v23",
];
/// A logarithmic slider's travel: 0 to 1 in this many steps.
const LOG_STEP: f32 = 0.005;
const EQ_HZ: (f32, f32) = (20.0, 16_000.0);
const EQ_Q: (f32, f32) = (0.3, 8.0);
/// The graph's top and bottom, dB.
const EQ_DB: f32 = 18.0;
const BAND_LETTERS: [&str; BANDS] = ["L", "1", "2", "H"];
const BAND_COLORS: [Color; BANDS] =
    [Color::srgb(1.0, 0.62, 0.35), Color::srgb(0.98, 0.86, 0.38), Color::srgb(0.55, 0.9, 0.5), Color::srgb(0.5, 0.75, 1.0)];

/// The band the EQ page's sliders turn (the one last grabbed on the graph), and how far from its
/// handle the laser took hold of it.
#[derive(Resource, Clone, Copy, Default)]
struct EqHand {
    band: usize,
    offset: Vec2,
}

/// Where a band's handle is on the graph.
fn handle_at(band: &Band) -> Vec2 {
    Vec2::new(to_log(band.freq, EQ_HZ), (0.5 - band.gain / (2.0 * EQ_DB)).clamp(0.0, 1.0))
}

/// The band a handle at `at` is: on round numbers, as fine as the ear is there.
fn band_at(at: Vec2, q: f32) -> Band {
    let freq = from_log(at.x, EQ_HZ);
    let step = if freq < 100.0 { 1.0 } else if freq < 1000.0 { 5.0 } else if freq < 5000.0 { 50.0 } else { 100.0 };
    let gain = ((0.5 - at.y) * 2.0 * EQ_DB * 2.0).round() / 2.0;
    Band { freq: (freq / step).round() * step, gain: gain.clamp(-EQ_DB, EQ_DB), q }
}

pub(super) fn eq_graph(eq: &[Band; BANDS], selected: usize) -> Graph {
    let line = |hz: f32, label: &'static str| (to_log(hz, EQ_HZ), label, !label.is_empty());
    let level = |db: f32, label: &'static str| (0.5 - db / (2.0 * EQ_DB), label, db == 0.0);
    Graph {
        id: "eq_graph",
        curve: (0..GRAPH_COLUMNS)
            .map(|i| {
                let freq = from_log((i as f32 + 0.5) / GRAPH_COLUMNS as f32, EQ_HZ);
                0.5 - response_db(eq, RATE as f32, freq) / (2.0 * EQ_DB)
            })
            .collect(),
        zero: 0.5,
        handles: (0..BANDS)
            .map(|b| GraphHandle { at: handle_at(&eq[b]), label: BAND_LETTERS[b], color: BAND_COLORS[b], selected: b == selected })
            .collect(),
        across: vec![
            line(50.0, ""),
            line(100.0, "100"),
            line(200.0, ""),
            line(500.0, ""),
            line(1000.0, "1k"),
            line(2000.0, ""),
            line(5000.0, ""),
            line(10_000.0, "10k"),
        ],
        down: vec![level(12.0, "+12"), level(6.0, ""), level(0.0, "0 dB"), level(-6.0, ""), level(-12.0, "-12")],
    }
}

pub(crate) struct SoundLabPages;

pub(super) fn dev(world: &World) -> bool {
    world.resource::<ArSettings>().dev || std::env::var_os("SK8_DEV").is_some()
}

fn to_log(value: f32, (min, max): (f32, f32)) -> f32 {
    ((value.clamp(min, max) / min).ln() / (max / min).ln()).clamp(0.0, 1.0)
}

fn from_log(v: f32, (min, max): (f32, f32)) -> f32 {
    min * (max / min).powf(v.clamp(0.0, 1.0))
}

fn log_slider(id: &'static str, label: impl Into<String>, value: f32, range: (f32, f32), text: String) -> Widget {
    Widget::slider(id, label, to_log(value, range), (0.0, 1.0, LOG_STEP), text)
}

fn hz_text(value: f32) -> String {
    if value >= 1000.0 { format!("{:.2} kHz", value / 1000.0) } else { format!("{value:.0} Hz") }
}

fn value_slider(id: &'static str, param: &Param, value: f32) -> Widget {
    if param.step == 0.0 {
        return log_slider(id, param.label, value, (param.min, param.max), hz_text(value));
    }
    let places = if param.step < 0.01 { 3 } else if param.step < 1.0 { 2 } else { 0 };
    let unit = if param.unit.is_empty() || param.unit == "x" { param.unit.to_string() } else { format!(" {}", param.unit) };
    Widget::slider(id, param.label, value, (param.min, param.max, param.step), format!("{value:.places$}{unit}"))
}

/// The rows every page starts with: which sound, and how it is being listened to.
fn head(lab: &SoundLab) -> Vec<Widget> {
    let kind = &KINDS[lab.selected];
    let mut w = vec![
        Widget::slider("sound", "Sound", lab.selected as f32, (0.0, (KINDS.len() - 1) as f32, 1.0), kind.name.replace('_', " ")),
        Widget::toggle("audition", if kind.looping { "Hold it playing" } else { "Keep playing it" }, lab.audition),
        Widget::toggle("solo", "Solo (the other sounds are muted)", lab.solo),
    ];
    if kind.looping {
        w.push(Widget::slider("pitch", "Held at pitch (the game: 0.85 slow to 1.2 fast)", lab.pitch, (0.5, 1.5, 0.01), format!("{:.2}x", lab.pitch)));
    } else {
        w.push(Widget::button("play", "Play it once"));
    }
    w
}

impl UiProvider for SoundLabPages {
    fn start_items(&self, world: &World) -> Vec<Widget> {
        if dev(world) && world.contains_resource::<SoundLab>() { vec![Widget::link("Sound lab (dev)", LAB)] } else { Vec::new() }
    }

    fn page(&self, id: PageId, world: &World) -> Option<UiPage> {
        if ![LAB, EQ, MADE_OF].contains(&id) {
            return None;
        }
        let lab = world.get_resource::<SoundLab>()?;
        let (kind, tune) = (&KINDS[lab.selected], lab.tune(lab.selected));
        let mut w = head(lab);
        Some(match id {
            LAB => {
                w.extend([
                    Widget::slider("level", "Level", tune.level_db, (-24.0, 12.0, 0.5), format!("{:+.1} dB", tune.level_db)),
                    Widget::link("EQ", EQ),
                    Widget::toggle("synth", "Synthesised (off: the recording)", tune.synth),
                ]);
                if tune.synth {
                    w.push(Widget::link(format!("What it is made of ({} values)", kind.params.len()), MADE_OF));
                }
                w.push(Widget::button("reset", "This sound as it came (level, EQ, recording)"));
                w.push(Widget::info(""));
                if crate::reverb::room().is_some() {
                    w.extend([
                        Widget::slider("reverb", "Room reverb level (every sound)", lab.reverb, (0.0, 4.0, 0.05), format!("{:.2}x", lab.reverb)),
                        Widget::slider("damping", "Room: how soft its surfaces are", lab.damping, (0.25, 4.0, 0.05), format!("{:.2}x", lab.damping)),
                    ]);
                } else {
                    w.push(Widget::info("No room reverb: Steam Audio isn't loaded."));
                }
                w.push(Widget::info(""));
                w.push(Widget::info(match lab.path() {
                    Some(path) => format!("Saved as you go: {}", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()),
                    None => "Not saved: nowhere to keep the tuning here.".to_string(),
                }));
                let tuned = (0..KINDS.len()).filter(|&k| *lab.tune(k) != Default::default()).map(|k| KINDS[k].name.replace('_', " ")).collect::<Vec<_>>();
                w.push(Widget::info(if tuned.is_empty() { "Nothing tuned yet.".to_string() } else { format!("Tuned: {}", tuned.join(", ")) }));
                UiPage::new("Sound lab", w)
            }
            EQ => {
                // The graph does the turning; under it, what is heard, the grabbed band's
                // sharpness, and its place and gain again for the sticks and for fine steps.
                let b = world.get_resource::<EqHand>().map_or(0, |hand| hand.band).min(BANDS - 1);
                let (name, band) = (BAND_NAMES[b], &tune.eq[b]);
                w.truncate(3);
                w.extend([
                    Widget::slider("eq_band", "Band (or grab one on the graph)", b as f32, (0.0, (BANDS - 1) as f32, 1.0), name),
                    log_slider("eq_q", format!("{name}: sharpness (Q)"), band.q, EQ_Q, format!("{:.2}", band.q)),
                    log_slider("eq_f", format!("{name}: at"), band.freq, EQ_HZ, hz_text(band.freq)),
                    Widget::slider("eq_g", format!("{name}: gain"), band.gain, (-EQ_DB, EQ_DB, 0.5), format!("{:+.1} dB", band.gain)),
                    Widget::button("flat", "Flat"),
                ]);
                UiPage { graph: Some(eq_graph(&tune.eq, b)), ..UiPage::new("EQ", w) }
            }
            _ => {
                if tune.synth {
                    w.extend(kind.params.iter().enumerate().map(|(p, param)| value_slider(VALUE_IDS[p], param, lab.value(lab.selected, p))));
                    w.push(Widget::button("defaults", "All as it came"));
                } else {
                    w.push(Widget::info("A recording: nothing to turn. Switch it to synthesised first."));
                }
                UiPage::new("What it is made of", w)
            }
        })
    }

    fn act(&self, page: PageId, widget: &'static str, act: Act, world: &mut World) -> Option<Outcome> {
        if ![LAB, EQ, MADE_OF].contains(&page) {
            return None;
        }
        let mut hand = world.get_resource::<EqHand>().copied().unwrap_or_default();
        let mut lab = world.get_resource_mut::<SoundLab>()?;
        let lab = &mut *lab;
        let selected = lab.selected;
        let b = hand.band.min(BANDS - 1);
        match (widget, act) {
            ("eq_graph", Act::Grab(at)) => {
                // The nearest handle, held where it was taken hold of.
                let eq = lab.tune(selected).eq;
                let far = |b: &usize| ((handle_at(&eq[*b]) - at) * GRAPH_SIZE).length_squared();
                hand.band = (0..BANDS).min_by(|a, b| far(a).total_cmp(&far(b))).unwrap_or(0);
                hand.offset = handle_at(&eq[hand.band]) - at;
            }
            ("eq_graph", Act::Drag(at)) => {
                let moved = band_at(at + hand.offset, lab.tune(selected).eq[b].q);
                if lab.tune(selected).eq[b] != moved {
                    lab.edit(selected, |tune| tune.eq[b] = moved);
                }
            }
            ("eq_band", Act::Set(v)) => hand.band = (v.round() as usize).min(BANDS - 1),
            ("eq_q", Act::Set(v)) => lab.edit(selected, |tune| tune.eq[b].q = (from_log(v, EQ_Q) * 100.0).round() / 100.0),
            ("eq_f", Act::Set(v)) => lab.edit(selected, |tune| tune.eq[b].freq = from_log(v, EQ_HZ).round()),
            ("eq_g", Act::Set(v)) => lab.edit(selected, |tune| tune.eq[b].gain = v),
            ("sound", Act::Set(v)) => lab.selected = (v.round() as usize).min(KINDS.len() - 1),
            ("audition", _) => lab.audition = !lab.audition,
            ("solo", _) => lab.solo = !lab.solo,
            ("pitch", Act::Set(v)) => lab.pitch = v,
            ("play", _) => lab.play_once(),
            ("reverb", Act::Set(v)) => lab.set_reverb(v, lab.damping),
            ("damping", Act::Set(v)) => lab.set_reverb(lab.reverb, v),
            ("level", Act::Set(v)) => lab.edit(selected, |tune| tune.level_db = v),
            ("synth", _) => lab.edit(selected, |tune| tune.synth = !tune.synth),
            ("reset", _) => lab.edit(selected, |tune| *tune = Default::default()),
            ("flat", _) => lab.edit(selected, |tune| tune.eq = FLAT),
            ("defaults", _) => lab.edit(selected, |tune| tune.params.clear()),
            (id, Act::Set(v)) => {
                if let Some(p) = VALUE_IDS.iter().position(|&i| i == id).filter(|&p| p < KINDS[selected].params.len()) {
                    let param = &KINDS[selected].params[p];
                    let value = if param.step == 0.0 { from_log(v, (param.min, param.max)) } else { v };
                    lab.set_value(selected, p, value);
                }
            }
            _ => {}
        }
        world.insert_resource(hand);
        Some(Outcome::Stay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_generator_value_has_a_slider() {
        assert!(KINDS.iter().all(|kind| kind.params.len() <= VALUE_IDS.len()));
    }

    #[test]
    fn a_band_dragged_on_the_graph_goes_where_the_laser_does() {
        let mut eq = FLAT;
        eq[1] = Band { freq: 1000.0, gain: 6.0, q: 2.0 };
        let graph = eq_graph(&eq, 1);
        assert_eq!(graph.curve.len(), GRAPH_COLUMNS);
        // The curve peaks where the handle is, at the handle's height; flat is the middle line.
        let handle = &graph.handles[1];
        assert!(handle.selected && !graph.handles[0].selected);
        let column = (handle.at.x * GRAPH_COLUMNS as f32) as usize;
        assert!((graph.curve[column] - handle.at.y).abs() < 0.01, "{} against {}", graph.curve[column], handle.at.y);
        assert!((graph.curve[2] - 0.5).abs() < 0.01 && graph.curve.iter().all(|&y| y >= handle.at.y - 0.01));
        assert!(eq_graph(&FLAT, 0).curve.iter().all(|&y| y == 0.5));
        // A handle put somewhere is a band that draws its handle there, on round numbers.
        let band = band_at(Vec2::new(0.7, 0.2), 1.5);
        assert!((handle_at(&band) - Vec2::new(0.7, 0.2)).abs().max_element() < 0.01, "{band:?}");
        assert!(band.freq % 50.0 == 0.0 && (band.gain * 2.0).fract() == 0.0 && band.q == 1.5, "{band:?}");
        // Off the top of the graph is the most there is.
        assert_eq!(band_at(Vec2::new(0.5, -0.3), 1.0).gain, EQ_DB);
    }

    #[test]
    fn a_logarithmic_slider_goes_there_and_back() {
        for hz in [20.0, 137.0, 1000.0, 16_000.0] {
            assert!((from_log(to_log(hz, EQ_HZ), EQ_HZ) / hz - 1.0).abs() < 1e-3);
        }
        // One step is a small turn: about a twentieth of an octave.
        let step = from_log(0.5 + LOG_STEP, EQ_HZ) / from_log(0.5, EQ_HZ);
        assert!(step > 1.0 && step < 1.05, "{step}");
    }
}
