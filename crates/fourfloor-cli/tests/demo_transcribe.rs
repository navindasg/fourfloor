//! Audio to MIDI on the demo's rendered bassline with Basic Pitch.
//!
//! Needs the 230 kB Basic Pitch model in the default model cache; nothing
//! is downloaded. When it is missing the test prints a notice and passes,
//! unless `FOURFLOOR_REQUIRE_MODELS=1` is set. Fetch it once with
//! `cargo test -p fourfloor-ml -- --ignored download`.

mod demo_common;

use demo_common::*;
use fourfloor_audio::AudioBuffer;
use fourfloor_cli::demo::parts::{offbeat_bass, DROP};
use fourfloor_ml::{BasicPitch, ModelCache, ModelRegistry, TranscribeOptions, BASIC_PITCH};
use fourfloor_model::command::{SetDeviceBypass, SetTrackSolo};
use fourfloor_model::Command;

const BARS: u32 = 4;

fn model() -> Option<BasicPitch> {
    let registry = ModelRegistry::builtin();
    let cache = ModelCache::in_default_dir().offline();
    let cached = registry
        .get(BASIC_PITCH)
        .is_ok_and(|spec| cache.is_cached(spec));
    if !cached {
        let message = format!("Basic Pitch is not in {}", cache.dir().display());
        if std::env::var("FOURFLOOR_REQUIRE_MODELS").is_ok_and(|v| v == "1") {
            panic!("{message}");
        }
        eprintln!("SKIPPED: {message}");
        return None;
    }
    Some(BasicPitch::load(&cache, &registry).expect("Basic Pitch loads"))
}

#[test]
fn the_rendered_bassline_transcribes_back_to_its_notes() {
    let Some(model) = model() else { return };
    // The bass alone (it still plays through the Master bus and limiter),
    // without its sidechain ducker so the notes are not chopped.
    let project = demo();
    let bass = track(&project, "Bass");
    let ducker = device(&project, "Bass", "fx.compressor");
    let solo = edit(
        &project,
        vec![
            Command::SetTrackSolo(SetTrackSolo {
                track: bass,
                solo: true,
            }),
            Command::SetDeviceBypass(SetDeviceBypass {
                device: ducker,
                bypassed: true,
            }),
        ],
    );
    let start = bar_seconds(DROP.start_bar);
    let audio = render(&solo).slice_seconds(start, start + bar_seconds(BARS));
    let buffer = AudioBuffer::new(RATE, vec![audio.left.clone(), audio.right.clone()]);
    let found = model
        .transcribe(&buffer, &TranscribeOptions::default())
        .expect("transcribe");

    let latency = latency() as f64 / RATE.as_f64();
    let tick_seconds = beat_seconds() / 960.0;
    let expected: Vec<(f64, u8)> = offbeat_bass(BARS)
        .iter()
        .map(|n| (n.start.get() as f64 * tick_seconds + latency, n.pitch))
        .collect();
    let mut used = vec![false; expected.len()];
    let mut matched = Vec::new();
    let mut extra = Vec::new();
    for note in &found.notes {
        let hit = expected.iter().enumerate().position(|(i, &(at, pitch))| {
            !used[i] && pitch == note.pitch && (note.start_seconds - at).abs() <= 0.05
        });
        match hit {
            Some(i) => {
                used[i] = true;
                matched.push(note);
            }
            None => extra.push(note),
        }
    }
    eprintln!(
        "{} notes found, {} expected, {} matched (pitch exact, onset within 50 ms), {} extra",
        found.notes.len(),
        expected.len(),
        matched.len(),
        extra.len()
    );
    // Every played note comes back with its pitch and onset.
    assert_eq!(matched.len(), expected.len(), "{:?}", found.notes);
    // Basic Pitch hears each note's organ tail as a second segment of the
    // same pitch that starts where the first one ends. Anything else extra
    // is a wrong note.
    for note in &extra {
        let continues = found.notes.iter().any(|other| {
            other.pitch == note.pitch && (other.end_seconds - note.start_seconds).abs() < 0.005
        });
        assert!(continues, "a stray note: {note:?}");
    }
}
