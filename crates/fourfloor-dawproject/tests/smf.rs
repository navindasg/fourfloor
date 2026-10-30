//! Standard MIDI File export and import of clips and tracks: exact note
//! round trips, the example file from the MIDI 1.0 specification, and a
//! render null test of a track rebuilt from its own exported file.

mod common;

use std::sync::Arc;

use common::{registry, render, rich_project};
use fourfloor_core::{ClipId, Ticks};
use fourfloor_dawproject::smf::{clip_to_smf, read_smf, track_to_smf, write_notes, SmfError, PPQ};
use fourfloor_model::{AutomationLane, Clip, MidiNote, MixerLane, Track, TrackKind};
use fourfloor_testkit::{null_residual_db, rms_db};

/// The sounding notes of a track's timeline clips, at timeline positions.
fn timeline_notes(track: &Track) -> Vec<MidiNote> {
    let mut notes: Vec<MidiNote> = track
        .clips
        .iter()
        .filter(|clip| clip.is_midi())
        .flat_map(|clip| {
            clip.sounding_notes().into_iter().map(|note| MidiNote {
                start: note.start + clip.start,
                ..note
            })
        })
        .collect();
    notes.sort_by_key(|n| (n.start, n.pitch));
    notes
}

#[test]
fn every_midi_track_round_trips_through_a_file_on_disk() {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let project = rich_project(&registry, dir.path());
    let mut checked = 0;
    for track in project.tracks.iter().filter(|t| t.kind == TrackKind::Midi) {
        let path = dir.path().join(format!("{}.mid", track.id.get()));
        std::fs::write(&path, track_to_smf(track, project.tempo)).unwrap();
        let smf = read_smf(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!((smf.format, smf.ppq), (1, PPQ));
        assert_eq!(smf.tempo, Some(project.tempo));
        assert_eq!(smf.time_signature, Some((4, 4)));
        assert_eq!(smf.tracks.len(), 2);
        assert_eq!(smf.tracks[1].name.as_deref(), Some(track.name.as_str()));
        let expected = timeline_notes(track);
        assert!(!expected.is_empty());
        assert_eq!(smf.notes(), expected, "track {}", track.name);
        checked += 1;
    }
    assert_eq!(checked, 3);
}

#[test]
fn a_looped_offset_clip_exports_as_it_sounds() {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let project = rich_project(&registry, dir.path());
    // "Acid C": a one-bar loop entered 1.5 bars into its content.
    let clip = project.tracks[1]
        .clips
        .iter()
        .find(|c| c.name == "Acid C")
        .unwrap();
    let smf = read_smf(&clip_to_smf(clip, project.tempo).unwrap()).unwrap();
    assert_eq!(smf.format, 0);
    assert_eq!(smf.tracks[0].name.as_deref(), Some("Acid C"));
    assert_eq!(smf.notes(), clip.sounding_notes());
    assert!(smf
        .notes()
        .iter()
        .all(|n| n.start >= Ticks::ZERO && n.start + n.length <= clip.length));

    let rebuilt = smf.to_clip(ClipId::new(99), "Rebuilt");
    assert_eq!(rebuilt.length, Ticks::new(3_840));
    assert_eq!(rebuilt.sounding_notes(), clip.sounding_notes());

    // An audio clip has no notes to write.
    let audio = &project.tracks[3].clips[0];
    assert_eq!(
        clip_to_smf(audio, project.tempo),
        Err(SmfError::NotMidiClip)
    );
}

/// Format 0 example from the MIDI 1.0 specification ("Standard MIDI Files
/// 1.0", appendix 1.4): 96 PPQ, 120 BPM, three channels, running status,
/// program changes, all notes ending on beat 4.
const SPEC_EXAMPLE: [u8; 81] = [
    0x4D, 0x54, 0x68, 0x64, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x01, 0x00, 0x60, //
    0x4D, 0x54, 0x72, 0x6B, 0x00, 0x00, 0x00, 0x3B, //
    0x00, 0xFF, 0x58, 0x04, 0x04, 0x02, 0x18, 0x08, //
    0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, //
    0x00, 0xC0, 0x05, //
    0x00, 0xC1, 0x2E, //
    0x00, 0xC2, 0x46, //
    0x00, 0x92, 0x30, 0x60, //
    0x00, 0x3C, 0x60, //
    0x60, 0x91, 0x43, 0x40, //
    0x60, 0x90, 0x4C, 0x20, //
    0x81, 0x40, 0x82, 0x30, 0x40, //
    0x00, 0x3C, 0x40, //
    0x00, 0x81, 0x43, 0x40, //
    0x00, 0x80, 0x4C, 0x40, //
    0x00, 0xFF, 0x2F, 0x00,
];

#[test]
fn the_specification_example_file_is_read() {
    let smf = read_smf(&SPEC_EXAMPLE).unwrap();
    assert_eq!((smf.format, smf.ppq), (0, 96));
    assert!((smf.tempo.unwrap().bpm() - 120.0).abs() < 1e-9);
    assert_eq!(smf.time_signature, Some((4, 4)));
    assert_eq!(smf.tracks.len(), 1);
    assert_eq!(smf.tracks[0].channels, 0b111);
    let found: Vec<(i64, i64, u8, u8)> = smf
        .notes()
        .iter()
        .map(|n| (n.start.get(), n.length.get(), n.pitch, n.velocity))
        .collect();
    // 96 PPQ scaled to fourfloor's 960 ticks per beat.
    assert_eq!(
        found,
        [
            (0, 3_840, 48, 96),
            (0, 3_840, 60, 96),
            (960, 2_880, 67, 64),
            (1_920, 1_920, 76, 32),
        ]
    );
    // Written back at 960 PPQ and read again, the notes are unchanged.
    let again = read_smf(&write_notes(&smf.notes(), smf.tempo.unwrap(), "spec")).unwrap();
    assert_eq!(again.notes(), smf.notes());
    // Truncating the file anywhere is an error, never a panic.
    for cut in 0..SPEC_EXAMPLE.len() {
        assert!(read_smf(&SPEC_EXAMPLE[..cut]).is_err(), "cut at {cut}");
    }
}

/// The track with its timeline clips replaced by one clip read from its
/// own exported file, and no automation (which is not in the file).
fn rebuilt(track: &Track, tempo: fourfloor_core::Tempo, id: u64) -> Track {
    let smf = read_smf(&track_to_smf(track, tempo)).unwrap();
    let clip: Clip = smf.to_clip(ClipId::new(id), "From MIDI");
    Track {
        clips: vec![Arc::new(clip)],
        ..track.clone()
    }
}

#[test]
fn tracks_rebuilt_from_their_midi_files_render_identically() {
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let rich = rich_project(&registry, dir.path());
    // Automation is not part of a MIDI file, so it is removed from both
    // sides; everything else stays.
    let mut base = rich.clone();
    base.tracks = base
        .tracks
        .iter()
        .map(|t| {
            Arc::new(Track {
                automation: Vec::<Arc<AutomationLane>>::new(),
                mixer_automation: Vec::<Arc<MixerLane>>::new(),
                ..(**t).clone()
            })
        })
        .collect();
    let mut from_midi = base.clone();
    from_midi.tracks = base
        .tracks
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if t.kind == TrackKind::Midi {
                Arc::new(rebuilt(t, base.tempo, 52 + i as u64))
            } else {
                Arc::clone(t)
            }
        })
        .collect();
    from_midi.validate().unwrap();

    let before = render(&base, &registry, dir.path());
    let after = render(&from_midi, &registry, dir.path());
    assert!(rms_db(&before.mono()) > -40.0, "the render is silent");
    assert_eq!(before.frames(), after.frames());
    for (a, b) in before.channels().into_iter().zip(after.channels()) {
        let residual = null_residual_db(a, b);
        assert!(residual <= -140.0, "residual {residual} dB");
    }
}
