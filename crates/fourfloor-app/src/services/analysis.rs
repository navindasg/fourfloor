//! Master metering on the UI thread: loudness (momentary, short-term,
//! integrated), true peak, stereo correlation and a band spectrum, fed with
//! the master samples the audio callback queues.

use fourfloor_analysis::{CorrelationMeter, SpectrumAnalyzer, SpectrumOptions};
use fourfloor_audio::LoudnessMeter;
use fourfloor_core::SampleRate;

/// Bands in the spectrum display.
pub const SPECTRUM_BANDS: usize = 40;

/// The latest master readings.
#[derive(Clone, Debug, PartialEq)]
pub struct MasterReadout {
    /// Momentary loudness (400 ms), LUFS.
    pub momentary_lufs: f64,
    /// Short-term loudness (3 s), LUFS.
    pub short_term_lufs: f64,
    /// Integrated loudness since the last reset, LUFS.
    pub integrated_lufs: f64,
    /// Highest true peak since the last reset, dBTP.
    pub true_peak_dbtp: f64,
    /// Stereo correlation, -1 to 1.
    pub correlation: f32,
    /// Band levels in dB, low to high.
    pub spectrum_db: Vec<f32>,
    /// Band centres in Hz.
    pub band_centers_hz: Vec<f64>,
}

impl Default for MasterReadout {
    fn default() -> Self {
        Self {
            momentary_lufs: f64::NEG_INFINITY,
            short_term_lufs: f64::NEG_INFINITY,
            integrated_lufs: f64::NEG_INFINITY,
            true_peak_dbtp: f64::NEG_INFINITY,
            correlation: 0.0,
            spectrum_db: vec![-120.0; SPECTRUM_BANDS],
            band_centers_hz: Vec::new(),
        }
    }
}

/// Meters fed with interleaved stereo master audio.
pub struct MasterAnalysis {
    sample_rate: SampleRate,
    loudness: Option<LoudnessMeter>,
    correlation: CorrelationMeter,
    spectrum: Option<SpectrumAnalyzer>,
    left: Vec<f32>,
    right: Vec<f32>,
    mono: Vec<f32>,
    readout: MasterReadout,
}

impl std::fmt::Debug for MasterAnalysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterAnalysis")
            .field("readout", &self.readout)
            .finish_non_exhaustive()
    }
}

fn finite_or_floor(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        f64::NEG_INFINITY
    }
}

impl MasterAnalysis {
    /// Meters for audio at `sample_rate`.
    pub fn new(sample_rate: SampleRate) -> Self {
        let options = SpectrumOptions {
            fft_size: 4096,
            hop: 2048,
            bands: SPECTRUM_BANDS,
            ..SpectrumOptions::default()
        };
        let spectrum = SpectrumAnalyzer::new(sample_rate, options).ok();
        let readout = MasterReadout {
            band_centers_hz: spectrum
                .as_ref()
                .map(SpectrumAnalyzer::band_centers_hz)
                .unwrap_or_default(),
            ..MasterReadout::default()
        };
        Self {
            sample_rate,
            loudness: LoudnessMeter::new(2, sample_rate).ok(),
            correlation: CorrelationMeter::new(sample_rate, 0.3),
            spectrum,
            left: Vec::new(),
            right: Vec::new(),
            mono: Vec::new(),
            readout,
        }
    }

    /// Starts integrated loudness and true peak over (on play, say).
    pub fn reset(&mut self) {
        self.loudness = LoudnessMeter::new(2, self.sample_rate).ok();
        self.correlation.reset();
        if let Some(spectrum) = &mut self.spectrum {
            spectrum.reset();
        }
        self.readout = MasterReadout {
            band_centers_hz: self.readout.band_centers_hz.clone(),
            ..MasterReadout::default()
        };
    }

    /// Feeds interleaved stereo samples.
    pub fn push(&mut self, interleaved: &[f32]) {
        if interleaved.len() < 2 {
            return;
        }
        self.left.clear();
        self.right.clear();
        self.mono.clear();
        for frame in interleaved.chunks_exact(2) {
            let (l, r) = (sanitize(frame[0]), sanitize(frame[1]));
            self.left.push(l);
            self.right.push(r);
            self.mono.push(0.5 * (l + r));
        }
        if let Some(meter) = &mut self.loudness {
            if meter.add_planar(&[&self.left, &self.right]).is_ok() {
                self.readout.momentary_lufs =
                    finite_or_floor(meter.momentary_lufs().unwrap_or(f64::NEG_INFINITY));
                self.readout.short_term_lufs =
                    finite_or_floor(meter.short_term_lufs().unwrap_or(f64::NEG_INFINITY));
                self.readout.integrated_lufs =
                    finite_or_floor(meter.integrated_lufs().unwrap_or(f64::NEG_INFINITY));
                self.readout.true_peak_dbtp =
                    finite_or_floor(meter.true_peak_dbtp().unwrap_or(f64::NEG_INFINITY));
            }
        }
        self.readout.correlation = self.correlation.process(&self.left, &self.right);
        if let Some(spectrum) = &mut self.spectrum {
            let readout = &mut self.readout;
            spectrum.push(&self.mono, |frame| {
                // Smooth: rise fast, fall slowly.
                for (shown, new) in readout.spectrum_db.iter_mut().zip(frame.bands_db) {
                    *shown = if new > *shown {
                        new
                    } else {
                        *shown * 0.7 + new * 0.3
                    };
                }
            });
        }
    }

    /// The latest readings.
    pub fn readout(&self) -> &MasterReadout {
        &self.readout
    }
}

fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_testkit::{sine, white_noise};

    fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
        left.iter().zip(right).flat_map(|(l, r)| [*l, *r]).collect()
    }

    #[test]
    fn a_full_scale_sine_reads_as_expected() {
        let rate = SampleRate::HZ_48000;
        let tone = sine(1000.0, 0.5, 4.0, rate);
        let mut analysis = MasterAnalysis::new(rate);
        for chunk in interleave(&tone, &tone).chunks(4800) {
            analysis.push(chunk);
        }
        let readout = analysis.readout();
        // A 1 kHz sine at -6 dBFS on both channels reads about -6 LUFS
        // (-3 dB per channel sum + 3 dB for two channels).
        assert!(
            (readout.integrated_lufs - (-6.0)).abs() < 0.5,
            "{readout:?}"
        );
        assert!((readout.true_peak_dbtp - (-6.02)).abs() < 0.3);
        assert!(readout.correlation > 0.99);
        let loudest = readout
            .spectrum_db
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(band, _)| band)
            .unwrap_or_default();
        let center = readout.band_centers_hz[loudest];
        assert!(
            (700.0..1400.0).contains(&center),
            "peak band at {center} Hz"
        );
    }

    #[test]
    fn opposite_channels_read_negative_correlation_and_reset_clears() {
        let rate = SampleRate::HZ_48000;
        let noise = white_noise(3, 0.3, 1.0, rate);
        let inverted: Vec<f32> = noise.iter().map(|s| -s).collect();
        let mut analysis = MasterAnalysis::new(rate);
        analysis.push(&interleave(&noise, &inverted));
        assert!(analysis.readout().correlation < -0.99);
        analysis.reset();
        assert_eq!(analysis.readout().integrated_lufs, f64::NEG_INFINITY);
        assert_eq!(analysis.readout().band_centers_hz.len(), SPECTRUM_BANDS);
    }
}
