//! Tools that render: a stereo mix and aligned stems.

use std::path::Path;

use fourfloor_audio::{measure, write_wav_file, Loudness, SampleFormat, WavSpec};
use fourfloor_compile::{
    normalize_loudness, render_project, render_stems, RenderOptions, RenderedProject,
    RenderedStems, CHANNELS,
};
use fourfloor_core::{SampleRate, Ticks};
use fourfloor_model::sequencer::BAR_TICKS;
use serde_json::{json, Value};

use super::args::{AudioFormat, ExportStemsArgs, RenderArgs};
use super::ToolOutput;
use crate::agent::Agent;
use crate::error::AgentError;

/// True-peak ceiling used whenever a tool normalises.
pub const CEILING_DBTP: f64 = -1.0;

fn sample_format(format: AudioFormat) -> SampleFormat {
    match format {
        AudioFormat::Float32 => SampleFormat::Float32,
        AudioFormat::Pcm24 => SampleFormat::Pcm24,
        AudioFormat::Pcm16 => SampleFormat::Pcm16,
    }
}

/// Render options for the agent's project.
pub(crate) fn render_options(
    agent: &Agent,
    bars: Option<u32>,
    tail_seconds: f64,
) -> Result<RenderOptions, AgentError> {
    if !tail_seconds.is_finite() || !(0.0..=30.0).contains(&tail_seconds) {
        return Err(AgentError::invalid(
            "render",
            format!("tail of {tail_seconds} s is outside 0 to 30"),
        ));
    }
    let settings = agent.settings();
    let rate = settings.sample_rate;
    let max_frames = (settings.max_render_seconds * f64::from(rate.hz())) as u64;
    let mut options = RenderOptions::new(rate)
        .with_tail(tail_seconds)
        .with_max_frames(max_frames);
    if let Some(bars) = bars {
        if bars == 0 {
            return Err(AgentError::invalid("render", "bars must be at least 1"));
        }
        options = options.with_length(Ticks::new(i64::from(bars) * BAR_TICKS));
    }
    if let Some(dir) = agent.project_dir() {
        options.compile = options.compile.with_project_dir(dir);
    }
    Ok(options)
}

/// Renders the project to interleaved stereo.
pub(crate) fn render_mix(
    agent: &Agent,
    bars: Option<u32>,
    tail_seconds: f64,
) -> Result<RenderedProject, AgentError> {
    let options = render_options(agent, bars, tail_seconds)?;
    render_project(agent.project(), agent.registry(), &options)
        .map_err(|error| AgentError::Render(error.to_string()))
}

/// Renders the master and stems in one pass.
pub(crate) fn render_all_stems(
    agent: &Agent,
    bars: Option<u32>,
    tail_seconds: f64,
) -> Result<RenderedStems, AgentError> {
    let options = render_options(agent, bars, tail_seconds)?;
    render_stems(agent.project(), agent.registry(), &options)
        .map_err(|error| AgentError::Render(error.to_string()))
}

/// Splits interleaved audio into planes.
pub(crate) fn planes(interleaved: &[f32], channels: usize) -> Vec<Vec<f32>> {
    let channels = channels.max(1);
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

/// Loudness of interleaved audio, as JSON. Non-finite values (silence)
/// become `null`.
pub(crate) fn loudness_json(
    interleaved: &[f32],
    channels: usize,
    rate: SampleRate,
) -> Result<Value, AgentError> {
    let planar = planes(interleaved, channels);
    let views: Vec<&[f32]> = planar.iter().map(Vec::as_slice).collect();
    let loudness = measure(&views, rate).map_err(|error| AgentError::Render(error.to_string()))?;
    Ok(loudness_value(&loudness))
}

fn finite(value: f64) -> Value {
    if value.is_finite() {
        json!((value * 100.0).round() / 100.0)
    } else {
        Value::Null
    }
}

pub(crate) fn loudness_value(loudness: &Loudness) -> Value {
    json!({
        "integrated_lufs": finite(loudness.integrated_lufs),
        "loudness_range_lu": finite(loudness.loudness_range_lu),
        "true_peak_dbtp": finite(loudness.true_peak_dbtp),
        "sample_peak_dbfs": finite(loudness.sample_peak_dbfs),
    })
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

/// `render`.
pub(crate) fn render(agent: &mut Agent, args: RenderArgs) -> Result<ToolOutput, AgentError> {
    let relative = args.path.clone().unwrap_or_else(|| {
        let name: String = agent
            .project()
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        format!("renders/{name}.wav")
    });
    if !relative.to_lowercase().ends_with(".wav") {
        return Err(AgentError::Path(format!("\"{relative}\" must end in .wav")));
    }
    let path = agent.output_path(&relative)?;
    let mut rendered = render_mix(agent, args.bars, args.tail_seconds)?;
    let rate = rendered.sample_rate;
    let normalized = match args.normalize_lufs {
        Some(target) => Some(
            normalize_loudness(
                &mut rendered.samples,
                rendered.channels,
                rate,
                target,
                CEILING_DBTP,
            )
            .map_err(|error| AgentError::Render(error.to_string()))?,
        ),
        None => None,
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            AgentError::Path(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    let spec = WavSpec {
        sample_rate: rate,
        channels: rendered.channels as u16,
        format: sample_format(args.format),
    };
    write_wav_file(&path, spec, &rendered.samples)
        .map_err(|error| AgentError::Path(format!("cannot write {}: {error}", path.display())))?;
    let loudness = loudness_json(&rendered.samples, rendered.channels, rate)?;
    let seconds = rendered.frames() as f64 / f64::from(rate.hz());
    let data = json!({
        "path": display_path(&path),
        "sample_rate": rate.hz(),
        "channels": rendered.channels,
        "frames": rendered.frames(),
        "seconds": seconds,
        "loudness": loudness,
        "normalize_gain_db": normalized.map(|n| n.gain_db),
        "sounding_tracks": rendered.sounding_tracks,
        "warnings": rendered.warnings,
    });
    Ok(ToolOutput::unchanged(
        format!(
            "rendered {seconds:.2} s to {} ({} LUFS)",
            display_path(&path),
            data["loudness"]["integrated_lufs"]
        ),
        data,
    ))
}

/// `export_stems`.
pub(crate) fn export_stems(
    agent: &mut Agent,
    args: ExportStemsArgs,
) -> Result<ToolOutput, AgentError> {
    let relative = args.dir.clone().unwrap_or_else(|| "stems".to_owned());
    let dir = agent.output_path(&relative)?;
    let mut stems = render_all_stems(agent, args.bars, args.tail_seconds)?;
    let normalized = match args.normalize_lufs {
        Some(target) => Some(
            stems
                .normalize(target, CEILING_DBTP)
                .map_err(|error| AgentError::Render(error.to_string()))?,
        ),
        None => None,
    };
    let paths = stems
        .write_wav(&dir, sample_format(args.format))
        .map_err(|error| AgentError::Path(error.to_string()))?;
    let names: Vec<Value> = stems
        .stems
        .iter()
        .map(|stem| json!({ "track": stem.track.get(), "name": stem.name }))
        .collect();
    let data = json!({
        "dir": display_path(&dir),
        "files": paths.iter().map(|path| display_path(path)).collect::<Vec<_>>(),
        "stems": names,
        "channels": CHANNELS,
        "normalize_gain_db": normalized.map(|n| n.gain_db),
        "warnings": stems.warnings,
    });
    Ok(ToolOutput::unchanged(
        format!(
            "wrote master and {} stem(s) to {}",
            stems.stems.len(),
            display_path(&dir)
        ),
        data,
    ))
}
