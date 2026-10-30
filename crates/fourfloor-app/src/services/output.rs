//! Audio output through cpal with device and buffer-size choice, CPU load
//! and dropout counting. (The engine's own backend only opens the default
//! device, so the app has its own.)
//!
//! Load is the time spent rendering over the time the buffer lasts. A
//! dropout is counted when a callback overran its buffer or arrived more
//! than one and a half buffers late; cpal itself reports no underruns on
//! most hosts, so this is an estimate from callback timing.

use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, FromSample, SampleFormat, SizedSample, StreamConfig};
use fourfloor_core::SampleRate;

use super::callback::AudioCallback;
use super::meters::MeterBank;

/// What went wrong opening the output.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    /// No output device by that name (or none at all).
    #[error("no audio output device{}", name.as_ref().map(|n| format!(" named \"{n}\"")).unwrap_or_default())]
    NoDevice {
        /// The name asked for.
        name: Option<String>,
    },
    /// The device reported a rate fourfloor cannot use.
    #[error("the output device reported an invalid sample rate")]
    InvalidRate,
    /// A sample format with no conversion here.
    #[error("unsupported output sample format {0}")]
    Format(String),
    /// Anything cpal reported.
    #[error("audio backend error: {0}")]
    Backend(String),
}

impl From<cpal::Error> for OutputError {
    fn from(error: cpal::Error) -> Self {
        Self::Backend(error.to_string())
    }
}

/// Which device and buffer size to use.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputSettings {
    /// Device name as listed by [`output_device_names`]; `None` for the
    /// system default.
    pub device: Option<String>,
    /// Buffer size in frames; `None` for the device default.
    pub buffer_frames: Option<u32>,
}

/// Names of every output device the default host lists.
pub fn output_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|devices| devices.map(|device| device.to_string()).collect())
        .unwrap_or_default()
}

fn find_device(name: Option<&str>) -> Result<cpal::Device, OutputError> {
    let host = cpal::default_host();
    let found = match name {
        None => host.default_output_device(),
        Some(wanted) => host
            .output_devices()?
            .find(|device| device.to_string() == wanted),
    };
    found.ok_or_else(|| OutputError::NoDevice {
        name: name.map(str::to_owned),
    })
}

/// The rate and channel count a device runs at by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFormat {
    /// Sample rate.
    pub sample_rate: SampleRate,
    /// Output channels.
    pub channels: usize,
}

/// Queries the device's default format; build the engine at this rate.
pub fn device_format(settings: &OutputSettings) -> Result<DeviceFormat, OutputError> {
    let device = find_device(settings.device.as_deref())?;
    let config = device.default_output_config()?;
    Ok(DeviceFormat {
        sample_rate: SampleRate::new(config.sample_rate()).map_err(|_| OutputError::InvalidRate)?,
        channels: usize::from(config.channels()),
    })
}

/// A running output stream. Dropping it stops the audio.
pub struct DeviceStream {
    _stream: cpal::Stream,
    /// The device's name.
    pub name: String,
    /// Output channels.
    pub channels: usize,
    /// Buffer size asked for (`None` = device default).
    pub buffer_frames: Option<u32>,
}

impl fmt::Debug for DeviceStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceStream")
            .field("name", &self.name)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

/// Starts `callback` on the chosen device at the callback's sample rate.
pub fn start_output(
    settings: &OutputSettings,
    callback: AudioCallback,
    max_block: usize,
) -> Result<DeviceStream, OutputError> {
    let device = find_device(settings.device.as_deref())?;
    let name = device.to_string();
    let default = device.default_output_config()?;
    let config = StreamConfig {
        channels: default.channels(),
        sample_rate: callback.sample_rate().hz(),
        buffer_size: settings
            .buffer_frames
            .map_or(BufferSize::Default, BufferSize::Fixed),
    };
    let channels = usize::from(config.channels).max(1);
    let stream = match default.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, callback, max_block),
        SampleFormat::I16 => build::<i16>(&device, config, callback, max_block),
        SampleFormat::U16 => build::<u16>(&device, config, callback, max_block),
        SampleFormat::I32 => build::<i32>(&device, config, callback, max_block),
        other => Err(OutputError::Format(other.to_string())),
    }?;
    stream.play()?;
    Ok(DeviceStream {
        _stream: stream,
        name,
        channels,
        buffer_frames: settings.buffer_frames,
    })
}

/// Load and dropout bookkeeping for one stream.
#[derive(Debug)]
pub struct CallbackTimer {
    rate: f64,
    last_start: Option<Instant>,
    last_frames: usize,
}

impl CallbackTimer {
    /// A timer for a stream at `rate`.
    pub fn new(rate: SampleRate) -> Self {
        Self {
            rate: rate.as_f64(),
            last_start: None,
            last_frames: 0,
        }
    }

    /// Records one callback that started at `start`, took `elapsed`
    /// seconds and rendered `frames`. Returns `(load, dropout)`.
    pub fn record(&mut self, start: Instant, elapsed: f64, frames: usize) -> (f32, bool) {
        let period = frames as f64 / self.rate;
        let load = if period > 0.0 { elapsed / period } else { 0.0 };
        let late = self.last_start.is_some_and(|last| {
            let expected = self.last_frames as f64 / self.rate;
            expected > 0.0 && start.duration_since(last).as_secs_f64() > expected * 1.5
        });
        self.last_start = Some(start);
        self.last_frames = frames;
        (load as f32, late || load > 1.0)
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut callback: AudioCallback,
    max_block: usize,
) -> Result<cpal::Stream, OutputError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels).max(1);
    let meters: Arc<MeterBank> = Arc::clone(callback.meters());
    let error_meters = Arc::clone(&meters);
    let mut timer = CallbackTimer::new(callback.sample_rate());
    // Converted in pieces through this scratch buffer, allocated here.
    let mut scratch = vec![0.0f32; max_block.max(1) * channels * 4];
    let stream = device.build_output_stream(
        config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let start = Instant::now();
            for piece in data.chunks_mut(scratch.len()) {
                let rendered = &mut scratch[..piece.len()];
                callback.render(rendered, channels);
                for (out, sample) in piece.iter_mut().zip(rendered.iter()) {
                    *out = T::from_sample(*sample);
                }
            }
            let (load, dropout) =
                timer.record(start, start.elapsed().as_secs_f64(), data.len() / channels);
            meters.record_load(load);
            if dropout {
                meters.record_xrun();
            }
        },
        // May run on the audio thread: count, do not log.
        move |_error| error_meters.record_stream_error(),
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_timer_reports_load_and_late_callbacks() {
        let mut timer = CallbackTimer::new(SampleRate::HZ_48000);
        let t0 = Instant::now();
        // 480 frames last 10 ms; 2 ms of work is 20 % load.
        let (load, dropout) = timer.record(t0, 0.002, 480);
        assert!((load - 0.2).abs() < 1e-6);
        assert!(!dropout);
        let (_, dropout) = timer.record(t0 + Duration::from_millis(10), 0.002, 480);
        assert!(!dropout, "on time");
        let (_, dropout) = timer.record(t0 + Duration::from_millis(40), 0.002, 480);
        assert!(dropout, "30 ms gap for a 10 ms buffer");
        let (load, dropout) = timer.record(t0 + Duration::from_millis(50), 0.02, 480);
        assert!(load > 1.0 && dropout, "overran its buffer");
    }

    #[test]
    fn errors_read_well() {
        let error = OutputError::NoDevice {
            name: Some("Speakers".to_owned()),
        };
        assert_eq!(
            error.to_string(),
            "no audio output device named \"Speakers\""
        );
        assert_eq!(
            OutputError::NoDevice { name: None }.to_string(),
            "no audio output device"
        );
    }
}
