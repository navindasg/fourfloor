//! Logic behind the AI panel: which logged edit can still be undone, which
//! clip the clip buttons act on, and turning the panel's fields into tool
//! arguments, an MCP address or a chat-model setting.

use std::net::SocketAddr;

use fourfloor_core::ClipId;
use fourfloor_model::{ClipContent, Project};
use serde_json::{json, Value};

use crate::services::assistant::{AppliedStep, LlmConfig};
use crate::state::Selection;

/// Default address of the MCP server (the agent binary's default).
pub const DEFAULT_MCP_ADDR: &str = "127.0.0.1:8765";
/// Default chat server (Ollama's OpenAI-compatible endpoint).
pub const DEFAULT_LLM_URL: &str = "http://127.0.0.1:11434/v1";

/// Drum flavours the agent knows.
pub const DRUM_STYLES: [&str; 4] = ["classic", "deep", "tech", "jackin"];
/// Bass styles the agent knows.
pub const BASS_STYLES: [&str; 4] = ["offbeat", "rolling", "octave", "walking"];
/// Keys offered for generated bass lines.
pub const KEYS: [&str; 8] = [
    "A minor", "C minor", "D minor", "E minor", "F minor", "G minor", "F# minor", "C major",
];

/// Where a logged edit stands against the document's undo history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    /// It is the newest step: its undo button undoes exactly it.
    Undoable,
    /// It was undone (the history is shallower than it).
    Undone,
    /// Newer edits sit on top of it.
    Buried,
}

/// The state of `step` given the document's undo depth and newest label.
pub fn step_state(step: &AppliedStep, undo_depth: usize, undo_label: Option<&str>) -> StepState {
    if undo_depth < step.depth {
        StepState::Undone
    } else if undo_depth == step.depth && undo_label == Some(step.label.as_str()) {
        StepState::Undoable
    } else {
        StepState::Buried
    }
}

/// The selected clip, when it is an audio clip (the stem and MIDI buttons
/// need one).
pub fn selected_audio_clip(project: &Project, selection: &Selection) -> Option<ClipId> {
    let id = selection.clip?;
    let (_, clip) = project.find_any_clip(id)?;
    matches!(clip.content, ClipContent::Audio { .. }).then_some(id)
}

/// Reads an MCP address; only loopback addresses are accepted, because the
/// server can edit and render the project.
pub fn parse_mcp_addr(text: &str) -> Result<SocketAddr, String> {
    let trimmed = text.trim().trim_start_matches("http://");
    let host_port = trimmed.split('/').next().unwrap_or_default();
    let addr: SocketAddr = host_port
        .parse()
        .map_err(|_| format!("\"{text}\" is not an address like {DEFAULT_MCP_ADDR}"))?;
    if !addr.ip().is_loopback() {
        return Err("the MCP server only listens on this machine (127.0.0.1)".to_owned());
    }
    Ok(addr)
}

/// A chat-model setting from the panel's fields; only servers on this
/// machine are accepted.
pub fn llm_config(url: &str, model: &str) -> Result<LlmConfig, String> {
    let url = url.trim();
    let model = model.trim();
    if model.is_empty() {
        return Err("name the model the server runs (for example qwen2.5:7b)".to_owned());
    }
    let rest = url
        .strip_prefix("http://")
        .ok_or("the chat server URL must start with http://")?;
    let host = rest.split(['/', ':']).next().unwrap_or_default();
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return Err("the chat server must run on this machine (127.0.0.1)".to_owned());
    }
    Ok(LlmConfig {
        url: url.to_owned(),
        model: model.to_owned(),
    })
}

/// What a tool step said, for the log: without the tool name repeated in
/// front and without raw JSON outcomes (`{"kind":"done"}`).
pub fn tool_summary(tool: &str, text: &str) -> Option<String> {
    let rest = text
        .strip_prefix(tool)
        .and_then(|r| r.strip_prefix(':'))
        .unwrap_or(text)
        .trim();
    (!rest.is_empty() && !rest.starts_with('{') && !rest.starts_with('[')).then(|| rest.to_owned())
}

/// A readable title for a prompt that is a tool's name (the buttons).
pub fn prompt_title(prompt: &str) -> String {
    let named = match prompt {
        "add_house_drum_pattern" => "Generate drums",
        "add_bassline" => "Generate bass",
        "add_chords" => "Generate chords",
        "mix_report" => "Mix report",
        "separate_stems" => "Stem-separate clip",
        "audio_to_midi" => "Audio to MIDI",
        other => return other.to_owned(),
    };
    named.to_owned()
}

/// Arguments of `add_house_drum_pattern`.
pub fn drum_args(bars: u32, style: &str, seed: u64) -> Value {
    json!({ "bars": bars.clamp(1, 64), "style": style, "seed": seed })
}

/// Arguments of `add_bassline`.
pub fn bass_args(bars: u32, key: &str, style: &str, seed: u64) -> Value {
    json!({ "bars": bars.clamp(1, 64), "key": key, "style": style, "seed": seed })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::command::{AddAudioClip, CreateTrack, WriteMidiClip};
    use fourfloor_model::{AudioRef, CommandBus, ContentHash, TrackKind};

    fn step(depth: usize, label: &str) -> AppliedStep {
        AppliedStep {
            label: label.to_owned(),
            commands: Vec::new(),
            depth,
        }
    }

    #[test]
    fn only_the_newest_step_is_undoable() {
        let s = step(3, "add_bassline");
        assert_eq!(step_state(&s, 3, Some("add_bassline")), StepState::Undoable);
        assert_eq!(step_state(&s, 2, Some("x")), StepState::Undone);
        assert_eq!(step_state(&s, 4, Some("set_tempo")), StepState::Buried);
        // Same depth, another label: undone then replaced by a new edit.
        assert_eq!(step_state(&s, 3, Some("set_tempo")), StepState::Buried);
    }

    #[test]
    fn tool_text_is_tidied_for_the_log() {
        assert_eq!(
            tool_summary("set_tempo", r#"set_tempo: {"kind":"done"}"#),
            None
        );
        assert_eq!(
            tool_summary("add_bassline", "wrote 4 bars in A minor"),
            Some("wrote 4 bars in A minor".to_owned())
        );
        assert_eq!(
            tool_summary("x", "x: moved 2 notes"),
            Some("moved 2 notes".to_owned())
        );
        assert_eq!(prompt_title("add_house_drum_pattern"), "Generate drums");
        assert_eq!(prompt_title("set tempo to 120"), "set tempo to 120");
    }

    #[test]
    fn addresses_and_models_must_be_local() {
        assert_eq!(
            parse_mcp_addr(" 127.0.0.1:9000 ").map(|a| a.port()),
            Ok(9000)
        );
        assert!(parse_mcp_addr("http://127.0.0.1:8765/mcp").is_ok());
        assert!(parse_mcp_addr("0.0.0.0:8765").is_err());
        assert!(parse_mcp_addr("192.168.1.2:8765").is_err());
        assert!(parse_mcp_addr("nonsense").is_err());
        assert!(llm_config(DEFAULT_LLM_URL, "qwen").is_ok());
        assert!(llm_config("http://localhost:1234/v1", "m").is_ok());
        assert!(llm_config("https://api.example.com/v1", "m").is_err());
        assert!(llm_config("http://10.0.0.2:11434/v1", "m").is_err());
        assert!(llm_config(DEFAULT_LLM_URL, "  ").is_err());
    }

    #[test]
    fn clip_buttons_need_a_selected_audio_clip() {
        let mut bus = CommandBus::new(Project::new("T"));
        let audio = bus
            .execute(CreateTrack {
                name: "Loop".to_owned(),
                kind: TrackKind::Audio,
                index: None,
            })
            .expect("track")
            .track()
            .expect("id");
        let clip = bus
            .execute(AddAudioClip {
                track: audio,
                name: "loop".to_owned(),
                start: fourfloor_core::Ticks::new(0),
                length: fourfloor_core::Ticks::new(3840),
                source: AudioRef {
                    hash: ContentHash::of_bytes(b"loop"),
                    extension: "wav".to_owned(),
                },
                source_offset_frames: 0,
            })
            .expect("clip")
            .clip()
            .expect("clip id");
        let midi = bus
            .execute(CreateTrack {
                name: "Keys".to_owned(),
                kind: TrackKind::Midi,
                index: None,
            })
            .expect("track")
            .track()
            .expect("id");
        let notes = bus
            .execute(WriteMidiClip {
                track: midi,
                clip: None,
                name: "keys".to_owned(),
                start: fourfloor_core::Ticks::new(0),
                length: fourfloor_core::Ticks::new(3840),
                notes: Vec::new(),
            })
            .expect("midi")
            .clip()
            .expect("clip id");
        let project = bus.project();
        let mut selection = Selection::default();
        assert_eq!(selected_audio_clip(project, &selection), None);
        selection.clip = Some(clip);
        assert_eq!(selected_audio_clip(project, &selection), Some(clip));
        selection.clip = Some(notes);
        assert_eq!(selected_audio_clip(project, &selection), None);
        assert_eq!(drum_args(99, "deep", 3)["bars"], 64);
        assert_eq!(bass_args(4, "F minor", "rolling", 1)["key"], "F minor");
    }
}
