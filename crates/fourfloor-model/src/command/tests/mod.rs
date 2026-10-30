//! Tests for every command: its effect, undo, redo and validation failures.

use fourfloor_core::{AudioMessage, ClipId, DeviceId, ParamId, Tempo, Ticks, TrackId};
use serde_json::json;

use super::*;
use crate::bus::CommandBus;
use crate::clip::{AudioRef, ClipContent, MidiNote, Warp};
use crate::device::{AutomationPoint, Param, SidechainSource};
use crate::groove::GrooveTemplate;
use crate::hash::ContentHash;
use crate::ids::{MacroId, ModulatorId, SceneId};
use crate::modulation::{LfoRate, LfoShape, ModSource};
use crate::music::{ChordKind, Scale};
use crate::project::{Groove, SwingGrid};
use crate::sequencer::StepLane;
use crate::track::{MixerParam, TrackKind};

const CUTOFF: ParamId = ParamId::new(1);
const RESONANCE: ParamId = ParamId::new(2);

struct Fixture {
    bus: CommandBus,
    midi: TrackId,
    audio: TrackId,
    group: TrackId,
    filter: DeviceId,
}

fn create(name: &str, kind: TrackKind) -> CreateTrack {
    CreateTrack {
        name: name.to_owned(),
        kind,
        index: None,
    }
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

fn note(start: i64, pitch: u8) -> MidiNote {
    MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(240),
        pitch,
        velocity: 100,
    }
}

fn audio_ref() -> AudioRef {
    AudioRef {
        hash: ContentHash::of_bytes(b"loop"),
        extension: "wav".to_owned(),
    }
}

/// A MIDI track with a filter device, an audio track and a bus.
fn fixture() -> Fixture {
    let mut bus = CommandBus::new(Project::new("Test"));
    let midi = bus
        .execute(create("Bass", TrackKind::Midi))
        .unwrap()
        .track()
        .unwrap();
    let audio = bus
        .execute(create("Loop", TrackKind::Audio))
        .unwrap()
        .track()
        .unwrap();
    let group = bus
        .execute(create("Group", TrackKind::Bus))
        .unwrap()
        .track()
        .unwrap();
    let filter = bus
        .execute(AddDevice {
            track: midi,
            kind: "builtin.filter".to_owned(),
            name: Some("Filter".to_owned()),
            index: None,
            params: vec![
                param(CUTOFF, "Cutoff", 1_000.0, 20.0, 20_000.0),
                param(RESONANCE, "Resonance", 0.2, 0.0, 1.0),
            ],
        })
        .unwrap()
        .device()
        .unwrap();
    Fixture {
        bus,
        midi,
        audio,
        group,
        filter,
    }
}

fn midi_clip(fixture: &mut Fixture, start: i64) -> ClipId {
    let track = fixture.midi;
    fixture
        .bus
        .execute(WriteMidiClip {
            track,
            clip: None,
            name: "Bassline".to_owned(),
            start: Ticks::new(start),
            length: Ticks::new(3_840),
            notes: vec![note(0, 36)],
        })
        .unwrap()
        .clip()
        .unwrap()
}

/// Runs a command and checks that undo restores the exact previous project
/// and redo restores the exact new one. Returns the outcome.
fn round_trip(bus: &mut CommandBus, command: impl Into<Command>) -> Outcome {
    let before = bus.project().clone();
    let outcome = bus.execute(command).unwrap();
    let after = bus.project().clone();
    assert_ne!(before, after, "the command should change the project");
    assert_eq!(after.validate(), Ok(()));
    bus.undo().unwrap();
    assert_eq!(
        bus.project(),
        &before,
        "undo should restore the old project"
    );
    bus.redo().unwrap();
    assert_eq!(bus.project(), &after, "redo should restore the new project");
    outcome
}

/// Runs a command that must fail and checks that nothing changed.
fn rejected(bus: &mut CommandBus, command: impl Into<Command>) -> ModelError {
    let before = bus.project().clone();
    let depth = bus.undo_depth();
    let error = bus.execute(command).unwrap_err();
    assert_eq!(bus.project(), &before);
    assert_eq!(bus.undo_depth(), depth);
    error
}

fn audio_clip(fixture: &mut Fixture, start: i64, length: i64) -> ClipId {
    let track = fixture.audio;
    fixture
        .bus
        .execute(AddAudioClip {
            track,
            name: "Loop".to_owned(),
            start: Ticks::new(start),
            length: Ticks::new(length),
            source: audio_ref(),
            source_offset_frames: 0,
        })
        .unwrap()
        .clip()
        .unwrap()
}

mod arrange;
mod clip;
mod device;
mod global;
mod mixer;
mod modulation;
mod pianoroll;
mod session;
mod track;
mod wire;
