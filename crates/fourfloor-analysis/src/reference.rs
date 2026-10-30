//! Reference A/B: compares a mix with a reference track and says, as
//! structured data, how the mix differs.
//!
//! The mix is first brought to the reference's loudness (on paper: no audio
//! is changed), so that "more bass" means more bass at equal loudness and
//! not just "louder". Then the two are compared band by band, and on stereo
//! width and crest factor.
//!
//! Matchering (GPL-3) is the reference for what is compared: overall level,
//! frequency response and stereo image against a reference. Matchering
//! goes on to build a correction filter and a limiter and to process the
//! audio; this module stops at the report. [`ReferenceReport::eq_correction`]
//! gives the per-band gains such a filter would aim for.

use fourfloor_audio::AudioBuffer;

use crate::error::AnalysisError;
use crate::levels::{measure_levels, LevelReport};
use crate::stft::{average_power, fft_size_for};

/// Window of the spectral comparison in seconds (4096 samples at 48 kHz).
const WINDOW_SECONDS: f64 = 0.085;
/// Edges of the low, mid and high bands in Hz.
const BROAD_EDGES: [f64; 4] = [20.0, 250.0, 4000.0, 20_000.0];
/// Centres of the octave bands in Hz.
const OCTAVE_CENTRES: [f64; 10] = [
    31.5, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0,
];

/// Thresholds below which a difference is not worth a suggestion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReferenceOptions {
    /// Loudness difference in LU. Default 1.
    pub loudness_lu: f64,
    /// Band level difference in dB. Default 1.5.
    pub band_db: f64,
    /// Width difference (share of side energy, 0 to 1). Default 0.05.
    pub width: f32,
    /// Crest factor difference in dB. Default 2.
    pub crest_db: f64,
    /// True peak the loudness-matched mix should stay under, in dBTP.
    /// Default -1.
    pub true_peak_ceiling_dbtp: f64,
}

impl Default for ReferenceOptions {
    fn default() -> Self {
        Self {
            loudness_lu: 1.0,
            band_db: 1.5,
            width: 0.05,
            crest_db: 2.0,
            true_peak_ceiling_dbtp: -1.0,
        }
    }
}

/// One frequency band of the comparison.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BandDelta {
    /// Lower edge in Hz.
    pub low_hz: f64,
    /// Upper edge in Hz.
    pub high_hz: f64,
    /// Level of the mix in this band after loudness matching, in dBFS RMS.
    pub mix_db: f64,
    /// Level of the reference in this band, in dBFS RMS.
    pub reference_db: f64,
    /// `mix_db - reference_db`: positive when the mix has more here than
    /// the reference at equal loudness. Zero when either has no energy in
    /// the band.
    pub delta_db: f64,
}

/// What a [`Suggestion`] is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SuggestionKind {
    /// Overall loudness.
    Loudness,
    /// The band below 250 Hz.
    LowBand,
    /// The band from 250 Hz to 4 kHz.
    MidBand,
    /// The band above 4 kHz.
    HighBand,
    /// Stereo width.
    StereoWidth,
    /// Crest factor: how much the peaks stand above the average level.
    Dynamics,
    /// True peak after matching loudness.
    TruePeak,
}

/// One suggested change to the mix.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Suggestion {
    /// What the suggestion is about.
    pub kind: SuggestionKind,
    /// The change to make to the mix to meet the reference, signed. In dB
    /// for loudness, bands, dynamics and true peak; in width units (share
    /// of side energy) for stereo width.
    pub change: f64,
    /// The suggestion in words.
    pub message: String,
}

/// The result of [`compare_to_reference`].
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ReferenceReport {
    /// Levels of the mix as supplied.
    pub mix: LevelReport,
    /// Levels of the reference.
    pub reference: LevelReport,
    /// Mix loudness minus reference loudness, in LU.
    pub loudness_delta_lu: f64,
    /// Gain in dB that brings the mix to the reference's loudness. Every
    /// band figure below assumes this gain has been applied.
    pub gain_to_match_db: f64,
    /// True peak the mix would have after that gain, in dBTP.
    pub true_peak_after_match_dbtp: f64,
    /// The band below 250 Hz.
    pub low: BandDelta,
    /// The band from 250 Hz to 4 kHz.
    pub mid: BandDelta,
    /// The band above 4 kHz.
    pub high: BandDelta,
    /// Ten octave bands from 31.5 Hz to 16 kHz.
    pub octaves: Vec<BandDelta>,
    /// Mix width minus reference width.
    pub width_delta: f32,
    /// Mix correlation minus reference correlation.
    pub correlation_delta: f32,
    /// Mix crest factor minus reference crest factor, in dB. Negative
    /// means the mix is more compressed than the reference.
    pub crest_delta_db: f64,
    /// Suggested changes, largest band differences included. Empty when
    /// the mix is within every threshold.
    pub suggestions: Vec<Suggestion>,
}

impl ReferenceReport {
    /// The equalizer curve that would move the mix's spectrum onto the
    /// reference's: `(centre frequency in Hz, gain in dB)` per octave band.
    pub fn eq_correction(&self) -> Vec<(f64, f64)> {
        self.octaves
            .iter()
            .map(|band| ((band.low_hz * band.high_hz).sqrt(), -band.delta_db))
            .collect()
    }

    /// The first suggestion of `kind`, if there is one.
    pub fn suggestion(&self, kind: SuggestionKind) -> Option<&Suggestion> {
        self.suggestions.iter().find(|s| s.kind == kind)
    }
}

/// Compares a mix with a reference track using the default thresholds.
pub fn compare_to_reference(
    mix: &AudioBuffer,
    reference: &AudioBuffer,
) -> Result<ReferenceReport, AnalysisError> {
    compare_to_reference_with(mix, reference, &ReferenceOptions::default())
}

/// Compares a mix with a reference track. The two may differ in length,
/// channel count and sample rate. Both need at least 400 ms of sound so
/// that their loudness can be measured.
pub fn compare_to_reference_with(
    mix: &AudioBuffer,
    reference: &AudioBuffer,
    options: &ReferenceOptions,
) -> Result<ReferenceReport, AnalysisError> {
    let mix_levels = measure_levels(mix)?;
    let reference_levels = measure_levels(reference)?;
    for (levels, buffer) in [(&mix_levels, mix), (&reference_levels, reference)] {
        if !levels.integrated_lufs.is_finite() {
            return Err(AnalysisError::TooShort {
                needed_seconds: 0.4,
                got_seconds: if levels.rms_dbfs.is_finite() {
                    buffer.duration_seconds()
                } else {
                    0.0
                },
            });
        }
    }
    let loudness_delta_lu = mix_levels.integrated_lufs - reference_levels.integrated_lufs;
    let gain_to_match_db = -loudness_delta_lu;

    let mix_spectrum = Spectrum::of(mix);
    let reference_spectrum = Spectrum::of(reference);
    let band = |low_hz: f64, high_hz: f64| {
        let mix_db = mix_spectrum.band_db(low_hz, high_hz) + gain_to_match_db;
        let reference_db = reference_spectrum.band_db(low_hz, high_hz);
        let delta_db = if mix_db.is_finite() && reference_db.is_finite() {
            mix_db - reference_db
        } else {
            0.0
        };
        BandDelta {
            low_hz,
            high_hz,
            mix_db,
            reference_db,
            delta_db,
        }
    };
    let low = band(BROAD_EDGES[0], BROAD_EDGES[1]);
    let mid = band(BROAD_EDGES[1], BROAD_EDGES[2]);
    let high = band(BROAD_EDGES[2], BROAD_EDGES[3]);
    let octaves = OCTAVE_CENTRES
        .iter()
        .map(|&centre| {
            band(
                centre / std::f64::consts::SQRT_2,
                centre * std::f64::consts::SQRT_2,
            )
        })
        .collect();

    let mut report = ReferenceReport {
        mix: mix_levels,
        reference: reference_levels,
        loudness_delta_lu,
        gain_to_match_db,
        true_peak_after_match_dbtp: mix_levels.true_peak_dbtp + gain_to_match_db,
        low,
        mid,
        high,
        octaves,
        width_delta: mix_levels.width - reference_levels.width,
        correlation_delta: mix_levels.correlation - reference_levels.correlation,
        crest_delta_db: mix_levels.crest_factor_db - reference_levels.crest_factor_db,
        suggestions: Vec::new(),
    };
    report.suggestions = suggest(&report, options);
    Ok(report)
}

/// Average power spectrum of the mono sum of a buffer.
struct Spectrum {
    power: Vec<f64>,
    bin_hz: f64,
}

impl Spectrum {
    fn of(buffer: &AudioBuffer) -> Self {
        let rate = buffer.sample_rate.as_f64();
        let size = fft_size_for(WINDOW_SECONDS, rate);
        let (power, bin_hz) = average_power(&buffer.to_mono(), size, rate);
        Self { power, bin_hz }
    }

    /// Level of the band in dBFS RMS; negative infinity when empty.
    fn band_db(&self, low_hz: f64, high_hz: f64) -> f64 {
        let first = ((low_hz / self.bin_hz).ceil() as usize).max(1);
        let last = ((high_hz / self.bin_hz).ceil() as usize).min(self.power.len());
        let power: f64 = self
            .power
            .get(first..last)
            .map_or(0.0, |bins| bins.iter().sum());
        10.0 * power.log10()
    }
}

fn suggest(report: &ReferenceReport, options: &ReferenceOptions) -> Vec<Suggestion> {
    let mut suggestions = Vec::new();
    let mut add = |kind, change: f64, message: String| {
        suggestions.push(Suggestion {
            kind,
            change,
            message,
        });
    };

    let loudness = report.loudness_delta_lu;
    if loudness.abs() >= options.loudness_lu {
        let (word, verb) = if loudness > 0.0 {
            ("louder", "Lower")
        } else {
            ("quieter", "Raise")
        };
        add(
            SuggestionKind::Loudness,
            report.gain_to_match_db,
            format!(
                "The mix is {:.1} LU {word} than the reference ({:.1} against {:.1} LUFS). \
                 {verb} it by {:.1} dB to match.",
                loudness.abs(),
                report.mix.integrated_lufs,
                report.reference.integrated_lufs,
                loudness.abs(),
            ),
        );
    }

    let bands = [
        (
            SuggestionKind::LowBand,
            "low end (below 250 Hz)",
            report.low,
        ),
        (
            SuggestionKind::MidBand,
            "mids (250 Hz to 4 kHz)",
            report.mid,
        ),
        (SuggestionKind::HighBand, "highs (above 4 kHz)", report.high),
    ];
    for (kind, name, band) in bands {
        if band.delta_db.abs() < options.band_db {
            continue;
        }
        let (word, verb) = if band.delta_db > 0.0 {
            ("more", "Cut")
        } else {
            ("less", "Boost")
        };
        add(
            kind,
            -band.delta_db,
            format!(
                "At matched loudness the mix has {:.1} dB {word} {name} than the reference. \
                 {verb} by about {:.1} dB.",
                band.delta_db.abs(),
                band.delta_db.abs(),
            ),
        );
    }

    if report.width_delta.abs() >= options.width {
        let (word, verb) = if report.width_delta > 0.0 {
            ("wider", "Narrow")
        } else {
            ("narrower", "Widen")
        };
        add(
            SuggestionKind::StereoWidth,
            -f64::from(report.width_delta),
            format!(
                "The mix is {word} than the reference (side share {:.2} against {:.2}). \
                 {verb} the stereo image.",
                report.mix.width, report.reference.width,
            ),
        );
    }

    let crest = report.crest_delta_db;
    if crest.abs() >= options.crest_db {
        let message = if crest < 0.0 {
            format!(
                "The mix has {:.1} dB less crest factor than the reference ({:.1} against \
                 {:.1} dB): it is more compressed or limited. Ease off the compression.",
                crest.abs(),
                report.mix.crest_factor_db,
                report.reference.crest_factor_db,
            )
        } else {
            format!(
                "The mix has {:.1} dB more crest factor than the reference ({:.1} against \
                 {:.1} dB): its peaks stand further above the body. More compression or \
                 limiting would bring it closer.",
                crest.abs(),
                report.mix.crest_factor_db,
                report.reference.crest_factor_db,
            )
        };
        add(SuggestionKind::Dynamics, -crest, message);
    }

    let over = report.true_peak_after_match_dbtp - options.true_peak_ceiling_dbtp;
    if over > 0.0 && over.is_finite() {
        add(
            SuggestionKind::TruePeak,
            -over,
            format!(
                "At the reference's loudness the mix would peak at {:.1} dBTP, {:.1} dB over \
                 the {:.1} dBTP ceiling. It needs {:.1} dB of limiting to get there.",
                report.true_peak_after_match_dbtp, over, options.true_peak_ceiling_dbtp, over,
            ),
        );
    }
    suggestions
}
