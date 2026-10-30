//! Export: stems alongside the master, loudness normalisation and WAV
//! files.

use std::path::{Path, PathBuf};

use fourfloor_audio::{measure, write_wav_file, SampleFormat, WavError, WavSpec};
use fourfloor_core::{DeviceRegistry, SampleRate, TrackId};
use fourfloor_model::Project;

use crate::options::CHANNELS;
use crate::render::{render_pass, RenderError, RenderOptions};

/// One stem: everything one track or bus sends to the master.
#[derive(Clone, Debug, PartialEq)]
pub struct Stem {
    /// The track or bus.
    pub track: TrackId,
    /// Its display name.
    pub name: String,
    /// Interleaved stereo samples, as long as the master.
    pub samples: Vec<f32>,
}

/// A master render with its stems.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedStems {
    /// Sample rate of every buffer.
    pub sample_rate: SampleRate,
    /// Channels in every buffer: always [`CHANNELS`].
    pub channels: usize,
    /// The interleaved master output.
    pub master: Vec<f32>,
    /// One stem per track or bus that feeds the master directly, in
    /// display order. Their sum is the master.
    pub stems: Vec<Stem>,
    /// Things in the project the compiler left out or changed.
    pub warnings: Vec<String>,
}

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// The project could not be rendered.
    #[error(transparent)]
    Render(#[from] RenderError),
    /// A file could not be written.
    #[error("cannot write {}: {source}", path.display())]
    Write {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: WavError,
    },
    /// The output directory could not be created.
    #[error("cannot create {}: {source}", path.display())]
    CreateDir {
        /// The directory.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// The audio could not be measured.
    #[error("cannot measure loudness: {0}")]
    Meter(String),
}

/// Renders `project` once and returns the master together with one stem
/// per track or bus that feeds the master directly.
///
/// A stem is the track's signal after its fader, in the same pass as the
/// master: sidechains, sends and automation behave exactly as in the mix,
/// and each stem is aligned with the master (it carries the same delay
/// compensation). A track routed to a bus is part of that bus's stem, and
/// a return bus is a stem of its own, so the stems add up to the master.
pub fn render_stems(
    project: &Project,
    registry: &DeviceRegistry,
    options: &RenderOptions,
) -> Result<RenderedStems, RenderError> {
    let mut chosen: Vec<TrackId> = Vec::new();
    let pass = render_pass(project, registry, options, |compiled| {
        compiled
            .tracks
            .iter()
            .filter(|track| track.to_master)
            .map(|track| {
                chosen.push(track.track);
                let delay = compiled
                    .graph
                    .edge_delay(track.fader, compiled.master, 0)
                    .unwrap_or(0);
                (track.fader, delay)
            })
            .collect()
    })?;
    let stems = chosen
        .into_iter()
        .zip(pass.taps)
        .map(|(track, samples)| Stem {
            track,
            name: project
                .track(track)
                .map_or_else(String::new, |track| track.name.clone()),
            samples,
        })
        .collect();
    Ok(RenderedStems {
        sample_rate: pass.sample_rate,
        channels: CHANNELS,
        master: pass.master,
        stems,
        warnings: pass.warnings,
    })
}

/// What [`normalize_loudness`] did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Normalized {
    /// Integrated loudness before, in LUFS (negative infinity for silence).
    pub measured_lufs: f64,
    /// Gain that was applied, in decibels.
    pub gain_db: f64,
    /// Whether the true-peak ceiling held the gain below what the loudness
    /// target asked for.
    pub peak_limited: bool,
}

fn planes(interleaved: &[f32], channels: usize) -> Vec<Vec<f32>> {
    (0..channels)
        .map(|channel| {
            interleaved
                .iter()
                .skip(channel)
                .step_by(channels)
                .copied()
                .collect()
        })
        .collect()
}

/// Scales interleaved audio so its integrated loudness is `target_lufs`
/// (EBU R128), but never so far that its true peak passes
/// `ceiling_dbtp`. Silence is left alone. This is a plain gain change: no
/// limiting or compression.
///
/// Returns what was measured and the gain applied; apply the same gain to
/// stems to keep them summing to the master.
pub fn normalize_loudness(
    interleaved: &mut [f32],
    channels: usize,
    sample_rate: SampleRate,
    target_lufs: f64,
    ceiling_dbtp: f64,
) -> Result<Normalized, ExportError> {
    let channels = channels.max(1);
    let planar = planes(interleaved, channels);
    let views: Vec<&[f32]> = planar.iter().map(Vec::as_slice).collect();
    let loudness =
        measure(&views, sample_rate).map_err(|error| ExportError::Meter(error.to_string()))?;
    let measured = loudness.integrated_lufs;
    if !measured.is_finite() || !loudness.true_peak_dbtp.is_finite() {
        return Ok(Normalized {
            measured_lufs: f64::NEG_INFINITY,
            gain_db: 0.0,
            peak_limited: false,
        });
    }
    let wanted = target_lufs - measured;
    let headroom = ceiling_dbtp - loudness.true_peak_dbtp;
    let gain_db = wanted.min(headroom);
    let gain = 10.0f64.powf(gain_db / 20.0) as f32;
    for sample in interleaved.iter_mut() {
        *sample *= gain;
    }
    Ok(Normalized {
        measured_lufs: measured,
        gain_db,
        peak_limited: headroom < wanted,
    })
}

/// A file-system-safe version of a track name.
fn file_stem(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    if cleaned.is_empty() {
        "track".to_owned()
    } else {
        cleaned
    }
}

impl RenderedStems {
    /// Applies a gain in decibels to the master and every stem.
    pub fn apply_gain_db(&mut self, gain_db: f64) {
        let gain = 10.0f64.powf(gain_db / 20.0) as f32;
        let buffers = std::iter::once(&mut self.master)
            .chain(self.stems.iter_mut().map(|stem| &mut stem.samples));
        for buffer in buffers {
            for sample in buffer.iter_mut() {
                *sample *= gain;
            }
        }
    }

    /// Normalises the master to `target_lufs` (see [`normalize_loudness`])
    /// and applies the same gain to every stem.
    pub fn normalize(
        &mut self,
        target_lufs: f64,
        ceiling_dbtp: f64,
    ) -> Result<Normalized, ExportError> {
        let mut master = std::mem::take(&mut self.master);
        let result = normalize_loudness(
            &mut master,
            self.channels,
            self.sample_rate,
            target_lufs,
            ceiling_dbtp,
        );
        self.master = master;
        let normalized = result?;
        let gain = 10.0f64.powf(normalized.gain_db / 20.0) as f32;
        for stem in &mut self.stems {
            for sample in stem.samples.iter_mut() {
                *sample *= gain;
            }
        }
        Ok(normalized)
    }

    /// Writes `master.wav` and one `NN-name.wav` per stem into `dir`
    /// (created if missing). Returns the paths, master first.
    pub fn write_wav(&self, dir: &Path, format: SampleFormat) -> Result<Vec<PathBuf>, ExportError> {
        std::fs::create_dir_all(dir).map_err(|source| ExportError::CreateDir {
            path: dir.to_path_buf(),
            source,
        })?;
        let spec = WavSpec {
            sample_rate: self.sample_rate,
            channels: self.channels as u16,
            format,
        };
        let named = std::iter::once(("master.wav".to_owned(), &self.master)).chain(
            self.stems.iter().enumerate().map(|(index, stem)| {
                (
                    format!("{:02}-{}.wav", index + 1, file_stem(&stem.name)),
                    &stem.samples,
                )
            }),
        );
        named
            .map(|(name, samples)| {
                let path = dir.join(name);
                write_wav_file(&path, spec, samples)
                    .map(|()| path.clone())
                    .map_err(|source| ExportError::Write { path, source })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_made_safe() {
        assert_eq!(file_stem("Kick 909"), "Kick_909");
        assert_eq!(file_stem("../etc/passwd"), "___etc_passwd");
        assert_eq!(file_stem("  "), "track");
        assert_eq!(file_stem(&"x".repeat(100)).len(), 48);
    }

    #[test]
    fn silence_is_not_normalised() {
        let mut silence = vec![0.0f32; 96_000];
        let result =
            normalize_loudness(&mut silence, 2, SampleRate::HZ_48000, -14.0, -1.0).unwrap();
        assert_eq!(result.gain_db, 0.0);
        assert!(silence.iter().all(|s| *s == 0.0));
    }
}
