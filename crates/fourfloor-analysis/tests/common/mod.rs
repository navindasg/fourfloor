//! Synthesized test material shared by the integration tests: drum hits,
//! four-on-the-floor loops, notes and chords. Everything is deterministic.

#![allow(dead_code)]

use fourfloor_audio::AudioBuffer;
use fourfloor_core::SampleRate;
use fourfloor_testkit::white_noise;

pub const RATE: SampleRate = SampleRate::HZ_48000;

pub fn frames(seconds: f64, rate: SampleRate) -> usize {
    (seconds * rate.as_f64()).round() as usize
}

/// Adds `sound` into `out` starting at `seconds`, dropping what runs off
/// the end.
pub fn place(out: &mut [f32], sound: &[f32], seconds: f64, rate: SampleRate) {
    let start = frames(seconds, rate);
    for (slot, &sample) in out.iter_mut().skip(start).zip(sound) {
        *slot += sample;
    }
}

/// A 909-ish kick: a sine swept from 160 Hz to 50 Hz with a fast decay.
pub fn kick(rate: SampleRate) -> Vec<f32> {
    let sr = rate.as_f64();
    let mut phase = 0.0f64;
    (0..frames(0.3, rate))
        .map(|n| {
            let t = n as f64 / sr;
            let hz = 50.0 + 110.0 * (-t / 0.03).exp();
            phase += std::f64::consts::TAU * hz / sr;
            // Fade the tail so the hit does not end in a click.
            let tail = ((0.3 - t) / 0.02).clamp(0.0, 1.0);
            (0.9 * phase.sin() * (-t / 0.09).exp() * tail) as f32
        })
        .collect()
}

/// One-pole high-pass, enough to make noise sound like a hat.
pub fn high_pass(signal: &[f32], cutoff_hz: f64, rate: SampleRate) -> Vec<f32> {
    let keep = (-std::f64::consts::TAU * cutoff_hz / rate.as_f64()).exp() as f32;
    let mut low = 0.0f32;
    signal
        .iter()
        .map(|&x| {
            low = keep * low + (1.0 - keep) * x;
            x - low
        })
        .collect()
}

pub fn low_pass(signal: &[f32], cutoff_hz: f64, rate: SampleRate) -> Vec<f32> {
    let keep = (-std::f64::consts::TAU * cutoff_hz / rate.as_f64()).exp() as f32;
    let mut low = 0.0f32;
    signal
        .iter()
        .map(|&x| {
            low = keep * low + (1.0 - keep) * x;
            low
        })
        .collect()
}

fn decay(signal: &mut [f32], seconds: f64, rate: SampleRate) {
    for (n, sample) in signal.iter_mut().enumerate() {
        *sample *= (-(n as f64) / (seconds * rate.as_f64())).exp() as f32;
    }
}

/// A hi-hat: high-passed noise. `length` is the decay time in seconds.
pub fn hat(seed: u32, amplitude: f32, length: f64, rate: SampleRate) -> Vec<f32> {
    let noise = white_noise(seed, amplitude, length * 5.0, rate);
    let mut out = high_pass(&high_pass(&noise, 7000.0, rate), 7000.0, rate);
    decay(&mut out, length, rate);
    out
}

/// A clap: band-limited noise around 1 to 3 kHz.
pub fn clap(seed: u32, rate: SampleRate) -> Vec<f32> {
    let noise = white_noise(seed, 0.6, 0.25, rate);
    let mut out = low_pass(&high_pass(&noise, 900.0, rate), 3500.0, rate);
    decay(&mut out, 0.05, rate);
    out
}

/// A note with a few harmonics and a soft attack and release.
pub fn note(midi: f64, amplitude: f32, seconds: f64, rate: SampleRate) -> Vec<f32> {
    let sr = rate.as_f64();
    let hz = 440.0 * 2f64.powf((midi - 69.0) / 12.0);
    let total = frames(seconds, rate);
    let fade = frames(0.01, rate).max(1);
    (0..total)
        .map(|n| {
            let t = n as f64 / sr;
            let wave: f64 = (1..=5)
                .map(|h| (std::f64::consts::TAU * hz * h as f64 * t).sin() / (h * h) as f64)
                .sum();
            let edge = n.min(total - 1 - n).min(fade) as f64 / fade as f64;
            (f64::from(amplitude) * wave * edge) as f32
        })
        .collect()
}

/// Several notes at once.
pub fn chord(midi: &[f64], amplitude: f32, seconds: f64, rate: SampleRate) -> Vec<f32> {
    let mut out = vec![0.0f32; frames(seconds, rate)];
    for &pitch in midi {
        place(&mut out, &note(pitch, amplitude, seconds, rate), 0.0, rate);
    }
    out
}

/// What goes into a [`drum_loop`].
#[derive(Clone, Copy)]
pub struct Loop {
    pub bpm: f64,
    pub beats: usize,
    /// Time of the first kick in seconds.
    pub offset: f64,
    pub rate: SampleRate,
    /// Claps on the second and fourth beat of each bar.
    pub claps: bool,
    /// Hats on the offbeats and quieter ones on the other sixteenths.
    pub hats: bool,
    /// Which beat (0 to 3) of the first kick's bar the loop starts on.
    pub starts_on: usize,
}

impl Loop {
    pub fn new(bpm: f64, beats: usize) -> Self {
        Self {
            bpm,
            beats,
            offset: 0.0,
            rate: RATE,
            claps: true,
            hats: true,
            starts_on: 0,
        }
    }

    pub fn offset(mut self, offset: f64) -> Self {
        self.offset = offset;
        self
    }

    pub fn rate(mut self, rate: SampleRate) -> Self {
        self.rate = rate;
        self
    }

    pub fn starts_on(mut self, beat: usize) -> Self {
        self.starts_on = beat;
        self
    }

    pub fn period(&self) -> f64 {
        60.0 / self.bpm
    }

    pub fn seconds(&self) -> f64 {
        self.offset + self.beats as f64 * self.period()
    }
}

/// A four-on-the-floor loop: kick on every beat, offbeat open hats, closed
/// hats on the remaining sixteenths, claps on two and four.
pub fn drum_loop(spec: &Loop) -> Vec<f32> {
    let rate = spec.rate;
    let period = spec.period();
    let mut out = vec![0.0f32; frames(spec.seconds(), rate)];
    let kick = kick(rate);
    let clap = clap(7, rate);
    for beat in 0..spec.beats {
        let time = spec.offset + beat as f64 * period;
        place(&mut out, &kick, time, rate);
        if spec.claps && (beat + spec.starts_on) % 2 == 1 {
            place(&mut out, &clap, time, rate);
        }
        if spec.hats {
            let seed = 100 + beat as u32;
            place(
                &mut out,
                &hat(seed, 0.35, 0.05, rate),
                time + period / 2.0,
                rate,
            );
            for sixteenth in [1.0, 3.0] {
                let at = time + sixteenth * period / 4.0;
                place(&mut out, &hat(seed + 1000, 0.15, 0.015, rate), at, rate);
            }
        }
    }
    out
}

/// A bass line over a [`drum_loop`]: one offbeat note per beat whose pitch
/// changes at every bar line, cycling through `roots` (MIDI pitches).
pub fn bass_line(spec: &Loop, roots: &[f64]) -> Vec<f32> {
    let period = spec.period();
    let mut out = vec![0.0f32; frames(spec.seconds(), spec.rate)];
    for beat in 0..spec.beats {
        let bar = (beat + spec.starts_on) / 4;
        let pitch = roots[bar % roots.len()];
        let time = spec.offset + (beat as f64 + 0.5) * period;
        place(
            &mut out,
            &note(pitch, 0.5, period * 0.45, spec.rate),
            time,
            spec.rate,
        );
    }
    out
}

pub fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    let mut out = a.to_vec();
    out.resize(a.len().max(b.len()), 0.0);
    for (slot, &sample) in out.iter_mut().zip(b) {
        *slot += sample;
    }
    out
}

pub fn gain(signal: &[f32], factor: f32) -> Vec<f32> {
    signal.iter().map(|&s| s * factor).collect()
}

pub fn stereo(left: Vec<f32>, right: Vec<f32>, rate: SampleRate) -> AudioBuffer {
    AudioBuffer::new(rate, vec![left, right])
}

pub fn dual_mono(signal: &[f32], rate: SampleRate) -> AudioBuffer {
    stereo(signal.to_vec(), signal.to_vec(), rate)
}

/// Difference between two times on a circle of length `period`.
pub fn circular_difference(a: f64, b: f64, period: f64) -> f64 {
    let difference = (a - b).rem_euclid(period);
    difference.min(period - difference)
}
