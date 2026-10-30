//! The drum humanizer, checked on note data and on audio rendered by the
//! 909 voices.

use fourfloor_core::EventKind;
use fourfloor_core::{SampleRate, Tempo, Ticks, TICKS_PER_BEAT};
use fourfloor_instruments::DrumKit;
use fourfloor_ml::{humanize, learn_groove, HumanizeOptions};
use fourfloor_model::MidiNote;
use fourfloor_testkit::{
    onset_times_with, render_instrument, OnsetOptions, RenderConfig, TimedEvent,
};

const SIXTEENTH: i64 = TICKS_PER_BEAT / 4;

fn note(start: i64, pitch: u8, velocity: u8) -> MidiNote {
    MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(SIXTEENTH / 2),
        pitch,
        velocity,
    }
}

/// Four bars of house: kick on the beat, clap on 2 and 4, closed hats on
/// every sixteenth, all at velocity 100 on the grid.
fn house_bars(bars: i64) -> Vec<MidiNote> {
    let mut notes = Vec::new();
    for step in 0..bars * 16 {
        let start = step * SIXTEENTH;
        if step % 4 == 0 {
            notes.push(note(start, 36, 100));
        }
        if step % 16 == 4 || step % 16 == 12 {
            notes.push(note(start, 39, 100));
        }
        notes.push(note(start, 42, 100));
    }
    notes
}

fn tempo() -> Tempo {
    Tempo::new(124.0).expect("tempo")
}

fn ms_per_tick() -> f64 {
    tempo().seconds_per_beat() * 1000.0 / TICKS_PER_BEAT as f64
}

fn offsets_ms(before: &[MidiNote], after: &[MidiNote], pitch: u8) -> Vec<f64> {
    let pick = |notes: &[MidiNote]| -> Vec<i64> {
        notes
            .iter()
            .filter(|n| n.pitch == pitch)
            .map(|n| n.start.get())
            .collect()
    };
    pick(before)
        .iter()
        .zip(pick(after))
        .map(|(a, b)| (b - a) as f64 * ms_per_tick())
        .collect()
}

fn std(values: &[f64]) -> f64 {
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
}

fn mean(values: &[f64]) -> f64 {
    values.iter().sum::<f64>() / values.len() as f64
}

#[test]
fn zero_amount_changes_nothing_and_seeds_are_repeatable() {
    let notes = house_bars(4);
    let none = HumanizeOptions {
        amount: 0.0,
        ..HumanizeOptions::default()
    };
    assert_eq!(humanize(&notes, tempo(), &none).expect("humanize"), notes);
    let options = HumanizeOptions::default();
    let a = humanize(&notes, tempo(), &options).expect("a");
    let b = humanize(&notes, tempo(), &options).expect("b");
    assert_eq!(a, b);
    let other = HumanizeOptions {
        seed: 99,
        ..HumanizeOptions::default()
    };
    assert_ne!(a, humanize(&notes, tempo(), &other).expect("c"));
    assert_eq!(a.len(), notes.len());
    assert!(a
        .iter()
        .all(|n| n.start.get() >= 0 && (1..=127).contains(&n.velocity)));
    assert!(a
        .windows(2)
        .all(|w| (w[0].start, w[0].pitch) <= (w[1].start, w[1].pitch)));
}

#[test]
fn timing_spread_follows_the_instrument_and_backbeats_lay_back() {
    let notes = house_bars(32);
    let options = HumanizeOptions::default();
    let human = humanize(&notes, tempo(), &options).expect("humanize");
    let hats = offsets_ms(&notes, &human, 42);
    let kicks = offsets_ms(&notes, &human, 36);
    let claps = offsets_ms(&notes, &human, 39);
    eprintln!(
        "timing std: hats {:.1} ms, kicks {:.1} ms; clap mean {:.1} ms",
        std(&hats),
        std(&kicks),
        mean(&claps)
    );
    assert!(std(&hats) > 4.0 && std(&hats) < 14.0);
    assert!(std(&kicks) < std(&hats));
    assert!(mean(&claps) > 2.0, "backbeats should sit late");
    assert!(hats.iter().all(|ms| ms.abs() < 50.0));
    // The walk makes neighbouring hats drift together: positive lag-1
    // correlation of the offsets.
    let m = mean(&hats);
    let lag: f64 = hats
        .windows(2)
        .map(|w| (w[0] - m) * (w[1] - m))
        .sum::<f64>();
    let var: f64 = hats.iter().map(|v| (v - m).powi(2)).sum::<f64>();
    assert!(lag / var > 0.2, "lag-1 correlation {:.2}", lag / var);
}

#[test]
fn velocities_follow_the_bar() {
    let notes = house_bars(32);
    let human = humanize(&notes, tempo(), &HumanizeOptions::default()).expect("humanize");
    // Hats are a sixteenth apart and move by milliseconds, so they keep
    // their order and pair up by index.
    let before: Vec<&MidiNote> = notes.iter().filter(|n| n.pitch == 42).collect();
    let after: Vec<&MidiNote> = human.iter().filter(|n| n.pitch == 42).collect();
    assert_eq!(before.len(), after.len());
    let hat_velocity = |position: i64| -> f64 {
        let picked: Vec<f64> = before
            .iter()
            .zip(&after)
            .filter(|(b, _)| (b.start.get() / SIXTEENTH) % 16 == position)
            .map(|(_, a)| f64::from(a.velocity))
            .collect();
        mean(&picked)
    };
    let downbeat = hat_velocity(0);
    let beat = hat_velocity(4);
    let sixteenth = hat_velocity(1);
    eprintln!("hat velocity: downbeat {downbeat:.1}, beat {beat:.1}, off-16th {sixteenth:.1}");
    assert!(downbeat > beat && beat > sixteenth + 8.0);
}

#[test]
fn swing_moves_every_second_sixteenth() {
    let notes: Vec<MidiNote> = (0..64)
        .map(|step| note(step * SIXTEENTH, 42, 100))
        .collect();
    let options = HumanizeOptions {
        timing_ms: 0.0,
        velocity_spread: 0.0,
        swing: 0.625,
        ..HumanizeOptions::default()
    };
    let swung = humanize(&notes, tempo(), &options).expect("humanize");
    for (index, n) in swung.iter().enumerate() {
        let expected = index as i64 * SIXTEENTH + if index % 2 == 1 { SIXTEENTH / 4 } else { 0 };
        assert_eq!(n.start.get(), expected, "note {index}");
    }
    let groove = learn_groove("learned", &swung, SIXTEENTH, 2).expect("learn");
    assert!((groove.steps[0].timing).abs() < 1e-9);
    assert!((groove.steps[1].timing - 0.25).abs() < 1e-9);
}

#[test]
fn a_learned_groove_recovers_a_played_feel() {
    let notes = house_bars(16);
    let human = humanize(&notes, tempo(), &HumanizeOptions::default()).expect("humanize");
    let hats: Vec<MidiNote> = human.iter().filter(|n| n.pitch == 42).copied().collect();
    let groove = learn_groove("from hats", &hats, SIXTEENTH, 16).expect("learn");
    assert_eq!(groove.steps.len(), 16);
    // Downbeat hats were accented above the off-sixteenths.
    assert!(groove.steps[0].velocity > groove.steps[1].velocity);
    assert!(groove.steps.iter().all(|s| s.timing.abs() < 0.3));
}

/// Renders a part through the 909 kit and returns its onset times.
fn rendered_onsets(notes: &[MidiNote], pitch: u8, seconds: f64) -> Vec<f64> {
    let config = RenderConfig::default().with_tempo(tempo());
    let rate: SampleRate = config.sample_rate;
    let seconds_per_tick = tempo().seconds_per_beat() / TICKS_PER_BEAT as f64;
    let mut events: Vec<TimedEvent> = notes
        .iter()
        .filter(|n| n.pitch == pitch)
        .flat_map(|n| {
            let start = n.start.get() as f64 * seconds_per_tick;
            let end = (n.start + n.length).get() as f64 * seconds_per_tick;
            [
                TimedEvent::at_seconds(start, rate, EventKind::note_on(n.pitch, n.velocity)),
                TimedEvent::at_seconds(end, rate, EventKind::note_off(n.pitch)),
            ]
        })
        .collect();
    events.sort_by_key(|e| e.frame);
    let audio = render_instrument(&mut DrumKit::new(), &events, seconds, &config);
    let options = OnsetOptions {
        threshold_db: -20.0,
        min_gap_ms: 40.0,
        ..OnsetOptions::default()
    };
    onset_times_with(&audio.mono(), rate, &options)
}

#[test]
fn rendered_claps_land_where_the_humanized_notes_say() {
    let notes = house_bars(4);
    let human = humanize(&notes, tempo(), &HumanizeOptions::default()).expect("humanize");
    let seconds_per_tick = tempo().seconds_per_beat() / TICKS_PER_BEAT as f64;
    let seconds = 4.0 * 4.0 * tempo().seconds_per_beat() + 0.5;
    let straight = rendered_onsets(&notes, 39, seconds);
    let played = rendered_onsets(&human, 39, seconds);
    let grid: Vec<f64> = notes
        .iter()
        .filter(|n| n.pitch == 39)
        .map(|n| n.start.get() as f64 * seconds_per_tick)
        .collect();
    let wanted: Vec<f64> = human
        .iter()
        .filter(|n| n.pitch == 39)
        .map(|n| n.start.get() as f64 * seconds_per_tick)
        .collect();
    assert_eq!(straight.len(), grid.len(), "{straight:?}");
    assert_eq!(played.len(), wanted.len(), "{played:?}");
    // The clap's detected onset sits at a fixed small offset from its note;
    // compare the two renders through that offset.
    let bias = mean(
        &straight
            .iter()
            .zip(&grid)
            .map(|(a, b)| a - b)
            .collect::<Vec<_>>(),
    );
    for (found, expected) in played.iter().zip(&wanted) {
        assert!(
            (found - bias - expected).abs() < 0.0015,
            "{found} vs {expected}"
        );
    }
    let moved: Vec<f64> = played
        .iter()
        .zip(&straight)
        .map(|(a, b)| (a - b) * 1000.0)
        .collect();
    eprintln!("rendered clap shifts (ms): {moved:.1?}");
    assert!(mean(&moved) > 1.0, "humanized backbeats render late");
}
