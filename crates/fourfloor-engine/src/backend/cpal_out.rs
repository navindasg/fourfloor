//! Realtime output through cpal (CoreAudio, WASAPI, ALSA, ...).
//!
//! [`start_default_output`] moves an [`AudioProcessor`] into the default
//! output device's callback. The device must run at the processor's sample
//! rate: query it first with [`default_output_format`] and build the engine
//! to match.

use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig};
use fourfloor_core::SampleRate;

use crate::engine::AudioProcessor;

/// Why the output could not be opened.
#[derive(Debug)]
pub enum BackendError {
    /// The host has no default output device.
    NoOutputDevice,
    /// The device reported a sample rate of zero.
    InvalidSampleRate,
    /// The device uses a sample format this backend does not convert to.
    UnsupportedSampleFormat(SampleFormat),
    /// cpal reported an error.
    Cpal(cpal::Error),
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOutputDevice => write!(f, "no default audio output device"),
            Self::InvalidSampleRate => write!(f, "the output device reported an invalid rate"),
            Self::UnsupportedSampleFormat(format) => {
                write!(f, "unsupported output sample format {format}")
            }
            Self::Cpal(error) => write!(f, "audio backend error: {error}"),
        }
    }
}

impl std::error::Error for BackendError {}

impl From<cpal::Error> for BackendError {
    fn from(error: cpal::Error) -> Self {
        Self::Cpal(error)
    }
}

/// What the default output device runs at by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputFormat {
    /// The device's default sample rate.
    pub sample_rate: SampleRate,
    /// The device's default channel count.
    pub channels: usize,
}

/// Asks the default output device for its default format, without opening a
/// stream.
pub fn default_output_format() -> Result<OutputFormat, BackendError> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or(BackendError::NoOutputDevice)?;
    let config = device.default_output_config()?;
    Ok(OutputFormat {
        sample_rate: SampleRate::new(config.sample_rate())
            .map_err(|_| BackendError::InvalidSampleRate)?,
        channels: usize::from(config.channels()),
    })
}

/// A running output stream. Audio stops when this is dropped. On some
/// platforms the stream cannot move between threads, so keep it on the
/// thread that created it.
pub struct OutputStream {
    stream: cpal::Stream,
    errors: Arc<AtomicU32>,
    channels: usize,
}

impl OutputStream {
    /// Number of stream errors (xruns, device changes, ...) reported so far.
    pub fn error_count(&self) -> u32 {
        self.errors.load(Ordering::Relaxed)
    }

    /// Channels the device stream was opened with.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Pauses the device stream.
    pub fn pause(&self) -> Result<(), BackendError> {
        Ok(self.stream.pause()?)
    }

    /// Resumes the device stream.
    pub fn resume(&self) -> Result<(), BackendError> {
        Ok(self.stream.play()?)
    }
}

/// Opens the default output device at the processor's sample rate, with the
/// device's default channel count and buffer size, and starts playing.
pub fn start_default_output(processor: AudioProcessor) -> Result<OutputStream, BackendError> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or(BackendError::NoOutputDevice)?;
    let default = device.default_output_config()?;
    let config = StreamConfig {
        channels: default.channels(),
        sample_rate: processor.config().sample_rate.hz(),
        buffer_size: BufferSize::Default,
    };
    let errors = Arc::new(AtomicU32::new(0));
    let stream = match default.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, processor, &errors),
        SampleFormat::I16 => build::<i16>(&device, config, processor, &errors),
        SampleFormat::U16 => build::<u16>(&device, config, processor, &errors),
        SampleFormat::I32 => build::<i32>(&device, config, processor, &errors),
        other => Err(BackendError::UnsupportedSampleFormat(other)),
    }?;
    stream.play()?;
    Ok(OutputStream {
        stream,
        errors,
        channels: usize::from(config.channels),
    })
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut processor: AudioProcessor,
    errors: &Arc<AtomicU32>,
) -> Result<cpal::Stream, BackendError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    // The engine renders f32 into this scratch buffer, allocated here and
    // owned by the callback, then each sample is converted to the device's
    // format. Callbacks longer than the scratch are handled in pieces.
    let mut scratch = vec![0.0f32; processor.config().max_block.max(1) * channels];
    let errors = Arc::clone(errors);
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            for piece in data.chunks_mut(scratch.len()) {
                let rendered = &mut scratch[..piece.len()];
                processor.process_interleaved(rendered, channels);
                for (out, sample) in piece.iter_mut().zip(rendered.iter()) {
                    *out = T::from_sample(*sample);
                }
            }
        },
        // May run on the audio thread: count, do not log.
        move |_error| {
            errors.fetch_add(1, Ordering::Relaxed);
        },
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{engine, EngineConfig};
    use crate::graph::CompiledGraph;

    #[test]
    fn errors_have_readable_messages() {
        assert_eq!(
            BackendError::NoOutputDevice.to_string(),
            "no default audio output device"
        );
        assert!(BackendError::UnsupportedSampleFormat(SampleFormat::U8)
            .to_string()
            .contains("unsupported"));
    }

    /// Needs a real output device. Plays 200 ms of silence.
    #[test]
    #[ignore = "opens the default audio output device"]
    fn opens_default_output_and_runs_a_silent_graph() {
        let format = default_output_format().expect("an output device");
        let config = EngineConfig::new(format.sample_rate);
        let graph =
            CompiledGraph::empty(config.channels, config.sample_rate, config.max_block).unwrap();
        let (mut controller, processor) = engine(config, graph).unwrap();
        controller.play().unwrap();
        let stream = start_default_output(processor).expect("stream starts");
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(stream.channels() >= 1);
        assert!(controller.is_playing());
        assert!(controller.playhead_samples() > 0, "the callback ran");
        stream.pause().unwrap();
    }
}
