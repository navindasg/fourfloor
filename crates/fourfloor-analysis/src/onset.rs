//! Onset (transient) detection by spectral flux.
//!
//! The signal is cut into short overlapping frames. For each frame the
//! log-compressed magnitude spectrum is compared with the previous frame's
//! and the increases are summed: that is the spectral flux, which jumps
//! whenever new energy appears. Peaks of the flux above an adaptive
//! threshold are onsets, and each one is then placed to about a
//! millisecond by finding the steepest rise of energy in the time domain.

use fourfloor_core::SampleRate;

use crate::stft::{fft_size_for, for_each_frame};

/// Length of the analysis window in seconds (1024 samples at 48 kHz).
const WINDOW_SECONDS: f64 = 0.0213;
/// Compression applied to magnitudes before differencing, so that quiet
/// transients are not swamped by loud ones.
const LOG_COMPRESSION: f32 = 1000.0;
/// Upper edge of the "low" band that follows kicks and bass, in Hz.
const LOW_BAND_HZ: f64 = 200.0;
/// The low band is measured over a window this many times longer than
/// the full band's (85 ms at 48 kHz). That is long enough to average out
/// the interference between the partials of a sustained bass note or
/// chord, which a short window sees as a ripple of false onsets.
const LOW_WINDOW_FACTOR: usize = 4;
/// Smallest low-band flux that can be an onset: a rise in the level of
/// the low band, from one frame to the next, of this fraction of the
/// signal's peak. Below this the low band holds only leakage, noise and
/// slow beating.
const LOW_MIN_FLUX: f32 = 0.004;
/// A low-band flux peak must reach this fraction of the largest one.
const LOW_FLOOR: f32 = 0.25;
/// Length of each side of the energy comparison used to place an onset.
const REFINE_SECONDS: f64 = 0.002;

/// Spectral flux of a signal, one value per analysis frame.
#[derive(Clone, Debug, PartialEq)]
pub struct OnsetEnvelope {
    /// Sample rate of the analyzed audio in Hz.
    pub sample_rate: u32,
    /// Samples between consecutive frames. Frame `i` is centred on sample
    /// `i * hop`.
    pub hop: usize,
    /// Length of the analysis window in samples.
    pub window: usize,
    /// Flux over the whole spectrum.
    pub flux: Vec<f32>,
    /// Flux below 200 Hz only: kicks and bass notes. This is the rise in
    /// the level (RMS) of that band from one frame to the next, as a
    /// fraction of the peak of the signal, so it is not on the same scale
    /// as `flux`. It is measured over a window four times as long.
    pub low_flux: Vec<f32>,
}

impl OnsetEnvelope {
    /// Frames per second.
    pub fn frame_rate(&self) -> f64 {
        f64::from(self.sample_rate) / self.hop as f64
    }

    /// Number of frames.
    pub fn len(&self) -> usize {
        self.flux.len()
    }

    /// Whether there are no frames.
    pub fn is_empty(&self) -> bool {
        self.flux.is_empty()
    }

    /// The time of the centre of frame `frame`, in seconds.
    pub fn frame_time(&self, frame: f64) -> f64 {
        frame * self.hop as f64 / f64::from(self.sample_rate)
    }

    /// Full-band and low-band flux, each scaled to a peak of one and added
    /// with `low_weight` on the low band. Raising `low_weight` makes the
    /// curve favour kicks over hats. All zeros for silence.
    pub fn combined(&self, low_weight: f32) -> Vec<f32> {
        let full_peak = peak(&self.flux);
        let low_peak = usable_low_peak(&self.low_flux);
        self.flux
            .iter()
            .zip(&self.low_flux)
            .map(|(&full, &low)| {
                let full = if full_peak > 0.0 {
                    full / full_peak
                } else {
                    0.0
                };
                let low = if low_peak > 0.0 { low / low_peak } else { 0.0 };
                full + low_weight * low
            })
            .collect()
    }
}

fn peak(values: &[f32]) -> f32 {
    values.iter().copied().fold(0.0, f32::max)
}

/// The largest low-band flux, or zero when the low band has no onsets of
/// its own and scaling by its peak would only magnify ripple.
fn usable_low_peak(low_flux: &[f32]) -> f32 {
    let low_peak = peak(low_flux);
    if low_peak >= LOW_MIN_FLUX {
        low_peak
    } else {
        0.0
    }
}

/// Computes the spectral flux of a mono signal.
pub fn onset_envelope(samples: &[f32], sample_rate: SampleRate) -> OnsetEnvelope {
    let rate = sample_rate.as_f64();
    let window = fft_size_for(WINDOW_SECONDS, rate);
    let hop = window / 4;

    // Magnitudes are taken relative to the peak of the signal, so that the
    // envelope does not change with the level of the audio.
    let level = samples.iter().fold(0.0f32, |top, &s| top.max(s.abs()));
    let scale = if level > 0.0 { 1.0 / level } else { 0.0 };

    let mut previous = vec![0.0f32; window / 2 + 1];
    let mut flux = Vec::with_capacity(samples.len() / hop + 1);
    for_each_frame(samples, window, hop, |_, power| {
        let mut full = 0.0f32;
        // Bin 0 is DC and carries no onset information.
        for (&value, last) in power.iter().zip(previous.iter_mut()).skip(1) {
            let magnitude = (1.0 + LOG_COMPRESSION * value.sqrt() * scale).ln();
            full += (magnitude - *last).max(0.0);
            *last = magnitude;
        }
        flux.push(full);
    });

    // The same frame centres, seen through the longer window.
    let low_window = window * LOW_WINDOW_FACTOR;
    let low_bins = ((LOW_BAND_HZ * low_window as f64 / rate).round() as usize).max(1);
    let mut previous_level = 0.0f32;
    let mut low_flux = Vec::with_capacity(flux.len());
    for_each_frame(samples, low_window, hop, |_, power| {
        let band: f32 = power.iter().skip(1).take(low_bins).sum();
        let level = band.sqrt() * scale;
        low_flux.push((level - previous_level).max(0.0));
        previous_level = level;
    });
    OnsetEnvelope {
        sample_rate: sample_rate.hz(),
        hop,
        window,
        flux,
        low_flux,
    }
}

/// Settings for [`detect_onsets`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnsetOptions {
    /// How far a flux peak must stand above the local average to count:
    /// the threshold is `sensitivity` times the mean flux of the
    /// surrounding 200 ms. Lower finds more onsets. Default 1.5.
    pub sensitivity: f32,
    /// A peak must also reach this fraction of the loudest flux peak in the
    /// signal. Default 0.08.
    pub floor: f32,
    /// Shortest time between two onsets, in seconds: a flux peak counts
    /// only if nothing within this distance is stronger. Default 0.05.
    pub min_gap_seconds: f64,
}

impl Default for OnsetOptions {
    fn default() -> Self {
        Self {
            sensitivity: 1.5,
            floor: 0.08,
            min_gap_seconds: 0.05,
        }
    }
}

/// One detected onset.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Onset {
    /// Sample index where the sound starts.
    pub sample: usize,
    /// The same position in seconds.
    pub time_seconds: f64,
    /// Flux at the onset relative to the strongest onset, 0 to 1.
    pub strength: f32,
    /// Low-band (below 200 Hz) flux relative to the strongest low-band
    /// onset, 0 to 1. High for kicks, near zero for hats, and zero
    /// throughout when the audio has nothing of its own down there.
    pub low_strength: f32,
}

/// Finds the onsets of a mono signal, in time order.
pub fn detect_onsets(
    samples: &[f32],
    sample_rate: SampleRate,
    options: &OnsetOptions,
) -> Vec<Onset> {
    let envelope = onset_envelope(samples, sample_rate);
    onsets_from_envelope(samples, &envelope, options)
}

/// Finds onsets using a flux envelope that was already computed for
/// `samples` with [`onset_envelope`].
pub fn onsets_from_envelope(
    samples: &[f32],
    envelope: &OnsetEnvelope,
    options: &OnsetOptions,
) -> Vec<Onset> {
    let flux = &envelope.flux;
    let low_flux = &envelope.low_flux;
    let full_peak = peak(flux);
    if full_peak <= 0.0 {
        return Vec::new();
    }
    let low_peak = usable_low_peak(low_flux);
    let frame_rate = envelope.frame_rate();
    let picker = PeakPicker {
        reach: ((0.1 * frame_rate).round() as usize).max(1),
        neighbours: ((options.min_gap_seconds.max(0.0) * frame_rate).round() as usize).max(1),
        sensitivity: options.sensitivity,
    };
    // Full-band peaks find everything with a broadband attack. Low-band
    // peaks add the kicks and bass notes that land on a busy mix and
    // barely move the full-band flux.
    let mut peaks = picker.pick(flux, options.floor.max(0.0) * full_peak);
    if low_peak > 0.0 {
        peaks.extend(picker.pick(low_flux, (LOW_FLOOR * low_peak).max(LOW_MIN_FLUX)));
        peaks.sort_unstable();
        peaks.dedup();
    }
    // The two curves can peak a frame or two apart for the same sound, so
    // read each strength at its largest nearby value.
    let around = |curve: &[f32], frame: usize| {
        let from = frame.saturating_sub(2);
        let to = (frame + 3).min(curve.len());
        peak(&curve[from..to])
    };

    let energy = energy_prefix(samples);
    let sample_rate = f64::from(envelope.sample_rate);
    let min_gap = (options.min_gap_seconds.max(0.0) * sample_rate) as usize;
    let mut onsets: Vec<Onset> = Vec::with_capacity(peaks.len());
    for frame in peaks {
        let sample = refine(&energy, frame * envelope.hop, envelope.window, sample_rate);
        let onset = Onset {
            sample,
            time_seconds: sample as f64 / sample_rate,
            strength: around(flux, frame) / full_peak,
            low_strength: if low_peak > 0.0 {
                (around(low_flux, frame) / low_peak).min(1.0)
            } else {
                0.0
            },
        };
        match onsets.last_mut() {
            Some(last) if sample < last.sample + min_gap => {
                // One sound seen twice: keep the placement of the stronger
                // sighting and the larger of each strength.
                let stronger =
                    onset.strength + onset.low_strength > last.strength + last.low_strength;
                let merged = Onset {
                    strength: last.strength.max(onset.strength),
                    low_strength: last.low_strength.max(onset.low_strength),
                    ..if stronger { onset } else { *last }
                };
                *last = merged;
            }
            _ => onsets.push(onset),
        }
    }
    onsets
}

/// Picks the peaks of a flux curve that stand out from their surroundings.
struct PeakPicker {
    /// Frames on each side that make up the local average.
    reach: usize,
    /// A peak must be the largest value within this many frames.
    neighbours: usize,
    /// A peak must reach this many times the local average.
    sensitivity: f32,
}

impl PeakPicker {
    /// The frames of the peaks of `curve` that reach `floor`.
    fn pick(&self, curve: &[f32], floor: f32) -> Vec<usize> {
        let mut prefix = Vec::with_capacity(curve.len() + 1);
        let mut total = 0.0f64;
        prefix.push(total);
        for &value in curve {
            total += f64::from(value);
            prefix.push(total);
        }
        let mut peaks = Vec::new();
        for (frame, &value) in curve.iter().enumerate() {
            if value < floor || value <= 0.0 {
                continue;
            }
            let from = frame.saturating_sub(self.neighbours);
            let to = (frame + self.neighbours + 1).min(curve.len());
            // Strictly greater than what came before and at least what
            // follows, so a flat top yields exactly one peak.
            let is_peak = curve[from..frame].iter().all(|&other| other < value)
                && curve[frame + 1..to].iter().all(|&other| other <= value);
            if !is_peak {
                continue;
            }
            let from = frame.saturating_sub(self.reach);
            let to = (frame + self.reach + 1).min(curve.len());
            let mean = ((prefix[to] - prefix[from]) / (to - from) as f64) as f32;
            if value >= self.sensitivity * mean {
                peaks.push(frame);
            }
        }
        peaks
    }
}

/// Running sum of squared samples: `prefix[i]` is the energy of
/// `samples[..i]`.
fn energy_prefix(samples: &[f32]) -> Vec<f64> {
    let mut prefix = Vec::with_capacity(samples.len() + 1);
    let mut total = 0.0f64;
    prefix.push(total);
    for &sample in samples {
        total += f64::from(sample) * f64::from(sample);
        prefix.push(total);
    }
    prefix
}

/// Places an onset near `centre` (the centre of the frame whose flux
/// peaked) at the sample where the energy of the next two milliseconds
/// most exceeds the energy of the previous two.
fn refine(energy: &[f64], centre: usize, window: usize, sample_rate: f64) -> usize {
    let len = energy.len() - 1;
    if len == 0 {
        return 0;
    }
    let span = ((REFINE_SECONDS * sample_rate) as usize).max(4);
    let between = |from: usize, to: usize| energy[to.min(len)] - energy[from.min(len)];
    let first = centre.saturating_sub(window / 2);
    let last = (centre + window / 2).min(len - 1);
    let mut best = centre.min(len - 1);
    let mut best_rise = 0.0f64;
    for sample in first..=last {
        let rise = between(sample, sample + span) - between(sample.saturating_sub(span), sample);
        // `>=` keeps the last sample of a flat top: for a single-sample
        // click every position up to the click sees the same rise, and the
        // click itself is the last of them.
        if rise > 0.0 && rise >= best_rise {
            best_rise = rise;
            best = sample;
        }
    }
    best
}
