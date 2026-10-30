//! Mix tools: sidechain ducking and filter risers.

use fourfloor_core::{DeviceId, Ticks, TrackId};
use fourfloor_effects::{Compressor, Filter};
use fourfloor_model::command::{SetDeviceSidechain, SetParam, WriteAutomation};
use fourfloor_model::sequencer::STEP_TICKS;
use fourfloor_model::AutomationPoint;
use serde_json::json;

use super::args::{AddFilterRiserArgs, RiserMode, SidechainArgs};
use super::edit::{add_device, bar_tick, project_bars};
use super::ToolOutput;
use crate::agent::Agent;
use crate::error::AgentError;
use crate::generate::MAX_BARS;

/// Device kind the `sidechain` tool adds.
pub const DUCKER_KIND: &str = "fx.compressor";
/// Device kind the `add_filter_riser` tool adds.
pub const RISER_KIND: &str = "fx.filter";
/// fourfloor's compressors, whose parameters the `sidechain` tool sets.
const COMPRESSOR_KINDS: &[&str] = &["fx.compressor", "fx.glue"];

/// Compressor settings for a given ducking depth (0 to 1).
fn ducker_settings(depth: f64, release_ms: f32) -> Vec<(fourfloor_core::ParamId, f32)> {
    let depth = if depth.is_finite() {
        depth.clamp(0.0, 1.0) as f32
    } else {
        0.75
    };
    vec![
        (Compressor::THRESHOLD, -18.0 - 18.0 * depth),
        (Compressor::RATIO, 2.0 + 18.0 * depth),
        (Compressor::ATTACK, 0.5),
        (Compressor::RELEASE, release_ms),
        (Compressor::KNEE, 3.0),
        (Compressor::MAKEUP, 0.0),
        (Compressor::MIX, 1.0),
        (Compressor::EXTERNAL_KEY, 1.0),
        (Compressor::KEY_HIGH_PASS, 20.0),
    ]
}

/// `sidechain`.
pub(crate) fn sidechain(agent: &mut Agent, args: SidechainArgs) -> Result<ToolOutput, AgentError> {
    let project = agent.project().clone();
    let track = args.track.resolve(&project)?;
    let key = args.key_track.resolve(&project)?;
    if track == key {
        return Err(AgentError::invalid(
            "sidechain",
            "a track cannot duck itself; pick a different key track",
        ));
    }
    let registry = agent.registry_arc();
    let depth = args.depth.unwrap_or(0.75);
    let release = args.release_ms.unwrap_or(160.0);
    let settings = ducker_settings(depth, release);
    // Reuse a compressor that can take a sidechain, if the track has one.
    let existing: Option<DeviceId> = project.track(track).and_then(|owner| {
        owner
            .devices
            .iter()
            .find(|device| {
                registry
                    .descriptor(&device.kind)
                    .is_some_and(|descriptor| descriptor.sidechain)
            })
            .map(|device| device.id)
    });
    let (device, reused) = agent.transaction("sidechain", |tx| {
        let (device, reused) = match existing {
            Some(device) => {
                // Only fourfloor's own compressors share these parameter
                // ids; a hosted plugin keeps its settings.
                let instance = tx.project().find_device(device).map(|(_, d)| d.clone());
                if let Some(instance) =
                    instance.filter(|d| COMPRESSOR_KINDS.contains(&d.kind.as_str()))
                {
                    for (param, value) in &settings {
                        if instance.param(*param).is_some() {
                            tx.execute(SetParam {
                                device,
                                param: *param,
                                value: *value,
                            })?;
                        }
                    }
                }
                (device, true)
            }
            None => (
                add_device(
                    tx,
                    &registry,
                    track,
                    DUCKER_KIND,
                    Some("Sidechain"),
                    &settings,
                )?,
                false,
            ),
        };
        tx.execute(SetDeviceSidechain {
            device,
            source: Some(key),
            pre_fader: args.pre_fader,
        })?;
        Ok((device, reused))
    })?;
    let name = |id: TrackId| {
        project
            .track(id)
            .map_or_else(String::new, |track| track.name.clone())
    };
    Ok(ToolOutput::changed(
        format!(
            "{} now ducks when {} hits ({} compressor, depth {depth:.2})",
            name(track),
            name(key),
            if reused { "existing" } else { "new" }
        ),
        json!({
            "track": track.get(),
            "key_track": key.get(),
            "device": device.get(),
            "reused_device": reused,
            "depth": depth,
            "release_ms": release,
        }),
    ))
}

/// The cutoff path of a riser: exponential from `from` to `to`.
fn sweep(from: f32, to: f32, position: f64) -> f32 {
    let ratio = f64::from(to) / f64::from(from);
    (f64::from(from) * ratio.powf(position.clamp(0.0, 1.0))) as f32
}

/// `add_filter_riser`.
pub(crate) fn add_filter_riser(
    agent: &mut Agent,
    args: AddFilterRiserArgs,
) -> Result<ToolOutput, AgentError> {
    if !(1..=MAX_BARS).contains(&args.bars) {
        return Err(AgentError::invalid(
            "add_filter_riser",
            format!("{} bars is outside 1 to {MAX_BARS}", args.bars),
        ));
    }
    let project = agent.project().clone();
    if project.tracks.is_empty() {
        return Err(AgentError::Unavailable(
            "the project has no tracks to filter".to_owned(),
        ));
    }
    let end_bar = args.end_bar.unwrap_or_else(|| project_bars(&project));
    let start_bar = end_bar.checked_sub(args.bars).ok_or_else(|| {
        AgentError::invalid(
            "add_filter_riser",
            format!(
                "a {}-bar riser cannot end at bar {end_bar}; it would start before the song",
                args.bars
            ),
        )
    })?;
    let targets: Vec<TrackId> = match &args.track {
        Some(reference) => vec![reference.resolve(&project)?],
        None => project
            .tracks
            .iter()
            .filter(|track| track.output.is_none())
            .map(|track| track.id)
            .collect(),
    };
    let (mode, from, to) = match args.mode {
        RiserMode::HighPass => (Filter::HIGH_PASS, 20.0f32, 2_500.0f32),
        RiserMode::LowPass => (Filter::LOW_PASS, 250.0, 20_000.0),
    };
    let resonance = args.resonance.unwrap_or(0.45).clamp(0.0, 1.0);
    let settings = [
        (Filter::MODE, mode as f32),
        (Filter::CUTOFF, from),
        (Filter::RESONANCE, resonance),
        (Filter::STEEP, 0.0),
        (Filter::LFO_DEPTH, 0.0),
    ];
    let start = bar_tick(start_bar).get();
    let end = bar_tick(end_bar).get();
    let mut points: Vec<AutomationPoint> = (0..)
        .map(|step| start + step * STEP_TICKS)
        .take_while(|time| *time < end)
        .map(|time| AutomationPoint {
            time: Ticks::new(time),
            value: sweep(from, to, (time - start) as f64 / (end - start) as f64),
        })
        .collect();
    points.push(AutomationPoint {
        time: Ticks::new(end - 1),
        value: to,
    });
    points.push(AutomationPoint {
        time: Ticks::new(end),
        value: from,
    });
    let registry = agent.registry_arc();
    let devices = agent.transaction("add_filter_riser", |tx| {
        let mut devices = Vec::new();
        for track in &targets {
            let device = add_device(tx, &registry, *track, RISER_KIND, Some("Riser"), &settings)?;
            tx.execute(WriteAutomation {
                device,
                param: Filter::CUTOFF,
                points: points.clone(),
            })?;
            devices.push(device.get());
        }
        Ok(devices)
    })?;
    Ok(ToolOutput::changed(
        format!(
            "{}-bar {} riser from bar {start_bar} to bar {end_bar} on {} track(s)",
            args.bars,
            match args.mode {
                RiserMode::HighPass => "high-pass",
                RiserMode::LowPass => "low-pass",
            },
            targets.len()
        ),
        json!({
            "tracks": targets.iter().map(|id| id.get()).collect::<Vec<_>>(),
            "devices": devices,
            "start_bar": start_bar,
            "end_bar": end_bar,
            "from_hz": from,
            "to_hz": to,
            "points": points.len(),
        }),
    ))
}
