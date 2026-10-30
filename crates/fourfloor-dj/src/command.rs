//! The DJ command set: plain, serializable instructions the UI and the
//! agent send to a [`DjSession`](crate::DjSession).
//!
//! In JSON every command is an object whose `"command"` field is its
//! stable snake_case name, for example
//! `{"command": "beat_loop", "deck": 0, "beats": 4}`. Decks are numbered
//! from 0. The names are listed in [`DjCommand::NAMES`] and never change.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::deck::SyncMode;
use crate::mixer::{CrossfaderCurve, CrossfaderSide};

/// A band of the deck isolator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EqBand {
    /// Below the low crossover (250 Hz).
    Low,
    /// Between the crossovers.
    Mid,
    /// Above the high crossover (2.5 kHz).
    High,
}

impl EqBand {
    /// The three bands, low first.
    pub const ALL: [EqBand; 3] = [EqBand::Low, EqBand::Mid, EqBand::High];

    /// Position in [`EqBand::ALL`].
    pub const fn index(self) -> usize {
        match self {
            Self::Low => 0,
            Self::Mid => 1,
            Self::High => 2,
        }
    }
}

/// A pre-separated stem file to load with a track.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StemFile {
    /// Stable stem name, e.g. `"drums"`.
    pub name: String,
    /// Audio file of the stem; same length as the track.
    pub path: PathBuf,
}

fn yes() -> bool {
    true
}

/// One instruction for a DJ session (see the [module docs](self)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum DjCommand {
    /// Decodes a file into a deck (blocking: call off the audio thread).
    /// With `analyze` (the default) tempo, grid and key come from the
    /// analysis cache or a fresh analysis.
    LoadTrack {
        /// Deck index.
        deck: usize,
        /// Audio file.
        path: PathBuf,
        /// Analyse tempo, beat grid and key.
        #[serde(default = "yes")]
        analyze: bool,
        /// Pre-separated stems to play instead of the mix.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        stems: Vec<StemFile>,
    },
    /// Unloads a deck.
    Eject {
        /// Deck index.
        deck: usize,
    },
    /// Starts a deck.
    Play {
        /// Deck index.
        deck: usize,
    },
    /// Stops a deck where it is.
    Pause {
        /// Deck index.
        deck: usize,
    },
    /// Play when stopped, pause when playing.
    TogglePlay {
        /// Deck index.
        deck: usize,
    },
    /// CDJ cue button: back to the cue point and stop while playing; set
    /// the cue point while stopped.
    Cue {
        /// Deck index.
        deck: usize,
    },
    /// Sets the cue point at the playhead.
    SetCuePoint {
        /// Deck index.
        deck: usize,
    },
    /// Moves the playhead.
    Seek {
        /// Deck index.
        deck: usize,
        /// Position in seconds from the track start.
        seconds: f64,
    },
    /// Pitch fader, in percent (-50 to 100).
    SetPitch {
        /// Deck index.
        deck: usize,
        /// Speed change in percent.
        percent: f64,
    },
    /// Keeps the pitch while the tempo changes.
    SetKeyLock {
        /// Deck index.
        deck: usize,
        /// On or off.
        on: bool,
    },
    /// How a deck follows the tempo master.
    SetSync {
        /// Deck index.
        deck: usize,
        /// Off, tempo only, or tempo and beat phase.
        mode: SyncMode,
    },
    /// Chooses the tempo master (`null` for none).
    SetMaster {
        /// Deck index, or none.
        deck: Option<usize>,
    },
    /// Temporary speed change in percent (-50 to 50); 0 releases it.
    Bend {
        /// Deck index.
        deck: usize,
        /// Speed change in percent.
        percent: f64,
    },
    /// Shifts a deck in time.
    Nudge {
        /// Deck index.
        deck: usize,
        /// Milliseconds forward (negative: back).
        milliseconds: f64,
    },
    /// Snaps cues and loops to the beat grid.
    SetQuantize {
        /// Deck index.
        deck: usize,
        /// On or off.
        on: bool,
    },
    /// A loop of `beats` beats (1/32 to 32) from the playhead; resizes an
    /// active loop.
    BeatLoop {
        /// Deck index.
        deck: usize,
        /// Loop length in beats.
        beats: f64,
    },
    /// A loop roll of `beats` beats; the track runs on underneath.
    LoopRoll {
        /// Deck index.
        deck: usize,
        /// Loop length in beats.
        beats: f64,
    },
    /// Leaves the active loop or loop roll.
    LoopExit {
        /// Deck index.
        deck: usize,
    },
    /// Re-enters the last loop, or leaves the active one.
    Reloop {
        /// Deck index.
        deck: usize,
    },
    /// Hot cue button: sets an empty slot, jumps to a set one.
    HotCue {
        /// Deck index.
        deck: usize,
        /// Slot, 0 to 7.
        slot: usize,
    },
    /// Empties a hot cue slot.
    ClearHotCue {
        /// Deck index.
        deck: usize,
        /// Slot, 0 to 7.
        slot: usize,
    },
    /// Jumps by beats (negative: back); an active loop moves along.
    BeatJump {
        /// Deck index.
        deck: usize,
        /// Beats to jump.
        beats: f64,
    },
    /// Isolator band gain in dB (-60 to 6; -60 is a full kill).
    SetEq {
        /// Deck index.
        deck: usize,
        /// Band.
        band: EqBand,
        /// Gain in dB.
        db: f32,
    },
    /// Kill switch of an isolator band; off restores the band's gain.
    SetEqKill {
        /// Deck index.
        deck: usize,
        /// Band.
        band: EqBand,
        /// Killed or not.
        on: bool,
    },
    /// DJ filter knob: -1 low-pass, 0 off, 1 high-pass.
    SetFilter {
        /// Deck index.
        deck: usize,
        /// Knob position.
        amount: f32,
    },
    /// Channel fader, 0 to 1.
    SetFader {
        /// Deck index.
        deck: usize,
        /// Fader position.
        value: f32,
    },
    /// Channel trim in dB (-24 to 12).
    SetTrim {
        /// Deck index.
        deck: usize,
        /// Gain in dB.
        db: f32,
    },
    /// Sends a deck to the headphone cue bus.
    SetCueMonitor {
        /// Deck index.
        deck: usize,
        /// On or off.
        on: bool,
    },
    /// Which crossfader side a deck is on.
    SetCrossfaderSide {
        /// Deck index.
        deck: usize,
        /// A, B or thru.
        side: CrossfaderSide,
    },
    /// Crossfader position, -1 (A) to 1 (B).
    SetCrossfader {
        /// Position.
        position: f32,
    },
    /// Crossfader curve.
    SetCrossfaderCurve {
        /// Curve.
        curve: CrossfaderCurve,
    },
    /// Master level in dB (-60 to 6).
    SetMasterGain {
        /// Gain in dB.
        db: f32,
    },
    /// Headphone blend: 0 cue only, 1 master only.
    SetCueMix {
        /// Blend.
        value: f32,
    },
    /// Headphone level in dB (-60 to 6).
    SetHeadphoneGain {
        /// Gain in dB.
        db: f32,
    },
    /// Switches a deck effect slot on or off.
    SetDeckEffect {
        /// Deck index.
        deck: usize,
        /// Effect slot.
        slot: usize,
        /// On or off.
        on: bool,
    },
    /// Sets a deck effect parameter by its name (as the effect lists it).
    SetDeckEffectParam {
        /// Deck index.
        deck: usize,
        /// Effect slot.
        slot: usize,
        /// Parameter name, e.g. `"Mix"`.
        param: String,
        /// Value in the parameter's own units.
        value: f32,
    },
    /// Linear gain of one stem (0 to 4).
    SetStemGain {
        /// Deck index.
        deck: usize,
        /// Stem name.
        stem: String,
        /// Gain.
        gain: f32,
    },
    /// Mutes or unmutes one stem, keeping its gain.
    SetStemMute {
        /// Deck index.
        deck: usize,
        /// Stem name.
        stem: String,
        /// Muted or not.
        muted: bool,
    },
    /// Records the master output to a float WAV file.
    StartRecording {
        /// File to write.
        path: PathBuf,
    },
    /// Stops the recording (the file closes on the next pump).
    StopRecording,
}

impl DjCommand {
    /// Every command name, as used in the `"command"` field.
    pub const NAMES: [&'static str; 40] = [
        "load_track",
        "eject",
        "play",
        "pause",
        "toggle_play",
        "cue",
        "set_cue_point",
        "seek",
        "set_pitch",
        "set_key_lock",
        "set_sync",
        "set_master",
        "bend",
        "nudge",
        "set_quantize",
        "beat_loop",
        "loop_roll",
        "loop_exit",
        "reloop",
        "hot_cue",
        "clear_hot_cue",
        "beat_jump",
        "set_eq",
        "set_eq_kill",
        "set_filter",
        "set_fader",
        "set_trim",
        "set_cue_monitor",
        "set_crossfader_side",
        "set_crossfader",
        "set_crossfader_curve",
        "set_master_gain",
        "set_cue_mix",
        "set_headphone_gain",
        "set_deck_effect",
        "set_deck_effect_param",
        "set_stem_gain",
        "set_stem_mute",
        "start_recording",
        "stop_recording",
    ];

    /// The command's stable name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::LoadTrack { .. } => "load_track",
            Self::Eject { .. } => "eject",
            Self::Play { .. } => "play",
            Self::Pause { .. } => "pause",
            Self::TogglePlay { .. } => "toggle_play",
            Self::Cue { .. } => "cue",
            Self::SetCuePoint { .. } => "set_cue_point",
            Self::Seek { .. } => "seek",
            Self::SetPitch { .. } => "set_pitch",
            Self::SetKeyLock { .. } => "set_key_lock",
            Self::SetSync { .. } => "set_sync",
            Self::SetMaster { .. } => "set_master",
            Self::Bend { .. } => "bend",
            Self::Nudge { .. } => "nudge",
            Self::SetQuantize { .. } => "set_quantize",
            Self::BeatLoop { .. } => "beat_loop",
            Self::LoopRoll { .. } => "loop_roll",
            Self::LoopExit { .. } => "loop_exit",
            Self::Reloop { .. } => "reloop",
            Self::HotCue { .. } => "hot_cue",
            Self::ClearHotCue { .. } => "clear_hot_cue",
            Self::BeatJump { .. } => "beat_jump",
            Self::SetEq { .. } => "set_eq",
            Self::SetEqKill { .. } => "set_eq_kill",
            Self::SetFilter { .. } => "set_filter",
            Self::SetFader { .. } => "set_fader",
            Self::SetTrim { .. } => "set_trim",
            Self::SetCueMonitor { .. } => "set_cue_monitor",
            Self::SetCrossfaderSide { .. } => "set_crossfader_side",
            Self::SetCrossfader { .. } => "set_crossfader",
            Self::SetCrossfaderCurve { .. } => "set_crossfader_curve",
            Self::SetMasterGain { .. } => "set_master_gain",
            Self::SetCueMix { .. } => "set_cue_mix",
            Self::SetHeadphoneGain { .. } => "set_headphone_gain",
            Self::SetDeckEffect { .. } => "set_deck_effect",
            Self::SetDeckEffectParam { .. } => "set_deck_effect_param",
            Self::SetStemGain { .. } => "set_stem_gain",
            Self::SetStemMute { .. } => "set_stem_mute",
            Self::StartRecording { .. } => "start_recording",
            Self::StopRecording => "stop_recording",
        }
    }

    /// Reads a command from JSON.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// The command as JSON.
    pub fn to_json(&self) -> String {
        // Every field is a plain number, string, bool or path: serializing
        // cannot fail.
        serde_json::to_string(self).unwrap_or_default()
    }
}
