//! `poly.pad`: a Juno-style polysynth. One oscillator per voice gives a
//! sawtooth, a pulse and a sub-octave square; each voice has a four-pole
//! lowpass and one ADSR; the summed voices go through a stereo chorus.

use std::f32::consts::TAU;

use fourfloor_core::{
    pitch_to_hz, velocity_to_unit, Device, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig,
    ProcessIo, TransportContext,
};

use crate::dsp::{saw, square, Adsr, AdsrParams, Ladder};
use crate::runtime::{clamped, pick_voice, run, Synth, VoiceMeta};

#[derive(Clone, Copy, Debug)]
struct Voice {
    meta: VoiceMeta,
    pitch: f32,
    freq: f32,
    gain: f32,
    phase: f32,
    sub_phase: f32,
    age: u32,
    env: Adsr,
    filter: Ladder,
}

impl Voice {
    const IDLE: Self = Self {
        meta: VoiceMeta::IDLE,
        pitch: 60.0,
        freq: 0.0,
        gain: 0.0,
        phase: 0.0,
        sub_phase: 0.0,
        age: 0,
        env: Adsr::new(),
        filter: Ladder::new(),
    };
}

/// Stereo chorus after the Juno's: one short delay line read at two
/// points that a triangle LFO sweeps in opposite directions, one per side.
#[derive(Clone, Debug, Default)]
struct Chorus {
    line: Vec<f32>,
    write: usize,
    lfo_phase: f32,
    rate: f32,
}

impl Chorus {
    /// Longest delay the line must hold, in seconds.
    const MAX_DELAY: f32 = 0.012;
    /// Sweep range of the delay in seconds.
    const DELAY_RANGE: (f32, f32) = (0.001_66, 0.005_35);
    /// LFO rates of modes I and II in Hz.
    const LFO_HZ: [f32; 2] = [0.513, 0.863];

    fn prepare(&mut self, rate: f32) {
        self.rate = rate;
        let len = ((Self::MAX_DELAY * rate) as usize + 4).next_power_of_two();
        self.line.clear();
        self.line.resize(len, 0.0);
        self.reset();
    }

    fn reset(&mut self) {
        self.line.fill(0.0);
        self.write = 0;
        self.lfo_phase = 0.0;
    }

    fn read(&self, delay_samples: f32) -> f32 {
        let len = self.line.len();
        if len == 0 {
            return 0.0;
        }
        let mask = len - 1;
        let whole = delay_samples as usize;
        let frac = delay_samples - whole as f32;
        let newer = self.line[(self.write + len - whole) & mask];
        let older = self.line[(self.write + len - whole - 1) & mask];
        newer + (older - newer) * frac
    }

    /// `mode` 0 passes the input to both sides; 1 and 2 are chorus I, II.
    fn process(&mut self, input: f32, mode: usize) -> (f32, f32) {
        let len = self.line.len();
        if len == 0 {
            return (input, input);
        }
        self.write = (self.write + 1) & (len - 1);
        self.line[self.write] = input;
        let Some(lfo_hz) = mode.checked_sub(1).and_then(|i| Self::LFO_HZ.get(i)) else {
            return (input, input);
        };
        let triangle = 1.0 - 4.0 * (self.lfo_phase - 0.5).abs(); // -1..1
        self.lfo_phase += lfo_hz / self.rate;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }
        let (low, high) = Self::DELAY_RANGE;
        let centre = 0.5 * (low + high) * self.rate;
        let depth = 0.5 * (high - low) * self.rate;
        let left = self.read(centre + depth * triangle);
        let right = self.read(centre - depth * triangle);
        let gain = std::f32::consts::FRAC_1_SQRT_2;
        (gain * (input + left), gain * (input + right))
    }
}

/// Juno-style polysynth, eight voices. Kind `poly.pad`.
///
/// When all voices are busy a new note takes the oldest released voice,
/// or the oldest held one. With the chorus off the output is the same on
/// both channels; chorus I and II make it stereo.
#[derive(Clone, Debug)]
pub struct PolyPad {
    saw: f32,
    pulse: f32,
    sub: f32,
    pulse_width: f32,
    pwm: f32,
    lfo_hz: f32,
    vibrato_cents: f32,
    cutoff: f32,
    resonance: f32,
    env_amount: f32,
    key_track: f32,
    adsr: AdsrParams,
    chorus_mode: usize,
    level: f32,
    rate: f32,
    lfo_phase: f32,
    voices: [Voice; PolyPad::VOICES],
    counter: u64,
    chorus: Chorus,
}

static PARAMS: [ParamInfo; 17] = [
    ParamInfo::new(PolyPad::SAW, "Saw", 0.0, 1.0, 0.7, ParamUnit::Generic),
    ParamInfo::new(PolyPad::PULSE, "Pulse", 0.0, 1.0, 0.4, ParamUnit::Generic),
    ParamInfo::new(PolyPad::SUB, "Sub", 0.0, 1.0, 0.25, ParamUnit::Generic),
    ParamInfo::new(
        PolyPad::PULSE_WIDTH,
        "Pulse Width",
        5.0,
        95.0,
        50.0,
        ParamUnit::Percent,
    ),
    ParamInfo::new(PolyPad::PWM, "PWM", 0.0, 1.0, 0.3, ParamUnit::Generic),
    ParamInfo::new(
        PolyPad::LFO_RATE,
        "LFO Rate",
        0.05,
        20.0,
        0.6,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        PolyPad::VIBRATO,
        "Vibrato",
        0.0,
        100.0,
        0.0,
        ParamUnit::Cents,
    ),
    ParamInfo::new(
        PolyPad::CUTOFF,
        "Cutoff",
        20.0,
        20_000.0,
        1_800.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        PolyPad::RESONANCE,
        "Resonance",
        0.0,
        1.0,
        0.15,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        PolyPad::ENV_AMOUNT,
        "Env Amount",
        0.0,
        1.0,
        0.3,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        PolyPad::KEY_TRACK,
        "Key Track",
        0.0,
        1.0,
        0.5,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        PolyPad::ATTACK,
        "Attack",
        0.0,
        5.0,
        0.12,
        ParamUnit::Seconds,
    ),
    ParamInfo::new(PolyPad::DECAY, "Decay", 0.01, 10.0, 1.5, ParamUnit::Seconds).logarithmic(),
    ParamInfo::new(
        PolyPad::SUSTAIN,
        "Sustain",
        0.0,
        1.0,
        0.8,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        PolyPad::RELEASE,
        "Release",
        0.01,
        10.0,
        0.8,
        ParamUnit::Seconds,
    )
    .logarithmic(),
    ParamInfo::new(PolyPad::CHORUS, "Chorus", 0.0, 2.0, 1.0, ParamUnit::Integer),
    ParamInfo::new(PolyPad::LEVEL, "Level", 0.0, 1.0, 0.6, ParamUnit::Generic),
];

impl PolyPad {
    /// Sawtooth level, 0 to 1.
    pub const SAW: ParamId = ParamId(0);
    /// Pulse level, 0 to 1.
    pub const PULSE: ParamId = ParamId(1);
    /// Level of the square one octave down, 0 to 1.
    pub const SUB: ParamId = ParamId(2);
    /// Pulse width in percent.
    pub const PULSE_WIDTH: ParamId = ParamId(3);
    /// How far the LFO moves the pulse width, 0 to 1.
    pub const PWM: ParamId = ParamId(4);
    /// LFO rate in Hz.
    pub const LFO_RATE: ParamId = ParamId(5);
    /// Vibrato depth in cents.
    pub const VIBRATO: ParamId = ParamId(6);
    /// Filter cutoff in Hz for middle C.
    pub const CUTOFF: ParamId = ParamId(7);
    /// Filter resonance, 0 to 1.
    pub const RESONANCE: ParamId = ParamId(8);
    /// How far the envelope opens the filter, 0 to 1 (1 is five octaves).
    pub const ENV_AMOUNT: ParamId = ParamId(9);
    /// How much the cutoff follows the note, 0 to 1.
    pub const KEY_TRACK: ParamId = ParamId(10);
    /// Attack time in seconds.
    pub const ATTACK: ParamId = ParamId(11);
    /// Decay time in seconds (time to fall 60 dB towards the sustain).
    pub const DECAY: ParamId = ParamId(12);
    /// Sustain level, 0 to 1.
    pub const SUSTAIN: ParamId = ParamId(13);
    /// Release time in seconds (time to fall 60 dB).
    pub const RELEASE: ParamId = ParamId(14);
    /// Chorus: 0 off, 1 chorus I, 2 chorus II.
    pub const CHORUS: ParamId = ParamId(15);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(16);

    /// Notes that can sound at once.
    pub const VOICES: usize = 8;

    /// Samples between filter coefficient updates. Counted from each
    /// voice's note-on, so the result does not depend on the block size.
    const CONTROL_INTERVAL: u32 = 16;
    const ENV_OCTAVES: f32 = 5.0;
    const VOICE_GAIN: f32 = 0.25;

    /// A synth with default parameters.
    pub fn new() -> Self {
        let mut synth = Self {
            saw: 0.0,
            pulse: 0.0,
            sub: 0.0,
            pulse_width: 0.5,
            pwm: 0.0,
            lfo_hz: 1.0,
            vibrato_cents: 0.0,
            cutoff: 1_000.0,
            resonance: 0.0,
            env_amount: 0.0,
            key_track: 0.0,
            adsr: AdsrParams {
                attack: 0.0,
                decay: 1.0,
                sustain: 1.0,
                release: 0.1,
            },
            chorus_mode: 0,
            level: 0.0,
            rate: 48_000.0,
            lfo_phase: 0.0,
            voices: [Voice::IDLE; Self::VOICES],
            counter: 0,
            chorus: Chorus::default(),
        };
        for info in &PARAMS {
            synth.apply(info.id, info.default);
        }
        synth
    }

    /// Applies an already clamped value.
    fn apply(&mut self, id: ParamId, value: f32) {
        match id {
            Self::SAW => self.saw = value,
            Self::PULSE => self.pulse = value,
            Self::SUB => self.sub = value,
            Self::PULSE_WIDTH => self.pulse_width = 0.01 * value,
            Self::PWM => self.pwm = value,
            Self::LFO_RATE => self.lfo_hz = value,
            Self::VIBRATO => self.vibrato_cents = value,
            Self::CUTOFF => self.cutoff = value,
            Self::RESONANCE => self.resonance = value,
            Self::ENV_AMOUNT => self.env_amount = value,
            Self::KEY_TRACK => self.key_track = value,
            Self::ATTACK => self.adsr.attack = value,
            Self::DECAY => self.adsr.decay = value,
            Self::SUSTAIN => self.adsr.sustain = value,
            Self::RELEASE => self.adsr.release = value,
            Self::CHORUS => self.chorus_mode = value.round() as usize,
            Self::LEVEL => self.level = value,
            _ => return,
        }
        if (Self::ATTACK.0..=Self::RELEASE.0).contains(&id.0) {
            for voice in &mut self.voices {
                voice.env.set(&self.adsr, self.rate);
            }
        }
    }

    fn note_on(&mut self, pitch: u8, velocity: u8, channel: u8) {
        let index = pick_voice(self.voices.iter().map(|voice| &voice.meta));
        self.counter += 1;
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
        voice.pitch = f32::from(pitch);
        voice.freq = pitch_to_hz(voice.pitch);
        voice.gain = 0.3 + 0.7 * velocity_to_unit(velocity);
        voice.age = 0;
        if !stolen {
            // A stolen voice keeps its phase, filter and envelope level so
            // the takeover does not click.
            voice.phase = 0.0;
            voice.sub_phase = 0.0;
            voice.filter.reset();
            voice.env.reset();
        }
        voice.env.set(&self.adsr, self.rate);
        voice.env.gate_on();
    }

    fn release(&mut self, matches: impl Fn(&VoiceMeta) -> bool) {
        for voice in &mut self.voices {
            if voice.meta.active && voice.meta.gate && matches(&voice.meta) {
                voice.meta.gate = false;
                voice.env.gate_off();
            }
        }
    }
}

impl Default for PolyPad {
    fn default() -> Self {
        Self::new()
    }
}

impl Synth for PolyPad {
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
        let lfo = (TAU * self.lfo_phase).sin();
        self.lfo_phase += self.lfo_hz / self.rate;
        if self.lfo_phase >= 1.0 {
            self.lfo_phase -= 1.0;
        }
        let width = (self.pulse_width + 0.45 * self.pwm * lfo).clamp(0.05, 0.95);
        // Small-angle form of 2^(cents/1200): exact enough for vibrato.
        let vibrato = 1.0 + self.vibrato_cents * lfo * (std::f32::consts::LN_2 / 1_200.0);
        let max_cutoff = 0.45 * self.rate;

        let mut sum = 0.0;
        for voice in &mut self.voices {
            if !voice.meta.active {
                continue;
            }
            let env = voice.env.next();
            if voice.env.is_idle() {
                voice.meta.active = false;
                continue;
            }
            if voice.age.is_multiple_of(Self::CONTROL_INTERVAL) {
                let octaves = Self::ENV_OCTAVES * self.env_amount * env
                    + self.key_track * (voice.pitch - 60.0) / 12.0;
                let cutoff = (self.cutoff * 2.0f32.powf(octaves)).clamp(20.0, max_cutoff);
                voice.filter.set(cutoff, self.resonance, self.rate);
            }
            voice.age = voice.age.wrapping_add(1);

            let step = (voice.freq * vibrato / self.rate).min(0.45);
            // The pulse is inverted (low first) so that its fundamental
            // adds to the sawtooth's instead of cancelling it.
            let osc = self.saw * saw(voice.phase, step)
                - self.pulse * square(voice.phase, step, width)
                - self.sub * square(voice.sub_phase, 0.5 * step, 0.5);
            voice.phase += step;
            if voice.phase >= 1.0 {
                voice.phase -= 1.0;
            }
            voice.sub_phase += 0.5 * step;
            if voice.sub_phase >= 1.0 {
                voice.sub_phase -= 1.0;
            }
            sum += voice.filter.process(osc) * env * voice.gain;
        }
        let dry = sum * self.level * Self::VOICE_GAIN;
        self.chorus.process(dry, self.chorus_mode)
    }
}

impl Device for PolyPad {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.rate = config.sample_rate.hz() as f32;
        self.chorus.prepare(self.rate);
        for voice in &mut self.voices {
            voice.env.set(&self.adsr, self.rate);
        }
        Device::reset(self);
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        run(self, io);
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if let Some(value) = clamped(&PARAMS, param, value) {
            self.apply(param, value);
        }
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            voice.meta = VoiceMeta::IDLE;
            voice.env.reset();
            voice.filter.reset();
        }
        self.lfo_phase = 0.0;
        self.chorus.reset();
    }
}
