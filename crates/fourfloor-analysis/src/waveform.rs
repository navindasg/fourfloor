//! Waveform overviews for drawing: minimum, maximum and RMS of the signal
//! in blocks, kept at several zoom levels so that any zoom can be drawn
//! from about one block per pixel.

use fourfloor_audio::AudioBuffer;
use fourfloor_core::SampleRate;

use crate::error::AnalysisError;

/// The extent of the signal over one block of samples.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Peak {
    /// Lowest sample in the block.
    pub min: f32,
    /// Highest sample in the block.
    pub max: f32,
    /// RMS of the block.
    pub rms: f32,
}

/// One zoom level: a peak per `samples_per_peak` samples.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaveformLevel {
    /// Samples summarized by each peak. The last peak may cover fewer.
    pub samples_per_peak: usize,
    /// The peaks, in time order.
    pub peaks: Vec<Peak>,
}

/// Settings for [`WaveformOverview::build`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveformOptions {
    /// Samples per peak at the finest level. Default 64.
    pub base_samples_per_peak: usize,
    /// How much coarser each level is than the one before; at least 2.
    /// Default 4.
    pub factor: usize,
    /// Levels stop once one has this few peaks or fewer. Default 512.
    pub min_peaks: usize,
}

impl Default for WaveformOptions {
    fn default() -> Self {
        Self {
            base_samples_per_peak: 64,
            factor: 4,
            min_peaks: 512,
        }
    }
}

/// A multi-resolution overview of one channel.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WaveformOverview {
    /// Sample rate of the audio in Hz.
    pub sample_rate: u32,
    /// Length of the audio in samples.
    pub frames: usize,
    /// Zoom levels, finest first. Never empty.
    pub levels: Vec<WaveformLevel>,
}

impl WaveformOverview {
    /// Builds the overview of one channel. An empty signal gives one level
    /// with no peaks.
    pub fn build(
        samples: &[f32],
        sample_rate: SampleRate,
        options: &WaveformOptions,
    ) -> Result<Self, AnalysisError> {
        if options.base_samples_per_peak == 0 {
            return Err(AnalysisError::InvalidOption {
                name: "base_samples_per_peak",
                reason: "must be at least 1",
            });
        }
        if options.factor < 2 {
            return Err(AnalysisError::InvalidOption {
                name: "factor",
                reason: "must be at least 2",
            });
        }
        let base = options.base_samples_per_peak;
        let mut levels = vec![WaveformLevel {
            samples_per_peak: base,
            peaks: samples.chunks(base).map(summarize).collect(),
        }];
        loop {
            let finer = &levels[levels.len() - 1];
            if finer.peaks.len() <= options.min_peaks.max(1) {
                break;
            }
            let Some(samples_per_peak) = finer.samples_per_peak.checked_mul(options.factor) else {
                break;
            };
            // Every peak of the finer level covers the same number of
            // samples except the last, so weigh each by what it covers.
            let per = finer.samples_per_peak;
            let peaks = finer
                .peaks
                .chunks(options.factor)
                .enumerate()
                .map(|(chunk, peaks)| {
                    let start = chunk * samples_per_peak;
                    merge(peaks.iter().enumerate().map(|(index, &peak)| {
                        let from = start + index * per;
                        (peak, (from + per).min(samples.len()) - from)
                    }))
                })
                .collect();
            levels.push(WaveformLevel {
                samples_per_peak,
                peaks,
            });
        }
        Ok(Self {
            sample_rate: sample_rate.hz(),
            frames: samples.len(),
            levels,
        })
    }

    /// Builds one overview per channel of a buffer, with default options.
    pub fn of_buffer(buffer: &AudioBuffer) -> Result<Vec<Self>, AnalysisError> {
        buffer
            .channels
            .iter()
            .map(|channel| Self::build(channel, buffer.sample_rate, &WaveformOptions::default()))
            .collect()
    }

    /// The coarsest level that still has at least one peak per pixel when
    /// each pixel shows `samples_per_pixel` samples; the finest level when
    /// the view is zoomed in further than that.
    pub fn level_for(&self, samples_per_pixel: f64) -> &WaveformLevel {
        self.levels
            .iter()
            .rev()
            .find(|level| level.samples_per_peak as f64 <= samples_per_pixel)
            .unwrap_or(&self.levels[0])
    }

    /// One peak per pixel column for the sample range `start..end` drawn
    /// `columns` pixels wide. Each column is the extent of the samples it
    /// covers, to within one block of the finest level at each edge.
    /// Columns past the end of the audio are zero.
    pub fn columns(&self, start: usize, end: usize, columns: usize) -> Vec<Peak> {
        if columns == 0 || end <= start {
            return Vec::new();
        }
        let per_column = (end - start) as f64 / columns as f64;
        let edge = |column: usize| start + (column as f64 * per_column).round() as usize;
        (0..columns)
            .map(|column| self.range(edge(column), edge(column + 1)))
            .collect()
    }

    /// The extent of the samples `from..to`, widened to whole blocks of the
    /// finest level (and to one block when the range is empty). Coarser
    /// levels are used wherever their blocks fit inside the range, so the
    /// cost does not grow with the length of the range. Zero past the end
    /// of the audio.
    pub fn range(&self, from: usize, to: usize) -> Peak {
        let Some(finest) = self.levels.first() else {
            return Peak::default();
        };
        let base = finest.samples_per_peak.max(1);
        let first = from / base;
        let last = to.div_ceil(base).max(first + 1).min(finest.peaks.len());
        let mut total = Extent::default();
        let mut block = first;
        while block < last {
            // The coarsest level with a peak that starts on this block and
            // ends inside the range.
            let mut chosen = (finest, 1usize);
            for level in &self.levels[1..] {
                let span = level.samples_per_peak / base;
                if span == 0 || !block.is_multiple_of(span) || block + span > last {
                    break;
                }
                chosen = (level, span);
            }
            let (level, span) = chosen;
            let Some(&peak) = level.peaks.get(block / span) else {
                break;
            };
            let begin = block * base;
            let covered = (begin + level.samples_per_peak)
                .min(self.frames)
                .saturating_sub(begin);
            total.add(peak, covered.max(1));
            block += span;
        }
        total.peak()
    }
}

fn summarize(block: &[f32]) -> Peak {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    let mut square = 0.0f64;
    for &sample in block {
        min = min.min(sample);
        max = max.max(sample);
        square += f64::from(sample) * f64::from(sample);
    }
    if block.is_empty() {
        return Peak::default();
    }
    Peak {
        min,
        max,
        rms: (square / block.len() as f64).sqrt() as f32,
    }
}

/// Combines peaks, each with the number of samples it covers.
fn merge(peaks: impl Iterator<Item = (Peak, usize)>) -> Peak {
    let mut total = Extent::default();
    for (peak, covered) in peaks {
        total.add(peak, covered);
    }
    total.peak()
}

/// A running combination of peaks.
#[derive(Default)]
struct Extent {
    /// The first peak added, returned untouched when it is the only one.
    only: Option<Peak>,
    min: f32,
    max: f32,
    square: f64,
    samples: usize,
    count: usize,
}

impl Extent {
    fn add(&mut self, peak: Peak, covered: usize) {
        if self.count == 0 {
            self.only = Some(peak);
            self.min = peak.min;
            self.max = peak.max;
        } else {
            self.min = self.min.min(peak.min);
            self.max = self.max.max(peak.max);
        }
        self.square += f64::from(peak.rms) * f64::from(peak.rms) * covered as f64;
        self.samples += covered;
        self.count += 1;
    }

    fn peak(&self) -> Peak {
        match (self.count, self.only) {
            (0, _) => Peak::default(),
            (1, Some(peak)) => peak,
            _ if self.samples == 0 => Peak::default(),
            _ => Peak {
                min: self.min,
                max: self.max,
                rms: (self.square / self.samples as f64).sqrt() as f32,
            },
        }
    }
}
