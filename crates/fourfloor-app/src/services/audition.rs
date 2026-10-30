//! Rendering a library file to audition it at the project's tempo and key.
//!
//! The library plans the preview ([`PreviewStream`]); this module makes
//! the audio. Original and varispeed plans are resampled by the library
//! itself. Key-locked plans (tempo and pitch changed separately) need a
//! time stretcher, which the library does not have, so they are rendered
//! here in two steps: a varispeed by the pitch ratio (which also speeds the
//! sound up by that ratio), then the DJ deck's key-locked playback at the
//! remaining speed, offline through a [`DjRenderer`].

use fourfloor_audio::{resample, AudioBuffer};
use fourfloor_core::{DeviceRegistry, SampleRate};
use fourfloor_dj::{DeckTrack, DjCommand, DjRenderer, SessionConfig};
use fourfloor_library::{PreviewMode, PreviewStream};

use super::callback::ReferenceAudio;

/// Longest audition rendered, in seconds.
pub const MAX_AUDITION_SECONDS: f64 = 30.0;

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn interleave(buffer: &AudioBuffer) -> ReferenceAudio {
    ReferenceAudio {
        samples: buffer.to_stereo().interleaved(),
    }
}

/// Renders `plan` from the decoded `source` to stereo at the plan's
/// output rate, capped at [`MAX_AUDITION_SECONDS`].
pub fn render_audition(
    plan: &PreviewStream,
    source: &AudioBuffer,
) -> Result<ReferenceAudio, String> {
    let rate = SampleRate::new(plan.output_rate).map_err(text)?;
    let max_frames = (MAX_AUDITION_SECONDS * rate.as_f64()) as u64;
    let mut plan = plan.clone();
    plan.output_frames = plan.output_frames.min(max_frames);
    let plain = plan.mode != PreviewMode::KeyLocked
        || ((plan.speed - 1.0).abs() < 1e-9 && plan.pitch_semitones.abs() < 1e-9);
    if plain {
        let mut varispeed = plan.clone();
        if varispeed.mode == PreviewMode::KeyLocked {
            varispeed.mode = PreviewMode::Original;
        }
        let audio = varispeed.render_varispeed(source).map_err(text)?;
        return Ok(interleave(&audio));
    }
    key_locked(&plan, source, rate)
}

fn key_locked(
    plan: &PreviewStream,
    source: &AudioBuffer,
    rate: SampleRate,
) -> Result<ReferenceAudio, String> {
    // Step 1: varispeed by the pitch ratio. Reading the file as if it had
    // been recorded at `rate * ratio` and converting to `rate` raises
    // pitch and speed together by `ratio`.
    let ratio = plan.pitch_ratio.clamp(0.5, 2.0);
    let pretend = SampleRate::new((source.sample_rate.as_f64() * ratio).round().max(1.0) as u32)
        .map_err(text)?;
    let channels: Vec<Vec<f32>> = source
        .channels
        .iter()
        .map(|channel| resample(channel, pretend, rate))
        .collect();
    let shifted = AudioBuffer::new(rate, channels).to_stereo();
    // Step 2: key-locked playback at the speed still missing.
    let rest = plan.speed / ratio;
    let percent = ((rest - 1.0) * 100.0).clamp(-50.0, 100.0);
    let config = SessionConfig::new(2, rate, 512);
    let mut renderer = DjRenderer::new(&config, &DeviceRegistry::new()).map_err(text)?;
    renderer
        .load(0, DeckTrack::from_buffer(&shifted).map_err(text)?)
        .map_err(text)?;
    renderer
        .apply_all(&[
            DjCommand::SetKeyLock { deck: 0, on: true },
            DjCommand::SetPitch { deck: 0, percent },
            DjCommand::SetCrossfader { position: 0.0 },
            DjCommand::Play { deck: 0 },
        ])
        .map_err(text)?;
    let frames = usize::try_from(plan.output_frames).unwrap_or(0);
    let output = renderer.render(frames).map_err(text)?;
    // The deck's own output, before the mixer's crossfader and fader.
    let [left, right] = output
        .decks
        .into_iter()
        .next()
        .unwrap_or_else(|| [Vec::new(), Vec::new()]);
    let samples = left
        .iter()
        .zip(&right)
        .flat_map(|(l, r)| [*l, *r])
        .collect();
    Ok(ReferenceAudio { samples })
}
