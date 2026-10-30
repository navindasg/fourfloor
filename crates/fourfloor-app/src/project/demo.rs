//! The built-in demo: an eight-bar house loop in A minor at 124 BPM, built
//! entirely through the command bus with fourfloor's own instruments and
//! effects. It is what the app opens on first start.
//!
//! Drums (909 kick, clap, hats) sum on a glued bus; an acid bass and a pad
//! are ducked by the kick through sidechain compressors; FM stabs go
//! through a filter that rises over the loop (automation) with a macro on
//! top; reverb and delay returns; a mix bus with glue and a limiter; an LFO
//! on the pad; two launcher scenes.

use fourfloor_core::{DeviceId, DeviceRegistry, ParamId, Ticks, TrackId};
use fourfloor_effects::{Compressor, Delay, Filter, Limiter, Reverb, Saturator};
use fourfloor_instruments::{Acid303, FmSynth, PolyPad};
use fourfloor_model::command::{
    AddDevice, AddMacro, AddModulator, AddScene, CreateTrack, RenameProject, SetDeviceSidechain,
    SetMacroTarget, SetModulation, SetSend, SetSwing, SetTempo, SetTrackGain, SetTrackOutput,
    SetTrackPan, WriteAutomation, WriteMidiClip, WriteMidiSlot, WriteStepPattern,
};
use fourfloor_model::sequencer::{StepLane, BAR_TICKS};
use fourfloor_model::{
    AutomationPoint, CommandBus, LfoRate, LfoShape, MidiNote, ModSource, ModelError, Project,
    TrackKind,
};

use crate::services::registry::{device_params, UnknownKind};

/// Bars in the demo loop.
pub const DEMO_BARS: u32 = 8;
/// Demo tempo.
pub const DEMO_BPM: f64 = 124.0;
/// Demo swing.
pub const DEMO_SWING: f64 = 56.0;
/// Track names, in order, for tests and the UI.
pub const DEMO_TRACKS: [&str; 10] = [
    "Kick", "Clap", "Hats", "Drums", "Bass", "Stabs", "Pad", "Reverb", "Delay", "Mix",
];

/// Building the demo failed (only if a device kind is missing or the
/// model refuses a command, both bugs).
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    /// The model refused a command.
    #[error(transparent)]
    Model(#[from] ModelError),
    /// A device kind is not registered.
    #[error(transparent)]
    Kind(#[from] UnknownKind),
    /// A command did not report the id it should have.
    #[error("{0} returned no id")]
    MissingId(&'static str),
}

fn note(start: i64, length: i64, pitch: u8, velocity: u8) -> MidiNote {
    MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(length),
        pitch,
        velocity,
    }
}

fn point(time: i64, value: f32) -> AutomationPoint {
    AutomationPoint {
        time: Ticks::new(time),
        value,
    }
}

fn lane(pitch: u8, velocity: u8, length: i64, hits: &[usize], steps: usize) -> StepLane {
    let mut cells = vec![false; steps];
    for hit in hits {
        if let Some(cell) = cells.get_mut(*hit) {
            *cell = true;
        }
    }
    StepLane {
        pitch,
        velocity,
        note_length: Ticks::new(length),
        steps: cells,
        velocities: Vec::new(),
    }
}

struct Builder<'a> {
    bus: CommandBus,
    registry: &'a DeviceRegistry,
}

impl Builder<'_> {
    fn track(&mut self, name: &str, kind: TrackKind) -> Result<TrackId, DemoError> {
        self.bus
            .execute(CreateTrack {
                name: name.to_owned(),
                kind,
                index: None,
            })?
            .track()
            .ok_or(DemoError::MissingId("create_track"))
    }

    fn device(
        &mut self,
        track: TrackId,
        kind: &str,
        values: &[(ParamId, f32)],
    ) -> Result<DeviceId, DemoError> {
        let params = device_params(self.registry, kind, values)?;
        let name = self.registry.descriptor(kind).map(|d| d.name.clone());
        self.bus
            .execute(AddDevice {
                track,
                kind: kind.to_owned(),
                name,
                index: None,
                params,
            })?
            .device()
            .ok_or(DemoError::MissingId("add_device"))
    }

    fn route(&mut self, track: TrackId, output: TrackId) -> Result<(), DemoError> {
        self.bus.execute(SetTrackOutput {
            track,
            output: Some(output),
        })?;
        Ok(())
    }

    fn gain(&mut self, track: TrackId, gain_db: f32) -> Result<(), DemoError> {
        self.bus.execute(SetTrackGain { track, gain_db })?;
        Ok(())
    }

    fn steps(&mut self, track: TrackId, name: &str, lanes: Vec<StepLane>) -> Result<(), DemoError> {
        self.bus.execute(WriteStepPattern {
            track,
            clip: None,
            name: name.to_owned(),
            start: Ticks::ZERO,
            bars: DEMO_BARS,
            lanes,
            swing: None,
            groove: None,
            groove_amount: 1.0,
        })?;
        Ok(())
    }

    fn clip(&mut self, track: TrackId, name: &str, notes: Vec<MidiNote>) -> Result<(), DemoError> {
        self.bus.execute(WriteMidiClip {
            track,
            clip: None,
            name: name.to_owned(),
            start: Ticks::ZERO,
            length: Ticks::new(i64::from(DEMO_BARS) * BAR_TICKS),
            notes,
        })?;
        Ok(())
    }

    fn duck(&mut self, track: TrackId, key: TrackId, depth: f32) -> Result<DeviceId, DemoError> {
        let device = self.device(
            track,
            "fx.compressor",
            &[
                (Compressor::THRESHOLD, -18.0 - 18.0 * depth),
                (Compressor::RATIO, 2.0 + 18.0 * depth),
                (Compressor::ATTACK, 0.5),
                (Compressor::RELEASE, 160.0),
                (Compressor::KNEE, 3.0),
                (Compressor::MAKEUP, 0.0),
                (Compressor::MIX, 1.0),
                (Compressor::EXTERNAL_KEY, 1.0),
            ],
        )?;
        self.bus.execute(SetDeviceSidechain {
            device,
            source: Some(key),
            pre_fader: true,
        })?;
        Ok(device)
    }
}

/// Chord roots of the progression, one per two bars: Am, F, C, G.
const ROOTS: [u8; 4] = [57, 53, 48, 55];
/// Minor or major triad plus seventh for each root, as intervals.
const QUALITIES: [[u8; 4]; 4] = [[0, 3, 7, 10], [0, 4, 7, 11], [0, 4, 7, 11], [0, 4, 7, 10]];

fn chord_notes(
    root: u8,
    quality: &[u8; 4],
    start: i64,
    length: i64,
    velocity: u8,
) -> Vec<MidiNote> {
    quality
        .iter()
        .map(|interval| note(start, length, root + interval, velocity))
        .collect()
}

/// Builds the demo project. `registry` must hold fourfloor's instruments
/// and effects (see [`crate::services::registry::build_registry`]).
pub fn build_demo_project(registry: &DeviceRegistry) -> Result<Project, DemoError> {
    let mut b = Builder {
        bus: CommandBus::new(Project::new("Untitled")),
        registry,
    };
    b.bus.execute(RenameProject {
        name: "fourfloor demo".to_owned(),
    })?;
    b.bus.execute(SetTempo { bpm: DEMO_BPM })?;
    b.bus.execute(SetSwing {
        percent: DEMO_SWING,
        grid: None,
    })?;
    let bars = i64::from(DEMO_BARS);
    let length = bars * BAR_TICKS;
    let steps = DEMO_BARS as usize * 16;
    let every =
        |offset: usize, period: usize| -> Vec<usize> { (offset..steps).step_by(period).collect() };

    // Drums.
    let kick = b.track("Kick", TrackKind::Midi)?;
    b.device(kick, "drum.kick909", &[])?;
    b.steps(
        kick,
        "Four to the floor",
        vec![lane(36, 120, 200, &every(0, 4), steps)],
    )?;
    let clap = b.track("Clap", TrackKind::Midi)?;
    b.device(clap, "drum.clap909", &[])?;
    b.steps(
        clap,
        "Backbeat",
        vec![lane(39, 105, 200, &every(4, 8), steps)],
    )?;
    let hats = b.track("Hats", TrackKind::Midi)?;
    b.device(hats, "drum.hat909", &[])?;
    let mut closed = lane(42, 70, 100, &every(0, 1), steps);
    closed
        .steps
        .iter_mut()
        .skip(2)
        .step_by(4)
        .for_each(|s| *s = false);
    closed.velocities = (0..steps)
        .map(|s| if s % 2 == 1 { 90 } else { 0 })
        .collect();
    b.steps(
        hats,
        "Hats",
        vec![closed, lane(46, 100, 200, &every(2, 4), steps)],
    )?;
    b.bus.execute(SetTrackPan {
        track: hats,
        pan: 0.15,
    })?;
    let drums = b.track("Drums", TrackKind::Bus)?;
    b.device(drums, "fx.glue", &[])?;
    for track in [kick, clap, hats] {
        b.route(track, drums)?;
    }
    b.gain(hats, -6.0)?;
    b.gain(clap, -3.0)?;

    // Bass: offbeat acid line, ducked by the kick.
    let bass = b.track("Bass", TrackKind::Midi)?;
    b.device(
        bass,
        "synth.acid303",
        &[
            (Acid303::CUTOFF, 500.0),
            (Acid303::RESONANCE, 70.0),
            (Acid303::ENV_MOD, 55.0),
            (Acid303::DECAY, 300.0),
        ],
    )?;
    b.device(bass, "fx.saturator", &[(Saturator::DRIVE, 6.0)])?;
    b.duck(bass, kick, 0.6)?;
    let bass_roots = [45u8, 41, 36, 43];
    let bass_notes = (0..bars * 16)
        .filter(|step| step % 4 == 2 || step % 16 == 15)
        .map(|step| {
            let root = bass_roots[((step / 32) % 4) as usize];
            let up = if step % 16 == 15 { 12 } else { 0 };
            let accent = if step % 8 == 6 { 110 } else { 90 };
            note(step * 240, 200, root + up, accent)
        })
        .collect();
    b.clip(bass, "Acid line", bass_notes)?;
    b.gain(bass, -4.0)?;

    // FM stabs through a rising filter.
    let stabs = b.track("Stabs", TrackKind::Midi)?;
    b.device(stabs, "fm.stab", &[(FmSynth::RELEASE, 0.15)])?;
    let filter = b.device(
        stabs,
        "fx.filter",
        &[(Filter::CUTOFF, 800.0), (Filter::RESONANCE, 0.4)],
    )?;
    let stab_notes = (0..bars * 4)
        .flat_map(|beat| {
            let bar_pair = ((beat / 8) % 4) as usize;
            let start = beat * 960 + 480;
            chord_notes(ROOTS[bar_pair] + 12, &QUALITIES[bar_pair], start, 180, 95)
        })
        .collect();
    b.clip(stabs, "Offbeat stabs", stab_notes)?;
    b.bus.execute(WriteAutomation {
        device: filter,
        param: Filter::CUTOFF,
        points: vec![
            point(0, 600.0),
            point(length - BAR_TICKS, 6000.0),
            point(length, 1200.0),
        ],
    })?;
    b.gain(stabs, -8.0)?;

    // Pad: sustained chords, ducked, slow LFO on its cutoff.
    let pad = b.track("Pad", TrackKind::Midi)?;
    let pad_synth = b.device(
        pad,
        "poly.pad",
        &[
            (PolyPad::ATTACK, 0.4),
            (PolyPad::RELEASE, 0.8),
            (PolyPad::CUTOFF, 1800.0),
        ],
    )?;
    b.duck(pad, kick, 0.8)?;
    let pad_notes = (0..4)
        .flat_map(|pair| {
            let start = pair * 2 * BAR_TICKS;
            chord_notes(
                ROOTS[pair as usize],
                &QUALITIES[pair as usize],
                start,
                2 * BAR_TICKS - 120,
                80,
            )
        })
        .collect();
    b.clip(pad, "Pad chords", pad_notes)?;
    b.gain(pad, -10.0)?;
    let wobble = b
        .bus
        .execute(AddModulator {
            name: "Pad LFO".to_owned(),
            source: ModSource::Lfo {
                shape: LfoShape::Sine,
                rate: LfoRate::Beats(8.0),
                phase: 0.0,
            },
        })?
        .modulator()
        .ok_or(DemoError::MissingId("add_modulator"))?;
    b.bus.execute(SetModulation {
        modulator: wobble,
        device: pad_synth,
        param: PolyPad::CUTOFF,
        depth: 0.05,
    })?;

    // Returns.
    let reverb_bus = b.track("Reverb", TrackKind::Bus)?;
    let reverb = b.device(
        reverb_bus,
        "fx.reverb",
        &[(Reverb::MIX, 1.0), (Reverb::DECAY, 2.5)],
    )?;
    let delay_bus = b.track("Delay", TrackKind::Bus)?;
    b.device(
        delay_bus,
        "fx.delay",
        &[
            (Delay::MIX, 1.0),
            (Delay::BEATS, 0.75),
            (Delay::FEEDBACK, 0.35),
        ],
    )?;
    for (track, target, gain_db) in [
        (clap, reverb_bus, -12.0),
        (pad, reverb_bus, -6.0),
        (stabs, reverb_bus, -10.0),
        (stabs, delay_bus, -12.0),
    ] {
        b.bus.execute(SetSend {
            track,
            target,
            gain_db,
            pre_fader: false,
        })?;
    }

    // Mix bus with glue and a limiter.
    let mix = b.track("Mix", TrackKind::Bus)?;
    b.device(mix, "fx.glue", &[])?;
    b.device(mix, "fx.limiter", &[(Limiter::CEILING, -1.0)])?;
    for track in [drums, bass, stabs, pad, reverb_bus, delay_bus] {
        b.route(track, mix)?;
    }

    // A macro over the stab filter and the reverb return.
    let energy = b
        .bus
        .execute(AddMacro {
            name: "Energy".to_owned(),
            value: 0.3,
        })?
        .macro_id()
        .ok_or(DemoError::MissingId("add_macro"))?;
    b.bus.execute(SetMacroTarget {
        macro_id: energy,
        device: reverb,
        param: Reverb::MIX,
        min: 0.6,
        max: 1.0,
    })?;

    // Launcher scenes for the bass.
    for (name, hits) in [
        ("Groove", vec![2i64, 6, 10, 14]),
        ("Rolling", vec![0, 2, 3, 6, 8, 10, 11, 14]),
    ] {
        let scene = b
            .bus
            .execute(AddScene {
                name: name.to_owned(),
                index: None,
            })?
            .scene()
            .ok_or(DemoError::MissingId("add_scene"))?;
        b.bus.execute(WriteMidiSlot {
            track: bass,
            scene,
            name: format!("{name} bass"),
            length: Ticks::new(BAR_TICKS),
            notes: hits
                .into_iter()
                .map(|step| note(step * 240, 200, 45, 100))
                .collect(),
        })?;
    }
    Ok(b.bus.project().clone())
}
