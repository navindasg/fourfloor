//! Offline rendering of a whole project: compile, run the engine without
//! an audio device, return the samples.

use fourfloor_core::{DeviceId, DeviceRegistry, SampleRate, Ticks};
use fourfloor_engine::{engine, EngineConfig, EngineError};
use fourfloor_model::{Project, SceneId};

use crate::compile::compile_project;
use crate::options::{CompileError, CompileOptions, CompiledProject, CHANNELS};

/// A scene launch during an offline render: what pressing a scene button
/// at that moment would do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SceneLaunch {
    /// When the button is pressed, in ticks from the project start. The
    /// clips start on the next launch-quantize boundary at or after it.
    pub at: Ticks,
    /// The scene, or `None` to hand every track back to the arrangement.
    pub scene: Option<SceneId>,
}

/// How to render a project offline.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    /// How the project is compiled. Its sample rate is the render's.
    pub compile: CompileOptions,
    /// Seconds rendered after the end of the last clip, for tails to ring
    /// out. Negative values count as zero.
    pub tail_seconds: f64,
    /// Refuse renders longer than this many frames (default: no limit).
    pub max_frames: Option<u64>,
    /// Render this far instead of to the end of the last clip; the tail is
    /// added after it. Needed for a project that only has launcher clips.
    pub length: Option<Ticks>,
    /// Scene launches to perform while rendering.
    pub launches: Vec<SceneLaunch>,
}

impl RenderOptions {
    /// A render at `sample_rate` in 512-frame blocks with a half-second
    /// tail.
    pub fn new(sample_rate: SampleRate) -> Self {
        Self {
            compile: CompileOptions::new(sample_rate, 512),
            tail_seconds: 0.5,
            max_frames: None,
            length: None,
            launches: Vec::new(),
        }
    }

    /// The same options with a different tail.
    pub fn with_tail(mut self, seconds: f64) -> Self {
        self.tail_seconds = seconds;
        self
    }

    /// The same options, rendering `length` ticks plus the tail.
    pub fn with_length(mut self, length: Ticks) -> Self {
        self.length = Some(length);
        self
    }

    /// The same options with one more scene launch.
    pub fn with_launch(mut self, at: Ticks, scene: Option<SceneId>) -> Self {
        self.launches.push(SceneLaunch { at, scene });
        self
    }

    /// The same options with a frame limit.
    pub fn with_max_frames(mut self, frames: u64) -> Self {
        self.max_frames = Some(frames);
        self
    }
}

/// Why a project could not be rendered.
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    /// The project could not be compiled.
    #[error(transparent)]
    Compile(#[from] CompileError),
    /// The engine refused the graph or a command.
    #[error("engine error: {0}")]
    Engine(String),
    /// The render is longer than [`RenderOptions::max_frames`] or than
    /// memory can address.
    #[error("a render of {frames} frames is too long")]
    TooLong {
        /// Frames the render would need.
        frames: u64,
    },
}

impl From<EngineError> for RenderError {
    fn from(error: EngineError) -> Self {
        // The error may own a whole compiled graph; keep only its message.
        Self::Engine(error.to_string())
    }
}

/// An offline render of a project.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedProject {
    /// Sample rate of the render.
    pub sample_rate: SampleRate,
    /// Channels in `samples`: always [`CHANNELS`].
    pub channels: usize,
    /// Interleaved samples.
    pub samples: Vec<f32>,
    /// Tracks that produced sound.
    pub sounding_tracks: usize,
    /// Things in the project the compiler left out.
    pub warnings: Vec<String>,
}

impl RenderedProject {
    /// Frames rendered.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    /// One channel, de-interleaved.
    pub fn channel(&self, channel: usize) -> Vec<f32> {
        self.samples
            .iter()
            .skip(channel)
            .step_by(self.channels.max(1))
            .copied()
            .collect()
    }
}

/// One offline pass over a project: the master output and, if asked for,
/// the output of some graph nodes alongside it.
pub(crate) struct Pass {
    pub(crate) sample_rate: SampleRate,
    /// Interleaved master output.
    pub(crate) master: Vec<f32>,
    /// Interleaved output of each tapped node, in the order asked for.
    pub(crate) taps: Vec<Vec<f32>>,
    pub(crate) sounding_tracks: usize,
    pub(crate) warnings: Vec<String>,
}

/// Compiles and renders `project`. `choose_taps` picks the nodes to record
/// besides the master, each with the delay (in frames) to apply to its
/// recording.
pub(crate) fn render_pass(
    project: &Project,
    registry: &DeviceRegistry,
    options: &RenderOptions,
    choose_taps: impl FnOnce(&CompiledProject) -> Vec<(DeviceId, u32)>,
) -> Result<Pass, RenderError> {
    let sample_rate = options.compile.sample_rate;
    let block = options.compile.max_block.max(1);
    let compiled = compile_project(project, registry, &options.compile)?;
    let taps = choose_taps(&compiled);

    let tail = (options.tail_seconds.max(0.0) * sample_rate.as_f64()).round() as u64;
    let frames = options
        .length
        .unwrap_or(compiled.length)
        .max(Ticks::ZERO)
        .to_sample_pos(project.tempo, sample_rate)
        .saturating_add(tail);
    if options.max_frames.is_some_and(|limit| frames > limit) {
        return Err(RenderError::TooLong { frames });
    }
    let frame_count = usize::try_from(frames)
        .ok()
        .filter(|count| count.checked_mul(CHANNELS * (taps.len() + 1)).is_some())
        .ok_or(RenderError::TooLong { frames })?;

    // Launches as (frame, messages), earliest first.
    let mut launches: Vec<_> = options
        .launches
        .iter()
        .map(|launch| {
            let at = launch
                .at
                .max(Ticks::ZERO)
                .to_sample_pos(project.tempo, sample_rate);
            (
                usize::try_from(at).unwrap_or(usize::MAX),
                compiled.launch_messages(launch.scene),
            )
        })
        .collect();
    launches.sort_by_key(|(at, _)| *at);
    let mut launches = launches.into_iter().peekable();

    let config = EngineConfig {
        tempo: project.tempo,
        max_block: block,
        channels: CHANNELS,
        ..EngineConfig::new(sample_rate)
    };
    let (sounding_tracks, warnings) = (compiled.sounding_tracks, compiled.warnings);
    let (mut controller, mut processor) = engine(config, compiled.graph)?;
    controller.play()?;

    let mut master = vec![0.0f32; frame_count * CHANNELS];
    let mut recorded: Vec<Vec<f32>> = taps
        .iter()
        .map(|_| vec![0.0f32; frame_count * CHANNELS])
        .collect();
    let mut position = 0usize;
    while position < frame_count {
        while let Some((_, messages)) = launches.next_if(|(at, _)| *at <= position) {
            for message in messages {
                controller.send(message)?;
            }
        }
        let until_launch = launches.peek().map_or(usize::MAX, |(at, _)| at - position);
        let n = block.min(frame_count - position).min(until_launch);
        let span = position * CHANNELS..(position + n) * CHANNELS;
        processor.process_interleaved(&mut master[span], CHANNELS);
        for ((node, delay), out) in taps.iter().zip(recorded.iter_mut()) {
            for channel in 0..CHANNELS {
                let Some(samples) = processor.graph().node_output(*node, channel) else {
                    continue;
                };
                for (frame, sample) in samples.iter().take(n).enumerate() {
                    let at = position + frame + *delay as usize;
                    if at < frame_count {
                        out[at * CHANNELS + channel] = *sample;
                    }
                }
            }
        }
        position += n;
    }
    Ok(Pass {
        sample_rate,
        master,
        taps: recorded,
        sounding_tracks,
        warnings,
    })
}

/// Renders `project` from the start of the timeline to
/// [`RenderOptions::tail_seconds`] after its last clip (or after
/// [`RenderOptions::length`]). A project with no clips renders just the
/// tail.
pub fn render_project(
    project: &Project,
    registry: &DeviceRegistry,
    options: &RenderOptions,
) -> Result<RenderedProject, RenderError> {
    let pass = render_pass(project, registry, options, |_| Vec::new())?;
    Ok(RenderedProject {
        sample_rate: pass.sample_rate,
        channels: CHANNELS,
        samples: pass.master,
        sounding_tracks: pass.sounding_tracks,
        warnings: pass.warnings,
    })
}
