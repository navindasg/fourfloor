//! Musical time: tempo, beats, ticks, sample rate and conversions.
//!
//! A beat is a quarter note. The edit model stores positions as integer
//! [`Ticks`] ([`TICKS_PER_BEAT`] per beat) so edits are exact; the audio
//! thread works in fractional [`Beats`] and integer sample positions.
//!
//! All conversions here assume a single constant tempo. A tempo map, when
//! it exists, belongs to the engine and model and will call these per
//! segment.

use core::fmt;
use core::ops::{Add, Sub};

/// Tick resolution: ticks per beat (quarter note).
pub const TICKS_PER_BEAT: i64 = 960;

/// An absolute position on the timeline, in sample frames from the start.
pub type SamplePos = u64;

/// Error returned when constructing a time value from an out-of-range number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeError {
    /// Tempo was not finite or was outside
    /// [`Tempo::MIN_BPM`]..=[`Tempo::MAX_BPM`].
    InvalidTempo,
    /// Sample rate was zero.
    InvalidSampleRate,
}

impl fmt::Display for TimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTempo => write!(
                f,
                "tempo must be finite and between {} and {} BPM",
                Tempo::MIN_BPM,
                Tempo::MAX_BPM
            ),
            Self::InvalidSampleRate => write!(f, "sample rate must be greater than zero"),
        }
    }
}

impl std::error::Error for TimeError {}

/// Audio sample rate in Hz. Always greater than zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "u32", into = "u32"))]
pub struct SampleRate(u32);

impl SampleRate {
    /// 44.1 kHz.
    pub const HZ_44100: Self = Self(44_100);
    /// 48 kHz.
    pub const HZ_48000: Self = Self(48_000);
    /// 96 kHz.
    pub const HZ_96000: Self = Self(96_000);

    /// Creates a sample rate. Fails if `hz` is zero.
    pub const fn new(hz: u32) -> Result<Self, TimeError> {
        if hz == 0 {
            Err(TimeError::InvalidSampleRate)
        } else {
            Ok(Self(hz))
        }
    }

    /// The rate in Hz.
    pub const fn hz(self) -> u32 {
        self.0
    }

    /// The rate in Hz as `f64`, for DSP maths.
    pub fn as_f64(self) -> f64 {
        f64::from(self.0)
    }
}

impl TryFrom<u32> for SampleRate {
    type Error = TimeError;
    fn try_from(hz: u32) -> Result<Self, TimeError> {
        Self::new(hz)
    }
}

impl From<SampleRate> for u32 {
    fn from(rate: SampleRate) -> u32 {
        rate.0
    }
}

/// Tempo in beats (quarter notes) per minute. Always finite and within
/// [`Tempo::MIN_BPM`]..=[`Tempo::MAX_BPM`].
#[derive(Clone, Copy, Debug, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "f64", into = "f64"))]
pub struct Tempo(f64);

impl Tempo {
    /// Lowest accepted tempo.
    pub const MIN_BPM: f64 = 20.0;
    /// Highest accepted tempo.
    pub const MAX_BPM: f64 = 999.0;
    /// 120 BPM, the tempo of a new project.
    pub const DEFAULT: Self = Self(120.0);

    /// Creates a tempo. Fails if `bpm` is not finite or is out of range.
    pub fn new(bpm: f64) -> Result<Self, TimeError> {
        if bpm.is_finite() && (Self::MIN_BPM..=Self::MAX_BPM).contains(&bpm) {
            Ok(Self(bpm))
        } else {
            Err(TimeError::InvalidTempo)
        }
    }

    /// Beats per minute.
    pub const fn bpm(self) -> f64 {
        self.0
    }

    /// Duration of one beat in seconds.
    pub fn seconds_per_beat(self) -> f64 {
        60.0 / self.0
    }

    /// Length of one beat in (fractional) sample frames at `rate`.
    pub fn samples_per_beat(self, rate: SampleRate) -> f64 {
        rate.as_f64() * 60.0 / self.0
    }
}

impl Default for Tempo {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<f64> for Tempo {
    type Error = TimeError;
    fn try_from(bpm: f64) -> Result<Self, TimeError> {
        Self::new(bpm)
    }
}

impl From<Tempo> for f64 {
    fn from(tempo: Tempo) -> f64 {
        tempo.0
    }
}

/// A musical position or duration in beats (quarter notes), fractional.
/// May be negative (e.g. a pre-roll or a difference).
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Beats(pub f64);

impl Beats {
    /// Zero beats.
    pub const ZERO: Self = Self(0.0);

    /// Wraps a beat count.
    pub const fn new(beats: f64) -> Self {
        Self(beats)
    }

    /// The beat count.
    pub const fn get(self) -> f64 {
        self.0
    }

    /// Converts to fractional sample frames at a constant tempo.
    pub fn to_samples(self, tempo: Tempo, rate: SampleRate) -> f64 {
        self.0 * tempo.samples_per_beat(rate)
    }

    /// Converts fractional sample frames to beats at a constant tempo.
    pub fn from_samples(samples: f64, tempo: Tempo, rate: SampleRate) -> Self {
        Self(samples / tempo.samples_per_beat(rate))
    }

    /// Converts to the nearest whole sample position. Negative positions
    /// clamp to zero.
    pub fn to_sample_pos(self, tempo: Tempo, rate: SampleRate) -> SamplePos {
        // `as` saturates: negatives and NaN become 0, huge values u64::MAX.
        self.to_samples(tempo, rate).round() as SamplePos
    }

    /// Converts a whole sample position to beats at a constant tempo.
    /// Exact for positions below 2^53 frames.
    pub fn from_sample_pos(pos: SamplePos, tempo: Tempo, rate: SampleRate) -> Self {
        Self::from_samples(pos as f64, tempo, rate)
    }

    /// Rounds to the nearest tick.
    pub fn to_ticks(self) -> Ticks {
        // `as` saturates on overflow and maps NaN to 0.
        Ticks((self.0 * TICKS_PER_BEAT as f64).round() as i64)
    }
}

impl Add for Beats {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Beats {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

/// A musical position or duration in integer ticks, [`TICKS_PER_BEAT`] per
/// beat. This is the edit model's exact, hashable, orderable time unit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Ticks(pub i64);

impl Ticks {
    /// Zero ticks.
    pub const ZERO: Self = Self(0);
    /// One beat (quarter note).
    pub const BEAT: Self = Self(TICKS_PER_BEAT);
    /// One sixteenth note, the house step grid.
    pub const SIXTEENTH: Self = Self(TICKS_PER_BEAT / 4);

    /// Wraps a tick count.
    pub const fn new(ticks: i64) -> Self {
        Self(ticks)
    }

    /// The tick count.
    pub const fn get(self) -> i64 {
        self.0
    }

    /// A whole number of beats. Saturates on overflow.
    pub const fn from_whole_beats(beats: i64) -> Self {
        Self(beats.saturating_mul(TICKS_PER_BEAT))
    }

    /// Converts to fractional beats.
    pub fn to_beats(self) -> Beats {
        Beats(self.0 as f64 / TICKS_PER_BEAT as f64)
    }

    /// Converts to the nearest whole sample position at a constant tempo.
    /// Negative positions clamp to zero.
    pub fn to_sample_pos(self, tempo: Tempo, rate: SampleRate) -> SamplePos {
        self.to_beats().to_sample_pos(tempo, rate)
    }

    /// Converts a whole sample position to the nearest tick at a constant
    /// tempo.
    pub fn from_sample_pos(pos: SamplePos, tempo: Tempo, rate: SampleRate) -> Self {
        Beats::from_sample_pos(pos, tempo, rate).to_ticks()
    }
}

impl Add for Ticks {
    type Output = Self;
    /// Saturating addition.
    fn add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Ticks {
    type Output = Self;
    /// Saturating subtraction.
    fn sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempo(bpm: f64) -> Tempo {
        Tempo::new(bpm).unwrap()
    }

    #[test]
    fn sample_rate_rejects_zero() {
        assert_eq!(SampleRate::new(0), Err(TimeError::InvalidSampleRate));
        assert_eq!(SampleRate::new(48_000), Ok(SampleRate::HZ_48000));
        assert_eq!(SampleRate::HZ_44100.hz(), 44_100);
        assert_eq!(SampleRate::try_from(96_000u32), Ok(SampleRate::HZ_96000));
        assert_eq!(u32::from(SampleRate::HZ_48000), 48_000);
    }

    #[test]
    fn tempo_rejects_out_of_range_and_non_finite() {
        for bad in [0.0, -120.0, 19.99, 999.01, f64::NAN, f64::INFINITY] {
            assert_eq!(Tempo::new(bad), Err(TimeError::InvalidTempo), "{bad}");
        }
        assert!(Tempo::new(Tempo::MIN_BPM).is_ok());
        assert!(Tempo::new(Tempo::MAX_BPM).is_ok());
        assert_eq!(Tempo::default().bpm(), 120.0);
    }

    #[test]
    fn time_error_has_a_message() {
        assert!(TimeError::InvalidTempo.to_string().contains("BPM"));
        assert!(TimeError::InvalidSampleRate.to_string().contains("zero"));
    }

    #[test]
    fn samples_per_beat_at_common_tempos() {
        assert_eq!(
            tempo(120.0).samples_per_beat(SampleRate::HZ_48000),
            24_000.0
        );
        assert_eq!(tempo(120.0).seconds_per_beat(), 0.5);
        let spb = tempo(124.0).samples_per_beat(SampleRate::HZ_44100);
        assert!((spb - 21_338.709_677).abs() < 1e-3);
    }

    #[test]
    fn beats_to_samples_and_back() {
        let (t, sr) = (tempo(125.0), SampleRate::HZ_48000);
        // 125 BPM at 48 kHz is exactly 23 040 samples per beat.
        assert_eq!(Beats(4.0).to_samples(t, sr), 92_160.0);
        assert_eq!(Beats(4.0).to_sample_pos(t, sr), 92_160);
        assert_eq!(Beats::from_sample_pos(92_160, t, sr), Beats(4.0));
        assert_eq!(Beats::from_samples(11_520.0, t, sr), Beats(0.5));
    }

    #[test]
    fn round_trip_is_stable_at_awkward_tempo() {
        let (t, sr) = (tempo(123.456), SampleRate::HZ_44100);
        for pos in [0u64, 1, 4_410, 44_100, 10_000_000, 1_000_000_007] {
            let back = Beats::from_sample_pos(pos, t, sr).to_sample_pos(t, sr);
            assert_eq!(back, pos);
        }
    }

    #[test]
    fn negative_beats_clamp_to_sample_zero() {
        assert_eq!(
            Beats(-1.0).to_sample_pos(Tempo::DEFAULT, SampleRate::HZ_48000),
            0
        );
        assert_eq!(
            Ticks(-960).to_sample_pos(Tempo::DEFAULT, SampleRate::HZ_48000),
            0
        );
    }

    #[test]
    fn ticks_and_beats_convert() {
        assert_eq!(Ticks::BEAT.to_beats(), Beats(1.0));
        assert_eq!(Ticks::SIXTEENTH.to_beats(), Beats(0.25));
        assert_eq!(Ticks::SIXTEENTH.get(), 240);
        assert_eq!(Beats(1.5).to_ticks(), Ticks(1440));
        assert_eq!(Beats(-0.25).to_ticks(), Ticks(-240));
        assert_eq!(Ticks::from_whole_beats(4), Ticks(3840));
        assert_eq!(Ticks::from_whole_beats(i64::MAX), Ticks(i64::MAX));
        assert_eq!(Beats(f64::NAN).to_ticks(), Ticks::ZERO);
    }

    #[test]
    fn ticks_to_samples_and_back() {
        let (t, sr) = (tempo(120.0), SampleRate::HZ_48000);
        // One sixteenth at 120 BPM / 48 kHz is 6 000 samples.
        assert_eq!(Ticks::SIXTEENTH.to_sample_pos(t, sr), 6_000);
        assert_eq!(Ticks::from_sample_pos(6_000, t, sr), Ticks::SIXTEENTH);
        // Every sixteenth across 64 bars survives a round trip.
        for step in 0..(64 * 16) {
            let ticks = Ticks(step * Ticks::SIXTEENTH.get());
            assert_eq!(
                Ticks::from_sample_pos(ticks.to_sample_pos(t, sr), t, sr),
                ticks
            );
        }
    }

    #[test]
    fn arithmetic() {
        assert_eq!(Beats(1.0) + Beats(0.5), Beats(1.5));
        assert_eq!(Beats(1.0) - Beats(0.5), Beats(0.5));
        assert_eq!(Ticks(960) + Ticks(240), Ticks(1200));
        assert_eq!(Ticks(960) - Ticks(240), Ticks(720));
        assert_eq!(Ticks(i64::MAX) + Ticks(1), Ticks(i64::MAX));
        assert_eq!(Ticks(i64::MIN) - Ticks(1), Ticks(i64::MIN));
        assert!(Ticks(1) < Ticks(2));
        assert!(Beats(1.0) < Beats(2.0));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_round_trips_and_validates() {
        assert_eq!(serde_json::to_string(&tempo(124.0)).unwrap(), "124.0");
        assert_eq!(
            serde_json::from_str::<Tempo>("124.0").unwrap(),
            tempo(124.0)
        );
        assert!(serde_json::from_str::<Tempo>("0.0").is_err());
        assert_eq!(
            serde_json::to_string(&SampleRate::HZ_48000).unwrap(),
            "48000"
        );
        assert!(serde_json::from_str::<SampleRate>("0").is_err());
        assert_eq!(serde_json::to_string(&Ticks(960)).unwrap(), "960");
        assert_eq!(serde_json::from_str::<Beats>("1.5").unwrap(), Beats(1.5));
    }
}
