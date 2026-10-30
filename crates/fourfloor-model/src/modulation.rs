//! Things that move parameters: modulators (LFOs and envelopes), macro
//! controls and MIDI controller mappings.

use fourfloor_core::{DeviceId, ParamId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{MacroId, ModulatorId, SceneId};

/// The waveform of an LFO. Every shape swings between -1 and 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LfoShape {
    /// A sine wave, starting at zero and rising.
    Sine,
    /// A triangle wave, starting at zero and rising.
    Triangle,
    /// A ramp from -1 up to 1.
    SawUp,
    /// A ramp from 1 down to -1.
    SawDown,
    /// 1 for the first half of the cycle, -1 for the second.
    Square,
}

/// How fast an LFO runs.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "unit", content = "value", rename_all = "snake_case")]
pub enum LfoRate {
    /// Cycles per second, 0.001 to 1000. Not tied to the tempo.
    Hertz(f64),
    /// Length of one cycle in beats, 1/64 to 1024. Follows the tempo and
    /// the playhead, so the cycle always starts on the grid.
    Beats(f64),
}

/// What a modulator produces.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModSource {
    /// A low-frequency oscillator, swinging between -1 and 1.
    Lfo {
        /// The waveform.
        shape: LfoShape,
        /// The speed.
        rate: LfoRate,
        /// Where in the cycle the LFO starts, 0 to 1.
        #[serde(default)]
        phase: f64,
    },
    /// An attack-decay-sustain-release envelope, from 0 to 1, started by
    /// every note on a track and released when its last note ends.
    Envelope {
        /// The MIDI track whose notes trigger the envelope.
        #[schemars(with = "u64")]
        trigger: TrackId,
        /// Time to rise from 0 to 1, in milliseconds (0 to 60000).
        attack_ms: f64,
        /// Time to fall from 1 to the sustain level, in milliseconds.
        decay_ms: f64,
        /// Level held while a note is down, 0 to 1.
        sustain: f64,
        /// Time to fall back to 0 after the last note ends, in milliseconds.
        release_ms: f64,
    },
}

/// One parameter a modulator moves.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModTarget {
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter.
    #[schemars(with = "u32")]
    pub param: ParamId,
    /// How far the modulator moves the parameter, as a fraction of its
    /// range: -1 to 1. The parameter's own value is the centre.
    pub depth: f32,
}

/// An LFO or envelope that can move any number of parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Modulator {
    /// Unique id.
    pub id: ModulatorId,
    /// Display name.
    pub name: String,
    /// What it produces.
    pub source: ModSource,
    /// The parameters it moves, at most one entry per parameter.
    pub targets: Vec<ModTarget>,
}

impl Modulator {
    /// Looks up the entry for one parameter.
    pub fn target(&self, device: DeviceId, param: ParamId) -> Option<&ModTarget> {
        self.targets
            .iter()
            .find(|target| target.device == device && target.param == param)
    }
}

/// One parameter a macro drives.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MacroTarget {
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter.
    #[schemars(with = "u32")]
    pub param: ParamId,
    /// The parameter's value when the macro is at 0.
    pub min: f32,
    /// The parameter's value when the macro is at 1. May be below `min`
    /// to make the parameter fall as the macro rises.
    pub max: f32,
}

impl MacroTarget {
    /// The parameter value for a macro position from 0 to 1.
    pub fn value_at(&self, position: f32) -> f32 {
        self.min + (self.max - self.min) * position.clamp(0.0, 1.0)
    }
}

/// One control that sets several parameters at once.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Macro {
    /// Unique id.
    pub id: MacroId,
    /// Display name.
    pub name: String,
    /// Position, 0 to 1.
    pub value: f32,
    /// The parameters it drives, at most one entry per parameter.
    pub targets: Vec<MacroTarget>,
}

impl Macro {
    /// Looks up the entry for one parameter.
    pub fn target(&self, device: DeviceId, param: ParamId) -> Option<&MacroTarget> {
        self.targets
            .iter()
            .find(|target| target.device == device && target.param == param)
    }
}

/// A MIDI controller (a knob or fader on a hardware surface) bound to a
/// device parameter.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ControllerMapping {
    /// MIDI channel 0 to 15, or null to accept the controller on any channel.
    #[serde(default)]
    pub channel: Option<u8>,
    /// MIDI controller number, 0 to 127.
    pub controller: u8,
    /// The device that owns the parameter.
    #[schemars(with = "u64")]
    pub device: DeviceId,
    /// The parameter.
    #[schemars(with = "u32")]
    pub param: ParamId,
    /// Parameter value when the controller is at 0.
    pub min: f32,
    /// Parameter value when the controller is at 127.
    pub max: f32,
}

impl ControllerMapping {
    /// Whether this mapping answers to a controller message.
    pub fn matches(&self, channel: u8, controller: u8) -> bool {
        self.controller == controller && self.channel.is_none_or(|own| own == channel)
    }

    /// The parameter value for a controller value from 0 to 127.
    pub fn value_at(&self, value: u8) -> f32 {
        self.min + (self.max - self.min) * (f32::from(value.min(127)) / 127.0)
    }
}

/// One row of the clip launcher.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scene {
    /// Unique id.
    pub id: SceneId,
    /// Display name.
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macro_and_controller_targets_interpolate_their_ranges() {
        let target = MacroTarget {
            device: DeviceId::new(1),
            param: ParamId::new(2),
            min: 200.0,
            max: 1_000.0,
        };
        assert_eq!(target.value_at(0.0), 200.0);
        assert_eq!(target.value_at(0.5), 600.0);
        assert_eq!(target.value_at(7.0), 1_000.0);

        let mapping = ControllerMapping {
            channel: Some(2),
            controller: 74,
            device: DeviceId::new(1),
            param: ParamId::new(2),
            min: 1.0,
            max: 0.0,
        };
        assert!(mapping.matches(2, 74) && !mapping.matches(3, 74) && !mapping.matches(2, 75));
        assert_eq!(mapping.value_at(0), 1.0);
        assert_eq!(mapping.value_at(127), 0.0);
        let omni = ControllerMapping {
            channel: None,
            ..mapping
        };
        assert!(omni.matches(9, 74));
    }

    #[test]
    fn sources_have_a_tagged_wire_format() {
        let lfo = ModSource::Lfo {
            shape: LfoShape::Sine,
            rate: LfoRate::Beats(4.0),
            phase: 0.0,
        };
        assert_eq!(
            serde_json::to_value(lfo).unwrap(),
            serde_json::json!({
                "type": "lfo",
                "shape": "sine",
                "rate": { "unit": "beats", "value": 4.0 },
                "phase": 0.0
            })
        );
        let decoded: ModSource = serde_json::from_value(serde_json::json!({
            "type": "lfo", "shape": "square", "rate": { "unit": "hertz", "value": 2.0 }
        }))
        .unwrap();
        assert_eq!(
            decoded,
            ModSource::Lfo {
                shape: LfoShape::Square,
                rate: LfoRate::Hertz(2.0),
                phase: 0.0
            }
        );
    }
}
