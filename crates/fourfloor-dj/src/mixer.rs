//! The DJ mixer device: channel faders, trims, crossfader with curves, and
//! the split into a master bus and a cue (headphone) bus.
//!
//! The mixer has one input port per deck. Its output carries the master
//! mix on channels 0 and 1 and, when the graph has four or more channels,
//! the headphone mix on channels 2 and 3.

use std::f32::consts::FRAC_PI_2;

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};
use serde::{Deserialize, Serialize};

use crate::sync::MAX_DECKS;

/// Shape of the crossfader's gain laws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossfaderCurve {
    /// Amplitudes add to one: each side is -6 dB at the centre.
    Linear,
    /// Powers add to one: each side is -3 dB at the centre.
    #[default]
    ConstantPower,
    /// Scratch cut: both sides at full level except within the last 5 %
    /// of travel, where one cuts out.
    Sharp,
}

impl CrossfaderCurve {
    /// Parameter value of the curve.
    pub const fn param_value(self) -> f32 {
        match self {
            Self::Linear => 0.0,
            Self::ConstantPower => 1.0,
            Self::Sharp => 2.0,
        }
    }

    /// The curve of a parameter value.
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            0 => Self::Linear,
            2 => Self::Sharp,
            _ => Self::ConstantPower,
        }
    }
}

/// Which side of the crossfader a deck is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossfaderSide {
    /// Not affected by the crossfader.
    #[default]
    Thru,
    /// The left side.
    A,
    /// The right side.
    B,
}

impl CrossfaderSide {
    /// Parameter value of the side.
    pub const fn param_value(self) -> f32 {
        match self {
            Self::Thru => 0.0,
            Self::A => 1.0,
            Self::B => 2.0,
        }
    }

    /// The side of a parameter value.
    pub fn from_param(value: f32) -> Self {
        match value.round() as i32 {
            1 => Self::A,
            2 => Self::B,
            _ => Self::Thru,
        }
    }
}

/// Share of crossfader travel over which [`CrossfaderCurve::Sharp`] cuts.
pub const SHARP_CUT: f32 = 0.05;

/// Gains of the A and B sides for a crossfader at `position` (-1 is fully
/// A, 1 fully B).
pub fn crossfader_gains(position: f32, curve: CrossfaderCurve) -> (f32, f32) {
    let t = if position.is_finite() {
        (position.clamp(-1.0, 1.0) + 1.0) * 0.5
    } else {
        0.5
    };
    match curve {
        CrossfaderCurve::Linear => (1.0 - t, t),
        CrossfaderCurve::ConstantPower => ((t * FRAC_PI_2).cos(), (t * FRAC_PI_2).sin()),
        CrossfaderCurve::Sharp => (
            ((1.0 - t) / SHARP_CUT).clamp(0.0, 1.0),
            (t / SHARP_CUT).clamp(0.0, 1.0),
        ),
    }
}

/// Gain of a channel fader at `position` (0 to 1): a square law, so the
/// middle of the fader is -12 dB.
pub fn fader_gain(position: f32) -> f32 {
    let position = if position.is_finite() {
        position.clamp(0.0, 1.0)
    } else {
        0.0
    };
    position * position
}

fn db_to_gain(db: f32) -> f32 {
    if db <= -96.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// Parameter ids of a deck's channel on the mixer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelParams {
    /// Channel fader, 0 to 1.
    pub fader: ParamId,
    /// Trim in dB.
    pub trim: ParamId,
    /// Whether the channel goes to the cue bus.
    pub cue: ParamId,
    /// Crossfader side ([`CrossfaderSide::param_value`]).
    pub side: ParamId,
}

const fn fader(deck: u32, name: &'static str) -> ParamInfo {
    ParamInfo::new(ParamId(deck * 16), name, 0.0, 1.0, 1.0, ParamUnit::Generic)
}

const fn trim(deck: u32, name: &'static str) -> ParamInfo {
    ParamInfo::new(
        ParamId(deck * 16 + 1),
        name,
        -24.0,
        12.0,
        0.0,
        ParamUnit::Decibels,
    )
}

const fn cue(deck: u32, name: &'static str) -> ParamInfo {
    ParamInfo::toggle(ParamId(deck * 16 + 2), name, false)
}

const fn side(deck: u32, name: &'static str, default: f32) -> ParamInfo {
    ParamInfo::new(
        ParamId(deck * 16 + 3),
        name,
        0.0,
        2.0,
        default,
        ParamUnit::Integer,
    )
}

const CHANNEL_PARAMS: usize = 4;
const GLOBAL_PARAMS: usize = 5;
const PARAM_COUNT: usize = MAX_DECKS * CHANNEL_PARAMS + GLOBAL_PARAMS;

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    fader(0, "Deck 1 Fader"),
    trim(0, "Deck 1 Trim"),
    cue(0, "Deck 1 Cue"),
    side(0, "Deck 1 Side", 1.0),
    fader(1, "Deck 2 Fader"),
    trim(1, "Deck 2 Trim"),
    cue(1, "Deck 2 Cue"),
    side(1, "Deck 2 Side", 2.0),
    fader(2, "Deck 3 Fader"),
    trim(2, "Deck 3 Trim"),
    cue(2, "Deck 3 Cue"),
    side(2, "Deck 3 Side", 0.0),
    fader(3, "Deck 4 Fader"),
    trim(3, "Deck 4 Trim"),
    cue(3, "Deck 4 Cue"),
    side(3, "Deck 4 Side", 0.0),
    ParamInfo::new(
        DjMixer::CROSSFADER,
        "Crossfader",
        -1.0,
        1.0,
        0.0,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        DjMixer::CURVE,
        "Crossfader Curve",
        0.0,
        2.0,
        1.0,
        ParamUnit::Integer,
    ),
    ParamInfo::new(
        DjMixer::MASTER,
        "Master",
        -60.0,
        6.0,
        0.0,
        ParamUnit::Decibels,
    ),
    ParamInfo::new(
        DjMixer::CUE_MIX,
        "Cue Mix",
        0.0,
        1.0,
        0.0,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        DjMixer::HEADPHONE,
        "Headphone",
        -60.0,
        6.0,
        0.0,
        ParamUnit::Decibels,
    ),
];

/// Gain glide time, in seconds.
const GLIDE_SECONDS: f32 = 0.005;

/// The DJ mixer (see the [module docs](self)).
#[derive(Clone, Debug)]
pub struct DjMixer {
    decks: usize,
    values: [f32; PARAM_COUNT],
    master_gains: [f32; MAX_DECKS],
    cue_gains: [f32; MAX_DECKS],
    master_out: f32,
    cue_mix: f32,
    headphone: f32,
    glide: f32,
}

impl DjMixer {
    /// Crossfader position, -1 (A) to 1 (B).
    pub const CROSSFADER: ParamId = ParamId(100);
    /// Crossfader curve ([`CrossfaderCurve::param_value`]).
    pub const CURVE: ParamId = ParamId(101);
    /// Master level in dB.
    pub const MASTER: ParamId = ParamId(102);
    /// Headphone blend: 0 is the cue bus only, 1 the master only.
    pub const CUE_MIX: ParamId = ParamId(103);
    /// Headphone level in dB.
    pub const HEADPHONE: ParamId = ParamId(104);

    /// A mixer for `decks` decks (clamped to 1..=[`MAX_DECKS`]). Deck 1 is
    /// on crossfader side A, deck 2 on B, the others thru.
    pub fn new(decks: usize) -> Self {
        let values = std::array::from_fn(|i| PARAMS[i].default);
        let mut mixer = Self {
            decks: decks.clamp(1, MAX_DECKS),
            values,
            master_gains: [0.0; MAX_DECKS],
            cue_gains: [0.0; MAX_DECKS],
            master_out: 1.0,
            cue_mix: 0.0,
            headphone: 1.0,
            glide: 1.0,
        };
        mixer.snap();
        mixer
    }

    /// Parameter ids of deck `deck`'s channel.
    pub const fn channel(deck: usize) -> ChannelParams {
        let base = (deck * 16) as u32;
        ChannelParams {
            fader: ParamId(base),
            trim: ParamId(base + 1),
            cue: ParamId(base + 2),
            side: ParamId(base + 3),
        }
    }

    fn index(param: ParamId) -> Option<usize> {
        PARAMS.iter().position(|info| info.id == param)
    }

    fn value(&self, param: ParamId) -> f32 {
        Self::index(param).map_or(0.0, |i| self.values[i])
    }

    /// Target gains of every deck on the master and the cue bus.
    fn targets(&self) -> ([f32; MAX_DECKS], [f32; MAX_DECKS]) {
        let curve = CrossfaderCurve::from_param(self.value(Self::CURVE));
        let (a, b) = crossfader_gains(self.value(Self::CROSSFADER), curve);
        let mut master = [0.0; MAX_DECKS];
        let mut cue = [0.0; MAX_DECKS];
        for deck in 0..self.decks {
            let ids = Self::channel(deck);
            let trim = db_to_gain(self.value(ids.trim));
            let side = match CrossfaderSide::from_param(self.value(ids.side)) {
                CrossfaderSide::Thru => 1.0,
                CrossfaderSide::A => a,
                CrossfaderSide::B => b,
            };
            master[deck] = trim * fader_gain(self.value(ids.fader)) * side;
            cue[deck] = if self.value(ids.cue) >= 0.5 {
                trim
            } else {
                0.0
            };
        }
        (master, cue)
    }

    /// Jumps every gain to its target (no glide).
    fn snap(&mut self) {
        let (master, cue) = self.targets();
        self.master_gains = master;
        self.cue_gains = cue;
        self.master_out = db_to_gain(self.value(Self::MASTER));
        self.cue_mix = self.value(Self::CUE_MIX);
        self.headphone = db_to_gain(self.value(Self::HEADPHONE));
    }
}

impl Device for DjMixer {
    fn prepare(&mut self, config: &PrepareConfig) {
        let rate = config.sample_rate.as_f64() as f32;
        self.glide = 1.0 - (-1.0 / (GLIDE_SECONDS * rate)).exp();
        self.snap();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let frames = io.frames();
        let channels = io.channels();
        if channels < 2 {
            io.silence_outputs();
            return;
        }
        let (master_target, cue_target) = self.targets();
        let master_level = db_to_gain(self.value(Self::MASTER));
        let cue_mix = self.value(Self::CUE_MIX).clamp(0.0, 1.0);
        let headphone = db_to_gain(self.value(Self::HEADPHONE));
        let ports = (io.inputs.len() / channels).min(self.decks);
        let glide = self.glide;
        for frame in 0..frames {
            let (mut master_l, mut master_r, mut cue_l, mut cue_r) = (0.0, 0.0, 0.0, 0.0);
            for deck in 0..ports {
                self.master_gains[deck] += (master_target[deck] - self.master_gains[deck]) * glide;
                self.cue_gains[deck] += (cue_target[deck] - self.cue_gains[deck]) * glide;
                let left = io.inputs[deck * channels][frame];
                let right = io.inputs[deck * channels + 1][frame];
                master_l += left * self.master_gains[deck];
                master_r += right * self.master_gains[deck];
                cue_l += left * self.cue_gains[deck];
                cue_r += right * self.cue_gains[deck];
            }
            self.master_out += (master_level - self.master_out) * glide;
            self.cue_mix += (cue_mix - self.cue_mix) * glide;
            self.headphone += (headphone - self.headphone) * glide;
            let (out_l, out_r) = (master_l * self.master_out, master_r * self.master_out);
            io.outputs[0][frame] = out_l;
            io.outputs[1][frame] = out_r;
            if channels >= 4 {
                let blend = self.cue_mix;
                io.outputs[2][frame] = (cue_l * (1.0 - blend) + out_l * blend) * self.headphone;
                io.outputs[3][frame] = (cue_r * (1.0 - blend) + out_r * blend) * self.headphone;
            }
        }
        for channel in io
            .outputs
            .iter_mut()
            .skip(if channels >= 4 { 4 } else { 2 })
        {
            channel.fill(0.0);
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if let Some(i) = Self::index(param) {
            self.values[i] = PARAMS[i].clamp(value);
        }
    }

    fn reset(&mut self) {
        self.snap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_have_their_documented_centre_and_ends() {
        let (a, b) = crossfader_gains(0.0, CrossfaderCurve::Linear);
        assert!((a - 0.5).abs() < 1e-6 && (b - 0.5).abs() < 1e-6);
        let (a, b) = crossfader_gains(0.0, CrossfaderCurve::ConstantPower);
        assert!((a * a + b * b - 1.0).abs() < 1e-6);
        assert!((20.0 * a.log10() + 3.01).abs() < 0.01);
        assert_eq!(crossfader_gains(0.0, CrossfaderCurve::Sharp), (1.0, 1.0));
        for curve in [
            CrossfaderCurve::Linear,
            CrossfaderCurve::ConstantPower,
            CrossfaderCurve::Sharp,
        ] {
            let (a, b) = crossfader_gains(-1.0, curve);
            assert!((a - 1.0).abs() < 1e-6 && b.abs() < 1e-6, "{curve:?}");
            let (a, b) = crossfader_gains(1.0, curve);
            assert!(a.abs() < 1e-6 && (b - 1.0).abs() < 1e-6, "{curve:?}");
        }
        assert_eq!(
            crossfader_gains(f32::NAN, CrossfaderCurve::Linear),
            (0.5, 0.5)
        );
    }

    #[test]
    fn params_have_unique_ids_and_round_trip_enums() {
        for (i, a) in PARAMS.iter().enumerate() {
            for b in &PARAMS[i + 1..] {
                assert_ne!(a.id, b.id);
            }
        }
        for curve in [
            CrossfaderCurve::Linear,
            CrossfaderCurve::ConstantPower,
            CrossfaderCurve::Sharp,
        ] {
            assert_eq!(CrossfaderCurve::from_param(curve.param_value()), curve);
        }
        for side in [CrossfaderSide::Thru, CrossfaderSide::A, CrossfaderSide::B] {
            assert_eq!(CrossfaderSide::from_param(side.param_value()), side);
        }
        assert_eq!(fader_gain(0.5), 0.25);
        assert_eq!(fader_gain(f32::NAN), 0.0);
    }
}
