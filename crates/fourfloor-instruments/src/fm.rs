//! `fm.organ` and `fm.stab`: a four-operator, eight-voice FM synth with
//! two factory voicings, an M1-style organ bass and a house stab.

use std::f32::consts::TAU;

use fourfloor_core::{
    pitch_to_hz, velocity_to_unit, Device, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig,
    ProcessIo, TransportContext,
};

use crate::dsp::{t60_coeff, Adsr, AdsrParams};
use crate::runtime::{clamped, pick_voice, run, Synth, VoiceMeta};

const OPS: usize = 4;
const PARAM_COUNT: usize = 6 + 4 * OPS;

/// How the four operators are wired. Operator 1 is always a carrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Algorithm {
    /// 4 modulates 3 modulates 2 modulates 1.
    Stack,
    /// 4 modulates 3 and 2 modulates 1; 1 and 3 are heard.
    Pairs,
    /// 4 modulates 1; 1, 2 and 3 are heard.
    Organ,
    /// All four are heard, nothing modulates.
    Additive,
}

impl Algorithm {
    fn from_value(value: f32) -> Self {
        match value.round() as i32 {
            i32::MIN..=0 => Self::Stack,
            1 => Self::Pairs,
            2 => Self::Organ,
            _ => Self::Additive,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Operator {
    ratio: f32,
    level: f32,
    decay: f32,
    sustain: f32,
    decay_coeff: f32,
}

#[derive(Clone, Copy, Debug)]
struct Voice {
    meta: VoiceMeta,
    freq: f32,
    gain: f32,
    phase: [f32; OPS],
    /// Each operator's own envelope: 1 at note-on, falling to its sustain.
    op_env: [f32; OPS],
    feedback: [f32; 2],
    amp: Adsr,
}

impl Voice {
    const IDLE: Self = Self {
        meta: VoiceMeta::IDLE,
        freq: 0.0,
        gain: 0.0,
        phase: [0.0; OPS],
        op_env: [0.0; OPS],
        feedback: [0.0; 2],
        amp: Adsr::new(),
    };
}

/// Four-operator FM synth, eight voices. Kinds `fm.organ` and `fm.stab`
/// (the same engine with different starting parameters).
///
/// Each operator is a sine at `Ratio` times the note's frequency with its
/// own level and a decay-to-sustain envelope; `Algorithm` picks which
/// operators modulate which. One attack / release pair shapes the whole
/// voice. Output is the same on both channels.
#[derive(Clone, Debug)]
pub struct FmSynth {
    params: &'static [ParamInfo; PARAM_COUNT],
    algorithm: Algorithm,
    feedback: f32,
    attack: f32,
    release: f32,
    level: f32,
    velocity_sense: f32,
    ops: [Operator; OPS],
    rate: f32,
    voices: [Voice; FmSynth::VOICES],
    counter: u64,
}

/// Builds a parameter table. Operators are `(ratio, level, decay, sustain)`.
const fn table(
    algorithm: f32,
    feedback: f32,
    attack: f32,
    release: f32,
    ops: [(f32, f32, f32, f32); OPS],
) -> [ParamInfo; PARAM_COUNT] {
    /// Field `field` (0 ratio, 1 level, 2 decay, 3 sustain) of operator
    /// `index`. One descriptor per call: arrays of descriptors cannot be
    /// taken apart in a `const fn`.
    const fn op(
        index: usize,
        field: u32,
        name: &'static str,
        v: (f32, f32, f32, f32),
    ) -> ParamInfo {
        let id = ParamId(FmSynth::OP_BASE + FmSynth::OP_STRIDE * index as u32 + field);
        match field {
            0 => ParamInfo::new(id, name, 0.25, 16.0, v.0, ParamUnit::Ratio),
            1 => ParamInfo::new(id, name, 0.0, 1.0, v.1, ParamUnit::Generic),
            2 => ParamInfo::new(id, name, 0.01, 10.0, v.2, ParamUnit::Seconds).logarithmic(),
            _ => ParamInfo::new(id, name, 0.0, 1.0, v.3, ParamUnit::Generic),
        }
    }
    [
        ParamInfo::new(
            FmSynth::ALGORITHM,
            "Algorithm",
            0.0,
            3.0,
            algorithm,
            ParamUnit::Integer,
        ),
        ParamInfo::new(
            FmSynth::FEEDBACK,
            "Feedback",
            0.0,
            1.0,
            feedback,
            ParamUnit::Generic,
        ),
        ParamInfo::new(
            FmSynth::ATTACK,
            "Attack",
            0.0,
            2.0,
            attack,
            ParamUnit::Seconds,
        ),
        ParamInfo::new(
            FmSynth::RELEASE,
            "Release",
            0.005,
            5.0,
            release,
            ParamUnit::Seconds,
        )
        .logarithmic(),
        ParamInfo::new(FmSynth::LEVEL, "Level", 0.0, 1.0, 0.6, ParamUnit::Generic),
        ParamInfo::new(
            FmSynth::VELOCITY,
            "Velocity",
            0.0,
            1.0,
            0.7,
            ParamUnit::Generic,
        ),
        op(0, 0, "Op 1 Ratio", ops[0]),
        op(0, 1, "Op 1 Level", ops[0]),
        op(0, 2, "Op 1 Decay", ops[0]),
        op(0, 3, "Op 1 Sustain", ops[0]),
        op(1, 0, "Op 2 Ratio", ops[1]),
        op(1, 1, "Op 2 Level", ops[1]),
        op(1, 2, "Op 2 Decay", ops[1]),
        op(1, 3, "Op 2 Sustain", ops[1]),
        op(2, 0, "Op 3 Ratio", ops[2]),
        op(2, 1, "Op 3 Level", ops[2]),
        op(2, 2, "Op 3 Decay", ops[2]),
        op(2, 3, "Op 3 Sustain", ops[2]),
        op(3, 0, "Op 4 Ratio", ops[3]),
        op(3, 1, "Op 4 Level", ops[3]),
        op(3, 2, "Op 4 Decay", ops[3]),
        op(3, 3, "Op 4 Sustain", ops[3]),
    ]
}

/// M1-style organ bass: fundamental, octave and twelfth held like
/// drawbars, with a short FM knock on the attack.
static ORGAN_PARAMS: [ParamInfo; PARAM_COUNT] = table(
    2.0,
    0.0,
    0.001,
    0.06,
    [
        (1.0, 1.0, 1.0, 1.0),
        (2.0, 0.5, 0.6, 0.7),
        (3.0, 0.22, 0.3, 0.5),
        (1.0, 0.3, 0.07, 0.04),
    ],
);

/// House stab: two modulated pairs an octave apart whose brightness falls
/// away quickly.
static STAB_PARAMS: [ParamInfo; PARAM_COUNT] = table(
    1.0,
    0.15,
    0.002,
    0.2,
    [
        (1.0, 1.0, 1.5, 0.6),
        (1.0, 0.32, 0.25, 0.12),
        (2.0, 0.45, 0.5, 0.25),
        (3.0, 0.3, 0.15, 0.08),
    ],
);

impl FmSynth {
    /// Operator wiring, 0 to 3: stack (4>3>2>1), pairs (4>3, 2>1), organ
    /// (4>1, plus 2 and 3 heard directly), additive (all heard).
    pub const ALGORITHM: ParamId = ParamId(0);
    /// How much operator 4 modulates itself, 0 to 1.
    pub const FEEDBACK: ParamId = ParamId(1);
    /// Attack time of the voice in seconds.
    pub const ATTACK: ParamId = ParamId(2);
    /// Release time of the voice in seconds (time to fall 60 dB).
    pub const RELEASE: ParamId = ParamId(3);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(4);
    /// How much velocity changes loudness, 0 (not at all) to 1.
    pub const VELOCITY: ParamId = ParamId(5);

    /// Notes that can sound at once.
    pub const VOICES: usize = 8;

    const OP_BASE: u32 = 8;
    const OP_STRIDE: u32 = 4;
    /// Modulation index, in radians, of a modulator at level 1.
    const INDEX: f32 = 8.0;
    /// Feedback index, in radians, at a feedback of 1.
    const FEEDBACK_INDEX: f32 = 1.6;
    /// Headroom so a full chord stays under full scale.
    const VOICE_GAIN: f32 = 0.3;

    /// Frequency ratio of operator `op` (1 to 4).
    pub const fn op_ratio(op: u32) -> ParamId {
        ParamId(Self::OP_BASE + Self::OP_STRIDE * (op - 1))
    }

    /// Level of operator `op` (1 to 4): loudness for a carrier, modulation
    /// depth for a modulator.
    pub const fn op_level(op: u32) -> ParamId {
        ParamId(Self::OP_BASE + Self::OP_STRIDE * (op - 1) + 1)
    }

    /// Time for operator `op` (1 to 4) to fall to its sustain, in seconds.
    pub const fn op_decay(op: u32) -> ParamId {
        ParamId(Self::OP_BASE + Self::OP_STRIDE * (op - 1) + 2)
    }

    /// Level operator `op` (1 to 4) settles at while the note is held.
    pub const fn op_sustain(op: u32) -> ParamId {
        ParamId(Self::OP_BASE + Self::OP_STRIDE * (op - 1) + 3)
    }

    /// The organ bass voicing (`fm.organ`).
    pub fn organ() -> Self {
        Self::with_table(&ORGAN_PARAMS)
    }

    /// The stab voicing (`fm.stab`).
    pub fn stab() -> Self {
        Self::with_table(&STAB_PARAMS)
    }

    fn with_table(params: &'static [ParamInfo; PARAM_COUNT]) -> Self {
        let mut synth = Self {
            params,
            algorithm: Algorithm::Stack,
            feedback: 0.0,
            attack: 0.0,
            release: 0.1,
            level: 0.0,
            velocity_sense: 0.0,
            ops: [Operator {
                ratio: 1.0,
                level: 0.0,
                decay: 1.0,
                sustain: 1.0,
                decay_coeff: 0.0,
            }; OPS],
            rate: 48_000.0,
            voices: [Voice::IDLE; Self::VOICES],
            counter: 0,
        };
        for info in params {
            synth.apply(info.id, info.default);
        }
        synth
    }

    fn adsr(&self) -> AdsrParams {
        AdsrParams {
            attack: self.attack,
            decay: 1.0,
            sustain: 1.0,
            release: self.release,
        }
    }

    fn refresh_envelopes(&mut self) {
        let params = self.adsr();
        for voice in &mut self.voices {
            voice.amp.set(&params, self.rate);
        }
        for op in &mut self.ops {
            op.decay_coeff = t60_coeff(op.decay, self.rate);
        }
    }

    /// Applies an already clamped value.
    fn apply(&mut self, id: ParamId, value: f32) {
        match id {
            Self::ALGORITHM => self.algorithm = Algorithm::from_value(value),
            Self::FEEDBACK => self.feedback = value,
            Self::ATTACK => self.attack = value,
            Self::RELEASE => self.release = value,
            Self::LEVEL => self.level = value,
            Self::VELOCITY => self.velocity_sense = value,
            ParamId(raw) if raw >= Self::OP_BASE => {
                let index = ((raw - Self::OP_BASE) / Self::OP_STRIDE) as usize;
                let Some(op) = self.ops.get_mut(index) else {
                    return;
                };
                match (raw - Self::OP_BASE) % Self::OP_STRIDE {
                    0 => op.ratio = value,
                    1 => op.level = value,
                    2 => op.decay = value,
                    _ => op.sustain = value,
                }
            }
            _ => return,
        }
        self.refresh_envelopes();
    }

    fn note_on(&mut self, pitch: u8, velocity: u8, channel: u8) {
        let index = pick_voice(self.voices.iter().map(|voice| &voice.meta));
        self.counter += 1;
        let velocity = velocity_to_unit(velocity);
        let params = self.adsr();
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        let stolen = voice.meta.active;
        voice.meta = VoiceMeta {
            pitch,
            channel,
            started: self.counter,
            gate: true,
            active: true,
        };
        voice.freq = pitch_to_hz(f32::from(pitch));
        voice.gain = 1.0 - self.velocity_sense * (1.0 - velocity);
        voice.phase = [0.0; OPS];
        voice.op_env = [1.0; OPS];
        voice.feedback = [0.0; 2];
        if !stolen {
            voice.amp.reset();
        }
        voice.amp.set(&params, self.rate);
        voice.amp.gate_on();
    }

    fn release(&mut self, matches: impl Fn(&VoiceMeta) -> bool) {
        for voice in &mut self.voices {
            if voice.meta.active && voice.meta.gate && matches(&voice.meta) {
                voice.meta.gate = false;
                voice.amp.gate_off();
            }
        }
    }

    fn render_voice(&self, voice: &mut Voice) -> f32 {
        let amp = voice.amp.next();
        if voice.amp.is_idle() {
            voice.meta.active = false;
            return 0.0;
        }
        let index = Self::INDEX / TAU;
        let mut level = [0.0f32; OPS];
        for ((level, op), env) in level.iter_mut().zip(&self.ops).zip(&mut voice.op_env) {
            *level = op.level * *env;
            *env = op.sustain + (*env - op.sustain) * op.decay_coeff;
        }
        let sine = |phase: f32, modulation: f32| (TAU * (phase + modulation)).sin();

        let fb = self.feedback * Self::FEEDBACK_INDEX / TAU
            * 0.5
            * (voice.feedback[0] + voice.feedback[1]);
        let op4 = sine(voice.phase[3], fb);
        voice.feedback = [op4, voice.feedback[0]];
        let op4 = op4 * level[3];

        let out = match self.algorithm {
            Algorithm::Stack => {
                let op3 = sine(voice.phase[2], op4 * index) * level[2];
                let op2 = sine(voice.phase[1], op3 * index) * level[1];
                sine(voice.phase[0], op2 * index) * level[0]
            }
            Algorithm::Pairs => {
                let op3 = sine(voice.phase[2], op4 * index) * level[2];
                let op2 = sine(voice.phase[1], 0.0) * level[1];
                sine(voice.phase[0], op2 * index) * level[0] + op3
            }
            Algorithm::Organ => {
                sine(voice.phase[0], op4 * index) * level[0]
                    + sine(voice.phase[1], 0.0) * level[1]
                    + sine(voice.phase[2], 0.0) * level[2]
            }
            Algorithm::Additive => {
                sine(voice.phase[0], 0.0) * level[0]
                    + sine(voice.phase[1], 0.0) * level[1]
                    + sine(voice.phase[2], 0.0) * level[2]
                    + op4
            }
        };
        for (phase, op) in voice.phase.iter_mut().zip(&self.ops) {
            *phase += (voice.freq * op.ratio / self.rate).min(0.49);
            if *phase >= 1.0 {
                *phase -= 1.0;
            }
        }
        out * amp * voice.gain
    }
}

impl Synth for FmSynth {
    fn handle_event(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn {
                pitch,
                velocity,
                channel,
            } => {
                if velocity == 0 {
                    self.release(|meta| meta.matches(pitch, channel));
                } else {
                    self.note_on(pitch, velocity, channel);
                }
            }
            EventKind::NoteOff { pitch, channel, .. } => {
                self.release(|meta| meta.matches(pitch, channel));
            }
            EventKind::AllNotesOff => self.release(|_| true),
            EventKind::ControlChange { .. } | EventKind::PitchBend { .. } => {}
        }
    }

    fn tick(&mut self) -> (f32, f32) {
        let mut sum = 0.0;
        let mut voices = self.voices;
        for voice in &mut voices {
            if voice.meta.active {
                sum += self.render_voice(voice);
            }
        }
        self.voices = voices;
        let out = sum * self.level * Self::VOICE_GAIN;
        (out, out)
    }
}

impl Device for FmSynth {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.rate = config.sample_rate.hz() as f32;
        self.refresh_envelopes();
        Device::reset(self);
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        run(self, io);
    }

    fn params(&self) -> &[ParamInfo] {
        self.params
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if let Some(value) = clamped(self.params, param, value) {
            self.apply(param, value);
        }
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.meta = VoiceMeta::IDLE;
            voice.amp.reset();
        }
    }
}
