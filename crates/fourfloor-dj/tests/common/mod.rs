//! Test material and helpers shared by the DJ tests.

#![allow(dead_code)]

use fourfloor_core::SampleRate;
use fourfloor_dj::{default_registry, DeckTrack, DjCommand, DjRenderer, SessionConfig};
use fourfloor_testkit::white_noise;

pub const RATE: SampleRate = SampleRate::HZ_48000;
pub const BLOCK: usize = 256;

pub fn seconds_to_frames(seconds: f64) -> usize {
    (seconds * RATE.as_f64()).round() as usize
}

/// A session renderer with `decks` decks and these deck effects.
pub fn renderer(decks: usize, effects: &[&str]) -> DjRenderer {
    let registry = default_registry().expect("registry");
    let config = SessionConfig::new(decks, RATE, BLOCK).with_deck_effects(effects);
    DjRenderer::new(&config, &registry).expect("session")
}

/// A kick on every beat: a 55 Hz sine with an instant attack and an 80 ms
/// decay, starting on beat 0 at time 0.
pub fn kick_signal(bpm: f64, seconds: f64, rate: SampleRate) -> Vec<f32> {
    let frames = (seconds * rate.as_f64()).round() as usize;
    let period = 60.0 / bpm * rate.as_f64();
    let mut signal = vec![0.0f32; frames];
    let mut beat = 0.0f64;
    while (beat * period) < frames as f64 {
        let start = (beat * period).round() as usize;
        for (i, sample) in signal[start..]
            .iter_mut()
            .take((0.2 * rate.as_f64()) as usize)
            .enumerate()
        {
            let t = i as f64 / rate.as_f64();
            *sample += (0.8 * (-t / 0.08).exp() * (std::f64::consts::TAU * 55.0 * t).sin()) as f32;
        }
        beat += 1.0;
    }
    signal
}

/// [`kick_signal`] as a track with a grid.
pub fn kick_track(bpm: f64, seconds: f64) -> DeckTrack {
    DeckTrack::from_mono(RATE, &kick_signal(bpm, seconds, RATE))
        .expect("track")
        .with_name(format!("kick {bpm}"))
        .with_bpm(bpm, 0.0)
        .expect("grid")
}

/// Stereo white noise (different on each side) with a grid: every sample
/// is distinct, so position errors show up in null tests.
pub fn noise_track(bpm: f64, seconds: f64) -> DeckTrack {
    let left = white_noise(1, 0.5, seconds, RATE);
    let right = white_noise(2, 0.5, seconds, RATE);
    DeckTrack::new(RATE, left, right)
        .expect("track")
        .with_bpm(bpm, 0.0)
        .expect("grid")
}

/// Short pulses of a tone with harmonics on every beat, for pitch and
/// tempo measurements.
pub fn tone_pulses(frequency: f64, bpm: f64, seconds: f64) -> DeckTrack {
    let frames = seconds_to_frames(seconds);
    let period = 60.0 / bpm * RATE.as_f64();
    let signal: Vec<f32> = (0..frames)
        .map(|i| {
            let t = i as f64 / RATE.as_f64();
            let in_beat = (i as f64 % period) / RATE.as_f64();
            let gate = if in_beat < 0.25 { 1.0 } else { 0.05 };
            let tone = (std::f64::consts::TAU * frequency * t).sin()
                + 0.3 * (std::f64::consts::TAU * 2.0 * frequency * t).sin();
            (0.4 * gate * tone) as f32
        })
        .collect();
    DeckTrack::from_mono(RATE, &signal)
        .expect("track")
        .with_bpm(bpm, 0.0)
        .expect("grid")
}

/// Applies a command that must succeed.
pub fn run(renderer: &mut DjRenderer, command: DjCommand) {
    let name = command.name();
    renderer
        .apply(&command)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
}

/// Index of the first sample where `found` starts matching `reference`
/// exactly (searching `range`), for locating jumps.
pub fn find_offset(reference: &[f32], window: &[f32]) -> Option<usize> {
    reference
        .windows(window.len())
        .position(|candidate| candidate == window)
}
