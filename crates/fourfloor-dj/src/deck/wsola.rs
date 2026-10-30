//! Built-in key lock: a WSOLA time stretcher (waveform-similarity
//! overlap-add). Grains are read from the track at normal speed, so pitch
//! is kept, while the playhead that places them moves at the deck's rate.

use super::playhead::Playhead;
use super::source::{interpolated, mono};
use crate::track::{DeckTrack, MAX_STEMS};

/// Grain hop in seconds (a grain is two hops long).
const HOP_SECONDS: f64 = 1024.0 / 48_000.0;
/// How far, in seconds, a grain may be moved to line up with the last one.
const SEARCH_SECONDS: f64 = 0.005;
/// Samples between the points compared when lining grains up.
const DECIMATION: usize = 8;

#[derive(Clone, Debug)]
pub(crate) struct Wsola {
    hop: usize,
    search: i64,
    /// Rising half of a Hann window, `hop` long.
    window: Vec<f32>,
    /// Track samples a grain reads per output frame (track rate over
    /// engine rate), so a grain plays at the track's own pitch.
    ratio: f64,
    current: f64,
    previous: f64,
    age: usize,
    primed: bool,
}

impl Wsola {
    pub fn new() -> Self {
        Self {
            hop: 0,
            search: 0,
            window: Vec::new(),
            ratio: 1.0,
            current: 0.0,
            previous: 0.0,
            age: 0,
            primed: false,
        }
    }

    /// Allocates the window for `sample_rate`.
    pub fn prepare(&mut self, sample_rate: f64) {
        self.hop = ((HOP_SECONDS * sample_rate).round() as usize).max(16);
        self.search = (SEARCH_SECONDS * sample_rate).round() as i64;
        self.window = (0..self.hop)
            .map(|i| {
                let phase = std::f64::consts::FRAC_PI_2 * i as f64 / self.hop as f64;
                phase.sin().powi(2) as f32
            })
            .collect();
        self.primed = false;
    }

    /// Sets how many track samples a grain reads per output frame.
    /// Ignored unless finite and positive.
    pub fn set_ratio(&mut self, ratio: f64) {
        if ratio.is_finite() && ratio > 0.0 {
            self.ratio = ratio;
        }
    }

    /// Starts over at the playhead on the next sample (after a jump).
    pub fn restart(&mut self) {
        self.primed = false;
    }

    /// How far (in track samples) to move a grain nominally starting at
    /// `nominal` so that it continues the audio at `natural` as closely as
    /// possible.
    fn best_offset(&self, track: &DeckTrack, head: &Playhead, natural: f64, nominal: f64) -> f64 {
        let search = (self.search as f64 * self.ratio).round().max(1.0) as i64;
        if (natural - nominal).abs() <= search as f64 {
            return natural - nominal;
        }
        let points = self.hop / DECIMATION;
        let natural_index = natural.round() as i64;
        let nominal_index = nominal.round() as i64;
        let (mut best, mut best_score) = (0, f32::NEG_INFINITY);
        for offset in -search..=search {
            let (mut dot, mut energy) = (0.0f32, 1e-9f32);
            for point in 0..points {
                let step = ((point * DECIMATION) as f64 * self.ratio).round() as i64;
                let wanted = mono(track, head.map(natural_index + step));
                let candidate = mono(track, head.map(nominal_index + offset + step));
                dot += wanted * candidate;
                energy += candidate * candidate;
            }
            let score = dot / energy.sqrt();
            if score > best_score {
                best_score = score;
                best = offset;
            }
        }
        (nominal_index + best) as f64 - nominal
    }

    /// The next output frame. The caller advances the playhead.
    #[inline]
    pub fn next(
        &mut self,
        track: &DeckTrack,
        head: &Playhead,
        gains: &[f32; MAX_STEMS],
    ) -> (f32, f32) {
        if self.hop == 0 {
            return (0.0, 0.0);
        }
        let span = self.hop as f64 * self.ratio;
        if !self.primed {
            // Pretend a grain started one hop ago in the same place, so
            // the first hop plays the track unchanged instead of fading in.
            self.current = head.pos;
            self.previous = self.current - span;
            self.age = 0;
            self.primed = true;
        } else if self.age >= self.hop {
            let natural = self.current + span;
            let nominal = head.pos;
            self.previous = self.current;
            self.current = nominal + self.best_offset(track, head, natural, nominal);
            self.age = 0;
        }
        let rising = self.window[self.age];
        let read = self.age as f64 * self.ratio;
        let new = interpolated(track, head.map_fractional(self.current + read), gains);
        let old = interpolated(
            track,
            head.map_fractional(self.previous + span + read),
            gains,
        );
        self.age += 1;
        (
            new.0 * rising + old.0 * (1.0 - rising),
            new.1 * rising + old.1 * (1.0 - rising),
        )
    }
}
