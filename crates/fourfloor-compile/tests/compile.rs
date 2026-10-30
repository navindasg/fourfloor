//! Compiles projects and listens to the result: every claim the compiler
//! makes about wiring is checked on rendered audio.

use fourfloor_compile::{
    compile_project, db_to_linear, render_project, CompileOptions, CompiledProject, RenderError,
    RenderOptions, CHANNELS, DEFAULT_INSTRUMENT,
};
use fourfloor_core::{
    pitch_to_hz, Device, DeviceCategory, DeviceDescriptor, DeviceId, DeviceInit, DeviceRegistry,
    Event, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, SampleRate, Ticks,
    TrackId, TransportContext,
};
use fourfloor_engine::devices::{Fader, Gain, KIND_GAIN};
use fourfloor_model::command::{
    AddDevice, CreateTrack, SetDeviceBypass, SetDeviceState, SetTrackGain, SetTrackMute,
    SetTrackOutput, SetTrackPan, SetTrackSolo, WriteMidiClip,
};
use fourfloor_model::sequencer::{write_house_pattern, HousePattern, HousePatternWritten};
use fourfloor_model::{CommandBus, MidiNote, Param, PluginStateStore, Project, TrackKind};
use fourfloor_testkit::{
    assert_buffer_ok, assert_null, dominant_frequency, is_silent, null_test, peak, render_graph,
    rms, spectrum_band_energy, AllocDisabler, StereoBuffer,
};

// Every render in this file runs under the engine's allocation guard.
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const RATE: SampleRate = SampleRate::HZ_48000;

/// A sine instrument: one voice at the pitch of the last note-on, at the
/// level of its `Level` parameter. Its saved state, if any, is one byte
/// that transposes it by that many semitones.
struct Tone {
    level: f32,
    transpose: u8,
    step: f32,
    phase: f32,
    rate: f32,
}

const TONE_LEVEL: ParamId = ParamId(0);
static TONE_PARAMS: [ParamInfo; 1] = [ParamInfo::new(
    TONE_LEVEL,
    "Level",
    0.0,
    1.0,
    0.5,
    ParamUnit::Generic,
)];

impl Tone {
    fn new(transpose: u8) -> Self {
        Self {
            level: 0.5,
            transpose,
            step: 0.0,
            phase: 0.0,
            rate: 48_000.0,
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
                        let pitch = f32::from(pitch) + f32::from(self.transpose);
                        self.step = pitch_to_hz(pitch) / self.rate;
                    }
                    EventKind::NoteOff { .. } | EventKind::AllNotesOff => self.step = 0.0,
                    _ => {}
                }
            }
            let sample = if self.step > 0.0 {
                self.level * (std::f32::consts::TAU * self.phase).sin()
            } else {
                0.0
            };
            self.phase = (self.phase + self.step).fract();
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
}

/// A note effect: transposes notes up an octave.
struct Octave;

impl Device for Octave {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        io.silence_outputs();
        for event in io.events {
            let kind = match event.kind {
                EventKind::NoteOn {
                    pitch,
                    velocity,
                    channel,
                } => EventKind::NoteOn {
                    pitch: pitch.saturating_add(12).min(127),
                    velocity,
                    channel,
                },
                other => other,
            };
            io.events_out.push(Event::new(event.frame, kind));
        }
    }
}

const KIND_TONE: &str = "test.tone";
const KIND_OCTAVE: &str = "test.octave";

fn registry() -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry).unwrap();
    let tone = DeviceDescriptor::new(KIND_TONE, "Tone", DeviceCategory::Instrument)
        .with_params(TONE_PARAMS.to_vec());
    registry
        .register(tone, |init: &DeviceInit<'_>| match init.state {
            None => Ok(Box::new(Tone::new(0))),
            Some([semitones]) => Ok(Box::new(Tone::new(*semitones))),
            Some(_) => Err(fourfloor_core::DeviceError::new("state must be one byte")),
        })
        .unwrap();
    registry
        .register_device(KIND_OCTAVE, "Octave", DeviceCategory::NoteEffect, || Octave)
        .unwrap();
    registry
}

fn options() -> CompileOptions {
    CompileOptions::new(RATE, 512)
}

fn house(bars: u32) -> (CommandBus, HousePatternWritten) {
    let mut bus = CommandBus::new(Project::new("Test"));
    let pattern = HousePattern {
        bars,
        ..HousePattern::default()
    };
    let written = write_house_pattern(&mut bus, &pattern).unwrap();
    (bus, written)
}

/// A project with one MIDI track holding one A4 (pitch 69) note that lasts
/// a whole bar at 120 BPM (two seconds).
fn one_note() -> (CommandBus, TrackId) {
    let mut bus = CommandBus::new(Project::new("Test"));
    let track = add_track(&mut bus, "Lead", TrackKind::Midi);
    bus.execute(WriteMidiClip {
        track,
        clip: None,
        name: "Note".to_owned(),
        start: Ticks::ZERO,
        length: Ticks::from_whole_beats(4),
        notes: vec![MidiNote {
            start: Ticks::ZERO,
            length: Ticks::from_whole_beats(4),
            pitch: 69,
            velocity: 127,
        }],
    })
    .unwrap();
    (bus, track)
}

fn add_track(bus: &mut CommandBus, name: &str, kind: TrackKind) -> TrackId {
    bus.execute(CreateTrack {
        name: name.to_owned(),
        kind,
        index: None,
    })
    .unwrap()
    .track()
    .unwrap()
}

fn add_device(bus: &mut CommandBus, track: TrackId, kind: &str, params: Vec<Param>) -> DeviceId {
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

fn param(id: ParamId, name: &str, value: f32, min: f32, max: f32) -> Param {
    Param {
        id,
        name: name.to_owned(),
        value,
        min,
        max,
    }
}

fn compile(bus: &CommandBus) -> CompiledProject {
    compile_project(bus.project(), &registry(), &options()).unwrap()
}

/// Renders the project with no tail.
fn render(bus: &CommandBus) -> StereoBuffer {
    let compiled = compile(bus);
    let seconds = compiled.length.to_beats().get() * bus.project().tempo.seconds_per_beat();
    let out = render_graph(compiled.graph, bus.project().tempo, seconds).unwrap();
    assert_buffer_ok(&out);
    out
}

fn scaled(signal: &[f32], gain: f32) -> Vec<f32> {
    signal.iter().map(|s| s * gain).collect()
}

#[test]
fn an_empty_project_compiles_to_a_silent_master() {
    let compiled = compile_project(&Project::new("Empty"), &registry(), &options()).unwrap();
    assert_eq!(compiled.length, Ticks::ZERO);
    assert_eq!(compiled.sounding_tracks, 0);
    assert_eq!(compiled.graph.order(), [compiled.master]);
    assert_eq!(compiled.graph.channels(), CHANNELS);
    assert!(compiled.tracks.is_empty());
    assert!(compiled.warnings.is_empty());
    let out = render_graph(compiled.graph, fourfloor_core::Tempo::DEFAULT, 0.1).unwrap();
    assert!(is_silent(&out.left, -200.0) && is_silent(&out.right, -200.0));
}

#[test]
fn a_house_pattern_becomes_a_player_an_instrument_and_a_fader_per_track() {
    let (bus, written) = house(4);
    let compiled = compile(&bus);
    assert_eq!(compiled.sounding_tracks, 2);
    // 4 bars: 16 kicks, 16 open hats, 32 closed hats.
    assert_eq!(compiled.notes, 64);
    assert_eq!(compiled.length, Ticks::from_whole_beats(16));
    assert_eq!(compiled.graph.order().len(), 7);
    assert_eq!(compiled.graph.order().last(), Some(&compiled.master));
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);

    // No graph-only node reuses an id the project handed out.
    let floor = bus.project().next_id;
    assert!(compiled.graph.order().iter().all(|id| id.get() >= floor));

    let kick = compiled.track(written.kick_track).unwrap();
    assert!(kick.player.is_some() && kick.instrument.is_some());
    assert!(kick.input.is_none() && kick.devices.is_empty());
    assert!(compiled.graph.contains(kick.fader));
    assert!(compiled.track(TrackId::new(9_999)).is_none());
    assert_eq!(DEFAULT_INSTRUMENT, "builtin.placeholder");
}

#[test]
fn a_chain_instrument_is_built_from_the_registry_with_its_parameters() {
    let (mut bus, track) = one_note();
    let tone = add_device(
        &mut bus,
        track,
        KIND_TONE,
        vec![param(TONE_LEVEL, "Level", 0.25, 0.0, 1.0)],
    );
    let compiled = compile(&bus);
    let strip = compiled.track(track).unwrap();
    assert_eq!(strip.instrument, Some(tone), "the node has the model's id");
    assert_eq!(strip.devices, [tone]);
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);

    let out = render(&bus);
    assert_eq!(out.frames(), 96_000);
    // The registered sine instrument at A4, not the placeholder synth, at
    // the level stored in the project.
    assert!((dominant_frequency(&out.left, RATE) - 440.0).abs() < 0.5);
    assert!((peak(&out.left) - 0.25).abs() < 1e-3);
    assert!(null_test(&out.left, &out.right, -140.0), "centred");
}

#[test]
fn a_parameter_message_to_the_model_device_id_reaches_the_running_device() {
    let (mut bus, track) = one_note();
    let tone = add_device(&mut bus, track, KIND_TONE, Vec::new());
    let mut compiled = compile(&bus);
    assert!(compiled.graph.set_param(tone, TONE_LEVEL, 1.0));
    let fader = compiled.track(track).unwrap().fader;
    assert!(compiled.graph.set_param(fader, Fader::GAIN_DB, -20.0));
    assert!(compiled.graph.set_param(compiled.master, Gain::GAIN, 0.5));
    let out = render_graph(compiled.graph, bus.project().tempo, 1.0).unwrap();
    assert!((peak(&out.left) - 0.05).abs() < 1e-3, "1.0 * -20 dB * 0.5");
}

#[test]
fn note_effects_sit_between_the_player_and_the_instrument() {
    let (mut bus, track) = one_note();
    // Added after the instrument in the chain: the role decides the wiring.
    let tone = add_device(&mut bus, track, KIND_TONE, Vec::new());
    let octave = add_device(&mut bus, track, KIND_OCTAVE, Vec::new());
    let compiled = compile(&bus);
    assert_eq!(compiled.track(track).unwrap().devices, [octave, tone]);
    let out = render(&bus);
    assert!((dominant_frequency(&out.left, RATE) - 880.0).abs() < 0.5);
}

#[test]
fn effects_run_in_chain_order_after_the_instrument() {
    let (mut bus, track) = one_note();
    add_device(&mut bus, track, KIND_TONE, Vec::new());
    let reference = render(&bus);

    let gain = |value: f32| vec![param(Gain::GAIN, "Gain", value, -16.0, 16.0)];
    let first = add_device(&mut bus, track, KIND_GAIN, gain(0.5));
    let second = add_device(&mut bus, track, KIND_GAIN, gain(0.5));
    let compiled = compile(&bus);
    assert_eq!(compiled.track(track).unwrap().devices.len(), 3);
    let out = render(&bus);
    assert_null(&out.left, &scaled(&reference.left, 0.25), -120.0);

    // Bypassing one takes it out of the graph.
    bus.execute(SetDeviceBypass {
        device: second,
        bypassed: true,
    })
    .unwrap();
    let compiled = compile(&bus);
    assert!(compiled.graph.contains(first) && !compiled.graph.contains(second));
    let out = render(&bus);
    assert_null(&out.left, &scaled(&reference.left, 0.5), -120.0);
}

#[test]
fn an_unknown_kind_is_left_out_with_a_warning_and_the_rest_still_plays() {
    let (mut bus, track) = one_note();
    add_device(&mut bus, track, KIND_TONE, Vec::new());
    let reference = render(&bus);
    let missing = add_device(&mut bus, track, "vst3:not.installed", Vec::new());

    let compiled = compile(&bus);
    assert!(!compiled.graph.contains(missing));
    assert_eq!(compiled.warnings.len(), 1, "{:?}", compiled.warnings);
    assert!(compiled.warnings[0].contains("vst3:not.installed"));
    assert!(compiled.warnings[0].contains("not registered"));
    let out = render(&bus);
    assert_null(&out.left, &reference.left, -140.0);
}

#[test]
fn a_track_without_an_instrument_uses_the_default_or_stays_silent() {
    let (bus, track) = one_note();
    // Default: the placeholder synth plays A4.
    let out = render(&bus);
    assert!((dominant_frequency(&out.left, RATE) - 440.0).abs() < 0.5);

    // A different default instrument.
    let tone = options().with_default_instrument(Some(KIND_TONE));
    let compiled = compile_project(bus.project(), &registry(), &tone).unwrap();
    assert!(compiled.warnings.is_empty());
    let instrument = compiled.track(track).unwrap().instrument.unwrap();
    assert!(instrument.get() >= bus.project().next_id, "a graph-only id");

    // No default, or one that is not an instrument: silent, with a warning.
    for default in [None, Some("no.such.kind"), Some(KIND_GAIN)] {
        let none = options().with_default_instrument(default);
        let compiled = compile_project(bus.project(), &registry(), &none).unwrap();
        assert_eq!(compiled.sounding_tracks, 0);
        assert_eq!(compiled.graph.order(), [compiled.master]);
        assert_eq!(compiled.warnings.len(), 1, "{:?}", compiled.warnings);
        assert!(compiled.warnings[0].contains("no instrument"));
    }
}

#[test]
fn misplaced_devices_are_reported() {
    let (mut bus, track) = one_note();
    add_device(&mut bus, track, KIND_TONE, Vec::new());
    add_device(&mut bus, track, KIND_TONE, Vec::new());
    let drums = add_track(&mut bus, "Bus", TrackKind::Bus);
    bus.execute(SetTrackOutput {
        track,
        output: Some(drums),
    })
    .unwrap();
    add_device(&mut bus, drums, KIND_TONE, Vec::new());
    add_device(&mut bus, drums, KIND_OCTAVE, Vec::new());

    let warnings = compile(&bus).warnings;
    assert_eq!(warnings.len(), 3, "{warnings:?}");
    assert!(warnings[0].contains("second instrument"));
    assert!(warnings[1].contains("instrument") && warnings[1].contains("bus track"));
    assert!(warnings[2].contains("note effect") && warnings[2].contains("bus track"));
}

#[test]
fn track_gain_and_pan_are_applied_per_channel() {
    let (mut bus, track) = one_note();
    add_device(&mut bus, track, KIND_TONE, Vec::new());
    let reference = render(&bus);

    bus.execute(SetTrackGain {
        track,
        gain_db: -6.0,
    })
    .unwrap();
    bus.execute(SetTrackPan { track, pan: 0.5 }).unwrap();
    let out = render(&bus);
    // Half right: the right channel keeps the fader gain, the left is
    // turned down by cos(45 degrees).
    let fader = db_to_linear(-6.0);
    assert_null(&out.right, &scaled(&reference.right, fader), -120.0);
    assert_null(
        &out.left,
        &scaled(&reference.left, fader * std::f32::consts::FRAC_1_SQRT_2),
        -120.0,
    );

    bus.execute(SetTrackPan { track, pan: -1.0 }).unwrap();
    let hard_left = render(&bus);
    assert!(is_silent(&hard_left.right, -120.0));
    assert!(rms(&hard_left.left) > 0.1);
}

/// Kick routed to a bus; hats straight to the master.
fn bussed() -> (CommandBus, HousePatternWritten, TrackId) {
    let (mut bus, written) = house(1);
    let drums = add_track(&mut bus, "Drums", TrackKind::Bus);
    bus.execute(SetTrackOutput {
        track: written.kick_track,
        output: Some(drums),
    })
    .unwrap();
    (bus, written, drums)
}

/// Low-band (kick) and high-band (hat) power of a render.
fn bands(out: &StereoBuffer) -> (f64, f64) {
    let mono = out.mono();
    (
        spectrum_band_energy(&mono, RATE, 30.0, 150.0),
        spectrum_band_energy(&mono, RATE, 6_000.0, 20_000.0),
    )
}

#[test]
fn a_bus_sums_its_tracks_and_its_gain_and_effects_apply_to_them_only() {
    let (plain, _) = house(1);
    let reference = render(&plain);
    let (kick_power, hat_power) = bands(&reference);
    assert!(
        kick_power > 1e-4 && hat_power > 1e-5,
        "{kick_power} {hat_power}"
    );

    // Routing through a unity bus changes nothing.
    let (mut bus, written, drums) = bussed();
    let compiled = compile(&bus);
    assert_eq!(compiled.sounding_tracks, 2);
    let strip = compiled.track(drums).unwrap();
    assert!(strip.input.is_some() && strip.player.is_none() && strip.instrument.is_none());
    assert_null(&render(&bus).left, &reference.left, -120.0);

    // Track and bus gains multiply along the route: -6 dB and -6 dB on the
    // kick's path is 12 dB less kick power, and the hats are untouched.
    bus.execute(SetTrackGain {
        track: written.kick_track,
        gain_db: -6.0,
    })
    .unwrap();
    bus.execute(SetTrackGain {
        track: drums,
        gain_db: -6.0,
    })
    .unwrap();
    let (kick_after, hat_after) = bands(&render(&bus));
    let drop_db = 10.0 * (kick_after / kick_power).log10();
    assert!((drop_db + 12.0).abs() < 0.1, "kick changed by {drop_db} dB");
    assert!((hat_after / hat_power - 1.0).abs() < 0.01);

    // An effect on the bus processes what is routed to it: a gain of zero
    // removes the kick and leaves the hats.
    add_device(
        &mut bus,
        drums,
        KIND_GAIN,
        vec![param(Gain::GAIN, "Gain", 0.0, -16.0, 16.0)],
    );
    let (kick_gone, hat_kept) = bands(&render(&bus));
    assert!(kick_gone < kick_power * 1e-4, "{kick_gone}");
    assert!((hat_kept / hat_power - 1.0).abs() < 0.01);
}

#[test]
fn mute_and_solo_decide_what_is_heard() {
    let (plain, _) = house(1);
    let (kick_power, hat_power) = bands(&render(&plain));
    let (mut bus, written, drums) = bussed();
    let heard = |bus: &CommandBus| {
        let (kick, hat) = bands(&render(bus));
        (kick > kick_power * 0.5, hat > hat_power * 0.5)
    };
    assert_eq!(heard(&bus), (true, true));

    // Muting the bus silences the track routed to it, not the others.
    bus.execute(SetTrackMute {
        track: drums,
        mute: true,
    })
    .unwrap();
    assert_eq!(heard(&bus), (false, true));
    assert_eq!(compile(&bus).sounding_tracks, 1);
    bus.undo().unwrap();

    // Solo the hats: the kick and its bus go quiet.
    bus.execute(SetTrackSolo {
        track: written.hat_track,
        solo: true,
    })
    .unwrap();
    assert_eq!(heard(&bus), (false, true));
    bus.undo().unwrap();

    // Solo the kick: its bus stays open to carry it.
    bus.execute(SetTrackSolo {
        track: written.kick_track,
        solo: true,
    })
    .unwrap();
    assert_eq!(heard(&bus), (true, false));
    bus.undo().unwrap();

    // Solo the bus: what feeds it is heard.
    bus.execute(SetTrackSolo {
        track: drums,
        solo: true,
    })
    .unwrap();
    assert_eq!(heard(&bus), (true, false));

    // Mute wins over solo.
    bus.execute(SetTrackMute {
        track: written.kick_track,
        mute: true,
    })
    .unwrap();
    assert_eq!(heard(&bus), (false, false));
    assert_eq!(compile(&bus).sounding_tracks, 0);
}

#[test]
fn saved_device_state_is_loaded_from_the_project_folder() {
    let dir = tempfile::tempdir().unwrap();
    let (mut bus, track) = one_note();
    let tone = add_device(&mut bus, track, KIND_TONE, Vec::new());
    // State: transpose by 12 semitones.
    let hash = PluginStateStore::new(dir.path()).put(&[12]).unwrap();
    bus.execute(SetDeviceState {
        device: tone,
        state_blob: Some(hash),
    })
    .unwrap();

    let with_dir = RenderOptions {
        compile: options().with_project_dir(dir.path()),
        ..RenderOptions::new(RATE).with_tail(0.0)
    };
    let rendered = render_project(bus.project(), &registry(), &with_dir).unwrap();
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    assert!((dominant_frequency(&rendered.channel(0), RATE) - 880.0).abs() < 0.5);

    // Without the folder the device starts from its parameters, and says so.
    let compiled = compile(&bus);
    assert_eq!(compiled.warnings.len(), 1, "{:?}", compiled.warnings);
    assert!(compiled.warnings[0].contains("saved state"));
    assert!((dominant_frequency(&render(&bus).left, RATE) - 440.0).abs() < 0.5);

    // A blob that is missing from the folder, or that the device rejects.
    let empty = tempfile::tempdir().unwrap();
    let missing = options().with_project_dir(empty.path());
    let compiled = compile_project(bus.project(), &registry(), &missing).unwrap();
    assert!(compiled.warnings[0].contains("cannot load the state"));
    let bad = PluginStateStore::new(dir.path()).put(&[1, 2, 3]).unwrap();
    bus.execute(SetDeviceState {
        device: tone,
        state_blob: Some(bad),
    })
    .unwrap();
    let compiled = compile_project(bus.project(), &registry(), &with_dir.compile).unwrap();
    assert!(
        compiled.warnings[0].contains("state must be one byte"),
        "{:?}",
        compiled.warnings
    );
    assert!(
        !compiled.graph.contains(tone),
        "left out; the default plays"
    );
    assert!(compiled.track(track).unwrap().instrument.is_some());
}

#[test]
fn render_project_renders_the_length_plus_the_tail_and_honours_limits() {
    let (bus, _) = house(1);
    let options = RenderOptions::new(SampleRate::HZ_44100).with_tail(0.25);
    let rendered = render_project(bus.project(), &registry(), &options).unwrap();
    // One bar at 120 BPM is two seconds.
    assert_eq!(rendered.frames(), 88_200 + 11_025);
    assert_eq!((rendered.channels, rendered.sounding_tracks), (2, 2));
    assert_eq!(rendered.sample_rate, SampleRate::HZ_44100);
    assert_eq!(
        rendered.channel(0),
        rendered.channel(1),
        "nothing is panned"
    );
    assert!(rendered
        .samples
        .iter()
        .all(|s| s.is_finite() && s.abs() <= 1.0));

    // Rendering twice gives identical audio.
    let again = render_project(bus.project(), &registry(), &options).unwrap();
    assert_eq!(rendered, again);

    let limited = options.clone().with_max_frames(1_000);
    let error = render_project(bus.project(), &registry(), &limited).unwrap_err();
    assert!(
        matches!(error, RenderError::TooLong { frames: 99_225 }),
        "{error}"
    );

    // A negative tail counts as none; an empty project renders the tail.
    let none = RenderOptions::new(RATE).with_tail(-1.0);
    let rendered = render_project(bus.project(), &registry(), &none).unwrap();
    assert_eq!(rendered.frames(), 96_000);
    let empty = render_project(
        &Project::new("Empty"),
        &registry(),
        &RenderOptions::new(RATE),
    );
    assert_eq!(empty.unwrap().frames(), 24_000);
}
