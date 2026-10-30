//! A constant-tempo beat grid: a tempo, the time of the first beat, and
//! which beat starts a bar.

use fourfloor_core::Tempo;

/// A constant-tempo beat grid over a piece of audio.
///
/// Beat `n` falls at `first_beat_seconds + n * beat_period_seconds()`. Beat
/// numbers may be negative: the grid extends in both directions.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BeatGrid {
    /// Tempo in beats per minute.
    pub bpm: f64,
    /// Time of beat 0 in seconds from the start of the audio. Analysis
    /// always places it within the first beat period.
    pub first_beat_seconds: f64,
    /// Beats in a bar. Analysis assumes 4.
    pub beats_per_bar: u32,
    /// Which beat is the first downbeat (start of a bar), counted from beat
    /// 0: a value in `0..beats_per_bar`. This is a guess.
    pub downbeat_index: u32,
}

impl BeatGrid {
    /// A 4/4 grid at `bpm` whose beat 0, a downbeat, is at
    /// `first_beat_seconds`.
    pub fn new(bpm: f64, first_beat_seconds: f64) -> Self {
        Self {
            bpm,
            first_beat_seconds,
            beats_per_bar: 4,
            downbeat_index: 0,
        }
    }

    /// Seconds between beats.
    pub fn beat_period_seconds(&self) -> f64 {
        60.0 / self.bpm
    }

    /// Seconds in a bar.
    pub fn bar_period_seconds(&self) -> f64 {
        self.beat_period_seconds() * f64::from(self.beats_per_bar.max(1))
    }

    /// The tempo as the engine's [`Tempo`], or `None` if it is outside the
    /// range the engine accepts.
    pub fn tempo(&self) -> Option<Tempo> {
        Tempo::new(self.bpm).ok()
    }

    /// Time of beat `beat` in seconds.
    pub fn beat_time(&self, beat: i64) -> f64 {
        self.first_beat_seconds + beat as f64 * self.beat_period_seconds()
    }

    /// Position of `seconds` on the grid in beats: 0.0 at beat 0, 1.5
    /// halfway between beats 1 and 2.
    pub fn beat_position(&self, seconds: f64) -> f64 {
        (seconds - self.first_beat_seconds) / self.beat_period_seconds()
    }

    /// The beat nearest to `seconds` and how far `seconds` is past it, in
    /// seconds (negative when the beat is still to come).
    pub fn nearest_beat(&self, seconds: f64) -> (i64, f64) {
        let beat = self.beat_position(seconds).round() as i64;
        (beat, seconds - self.beat_time(beat))
    }

    /// Times of every beat in `0.0..duration_seconds`.
    pub fn beats(&self, duration_seconds: f64) -> Vec<f64> {
        self.times(duration_seconds, 1, 0)
    }

    /// Time of the first downbeat at or after time zero.
    pub fn first_downbeat_seconds(&self) -> f64 {
        let period = self.beat_period_seconds();
        if !(period.is_finite() && period > 0.0 && self.first_beat_seconds.is_finite()) {
            return self.first_beat_seconds;
        }
        let per_bar = i64::from(self.beats_per_bar.max(1));
        let phase = i64::from(self.downbeat_index).rem_euclid(per_bar);
        let mut beat = phase;
        // Step back over whole bars while an earlier downbeat is still at
        // or after time zero.
        while self.beat_time(beat - per_bar) >= 0.0 {
            beat -= per_bar;
        }
        while self.beat_time(beat) < 0.0 {
            beat += per_bar;
        }
        self.beat_time(beat)
    }

    /// Times of every downbeat in `0.0..duration_seconds`.
    pub fn downbeats(&self, duration_seconds: f64) -> Vec<f64> {
        self.times(
            duration_seconds,
            self.beats_per_bar.max(1),
            self.downbeat_index,
        )
    }

    /// Whether beat `beat` starts a bar.
    pub fn is_downbeat(&self, beat: i64) -> bool {
        let per_bar = i64::from(self.beats_per_bar.max(1));
        (beat - i64::from(self.downbeat_index)).rem_euclid(per_bar) == 0
    }

    fn times(&self, duration_seconds: f64, every: u32, phase: u32) -> Vec<f64> {
        let period = self.beat_period_seconds();
        if !(period.is_finite() && period > 0.0 && duration_seconds > 0.0) {
            return Vec::new();
        }
        let every = i64::from(every);
        let phase = i64::from(phase).rem_euclid(every);
        // The first beat with the right phase at or after time zero.
        let first = (-self.first_beat_seconds / period).ceil() as i64;
        let mut beat = first + (phase - first).rem_euclid(every);
        let mut times = Vec::new();
        loop {
            let time = self.beat_time(beat);
            if time >= duration_seconds {
                return times;
            }
            if time >= 0.0 {
                times.push(time);
            }
            beat += every;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beats_and_downbeats_follow_the_grid() {
        let grid = BeatGrid {
            bpm: 120.0,
            first_beat_seconds: 0.25,
            beats_per_bar: 4,
            downbeat_index: 2,
        };
        assert_eq!(grid.beat_period_seconds(), 0.5);
        assert_eq!(grid.bar_period_seconds(), 2.0);
        assert_eq!(grid.beats(2.0), [0.25, 0.75, 1.25, 1.75]);
        assert_eq!(grid.downbeats(4.0), [1.25, 3.25]);
        assert_eq!(grid.first_downbeat_seconds(), 1.25);
        assert!(grid.is_downbeat(2) && grid.is_downbeat(-2) && !grid.is_downbeat(3));
        assert_eq!(grid.nearest_beat(0.8), (1, 0.8 - 0.75));
        assert_eq!(grid.beat_position(1.0), 1.5);
        assert_eq!(grid.tempo().map(Tempo::bpm), Some(120.0));
    }

    #[test]
    fn a_grid_that_starts_before_zero_lists_only_audible_beats() {
        let grid = BeatGrid::new(60.0, -2.5);
        assert_eq!(grid.beats(2.0), [0.5, 1.5]);
        assert_eq!(grid.downbeats(3.0), [1.5]);
        assert_eq!(grid.first_downbeat_seconds(), 1.5);
        assert!(BeatGrid::new(0.0, 0.0).beats(1.0).is_empty());
        assert!(BeatGrid::new(120.0, 0.0).beats(0.0).is_empty());
    }
}
