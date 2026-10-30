//! `synth.acid303`: a monophonic TB-303-style bass line synth.
//!
//! The signal path and the control logic follow Open303 (MIT, Copyright (c)
//! 2009 Robin Schmidt, <https://github.com/RobinSchmidt/Open303>):
//! oscillator, 44 Hz high-pass, the `TeeBeeFilter` ladder at four times
//! the sample rate, anti-alias filter, then allpass, 24 Hz high-pass and
//! 7.5 Hz notch at the base rate; one decay envelope drives the cutoff
//! (with Open303's measured envelope-modulation mapping) and, through a
//! 15 ms smoother, the accent; the amplitude envelope, accent gain, slide
//! and note handling are Open303's.
//!
//! Differences from Open303, all deliberate:
//! - The oscillator is a polyBLEP saw / square blend, not Open303's
//!   wavetables shaped after the 303's own waveforms.
//! - The anti-alias filter is a sixth-order Butterworth, not Open303's
//!   elliptic quarter-band filter.
//! - The built-in sequencer is not ported; notes come from events.
//! - The voice goes idle (and restarts cleanly) once it has faded out.

mod parts;

use fourfloor_core::{
    Device, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::runtime::{clamped, run, Synth};
use parts::{AmpEnvelope, Biquad, DecayEnvelope, LeakyIntegrator, NoteList, OnePole, TeeBeeFilter};

/// Monophonic 303-style synth. Kind `synth.acid303`.
///
/// - **Accent:** a note with velocity 100 or more is accented: louder,
///   with a shorter, deeper filter sweep, by the amount of
///   [`Acid303::ACCENT`].
/// - **Slide:** a note that starts while another is still held glides to
///   its pitch without restarting the envelopes. Overlap two notes in a
///   clip to slide between them.
/// - Releasing a key while an older one is still held slides back to it.
///
/// Output is the same on both channels.
#[derive(Clone, Debug)]
pub struct Acid303 {
    waveform: f64,
    tune: f64,
    cutoff: f64,
    resonance: f64,
    env_mod: f64,
    decay_ms: f64,
    accent: f64,
    slide_ms: f64,
    level_db: f64,

    rate: f64,
    env_scaler: f64,
    env_offset: f64,
    amp_scaler: f64,

    phase: f64,
    osc_freq: f64,
    bend_factor: f64,
    accent_gain: f64,
    idle: bool,
    notes: NoteList,

    pitch_slew: LeakyIntegrator,
    main_env: DecayEnvelope,
    accent_smoother: LeakyIntegrator,
    amp_env: AmpEnvelope,
    declicker: Biquad,
    highpass1: OnePole,
    filter: TeeBeeFilter,
    anti_alias: [Biquad; 3],
    allpass: OnePole,
    highpass2: OnePole,
    notch: Biquad,
}

static PARAMS: [ParamInfo; 9] = [
    ParamInfo::new(
        Acid303::WAVEFORM,
        "Waveform",
        0.0,
        1.0,
        0.0,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        Acid303::TUNE,
        "Tune",
        -24.0,
        24.0,
        0.0,
        ParamUnit::Semitones,
    ),
    ParamInfo::new(
        Acid303::CUTOFF,
        "Cutoff",
        200.0,
        5_000.0,
        700.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        Acid303::RESONANCE,
        "Resonance",
        0.0,
        100.0,
        60.0,
        ParamUnit::Percent,
    ),
    ParamInfo::new(
        Acid303::ENV_MOD,
        "Env Mod",
        0.0,
        100.0,
        40.0,
        ParamUnit::Percent,
    ),
    ParamInfo::new(
        Acid303::DECAY,
        "Decay",
        200.0,
        2_000.0,
        500.0,
        ParamUnit::Milliseconds,
    )
    .logarithmic(),
    ParamInfo::new(
        Acid303::ACCENT,
        "Accent",
        0.0,
        100.0,
        60.0,
        ParamUnit::Percent,
    ),
    ParamInfo::new(
        Acid303::SLIDE,
        "Slide",
        1.0,
        500.0,
        60.0,
        ParamUnit::Milliseconds,
    ),
    ParamInfo::new(
        Acid303::LEVEL,
        "Level",
        -40.0,
        0.0,
        -16.0,
        ParamUnit::Decibels,
    ),
];

impl Acid303 {
    /// Oscillator shape: 0 is a sawtooth, 1 a square, between is a blend.
    pub const WAVEFORM: ParamId = ParamId(0);
    /// Transposition in semitones.
    pub const TUNE: ParamId = ParamId(1);
    /// Filter cutoff in Hz, before the envelope moves it.
    pub const CUTOFF: ParamId = ParamId(2);
    /// Filter resonance, 0 to 100.
    pub const RESONANCE: ParamId = ParamId(3);
    /// How far the envelope sweeps the cutoff, 0 to 100.
    pub const ENV_MOD: ParamId = ParamId(4);
    /// Time constant of the filter envelope on notes without accent, in
    /// milliseconds. Accented notes always use 200 ms.
    pub const DECAY: ParamId = ParamId(5);
    /// Strength of the accent, 0 to 100.
    pub const ACCENT: ParamId = ParamId(6);
    /// Slide time in milliseconds.
    pub const SLIDE: ParamId = ParamId(7);
    /// Output level in decibels.
    pub const LEVEL: ParamId = ParamId(8);

    /// Lowest velocity that plays a note with accent.
    pub const ACCENT_VELOCITY: u8 = 100;

    const OVERSAMPLING: usize = 4;
    const ACCENT_DECAY_MS: f64 = 200.0;
    const NORMAL_RELEASE_MS: f64 = 1.0;
    const ACCENT_RELEASE_MS: f64 = 50.0;
    /// Pitch-bend range in semitones.
    const BEND_RANGE: f64 = 2.0;

    /// A synth with default parameters.
    pub fn new() -> Self {
        let p = &PARAMS;
        let mut synth = Self {
            waveform: f64::from(p[0].default),
            tune: f64::from(p[1].default),
            cutoff: f64::from(p[2].default),
            resonance: f64::from(p[3].default),
            env_mod: f64::from(p[4].default),
            decay_ms: f64::from(p[5].default),
            accent: f64::from(p[6].default) * 0.01,
            slide_ms: f64::from(p[7].default),
            level_db: f64::from(p[8].default),
            rate: 48_000.0,
            env_scaler: 1.0,
            env_offset: 0.0,
            amp_scaler: 1.0,
            phase: 0.0,
            osc_freq: 440.0,
            bend_factor: 1.0,
            accent_gain: 0.0,
            idle: true,
            notes: NoteList::new(),
            pitch_slew: LeakyIntegrator::new(),
            main_env: DecayEnvelope::new(),
            accent_smoother: LeakyIntegrator::new(),
            amp_env: AmpEnvelope::new(),
            declicker: Biquad::new(),
            highpass1: OnePole::new(),
            filter: TeeBeeFilter::new(),
            anti_alias: [Biquad::new(); 3],
            allpass: OnePole::new(),
            highpass2: OnePole::new(),
            notch: Biquad::new(),
        };
        synth.configure();
        synth
    }

    /// Recomputes everything that depends on the sample rate or on a
    /// parameter. No allocation.
    fn configure(&mut self) {
        let rate = self.rate;
        let oversampled = rate * Self::OVERSAMPLING as f64;
        self.pitch_slew.set_time_constant(0.2 * self.slide_ms, rate);
        self.accent_smoother.set_time_constant(15.0, rate);
        self.amp_env.set_rate(rate);
        self.declicker
            .set_lowpass(200.0, std::f64::consts::FRAC_1_SQRT_2, rate);
        self.highpass1.set_highpass(44.486, oversampled);
        self.filter.set_rate(oversampled);
        self.filter.set_resonance(self.resonance);
        let edge = (0.45 * rate).min(20_000.0);
        for (stage, q) in
            self.anti_alias
                .iter_mut()
                .zip([0.517_638, std::f64::consts::FRAC_1_SQRT_2, 1.931_852])
        {
            stage.set_lowpass(edge, q, oversampled);
        }
        self.allpass.set_allpass(14.008, rate);
        self.highpass2.set_highpass(24.167, rate);
        self.notch.set_notch(7.5164, 4.7, rate);
        self.amp_scaler = 10.0f64.powf(self.level_db / 20.0);
        self.update_env_mod();
        self.apply_accent_state(self.accent_gain > 0.0);
    }

    /// Open303's measured mapping from the Env Mod and Cutoff knobs to how
    /// the envelope scales and offsets the cutoff, in octaves.
    fn update_env_mod(&mut self) {
        const C0: f64 = 3.138_152_786_059_267e2;
        const C1: f64 = 2.394_411_986_817_546e3;
        const O_F: f64 = 0.048_292_930_943_553;
        const O_C: f64 = 0.294_391_201_442_418;
        const S_LO_F: f64 = 3.773_996_325_111_173;
        const S_LO_C: f64 = 0.736_965_594_166_206;
        const S_HI_F: f64 = 4.194_548_788_411_135;
        const S_HI_C: f64 = 0.864_344_900_642_434;

        let e = 0.01 * self.env_mod;
        let c = ((self.cutoff / C0).ln() / (C1 / C0).ln()).clamp(-0.5, 1.5);
        let s_lo = S_LO_F * e + S_LO_C;
        let s_hi = S_HI_F * e + S_HI_C;
        self.env_scaler = (1.0 - c) * s_lo + c * s_hi;
        self.env_offset = O_F * c + O_C;
    }

    /// Sets the envelope times for a note with or without accent.
    fn apply_accent_state(&mut self, accented: bool) {
        let (decay, release) = if accented {
            (Self::ACCENT_DECAY_MS, Self::ACCENT_RELEASE_MS)
        } else {
            (self.decay_ms, Self::NORMAL_RELEASE_MS)
        };
        self.main_env.set_time_constant(decay, self.rate);
        self.amp_env.set_release(release, self.rate);
    }

    fn key_to_freq(&self, key: u8) -> f64 {
        440.0 * 2.0f64.powf((f64::from(key) - 69.0 + self.tune) / 12.0)
    }

    fn reset_signal_path(&mut self) {
        self.phase = 0.0;
        self.filter.reset();
        self.highpass1.reset();
        self.highpass2.reset();
        self.allpass.reset();
        self.notch.reset();
        self.declicker.reset();
        for stage in &mut self.anti_alias {
            stage.reset();
        }
    }

    fn set_accent(&mut self, accented: bool) {
        self.accent_gain = if accented { self.accent } else { 0.0 };
        self.apply_accent_state(accented);
    }

    fn trigger_note(&mut self, key: u8, accented: bool) {
        if self.idle {
            self.reset_signal_path();
            self.accent_smoother.set_state(0.0);
        }
        self.set_accent(accented);
        self.osc_freq = self.key_to_freq(key);
        self.pitch_slew.set_state(self.osc_freq);
        self.main_env.trigger();
        self.amp_env.note_on();
        self.idle = false;
    }

    fn slide_to_note(&mut self, key: u8, accented: bool) {
        self.osc_freq = self.key_to_freq(key);
        self.set_accent(accented);
        self.idle = false;
    }

    fn note_on(&mut self, key: u8, velocity: u8) {
        let accented = velocity >= Self::ACCENT_VELOCITY;
        if self.notes.is_empty() {
            self.trigger_note(key, accented);
        } else {
            self.slide_to_note(key, accented);
        }
        self.notes.push_front(key);
    }

    fn note_off(&mut self, key: u8) {
        self.notes.remove(key);
        match self.notes.front() {
            // Slide back to the most recent key still held.
            Some(held) => self.osc_freq = self.key_to_freq(held),
            None => self.amp_env.note_off(),
        }
    }

    fn all_notes_off(&mut self) {
        self.notes.clear();
        self.amp_env.note_off();
    }

    fn oscillator(&mut self, step: f64) -> f64 {
        let t = self.phase;
        let saw = 2.0 * t - 1.0 - blep(t, step);
        let mut shifted = t + 0.5;
        if shifted >= 1.0 {
            shifted -= 1.0;
        }
        let square = if t < 0.5 { 1.0 } else { -1.0 } + blep(t, step) - blep(shifted, step);
        self.phase += step;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }
        (1.0 - self.waveform) * saw + self.waveform * 0.5 * square
    }

    fn next_sample(&mut self) -> f64 {
        if self.idle {
            return 0.0;
        }
        let freq = self.pitch_slew.process(self.osc_freq) * self.bend_factor;
        let step = (freq / (self.rate * Self::OVERSAMPLING as f64)).min(0.45);

        let main_env = self.main_env.process();
        let accent_in = if self.accent_gain > 0.0 {
            main_env
        } else {
            0.0
        };
        let accent_env = self.accent_smoother.process(accent_in);
        let sweep = self.env_scaler * (main_env - self.env_offset);
        let cutoff = self.cutoff * 2.0f64.powf(sweep + self.accent_gain * accent_env);
        self.filter.set_cutoff(cutoff);

        let mut amp = self.amp_env.process();
        if self.amp_env.is_note_on() {
            amp += 0.45 * main_env + self.accent_gain * 4.0 * main_env;
        }
        let amp = self.declicker.process(amp);

        let mut sample = 0.0;
        for _ in 0..Self::OVERSAMPLING {
            sample = -self.oscillator(step);
            sample = self.highpass1.process(sample);
            sample = self.filter.process(sample);
            for stage in &mut self.anti_alias {
                sample = stage.process(sample);
            }
        }
        sample = self.allpass.process(sample);
        sample = self.highpass2.process(sample);
        sample = self.notch.process(sample);
        sample *= amp * self.amp_scaler;

        if !self.amp_env.is_note_on() && self.amp_env.level() < 1.0e-6 && amp.abs() < 1.0e-6 {
            self.idle = true;
            self.main_env.reset();
            self.amp_env.reset();
        }
        if sample.is_finite() {
            sample
        } else {
            // Never let a blown-up filter state reach the output.
            self.reset_signal_path();
            0.0
        }
    }
}

/// PolyBLEP step correction in double precision.
fn blep(t: f64, dt: f64) -> f64 {
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

impl Default for Acid303 {
    fn default() -> Self {
        Self::new()
    }
}

impl Synth for Acid303 {
    fn handle_event(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn {
                pitch, velocity, ..
            } => {
                if velocity == 0 {
                    self.note_off(pitch);
                } else {
                    self.note_on(pitch, velocity);
                }
            }
            EventKind::NoteOff { pitch, .. } => self.note_off(pitch),
            EventKind::AllNotesOff => self.all_notes_off(),
            EventKind::PitchBend { value, .. } => {
                let semitones = Self::BEND_RANGE * f64::from(value) / 8_192.0;
                self.bend_factor = 2.0f64.powf(semitones / 12.0);
            }
            EventKind::ControlChange { .. } => {}
        }
    }

    fn tick(&mut self) -> (f32, f32) {
        let sample = self.next_sample() as f32;
        (sample, sample)
    }
}

impl Device for Acid303 {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.rate = f64::from(config.sample_rate.hz());
        self.configure();
        Device::reset(self);
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        run(self, io);
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        let Some(value) = clamped(&PARAMS, param, value) else {
            return;
        };
        let value = f64::from(value);
        match param {
            Self::WAVEFORM => self.waveform = value,
            Self::TUNE => {
                self.tune = value;
                if let Some(key) = self.notes.front() {
                    self.osc_freq = self.key_to_freq(key);
                }
            }
            Self::CUTOFF => {
                self.cutoff = value;
                self.update_env_mod();
            }
            Self::RESONANCE => {
                self.resonance = value;
                self.filter.set_resonance(value);
            }
            Self::ENV_MOD => {
                self.env_mod = value;
                self.update_env_mod();
            }
            Self::DECAY => {
                self.decay_ms = value;
                self.apply_accent_state(self.accent_gain > 0.0);
            }
            Self::ACCENT => self.accent = 0.01 * value,
            Self::SLIDE => {
                self.slide_ms = value;
                self.pitch_slew.set_time_constant(0.2 * value, self.rate);
            }
            Self::LEVEL => {
                self.level_db = value;
                self.amp_scaler = 10.0f64.powf(value / 20.0);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.notes.clear();
        self.idle = true;
        self.accent_gain = 0.0;
        self.bend_factor = 1.0;
        self.main_env.reset();
        self.amp_env.reset();
        self.accent_smoother.set_state(0.0);
        self.reset_signal_path();
    }
}
