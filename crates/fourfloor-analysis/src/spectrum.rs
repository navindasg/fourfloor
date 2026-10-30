//! Spectrum analyzer frames for UI metering: levels in logarithmically
//! spaced bands, one frame per hop.

use fourfloor_core::SampleRate;

use crate::error::AnalysisError;
use crate::stft::PowerSpectrum;

/// Settings for a [`SpectrumAnalyzer`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectrumOptions {
    /// FFT length in samples; a power of two from 64 to 65536. Default
    /// 4096.
    pub fft_size: usize,
    /// Samples between frames, 1 to `fft_size`. Default 1024.
    pub hop: usize,
    /// Number of bands. Default 64.
    pub bands: usize,
    /// Lower edge of the first band in Hz. Default 20.
    pub min_hz: f64,
    /// Upper edge of the last band in Hz; limited to half the sample rate.
    /// Default 20000.
    pub max_hz: f64,
    /// Lowest level reported, in dB. Default -120.
    pub floor_db: f32,
}

impl Default for SpectrumOptions {
    fn default() -> Self {
        Self {
            fft_size: 4096,
            hop: 1024,
            bands: 64,
            min_hz: 20.0,
            max_hz: 20_000.0,
            floor_db: -120.0,
        }
    }
}

/// The levels of one analysis frame.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SpectrumFrame {
    /// Time of the end of the frame, in seconds since the analyzer was
    /// created or reset.
    pub time_seconds: f64,
    /// Level of each band in dB, where 0 dB is a full-scale sine falling
    /// wholly inside the band. Never below the configured floor.
    ///
    /// The Hann window spreads a tone over about four FFT bins, so a band
    /// narrower than that (the low bands, at the default settings those
    /// below roughly 400 Hz) shares a tone with its neighbours and reads a
    /// few dB low. The bands together still hold all of the power. Use a
    /// longer `fft_size` for exact low-frequency levels.
    pub bands_db: Vec<f32>,
}

/// A streaming spectrum analyzer. Feed it mono audio in blocks of any
/// size; it produces a frame every `hop` samples.
///
/// It allocates when built and may allocate when fed, so it runs on a UI
/// or worker thread fed from a ring buffer, never on the audio thread.
pub struct SpectrumAnalyzer {
    options: SpectrumOptions,
    sample_rate: f64,
    spectrum: PowerSpectrum,
    power: Vec<f32>,
    pending: Vec<f32>,
    consumed: u64,
    /// For each band, the bins it overlaps and how much of each it takes.
    bands: Vec<Vec<(usize, f32)>>,
    edges: Vec<f64>,
}

impl std::fmt::Debug for SpectrumAnalyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectrumAnalyzer")
            .field("options", &self.options)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

impl SpectrumAnalyzer {
    /// Builds an analyzer for mono audio at `sample_rate`.
    pub fn new(sample_rate: SampleRate, options: SpectrumOptions) -> Result<Self, AnalysisError> {
        let invalid = |name, reason| Err(AnalysisError::InvalidOption { name, reason });
        if !(options.fft_size.is_power_of_two() && (64..=65_536).contains(&options.fft_size)) {
            return invalid("fft_size", "must be a power of two from 64 to 65536");
        }
        if !(1..=options.fft_size).contains(&options.hop) {
            return invalid("hop", "must be 1 to fft_size");
        }
        if !(1..=4096).contains(&options.bands) {
            return invalid("bands", "must be 1 to 4096");
        }
        let rate = sample_rate.as_f64();
        let max_hz = options.max_hz.min(rate / 2.0);
        if !(options.min_hz.is_finite() && options.min_hz > 0.0) {
            return invalid("min_hz", "must be positive");
        }
        if !(max_hz.is_finite() && max_hz > options.min_hz) {
            return invalid("max_hz", "must be above min_hz");
        }

        let ratio = max_hz / options.min_hz;
        let edges: Vec<f64> = (0..=options.bands)
            .map(|edge| options.min_hz * ratio.powf(edge as f64 / options.bands as f64))
            .collect();
        let bin_hz = rate / options.fft_size as f64;
        let last_bin = options.fft_size / 2;
        let bands = edges
            .windows(2)
            .map(|edge| {
                // Bin k covers (k - 0.5) to (k + 0.5) bin widths. A band
                // takes from each bin the share that it overlaps, so the
                // bands together hold exactly the power between the edges.
                let (low, high) = (edge[0] / bin_hz, edge[1] / bin_hz);
                let first = (low + 0.5).floor() as usize;
                let last = ((high + 0.5).floor() as usize).min(last_bin);
                (first..=last)
                    .filter_map(|bin| {
                        let from = (bin as f64 - 0.5).max(low);
                        let to = (bin as f64 + 0.5).min(high);
                        (to > from).then_some((bin, (to - from) as f32))
                    })
                    .collect()
            })
            .collect();
        let spectrum = PowerSpectrum::new(options.fft_size);
        Ok(Self {
            power: vec![0.0; spectrum.bins()],
            spectrum,
            options,
            sample_rate: rate,
            pending: Vec::with_capacity(options.fft_size * 2),
            consumed: 0,
            bands,
            edges,
        })
    }

    /// The settings the analyzer was built with.
    pub fn options(&self) -> &SpectrumOptions {
        &self.options
    }

    /// Band edges in Hz: one more value than there are bands.
    pub fn band_edges_hz(&self) -> &[f64] {
        &self.edges
    }

    /// Centre frequency of each band in Hz (the geometric mean of its
    /// edges).
    pub fn band_centers_hz(&self) -> Vec<f64> {
        self.edges
            .windows(2)
            .map(|edge| (edge[0] * edge[1]).sqrt())
            .collect()
    }

    /// Feeds mono samples and calls `emit` for every frame completed.
    pub fn push(&mut self, samples: &[f32], mut emit: impl FnMut(SpectrumFrame)) {
        let size = self.spectrum.size();
        let mut rest = samples;
        while !rest.is_empty() {
            let take = (size - self.pending.len()).min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() < size {
                break;
            }
            self.spectrum.power_at(&self.pending, 0, &mut self.power);
            self.consumed += self.options.hop as u64;
            let end = self.consumed + (size - self.options.hop) as u64;
            emit(SpectrumFrame {
                time_seconds: end as f64 / self.sample_rate,
                bands_db: self.band_levels(),
            });
            self.pending.drain(..self.options.hop);
        }
    }

    /// Drops buffered audio and restarts the clock.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.consumed = 0;
    }

    fn band_levels(&self) -> Vec<f32> {
        self.bands
            .iter()
            .map(|band| {
                let power: f32 = band
                    .iter()
                    .map(|&(bin, share)| share * self.power[bin])
                    .sum();
                // Twice the mean square, so a full-scale sine reads 0 dB.
                (10.0 * (2.0 * power).max(1e-30).log10()).max(self.options.floor_db)
            })
            .collect()
    }
}

/// Analyzes a whole mono signal and returns every frame.
pub fn spectrum_frames(
    samples: &[f32],
    sample_rate: SampleRate,
    options: SpectrumOptions,
) -> Result<Vec<SpectrumFrame>, AnalysisError> {
    let mut analyzer = SpectrumAnalyzer::new(sample_rate, options)?;
    let mut frames = Vec::with_capacity(samples.len() / options.hop.max(1) + 1);
    analyzer.push(samples, |frame| frames.push(frame));
    Ok(frames)
}
