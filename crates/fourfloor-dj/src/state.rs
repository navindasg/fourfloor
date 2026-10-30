//! A serializable snapshot of a DJ session, for the UI and the agent.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::command::{DjCommand, EqBand};
use crate::deck::SyncMode;
use crate::mixer::{CrossfaderCurve, CrossfaderSide};

/// What is loaded in a deck.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackSummary {
    /// Display name.
    pub name: String,
    /// File the track came from, when it came from one.
    pub path: Option<PathBuf>,
    /// Tempo at normal speed, when known.
    pub bpm: Option<f64>,
    /// Key name, when known.
    pub key: Option<String>,
    /// Camelot code, when the key is known.
    pub camelot: Option<String>,
    /// Length in seconds.
    pub duration_seconds: f64,
}

/// One stem's mix settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StemState {
    /// Stem name.
    pub name: String,
    /// Linear gain.
    pub gain: f32,
    /// Muted (the gain is kept).
    pub muted: bool,
}

/// One deck effect slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EffectState {
    /// Registry kind, e.g. `"fx.delay"`.
    pub kind: String,
    /// Switched on.
    pub enabled: bool,
    /// Every parameter by name, in its own units.
    pub params: BTreeMap<String, f32>,
}

/// A loop, in seconds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoopState {
    /// Loop start.
    pub start_seconds: f64,
    /// Loop end.
    pub end_seconds: f64,
    /// Whether it is a loop roll.
    pub roll: bool,
}

/// One deck: its settings and its live playback state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeckState {
    /// Deck index.
    pub index: usize,
    /// The loaded track.
    pub track: Option<TrackSummary>,
    /// Playing.
    pub playing: bool,
    /// Stopped at the end of the track.
    pub at_end: bool,
    /// Playhead in seconds.
    pub position_seconds: f64,
    /// Beat position in the track's grid, when it has one.
    pub beat: Option<f64>,
    /// Pitch fader in percent.
    pub pitch_percent: f64,
    /// Playback rate actually used (1 is normal), including sync.
    pub rate: f64,
    /// Sounding tempo.
    pub bpm: Option<f64>,
    /// Key lock on.
    pub key_lock: bool,
    /// Sync mode.
    pub sync: SyncMode,
    /// Quantize on.
    pub quantize: bool,
    /// Cue point in seconds.
    pub cue_seconds: f64,
    /// Hot cues in seconds, eight slots.
    pub hot_cues: Vec<Option<f64>>,
    /// The active loop.
    #[serde(rename = "loop")]
    pub loop_state: Option<LoopState>,
    /// Isolator gains in dB, low, mid, high (the value kept under a kill).
    pub eq_db: [f32; 3],
    /// Isolator kills, low, mid, high.
    pub eq_kill: [bool; 3],
    /// DJ filter knob.
    pub filter: f32,
    /// Channel fader.
    pub fader: f32,
    /// Trim in dB.
    pub trim_db: f32,
    /// On the cue bus.
    pub cue_monitor: bool,
    /// Crossfader side.
    pub crossfader_side: CrossfaderSide,
    /// Effect slots.
    pub effects: Vec<EffectState>,
    /// Stems of the loaded track.
    pub stems: Vec<StemState>,
}

/// The mixer's global controls.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MixerState {
    /// Crossfader position, -1 to 1.
    pub crossfader: f32,
    /// Crossfader curve.
    pub curve: CrossfaderCurve,
    /// Master level in dB.
    pub master_db: f32,
    /// Headphone blend, 0 cue to 1 master.
    pub cue_mix: f32,
    /// Headphone level in dB.
    pub headphone_db: f32,
}

impl Default for MixerState {
    fn default() -> Self {
        Self {
            crossfader: 0.0,
            curve: CrossfaderCurve::ConstantPower,
            master_db: 0.0,
            cue_mix: 0.0,
            headphone_db: 0.0,
        }
    }
}

/// The whole session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    /// Engine sample rate.
    pub sample_rate: u32,
    /// The decks.
    pub decks: Vec<DeckState>,
    /// The mixer.
    pub mixer: MixerState,
    /// The tempo master.
    pub master: Option<usize>,
    /// Tempo of the master as it sounds now.
    pub master_bpm: Option<f64>,
    /// The file being recorded.
    pub recording: Option<PathBuf>,
    /// Which key lock this build uses (`"wsola"` or `"signalsmith"`).
    pub key_lock_engine: String,
}

impl SessionState {
    /// The state as pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Commands that bring another session's mixer and deck settings to
    /// this state (tracks, playback, cues and loops are not included).
    pub fn settings_commands(&self) -> Vec<DjCommand> {
        let mixer = &self.mixer;
        let mut commands = vec![
            DjCommand::SetCrossfader {
                position: mixer.crossfader,
            },
            DjCommand::SetCrossfaderCurve { curve: mixer.curve },
            DjCommand::SetMasterGain {
                db: mixer.master_db,
            },
            DjCommand::SetCueMix {
                value: mixer.cue_mix,
            },
            DjCommand::SetHeadphoneGain {
                db: mixer.headphone_db,
            },
        ];
        for deck in &self.decks {
            let index = deck.index;
            commands.extend([
                DjCommand::SetPitch {
                    deck: index,
                    percent: deck.pitch_percent,
                },
                DjCommand::SetKeyLock {
                    deck: index,
                    on: deck.key_lock,
                },
                DjCommand::SetQuantize {
                    deck: index,
                    on: deck.quantize,
                },
                DjCommand::SetFilter {
                    deck: index,
                    amount: deck.filter,
                },
                DjCommand::SetFader {
                    deck: index,
                    value: deck.fader,
                },
                DjCommand::SetTrim {
                    deck: index,
                    db: deck.trim_db,
                },
                DjCommand::SetCueMonitor {
                    deck: index,
                    on: deck.cue_monitor,
                },
                DjCommand::SetCrossfaderSide {
                    deck: index,
                    side: deck.crossfader_side,
                },
            ]);
            for band in EqBand::ALL {
                commands.push(DjCommand::SetEq {
                    deck: index,
                    band,
                    db: deck.eq_db[band.index()],
                });
                commands.push(DjCommand::SetEqKill {
                    deck: index,
                    band,
                    on: deck.eq_kill[band.index()],
                });
            }
            for (slot, effect) in deck.effects.iter().enumerate() {
                for (param, &value) in &effect.params {
                    commands.push(DjCommand::SetDeckEffectParam {
                        deck: index,
                        slot,
                        param: param.clone(),
                        value,
                    });
                }
                commands.push(DjCommand::SetDeckEffect {
                    deck: index,
                    slot,
                    on: effect.enabled,
                });
            }
        }
        commands.push(DjCommand::SetMaster { deck: self.master });
        for deck in &self.decks {
            commands.push(DjCommand::SetSync {
                deck: deck.index,
                mode: deck.sync,
            });
        }
        commands
    }
}
