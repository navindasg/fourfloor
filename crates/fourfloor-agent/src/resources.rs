//! Read-only views of the session, served as MCP resources and as query
//! tools: a project summary, a track/device/parameter listing, the device
//! registry, and a rendered loudness and mix report.

use fourfloor_analysis::{
    measure_levels, spectrum_frames, LevelReport, SpectrumAnalyzer, SpectrumOptions,
};
use fourfloor_audio::AudioBuffer;
use fourfloor_core::SampleRate;
use fourfloor_model::sequencer::BAR_TICKS;
use fourfloor_model::{ClipContent, Track};
use serde_json::{json, Value};

use crate::agent::Agent;
use crate::error::AgentError;
use crate::tools::output::{loudness_json, render_all_stems};

/// URI of the project summary.
pub const SUMMARY_URI: &str = "fourfloor://project/summary";
/// URI of the track, device and parameter listing.
pub const TRACKS_URI: &str = "fourfloor://project/tracks";
/// URI of the rendered loudness and mix-analysis report.
pub const MIX_REPORT_URI: &str = "fourfloor://project/mix-report";
/// URI of the device registry listing.
pub const DEVICES_URI: &str = "fourfloor://devices";

/// A resource's URI, name and description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceInfo {
    /// URI.
    pub uri: &'static str,
    /// Short name.
    pub name: &'static str,
    /// What it holds.
    pub description: &'static str,
}

/// Every resource the agent serves. All are JSON.
pub const RESOURCES: [ResourceInfo; 4] = [
    ResourceInfo {
        uri: SUMMARY_URI,
        name: "project-summary",
        description: "Name, tempo, swing, length, tracks with clip and device counts, undo state",
    },
    ResourceInfo {
        uri: TRACKS_URI,
        name: "project-tracks",
        description: "Every track with its mixer settings, clips, devices and parameters (ids, values, ranges)",
    },
    ResourceInfo {
        uri: MIX_REPORT_URI,
        name: "mix-report",
        description: "Renders the project and reports LUFS, true peak, crest, stereo correlation and spectral balance for the master and each stem, with suggestions",
    },
    ResourceInfo {
        uri: DEVICES_URI,
        name: "device-kinds",
        description: "Every device kind the registry can build, with category, parameters and sidechain support",
    },
];

/// The contents of a resource as JSON text.
pub fn read_resource(agent: &Agent, uri: &str) -> Result<String, AgentError> {
    let value = match uri {
        SUMMARY_URI => project_summary(agent),
        TRACKS_URI => track_listing(agent),
        MIX_REPORT_URI => mix_report(agent, None)?,
        DEVICES_URI => device_listing(agent),
        _ => return Err(AgentError::UnknownResource(uri.to_owned())),
    };
    serde_json::to_string_pretty(&value).map_err(|error| AgentError::Render(error.to_string()))
}

fn track_brief(track: &Track) -> Value {
    json!({
        "id": track.id.get(),
        "name": track.name,
        "kind": track.kind,
        "clips": track.clips.len(),
        "notes": track.clips.iter().map(|clip| clip.notes().len()).sum::<usize>(),
        "devices": track.devices.iter().map(|d| d.kind.clone()).collect::<Vec<_>>(),
        "gain_db": track.gain_db,
        "pan": track.pan,
        "mute": track.mute,
        "solo": track.solo,
        "output": track.output.map(|id| id.get()),
    })
}

/// The project at a glance.
pub fn project_summary(agent: &Agent) -> Value {
    let project = agent.project();
    let length = fourfloor_compile::project_length(project).get();
    let bus = agent.bus();
    json!({
        "name": project.name,
        "bpm": project.tempo.bpm(),
        "swing": project.groove,
        "length_ticks": length,
        "length_bars": length as f64 / BAR_TICKS as f64,
        "tracks": project.tracks.iter().map(|t| track_brief(t)).collect::<Vec<_>>(),
        "scenes": project.scenes.len(),
        "undo": bus.undo_label(),
        "redo": bus.redo_label(),
        "undo_depth": bus.undo_depth(),
        "revision": bus.revision(),
        "project_dir": agent.project_dir().map(|dir| dir.display().to_string()),
    })
}

/// One line per track, for people.
pub fn summary_text(agent: &Agent) -> String {
    let project = agent.project();
    let mut text = format!(
        "{} at {} BPM, swing {}%, {} track(s)",
        project.name,
        project.tempo.bpm(),
        project.groove.swing_percent,
        project.tracks.len()
    );
    for track in &project.tracks {
        let devices: Vec<&str> = track.devices.iter().map(|d| d.kind.as_str()).collect();
        text.push_str(&format!(
            "\n- [{}] {} ({}): {} clip(s), devices [{}]{}",
            track.id.get(),
            track.name,
            track.kind,
            track.clips.len(),
            devices.join(", "),
            if track.mute { ", muted" } else { "" }
        ));
    }
    text
}

/// Every track with clips, devices and parameters.
pub fn track_listing(agent: &Agent) -> Value {
    let project = agent.project();
    let registry = agent.registry();
    let tracks: Vec<Value> = project
        .tracks
        .iter()
        .map(|track| {
            let clips: Vec<Value> = track
                .clips
                .iter()
                .map(|clip| {
                    let kind = match clip.content {
                        ClipContent::Midi { .. } => "midi",
                        ClipContent::Audio { .. } => "audio",
                    };
                    json!({
                        "id": clip.id.get(),
                        "name": clip.name,
                        "kind": kind,
                        "start": clip.start.get(),
                        "length": clip.length.get(),
                        "notes": clip.notes().len(),
                    })
                })
                .collect();
            let devices: Vec<Value> = track
                .devices
                .iter()
                .map(|device| {
                    let descriptor = registry.descriptor(&device.kind);
                    json!({
                        "id": device.id.get(),
                        "kind": device.kind,
                        "name": device.name,
                        "category": descriptor.map(|d| format!("{:?}", d.category)),
                        "known_kind": descriptor.is_some(),
                        "bypassed": device.bypassed,
                        "sidechain": device.sidechain.map(|s| json!({
                            "track": s.track.get(), "pre_fader": s.pre_fader
                        })),
                        "params": device.params.iter().map(|p| json!({
                            "id": p.id.0, "name": p.name, "value": p.value,
                            "min": p.min, "max": p.max,
                            "unit": descriptor
                                .and_then(|d| d.param(p.id))
                                .map(|info| info.unit.suffix()),
                        })).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let mut brief = track_brief(track);
            brief["clips"] = json!(clips);
            brief["devices"] = json!(devices);
            brief["sends"] = json!(track
                .sends
                .iter()
                .map(|s| json!({ "target": s.target.get(), "gain_db": s.gain_db, "pre_fader": s.pre_fader }))
                .collect::<Vec<_>>());
            brief
        })
        .collect();
    json!({ "tracks": tracks })
}

/// Every device kind the registry can build.
pub fn device_listing(agent: &Agent) -> Value {
    let kinds: Vec<Value> = agent
        .registry()
        .descriptors()
        .map(|descriptor| {
            json!({
                "kind": descriptor.kind,
                "name": descriptor.name,
                "category": format!("{:?}", descriptor.category),
                "sidechain": descriptor.sidechain,
                "params": descriptor.params.iter().map(|p| json!({
                    "id": p.id.0, "name": p.name, "min": p.min, "max": p.max,
                    "default": p.default, "unit": p.unit.suffix(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "devices": kinds })
}

fn level_json(report: &LevelReport) -> Value {
    serde_json::to_value(report).unwrap_or(Value::Null)
}

fn buffer(interleaved: &[f32], rate: SampleRate) -> AudioBuffer {
    AudioBuffer::from_interleaved(rate, 2, interleaved)
}

/// Share of spectral power in four bands of the master, in dB relative to
/// the total.
fn spectral_balance(mono: &[f32], rate: SampleRate) -> Result<Value, AgentError> {
    let options = SpectrumOptions::default();
    let analyzer = SpectrumAnalyzer::new(rate, options)
        .map_err(|error| AgentError::Render(error.to_string()))?;
    let edges = analyzer.band_edges_hz().to_vec();
    let frames = spectrum_frames(mono, rate, options)
        .map_err(|error| AgentError::Render(error.to_string()))?;
    if frames.is_empty() {
        return Ok(Value::Null);
    }
    let bands = edges.len().saturating_sub(1);
    let mut power = vec![0.0f64; bands];
    for frame in &frames {
        for (band, db) in frame.bands_db.iter().enumerate().take(bands) {
            power[band] += 10f64.powf(f64::from(*db) / 10.0);
        }
    }
    let total: f64 = power.iter().sum();
    let ranges = [
        ("sub_and_bass_20_150", 20.0, 150.0),
        ("low_mid_150_800", 150.0, 800.0),
        ("mid_800_5000", 800.0, 5_000.0),
        ("high_5000_20000", 5_000.0, 20_000.0),
    ];
    let mut out = serde_json::Map::new();
    for (name, low, high) in ranges {
        let share: f64 = (0..bands)
            .filter(|band| {
                let centre = (edges[*band] * edges[*band + 1]).sqrt();
                centre >= low && centre < high
            })
            .map(|band| power[band])
            .sum();
        let db = if total > 0.0 && share > 0.0 {
            10.0 * (share / total).log10()
        } else {
            f64::NEG_INFINITY
        };
        out.insert(
            name.to_owned(),
            if db.is_finite() {
                json!((db * 10.0).round() / 10.0)
            } else {
                Value::Null
            },
        );
    }
    Ok(Value::Object(out))
}

fn suggestions(master: &LevelReport, balance: &Value) -> Vec<String> {
    let mut list = Vec::new();
    if master.true_peak_dbtp > -1.0 {
        list.push(format!(
            "True peak is {:.1} dBTP; turn the master down or add fx.limiter so it stays under -1 dBTP.",
            master.true_peak_dbtp
        ));
    }
    if master.integrated_lufs.is_finite() {
        if master.integrated_lufs < -16.0 {
            list.push(format!(
                "The mix is quiet ({:.1} LUFS); club masters usually sit around -9 to -6 LUFS.",
                master.integrated_lufs
            ));
        } else if master.integrated_lufs > -5.0 {
            list.push(format!(
                "The mix is very loud ({:.1} LUFS); expect pumping and distortion.",
                master.integrated_lufs
            ));
        }
    } else {
        list.push("The render is silent.".to_owned());
    }
    if master.crest_factor_db.is_finite() && master.crest_factor_db < 6.0 {
        list.push(format!(
            "Crest factor is {:.1} dB, which sounds over-compressed.",
            master.crest_factor_db
        ));
    }
    if master.correlation < 0.2 {
        list.push(format!(
            "Stereo correlation is {:.2}; check for phase problems, it may collapse in mono.",
            master.correlation
        ));
    }
    if let Some(low) = balance.get("sub_and_bass_20_150").and_then(Value::as_f64) {
        if low > -1.0 {
            list.push("The low end dominates the spectrum; check kick and bass levels.".to_owned());
        } else if low < -12.0 {
            list.push(
                "The low end is thin for house; the kick and bass may be too quiet.".to_owned(),
            );
        }
    }
    list
}

/// Renders the project and analyses the master and every stem.
pub fn mix_report(agent: &Agent, bars: Option<u32>) -> Result<Value, AgentError> {
    if agent.project().tracks.is_empty() {
        return Err(AgentError::Unavailable(
            "the project has no tracks to analyse".to_owned(),
        ));
    }
    let stems = render_all_stems(agent, bars, 0.5)?;
    let rate = stems.sample_rate;
    let master = measure_levels(&buffer(&stems.master, rate))
        .map_err(|error| AgentError::Render(error.to_string()))?;
    let mono: Vec<f32> = stems
        .master
        .chunks_exact(2)
        .map(|frame| 0.5 * (frame[0] + frame[1]))
        .collect();
    let balance = spectral_balance(&mono, rate)?;
    let stem_reports: Vec<Value> = stems
        .stems
        .iter()
        .map(|stem| {
            let levels = measure_levels(&buffer(&stem.samples, rate)).ok();
            json!({
                "track": stem.track.get(),
                "name": stem.name,
                "levels": levels.as_ref().map(level_json),
            })
        })
        .collect();
    Ok(json!({
        "sample_rate": rate.hz(),
        "seconds": stems.master.len() as f64 / 2.0 / f64::from(rate.hz()),
        "master": level_json(&master),
        "loudness": loudness_json(&stems.master, 2, rate)?,
        "spectral_balance_db": balance,
        "stems": stem_reports,
        "suggestions": suggestions(&master, &balance),
        "warnings": stems.warnings,
    }))
}

/// A short reading of a mix report.
pub fn mix_report_text(report: &Value) -> String {
    let master = &report["master"];
    let mut text = format!(
        "master {} LUFS, true peak {} dBTP, crest {} dB, correlation {}",
        master["integrated_lufs"],
        master["true_peak_dbtp"],
        master["crest_factor_db"],
        master["correlation"]
    );
    if let Some(list) = report["suggestions"].as_array() {
        for item in list.iter().filter_map(Value::as_str) {
            text.push_str("\n- ");
            text.push_str(item);
        }
    }
    text
}
