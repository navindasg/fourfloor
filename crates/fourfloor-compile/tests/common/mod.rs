//! Small local devices and project helpers shared by the feature tests.
//! The compiler only knows devices through the registry, so the tests
//! bring their own.

#![allow(dead_code)]

use fourfloor_compile::{
    compile_project, render_project, render_stems, CompileOptions, CompiledProject, RenderOptions,
    RenderedStems,
};
use fourfloor_core::{
    pitch_to_hz, Device, DeviceCategory, DeviceDescriptor, DeviceId, DeviceRegistry, EventKind,
    ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, SampleRate, Ticks, TrackId,
    TransportContext,
};
use fourfloor_engine::devices::LatentDelay;
use fourfloor_model::command::{AddDevice, CreateTrack, WriteMidiClip};
use fourfloor_model::{CommandBus, MidiNote, Param, Project, TrackKind};
use fourfloor_testkit::{assert_buffer_ok, StereoBuffer};

pub const RATE: SampleRate = SampleRate::HZ_48000;
/// Frames per beat at 120 BPM and 48 kHz.
pub const BEAT: usize = 24_000;

pub const KIND_TONE: &str = "test.tone";
pub const KIND_DUCKER: &str = "test.ducker";
pub const KIND_LATENT: &str = "test.latent";
/// Latency of [`KIND_LATENT`], in frames.
pub const LATENCY: usize = 480;

pub const TONE_LEVEL: ParamId = ParamId(0);
static TONE_PARAMS: [ParamInfo; 1] = [ParamInfo::new(
    TONE_LEVEL,
    "Level",
    0.0,
    1.0,
    0.25,
    ParamUnit::Generic,
)];

const VOICES: usize = 8;

/// A sine instrument with up to eight voices. A voice starts and stops on
/// the exact sample of its note, at full level, with phase zero, so onsets
/// are sharp and renders are repeatable.
pub struct Tone {
    level: f32,
    rate: f32,
    /// `(pitch, phase)` per sounding voice.
    voices: [Option<(u8, f32)>; VOICES],
}

impl Tone {
    pub fn new() -> Self {
        Self {
            level: TONE_PARAMS[0].default,
            rate: 48_000.0,
            voices: [None; VOICES],
        }
    }
}

impl Device for Tone {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.rate = config.sample_rate.hz() as f32;
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let mut events = io.events.iter().peekable();
        for frame in 0..io.frames() {
            while let Some(event) = events.next_if(|e| e.frame as usize <= frame) {
                match event.kind {
                    EventKind::NoteOn { pitch, .. } => {
                        let taken = self
                            .voices
                            .iter()
                            .any(|v| v.is_some_and(|(p, _)| p == pitch));
                        if let (false, Some(free)) =
                            (taken, self.voices.iter_mut().find(|v| v.is_none()))
                        {
                            *free = Some((pitch, 0.0));
                        }
                    }
                    EventKind::NoteOff { pitch, .. } => {
                        for voice in &mut self.voices {
                            if voice.is_some_and(|(p, _)| p == pitch) {
                                *voice = None;
                            }
                        }
                    }
                    EventKind::AllNotesOff => self.voices = [None; VOICES],
                    _ => {}
                }
            }
            let mut sample = 0.0;
            for (pitch, phase) in self.voices.iter_mut().flatten() {
                sample += self.level * (std::f32::consts::TAU * *phase).sin();
                *phase = (*phase + pitch_to_hz(f32::from(*pitch)) / self.rate).fract();
            }
            for channel in io.outputs.iter_mut() {
                channel[frame] = sample;
            }
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &TONE_PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if param == TONE_LEVEL && value.is_finite() {
            self.level = TONE_PARAMS[0].clamp(value);
        }
    }

    fn reset(&mut self) {
        self.voices = [None; VOICES];
    }
}

/// An effect that turns its input down while its sidechain is loud: gain
/// is `1 - min(1, 4 * follower)`, where the follower tracks the sidechain
/// peak instantly and lets go over about 20 ms. With no sidechain routed
/// it passes its input through.
pub struct Ducker {
    follower: f32,
    release: f32,
}

impl Ducker {
    pub fn new() -> Self {
        Self {
            follower: 0.0,
            release: 0.999,
        }
    }
}

impl Device for Ducker {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.release = (-1.0 / (0.02 * config.sample_rate.hz() as f32)).exp();
        self.follower = 0.0;
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        for frame in 0..io.frames() {
            let key = io.sidechain.map_or(0.0, |channels| {
                channels
                    .iter()
                    .fold(0.0f32, |peak, c| peak.max(c[frame].abs()))
            });
            self.follower = key.max(self.follower * self.release);
            let gain = 1.0 - (4.0 * self.follower).min(1.0);
            for (channel, output) in io.outputs.iter_mut().enumerate() {
                output[frame] = io.inputs.get(channel).map_or(0.0, |input| input[frame]) * gain;
            }
        }
    }

    fn reset(&mut self) {
        self.follower = 0.0;
    }
}

pub fn registry() -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry).unwrap();
    registry
        .register_device(KIND_TONE, "Tone", DeviceCategory::Instrument, Tone::new)
        .unwrap();
    registry
        .register(
            DeviceDescriptor::new(KIND_DUCKER, "Ducker", DeviceCategory::Effect).with_sidechain(),
            |_| Ok(Box::new(Ducker::new())),
        )
        .unwrap();
    registry
        .register_device(KIND_LATENT, "Latent", DeviceCategory::Effect, || {
            LatentDelay::new(LATENCY as u32)
        })
        .unwrap();
    registry
}

pub fn new_bus() -> CommandBus {
    CommandBus::new(Project::new("Test"))
}

pub fn add_track(bus: &mut CommandBus, name: &str, kind: TrackKind) -> TrackId {
    bus.execute(CreateTrack {
        name: name.to_owned(),
        kind,
        index: None,
    })
    .unwrap()
    .track()
    .unwrap()
}

pub fn add_device(
    bus: &mut CommandBus,
    track: TrackId,
    kind: &str,
    params: Vec<Param>,
) -> DeviceId {
    bus.execute(AddDevice {
        track,
        kind: kind.to_owned(),
        name: None,
        index: None,
        params,
    })
    .unwrap()
    .device()
    .unwrap()
}

pub fn param(id: ParamId, name: &str, value: f32, min: f32, max: f32) -> Param {
    Param {
        id,
        name: name.to_owned(),
        value,
        min,
        max,
    }
}

pub fn note(start: i64, length: i64, pitch: u8) -> MidiNote {
    MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(length),
        pitch,
        velocity: 100,
    }
}

/// Adds a MIDI track that plays `notes` through a [`Tone`], in a clip of
/// `beats` beats at the project start. Returns the track and the tone.
pub fn tone_track(
    bus: &mut CommandBus,
    name: &str,
    beats: i64,
    notes: Vec<MidiNote>,
) -> (TrackId, DeviceId) {
    let track = add_track(bus, name, TrackKind::Midi);
    let level = param(TONE_LEVEL, "Level", 0.25, 0.0, 1.0);
    let tone = add_device(bus, track, KIND_TONE, vec![level]);
    bus.execute(WriteMidiClip {
        track,
        clip: None,
        name: name.to_owned(),
        start: Ticks::ZERO,
        length: Ticks::from_whole_beats(beats),
        notes,
    })
    .unwrap();
    (track, tone)
}

/// A track holding one note for `beats` beats.
pub fn held_track(bus: &mut CommandBus, name: &str, beats: i64, pitch: u8) -> (TrackId, DeviceId) {
    tone_track(bus, name, beats, vec![note(0, beats * 960, pitch)])
}

pub fn options() -> RenderOptions {
    RenderOptions::new(RATE).with_tail(0.0)
}

pub fn compile(bus: &CommandBus) -> CompiledProject {
    compile_project(bus.project(), &registry(), &CompileOptions::new(RATE, 512)).unwrap()
}

/// Renders the project with no tail and checks it compiled cleanly.
pub fn render(bus: &CommandBus) -> StereoBuffer {
    render_with(bus, &options())
}

pub fn render_with(bus: &CommandBus, options: &RenderOptions) -> StereoBuffer {
    let rendered = render_project(bus.project(), &registry(), options).unwrap();
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    let out = StereoBuffer::from_interleaved(RATE, &rendered.samples);
    assert_buffer_ok(&out);
    out
}

pub fn stems(bus: &CommandBus) -> RenderedStems {
    let stems = render_stems(bus.project(), &registry(), &options()).unwrap();
    assert!(stems.warnings.is_empty(), "{:?}", stems.warnings);
    stems
}

pub fn stem(stems: &RenderedStems, track: TrackId) -> StereoBuffer {
    let stem = stems
        .stems
        .iter()
        .find(|stem| stem.track == track)
        .expect("the track has a stem");
    StereoBuffer::from_interleaved(RATE, &stem.samples)
}

pub fn scaled(signal: &[f32], gain: f32) -> Vec<f32> {
    signal.iter().map(|s| s * gain).collect()
}

pub fn added(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(a, b)| a + b).collect()
}
