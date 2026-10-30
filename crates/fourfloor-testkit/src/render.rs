//! Offline rendering: run a [`Device`], a graph or a whole project and get
//! a [`StereoBuffer`] back. No audio device is opened.

use assert_no_alloc::assert_no_alloc;
use fourfloor_compile::{RenderError, RenderOptions};
use fourfloor_core::{
    Device, DeviceRegistry, Event, EventBuffer, EventKind, PrepareConfig, ProcessIo, SampleRate,
    Tempo, TransportContext,
};
use fourfloor_engine::offline::render_frames;
use fourfloor_engine::{engine, CompiledGraph, EngineConfig, EngineError};
use fourfloor_model::Project;

use crate::buffer::StereoBuffer;
use crate::signal::seconds_to_frames;

/// How a device is driven by [`render_device`] and its shorthands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderConfig {
    /// Sample rate. Default 48 kHz.
    pub sample_rate: SampleRate,
    /// Frames per `process` call. Default 512. The last block of a render
    /// is shorter when the length is not a multiple.
    pub block: usize,
    /// Tempo reported to the device. Default 120 BPM.
    pub tempo: Tempo,
    /// Whether the transport rolls (from position zero). Default true.
    pub playing: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::HZ_48000,
            block: 512,
            tempo: Tempo::DEFAULT,
            playing: true,
        }
    }
}

impl RenderConfig {
    /// The default configuration at another sample rate.
    pub fn at_rate(sample_rate: SampleRate) -> Self {
        Self {
            sample_rate,
            ..Self::default()
        }
    }

    /// The same configuration with another block size (at least 1).
    pub fn with_block(mut self, block: usize) -> Self {
        self.block = block.max(1);
        self
    }

    /// The same configuration with another tempo.
    pub fn with_tempo(mut self, tempo: Tempo) -> Self {
        self.tempo = tempo;
        self
    }

    /// The same configuration with the transport stopped.
    pub fn stopped(mut self) -> Self {
        self.playing = false;
        self
    }
}

/// An event at an absolute frame of a render (frame 0 is the first sample).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedEvent {
    /// Frame, counted from the start of the render.
    pub frame: u64,
    /// What happens.
    pub kind: EventKind,
}

impl TimedEvent {
    /// An event at `frame`.
    pub const fn new(frame: u64, kind: EventKind) -> Self {
        Self { frame, kind }
    }

    /// An event at `seconds` into the render.
    pub fn at_seconds(seconds: f64, sample_rate: SampleRate, kind: EventKind) -> Self {
        Self::new(seconds_to_frames(seconds, sample_rate) as u64, kind)
    }
}

/// A note-on and its note-off on channel 0: the note starts `start_seconds`
/// into the render and lasts `length_seconds`.
pub fn note(
    start_seconds: f64,
    length_seconds: f64,
    pitch: u8,
    velocity: u8,
    sample_rate: SampleRate,
) -> [TimedEvent; 2] {
    [
        TimedEvent::at_seconds(
            start_seconds,
            sample_rate,
            EventKind::note_on(pitch, velocity),
        ),
        TimedEvent::at_seconds(
            start_seconds + length_seconds,
            sample_rate,
            EventKind::note_off(pitch),
        ),
    ]
}

/// The events of several notes, each `(start_seconds, length_seconds,
/// pitch, velocity)`, sorted by time with note-offs first at equal times.
pub fn notes(specs: &[(f64, f64, u8, u8)], sample_rate: SampleRate) -> Vec<TimedEvent> {
    let mut events: Vec<TimedEvent> = specs
        .iter()
        .flat_map(|&(start, length, pitch, velocity)| {
            note(start, length, pitch, velocity, sample_rate)
        })
        .collect();
    events.sort_by_key(|event| (event.frame, event.kind.is_note_on()));
    events
}

/// What a device is fed during [`render_device`].
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceInput<'a> {
    /// Main audio input (port 0). `None` gives the device no input, as an
    /// instrument has. Shorter than the render: the rest is silence.
    pub audio: Option<&'a StereoBuffer>,
    /// Sidechain input. `None` means no sidechain is connected.
    pub sidechain: Option<&'a StereoBuffer>,
    /// Events, in any order.
    pub events: &'a [TimedEvent],
}

/// What a device produced during [`render_device`].
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceOutput {
    /// The audio it wrote.
    pub audio: StereoBuffer,
    /// The events it emitted, at absolute frames.
    pub events: Vec<TimedEvent>,
}

/// Copies `source[start..start + target.len()]` into `target`, with silence
/// past the end of the source.
fn fill_block(target: &mut [f32], source: &[f32], start: usize) {
    let available = source.len().saturating_sub(start).min(target.len());
    if available > 0 {
        target[..available].copy_from_slice(&source[start..start + available]);
    }
    target[available..].fill(0.0);
}

/// Renders `frames` frames of a device in stereo, the way the engine would
/// drive it: `prepare` once, then `process` block by block.
///
/// - Events are delivered in the block they fall in, at their exact frame.
///   Events past the end of the render are dropped.
/// - The output buffers hold NaN before each `process` call. The contract
///   says a device must write every output frame; one that does not leaves
///   NaN behind, which [`assert_no_nan_or_clipping`](crate::assert_no_nan_or_clipping)
///   then catches.
/// - Each `process` call runs inside `assert_no_alloc`. In a test binary
///   that installs [`AllocDisabler`](crate::AllocDisabler) as its global
///   allocator, a device that allocates while processing aborts the test.
/// - With [`RenderConfig::playing`] the transport position advances from
///   zero; otherwise it stays at zero, stopped.
pub fn render_device(
    device: &mut dyn Device,
    input: &DeviceInput<'_>,
    frames: usize,
    config: &RenderConfig,
) -> DeviceOutput {
    let block = config.block.max(1);
    device.prepare(&PrepareConfig::stereo(config.sample_rate, block));

    let mut pending: Vec<TimedEvent> = input.events.to_vec();
    pending.sort_by_key(|event| event.frame);
    let mut next_event = 0;

    let mut out = StereoBuffer::silent(config.sample_rate, frames);
    let mut emitted = Vec::new();
    let mut events_in: Vec<Event> = Vec::with_capacity(pending.len().max(1));
    let mut events_out = EventBuffer::with_capacity(4_096);
    let (mut in_left, mut in_right) = (vec![0.0f32; block], vec![0.0f32; block]);
    let (mut key_left, mut key_right) = (vec![0.0f32; block], vec![0.0f32; block]);

    let mut start = 0usize;
    while start < frames {
        let n = block.min(frames - start);
        let end = (start + n) as u64;

        events_in.clear();
        while let Some(event) = pending.get(next_event).filter(|event| event.frame < end) {
            let frame = event.frame.saturating_sub(start as u64) as u32;
            events_in.push(Event::new(frame, event.kind));
            next_event += 1;
        }
        events_out.clear();

        if let Some(audio) = input.audio {
            fill_block(&mut in_left[..n], &audio.left, start);
            fill_block(&mut in_right[..n], &audio.right, start);
        }
        if let Some(key) = input.sidechain {
            fill_block(&mut key_left[..n], &key.left, start);
            fill_block(&mut key_right[..n], &key.right, start);
        }
        let inputs: [&[f32]; 2] = [&in_left[..n], &in_right[..n]];
        let sidechain: [&[f32]; 2] = [&key_left[..n], &key_right[..n]];

        let ctx = if config.playing {
            TransportContext::playing_at(config.sample_rate, config.tempo, start as u64)
        } else {
            TransportContext::stopped(config.sample_rate, config.tempo)
        };
        let (out_left, out_right) = (
            &mut out.left[start..end as usize],
            &mut out.right[start..end as usize],
        );
        out_left.fill(f32::NAN);
        out_right.fill(f32::NAN);
        {
            let mut outputs: [&mut [f32]; 2] = [out_left, out_right];
            let mut io = ProcessIo {
                inputs: if input.audio.is_some() { &inputs } else { &[] },
                sidechain: input.sidechain.is_some().then_some(&sidechain[..]),
                outputs: &mut outputs,
                events: &events_in,
                events_out: &mut events_out,
            };
            assert_no_alloc(|| device.process(&ctx, &mut io));
        }
        emitted.extend(
            events_out
                .iter()
                .map(|event| TimedEvent::new(start as u64 + u64::from(event.frame), event.kind)),
        );
        start += n;
    }
    DeviceOutput {
        audio: out,
        events: emitted,
    }
}

/// Renders an instrument: no audio input, `events` in, `seconds` of audio
/// out. Build the events with [`note`] or [`notes`].
pub fn render_instrument(
    device: &mut dyn Device,
    events: &[TimedEvent],
    seconds: f64,
    config: &RenderConfig,
) -> StereoBuffer {
    let input = DeviceInput {
        events,
        ..DeviceInput::default()
    };
    let frames = seconds_to_frames(seconds, config.sample_rate);
    render_device(device, &input, frames, config).audio
}

/// Renders an effect over `input`. The output is as long as the input; to
/// hear a tail, pad the input with silence first.
pub fn render_effect(
    device: &mut dyn Device,
    input: &StereoBuffer,
    config: &RenderConfig,
) -> StereoBuffer {
    let feed = DeviceInput {
        audio: Some(input),
        ..DeviceInput::default()
    };
    render_device(device, &feed, input.frames(), config).audio
}

/// Renders an effect over `input` with `sidechain` connected as its key
/// signal (a compressor ducking to a kick, for example).
pub fn render_effect_with_sidechain(
    device: &mut dyn Device,
    input: &StereoBuffer,
    sidechain: &StereoBuffer,
    config: &RenderConfig,
) -> StereoBuffer {
    let feed = DeviceInput {
        audio: Some(input),
        sidechain: Some(sidechain),
        ..DeviceInput::default()
    };
    render_device(device, &feed, input.frames(), config).audio
}

/// Renders a compiled stereo graph through the real engine for `seconds`,
/// with the transport rolling from zero at `tempo`. The engine runs at the
/// graph's sample rate and block size.
pub fn render_graph(
    graph: CompiledGraph,
    tempo: Tempo,
    seconds: f64,
) -> Result<StereoBuffer, EngineError> {
    let sample_rate = graph.sample_rate();
    let config = EngineConfig {
        tempo,
        max_block: graph.max_block(),
        channels: graph.channels(),
        ..EngineConfig::new(sample_rate)
    };
    let block = config.max_block;
    let channels = config.channels;
    let (mut controller, mut processor) = engine(config, graph)?;
    controller.play()?;
    let frames = seconds_to_frames(seconds, sample_rate);
    let samples = render_frames(&mut processor, frames, block);
    let channel = |index: usize| -> Vec<f32> {
        if index < channels {
            samples
                .iter()
                .skip(index)
                .step_by(channels)
                .copied()
                .collect()
        } else {
            vec![0.0; frames]
        }
    };
    Ok(StereoBuffer::new(sample_rate, channel(0), channel(1)))
}

/// Compiles and renders a whole project offline, from the start of the
/// timeline to `tail_seconds` after its last clip, building devices through
/// `registry`. The same path as `fourfloor render`.
pub fn render_project(
    project: &Project,
    registry: &DeviceRegistry,
    sample_rate: SampleRate,
    tail_seconds: f64,
) -> Result<StereoBuffer, RenderError> {
    render_project_with(
        project,
        registry,
        &RenderOptions::new(sample_rate).with_tail(tail_seconds),
    )
}

/// [`render_project`] with explicit [`RenderOptions`] (block size, default
/// instrument, project folder for device state).
pub fn render_project_with(
    project: &Project,
    registry: &DeviceRegistry,
    options: &RenderOptions,
) -> Result<StereoBuffer, RenderError> {
    let rendered = fourfloor_compile::render_project(project, registry, options)?;
    Ok(StereoBuffer::new(
        rendered.sample_rate,
        rendered.channel(0),
        rendered.channel(1),
    ))
}
