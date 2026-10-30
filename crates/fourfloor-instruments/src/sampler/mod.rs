//! `sampler`: plays a sample across the keyboard, or its slices on
//! consecutive notes.
//!
//! Sample data is built off the audio thread ([`SampleData`]) and reaches
//! the device either before it is prepared ([`Sampler::set_sample`]) or,
//! while it is running, through a [`SampleLoader`]: a pair of lock-free
//! queues that carry the new sample in and the replaced one back out, so
//! the audio thread never allocates or frees sample memory.
//!
//! Playback reads the sample with four-point Hermite interpolation. There
//! is no band-limiting when a sample is played above its own pitch, so
//! very bright samples transposed far up will alias.

mod sample;
mod slicing;

pub use sample::{SampleData, SampleError, SampleLoader};
pub use slicing::{detect_slices, SliceOptions};

use std::fmt;
use std::sync::Arc;

use fourfloor_core::{
    velocity_to_unit, Device, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo,
    TransportContext,
};

use crate::dsp::{Adsr, AdsrParams};
use crate::runtime::{clamped, pick_voice, run, Synth, VoiceMeta};

#[derive(Clone, Copy, Debug)]
struct Voice {
    meta: VoiceMeta,
    /// Read position in source frames.
    pos: f64,
    /// Source frames per output frame.
    step: f64,
    start: usize,
    end: usize,
    looping: bool,
    one_shot: bool,
    /// Source frames over which a voice fades out before `end`.
    fade: f64,
    gain: f32,
    env: Adsr,
}

impl Voice {
    const IDLE: Self = Self {
        meta: VoiceMeta::IDLE,
        pos: 0.0,
        step: 1.0,
        start: 0,
        end: 0,
        looping: false,
        one_shot: false,
        fade: 1.0,
        gain: 0.0,
        env: Adsr::new(),
    };
}

/// The audio-thread ends of a [`SampleLoader`]'s queues.
struct Link {
    incoming: rtrb::Consumer<Arc<SampleData>>,
    outgoing: rtrb::Producer<Arc<SampleData>>,
}

/// Sample player, sixteen voices. Kind `sampler`.
///
/// - **Pitched mode** ([`Sampler::MODE`] 0): the whole sample plays at its
///   own pitch on [`Sampler::ROOT`] and is transposed on other notes.
///   With [`Sampler::LOOP`] on it repeats between the loop points while
///   the note sounds.
/// - **Slice mode** (1): note [`Sampler::SLICE_BASE`] plays the first
///   slice, the next note the second, and so on, at the original speed.
///   Notes without a slice are ignored.
/// - [`Sampler::ONE_SHOT`] makes a note play to the end of its sample or
///   slice whatever its length (ignored while looping).
///
/// Without a sample the device is silent.
pub struct Sampler {
    sample: Option<Arc<SampleData>>,
    link: Option<Link>,
    slice_mode: bool,
    root: f32,
    tune: f32,
    looping: bool,
    one_shot: bool,
    loop_start: f32,
    loop_end: f32,
    adsr: AdsrParams,
    slice_base: i32,
    level: f32,
    rate: f32,
    voices: [Voice; Sampler::VOICES],
    counter: u64,
}

static PARAMS: [ParamInfo; 13] = [
    ParamInfo::new(Sampler::MODE, "Mode", 0.0, 1.0, 0.0, ParamUnit::Integer),
    ParamInfo::new(Sampler::ROOT, "Root", 0.0, 127.0, 60.0, ParamUnit::Integer),
    ParamInfo::new(
        Sampler::TUNE,
        "Tune",
        -24.0,
        24.0,
        0.0,
        ParamUnit::Semitones,
    ),
    ParamInfo::toggle(Sampler::LOOP, "Loop", false),
    ParamInfo::toggle(Sampler::ONE_SHOT, "One Shot", false),
    ParamInfo::new(
        Sampler::LOOP_START,
        "Loop Start",
        0.0,
        100.0,
        0.0,
        ParamUnit::Percent,
    ),
    ParamInfo::new(
        Sampler::LOOP_END,
        "Loop End",
        0.0,
        100.0,
        100.0,
        ParamUnit::Percent,
    ),
    ParamInfo::new(Sampler::ATTACK, "Attack", 0.0, 5.0, 0.0, ParamUnit::Seconds),
    ParamInfo::new(Sampler::DECAY, "Decay", 0.01, 10.0, 0.5, ParamUnit::Seconds).logarithmic(),
    ParamInfo::new(
        Sampler::SUSTAIN,
        "Sustain",
        0.0,
        1.0,
        1.0,
        ParamUnit::Generic,
    ),
    ParamInfo::new(
        Sampler::RELEASE,
        "Release",
        0.005,
        10.0,
        0.05,
        ParamUnit::Seconds,
    )
    .logarithmic(),
    ParamInfo::new(
        Sampler::SLICE_BASE,
        "Slice Base",
        0.0,
        127.0,
        36.0,
        ParamUnit::Integer,
    ),
    ParamInfo::new(Sampler::LEVEL, "Level", 0.0, 1.0, 0.8, ParamUnit::Generic),
];

impl Sampler {
    /// 0 plays the sample pitched across the keyboard, 1 plays slices.
    pub const MODE: ParamId = ParamId(0);
    /// MIDI pitch at which the sample plays at its recorded pitch.
    pub const ROOT: ParamId = ParamId(1);
    /// Transposition in semitones (both modes).
    pub const TUNE: ParamId = ParamId(2);
    /// Repeats the loop region (pitched mode) or the slice (slice mode).
    pub const LOOP: ParamId = ParamId(3);
    /// Notes play to the end regardless of their length.
    pub const ONE_SHOT: ParamId = ParamId(4);
    /// Start of the loop region in percent of the sample (pitched mode).
    pub const LOOP_START: ParamId = ParamId(5);
    /// End of the loop region in percent of the sample (pitched mode).
    pub const LOOP_END: ParamId = ParamId(6);
    /// Attack time in seconds.
    pub const ATTACK: ParamId = ParamId(7);
    /// Decay time in seconds (time to fall 60 dB towards the sustain).
    pub const DECAY: ParamId = ParamId(8);
    /// Sustain level, 0 to 1.
    pub const SUSTAIN: ParamId = ParamId(9);
    /// Release time in seconds (time to fall 60 dB).
    pub const RELEASE: ParamId = ParamId(10);
    /// MIDI pitch that plays the first slice.
    pub const SLICE_BASE: ParamId = ParamId(11);
    /// Output level, 0 to 1.
    pub const LEVEL: ParamId = ParamId(12);

    /// Notes that can sound at once.
    pub const VOICES: usize = 16;

    /// Length of the fade at the end of a sample or slice, in seconds.
    const END_FADE: f64 = 0.002;

    /// A sampler with default parameters and no sample.
    pub fn new() -> Self {
        let mut sampler = Self {
            sample: None,
            link: None,
            slice_mode: false,
            root: 60.0,
            tune: 0.0,
            looping: false,
            one_shot: false,
            loop_start: 0.0,
            loop_end: 1.0,
            adsr: AdsrParams {
                attack: 0.0,
                decay: 0.5,
                sustain: 1.0,
                release: 0.05,
            },
            slice_base: 36,
            level: 0.8,
            rate: 48_000.0,
            voices: [Voice::IDLE; Self::VOICES],
            counter: 0,
        };
        for info in &PARAMS {
            sampler.apply(info.id, info.default);
        }
        sampler
    }

    /// A sampler that plays `sample`.
    pub fn with_sample(sample: Arc<SampleData>) -> Self {
        let mut sampler = Self::new();
        sampler.sample = Some(sample);
        sampler
    }

    /// A sampler and the [`SampleLoader`] that feeds it new samples while
    /// it runs on the audio thread.
    pub fn with_loader() -> (Self, SampleLoader) {
        let (to_audio, incoming) = rtrb::RingBuffer::new(SampleLoader::CAPACITY);
        // Room for every queued sample and the current one to come back.
        let (outgoing, from_audio) = rtrb::RingBuffer::new(SampleLoader::CAPACITY + 2);
        let mut sampler = Self::new();
        sampler.link = Some(Link { incoming, outgoing });
        (
            sampler,
            SampleLoader {
                to_audio,
                from_audio,
            },
        )
    }

    /// Replaces the sample directly. Not for the audio thread: the old
    /// sample may be freed here. Sounding voices are cut.
    pub fn set_sample(&mut self, sample: Option<Arc<SampleData>>) {
        self.sample = sample;
        self.cut_voices();
    }

    /// The sample being played.
    pub fn sample(&self) -> Option<&Arc<SampleData>> {
        self.sample.as_ref()
    }

    fn cut_voices(&mut self) {
        for voice in &mut self.voices {
            voice.meta = VoiceMeta::IDLE;
            voice.env.reset();
        }
    }

    /// Takes at most one waiting sample from the loader and sends the
    /// replaced one back. Never frees: if the way back is full the swap
    /// waits for a later block.
    fn poll_loader(&mut self) {
        let Some(link) = &mut self.link else {
            return;
        };
        if link.outgoing.is_full() {
            return;
        }
        let Ok(new) = link.incoming.pop() else {
            return;
        };
        if let Some(old) = self.sample.replace(new) {
            // Cannot fail: this is the only producer and it was not full.
            if let Err(rtrb::PushError::Full(old)) = link.outgoing.push(old) {
                std::mem::forget(old);
            }
        }
        self.cut_voices();
    }

    /// Applies an already clamped value.
    fn apply(&mut self, id: ParamId, value: f32) {
        match id {
            Self::MODE => self.slice_mode = value >= 0.5,
            Self::ROOT => self.root = value.round(),
            Self::TUNE => self.tune = value,
            Self::LOOP => self.looping = value >= 0.5,
            Self::ONE_SHOT => self.one_shot = value >= 0.5,
            Self::LOOP_START => self.loop_start = 0.01 * value,
            Self::LOOP_END => self.loop_end = 0.01 * value,
            Self::ATTACK => self.adsr.attack = value,
            Self::DECAY => self.adsr.decay = value,
            Self::SUSTAIN => self.adsr.sustain = value,
            Self::RELEASE => self.adsr.release = value,
            Self::SLICE_BASE => self.slice_base = value.round() as i32,
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
        let Some(sample) = self.sample.as_deref() else {
            return;
        };
        let frames = sample.frames();
        let rate_ratio = sample.sample_rate().as_f64() / f64::from(self.rate);
        let (start, end, semitones) = if self.slice_mode {
            let Some((start, end)) = usize::try_from(i32::from(pitch) - self.slice_base)
                .ok()
                .and_then(|index| sample.slice_range(index))
            else {
                return;
            };
            (start, end, self.tune)
        } else {
            let (mut start, mut end) = (0, frames);
            if self.looping {
                let lo = (f64::from(self.loop_start) * frames as f64) as usize;
                let hi = ((f64::from(self.loop_end) * frames as f64) as usize).min(frames);
                if hi > lo + 1 {
                    (start, end) = (lo, hi);
                }
            }
            (start, end, f32::from(pitch) - self.root + self.tune)
        };
        if end <= start {
            return;
        }
        let index = pick_voice(self.voices.iter().map(|voice| &voice.meta));
        self.counter += 1;
        let Some(voice) = self.voices.get_mut(index) else {
            return;
        };
        voice.meta = VoiceMeta {
            pitch,
            channel,
            started: self.counter,
            gate: true,
            active: true,
        };
        // A pitched loop starts from the top of the sample and then stays
        // inside the loop region.
        voice.pos = if self.slice_mode { start as f64 } else { 0.0 };
        voice.step = (rate_ratio * 2.0f64.powf(f64::from(semitones) / 12.0)).min(64.0);
        voice.start = start;
        voice.end = end;
        voice.looping = self.looping;
        voice.one_shot = self.one_shot && !self.looping;
        voice.fade = (Self::END_FADE * sample.sample_rate().as_f64())
            .min((end - start) as f64 * 0.25)
            .max(1.0);
        voice.gain = velocity_to_unit(velocity);
        voice.env.reset();
        voice.env.set(&self.adsr, self.rate);
        voice.env.gate_on();
    }

    fn release(&mut self, force: bool, matches: impl Fn(&VoiceMeta) -> bool) {
        for voice in &mut self.voices {
            if voice.meta.active && voice.meta.gate && matches(&voice.meta) {
                if voice.one_shot && !force {
                    continue;
                }
                voice.meta.gate = false;
                voice.env.gate_off();
            }
        }
    }
}

/// Four-point Hermite interpolation between `y[1]` and `y[2]`.
fn hermite(y: [f32; 4], t: f32) -> f32 {
    let c1 = 0.5 * (y[2] - y[0]);
    let c2 = y[0] - 2.5 * y[1] + 2.0 * y[2] - 0.5 * y[3];
    let c3 = 0.5 * (y[3] - y[0]) + 1.5 * (y[1] - y[2]);
    ((c3 * t + c2) * t + c1) * t + y[1]
}

/// Reads one frame of `voice` from `sample`.
fn read(sample: &SampleData, voice: &Voice) -> (f32, f32) {
    let base = voice.pos.floor();
    let t = (voice.pos - base) as f32;
    let base = base as isize;
    let (start, end) = (voice.start as isize, voice.end as isize);
    let span = end - start;
    let mut left = [0.0f32; 4];
    let mut right = [0.0f32; 4];
    for (k, (l, r)) in left.iter_mut().zip(right.iter_mut()).enumerate() {
        let mut index = base + k as isize - 1;
        if voice.looping {
            if index >= end {
                index -= span;
            }
        } else if index >= end {
            continue;
        }
        if index < 0 {
            continue;
        }
        let index = index as usize;
        *l = sample.left().get(index).copied().unwrap_or(0.0);
        *r = sample.right().get(index).copied().unwrap_or(0.0);
    }
    (hermite(left, t), hermite(right, t))
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Sampler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sampler")
            .field(
                "frames",
                &self.sample.as_ref().map(|sample| sample.frames()),
            )
            .field("has_loader", &self.link.is_some())
            .field("slice_mode", &self.slice_mode)
            .finish_non_exhaustive()
    }
}

impl Synth for Sampler {
    fn handle_event(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn {
                pitch,
                velocity,
                channel,
            } => {
                if velocity == 0 {
                    self.release(false, |meta| meta.matches(pitch, channel));
                } else {
                    self.note_on(pitch, velocity, channel);
                }
            }
            EventKind::NoteOff { pitch, channel, .. } => {
                self.release(false, |meta| meta.matches(pitch, channel));
            }
            EventKind::AllNotesOff => self.release(true, |_| true),
            EventKind::ControlChange { .. } | EventKind::PitchBend { .. } => {}
        }
    }

    fn tick(&mut self) -> (f32, f32) {
        let Some(sample) = self.sample.as_deref() else {
            return (0.0, 0.0);
        };
        let (mut left, mut right) = (0.0, 0.0);
        for voice in &mut self.voices {
            if !voice.meta.active {
                continue;
            }
            let env = voice.env.next();
            let end = voice.end as f64;
            if voice.env.is_idle() || (!voice.looping && voice.pos >= end) {
                voice.meta = VoiceMeta::IDLE;
                voice.env.reset();
                continue;
            }
            let (l, r) = read(sample, voice);
            let fade = if voice.looping {
                1.0
            } else {
                ((end - voice.pos) / voice.fade).min(1.0) as f32
            };
            let gain = env * voice.gain * fade;
            left += l * gain;
            right += r * gain;
            voice.pos += voice.step;
            if voice.looping {
                let span = (voice.end - voice.start) as f64;
                while voice.pos >= end {
                    voice.pos -= span;
                }
            }
        }
        (left * self.level, right * self.level)
    }
}

impl Device for Sampler {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.rate = config.sample_rate.hz() as f32;
        for voice in &mut self.voices {
            voice.env.set(&self.adsr, self.rate);
        }
        self.cut_voices();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        self.poll_loader();
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
        self.cut_voices();
    }
}
