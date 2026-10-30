//! Audio clips: decodes pool files, converts them to the graph's sample
//! rate, warps them to the project tempo and turns clips into player
//! regions. Everything here runs off the audio thread, before the graph
//! exists ("offline pre-stretch").

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use fourfloor_audio::{decode_file, resample, AudioBuffer};
use fourfloor_core::{SampleRate, Tempo};
use fourfloor_engine::devices::{AudioRegion, AudioSource};
use fourfloor_model::{AudioPool, AudioRef, Clip, ClipContent, Warp};

use crate::routing::db_to_linear;

/// Gain at or below this many decibels is silence, as on the fader.
const SILENT_DB: f32 = -96.0;

/// A pool file prepared for playback.
#[derive(Clone, Debug)]
struct Prepared {
    source: Arc<AudioSource>,
    /// Prepared frames per frame of the file: sample-rate change times
    /// warp. Scales a clip's `source_offset_frames`.
    frame_scale: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    source: AudioRef,
    /// `(source bpm bits, key lock)` of the warp, if any.
    warp: Option<(u64, bool)>,
}

/// Loads and caches the audio a project's clips play.
pub(crate) struct AudioLoader {
    pool: Option<AudioPool>,
    sample_rate: SampleRate,
    tempo: Tempo,
    cache: HashMap<Key, Result<Prepared, String>>,
    /// Notes about quality fallbacks, each reported once.
    pub(crate) notes: Vec<String>,
}

/// The length factor of a warp: prepared length over source length.
pub fn warp_ratio(warp: &Warp, tempo: Tempo) -> f64 {
    warp.source_bpm / tempo.bpm()
}

/// Changes the length of one channel by `factor` by resampling, so the
/// pitch moves with the speed (like a record deck).
fn varispeed(input: &[f32], factor: f64) -> Vec<f32> {
    if (factor - 1.0).abs() < 1e-9 || input.is_empty() {
        return input.to_vec();
    }
    // The resampler takes a pair of integer rates; pick a pair whose ratio
    // is `factor` to better than one part in 100 000.
    let (from, to) = if factor >= 1.0 {
        (200_000.0, 200_000.0 * factor)
    } else {
        (200_000.0 / factor, 200_000.0)
    };
    let rate = |hz: f64| SampleRate::new((hz.round() as u32).max(1));
    match (rate(from), rate(to)) {
        (Ok(from), Ok(to)) => resample(input, from, to),
        _ => input.to_vec(),
    }
}

#[cfg(feature = "stretch")]
fn key_locked(input: &[f32], sample_rate: SampleRate, ratio: f64) -> Result<Vec<f32>, String> {
    fourfloor_audio::stretch::stretch_mono(input, sample_rate, ratio)
        .map_err(|error| error.to_string())
}

#[cfg(not(feature = "stretch"))]
fn key_locked(_input: &[f32], _sample_rate: SampleRate, _ratio: f64) -> Result<Vec<f32>, String> {
    Err("this build has no time-stretch (the `stretch` feature is off)".to_owned())
}

impl AudioLoader {
    pub(crate) fn new(project_dir: Option<PathBuf>, sample_rate: SampleRate, tempo: Tempo) -> Self {
        Self {
            pool: project_dir.map(|dir| AudioPool::new(&dir)),
            sample_rate,
            tempo,
            cache: HashMap::new(),
            notes: Vec::new(),
        }
    }

    fn decode(&self, source: &AudioRef) -> Result<AudioBuffer, String> {
        let pool = self
            .pool
            .as_ref()
            .ok_or("no project folder was given, so its audio cannot be loaded")?;
        let path = pool.path_for(source).map_err(|error| error.to_string())?;
        decode_file(&path).map_err(|error| format!("cannot decode {}: {error}", source.file_name()))
    }

    fn prepare(&mut self, source: &AudioRef, warp: Option<&Warp>) -> Result<Prepared, String> {
        let decoded = self.decode(source)?;
        let rate_factor = self.sample_rate.as_f64() / decoded.sample_rate.as_f64();
        let ratio = warp.map_or(1.0, |warp| warp_ratio(warp, self.tempo));
        let mut fallback = None;
        let channels: Vec<Vec<f32>> = decoded
            .channels
            .iter()
            .map(|channel| {
                let stretched = match warp {
                    Some(warp) if warp.key_lock && (ratio - 1.0).abs() > 1e-9 => {
                        let at_rate = varispeed(channel, rate_factor);
                        match key_locked(&at_rate, self.sample_rate, ratio) {
                            Ok(stretched) => Some(stretched),
                            Err(reason) => {
                                fallback = Some(reason);
                                None
                            }
                        }
                    }
                    _ => None,
                };
                stretched.unwrap_or_else(|| varispeed(channel, rate_factor * ratio))
            })
            .collect();
        if let Some(reason) = fallback {
            self.notes.push(format!(
                "{} is warped without key lock, so its pitch follows the tempo: {reason}",
                source.file_name()
            ));
        }
        Ok(Prepared {
            source: Arc::new(AudioSource::new(channels)),
            frame_scale: rate_factor * ratio,
        })
    }

    /// The player region for an audio clip, or why it cannot play.
    /// `None` for a MIDI clip.
    pub(crate) fn region(&mut self, clip: &Clip) -> Option<Result<AudioRegion, String>> {
        let ClipContent::Audio {
            source,
            source_offset_frames,
            gain_db,
            fade_in,
            fade_out,
            warp,
        } = &clip.content
        else {
            return None;
        };
        let key = Key {
            source: source.clone(),
            warp: warp.map(|warp| (warp.source_bpm.to_bits(), warp.key_lock)),
        };
        let prepared = match self.cache.get(&key) {
            Some(prepared) => prepared.clone(),
            None => {
                let prepared = self.prepare(source, warp.as_ref());
                self.cache.insert(key, prepared.clone());
                prepared
            }
        };
        Some(prepared.map(|prepared| AudioRegion {
            source_offset_frames: (*source_offset_frames as f64 * prepared.frame_scale).round()
                as u64,
            content_offset: clip.offset,
            loop_length: clip.loop_length,
            gain: if *gain_db <= SILENT_DB {
                0.0
            } else {
                db_to_linear(*gain_db)
            },
            fade_in: *fade_in,
            fade_out: *fade_out,
            ..AudioRegion::new(prepared.source, clip.start, clip.length)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varispeed_changes_the_length_by_the_factor() {
        let input: Vec<f32> = (0..4_800).map(|n| (n as f32 * 0.05).sin()).collect();
        assert_eq!(varispeed(&input, 1.0), input);
        let longer = varispeed(&input, 1.25);
        assert!((longer.len() as i64 - 6_000).abs() <= 1, "{}", longer.len());
        let shorter = varispeed(&input, 0.5);
        assert!(
            (shorter.len() as i64 - 2_400).abs() <= 1,
            "{}",
            shorter.len()
        );
        assert!(varispeed(&[], 2.0).is_empty());
    }

    #[test]
    fn a_warp_ratio_is_source_tempo_over_project_tempo() {
        let warp = Warp {
            source_bpm: 100.0,
            key_lock: true,
        };
        let ratio = warp_ratio(&warp, Tempo::new(125.0).unwrap());
        assert!((ratio - 0.8).abs() < 1e-12, "faster project, shorter audio");
    }
}
