//! Piano-roll editing: hit-testing notes and the edits a gesture makes.
//! Each finished gesture becomes one `write_midi_clip` replacing the clip's
//! notes, so it is one undo step.

use fourfloor_core::{ClipId, Ticks, TrackId};
use fourfloor_model::command::{StampChord, WriteMidiClip};
use fourfloor_model::{ChordKind, Clip, MidiNote};

/// Pixels from a note's right edge that grab the resize handle.
pub const EDGE_GRAB_PX: f32 = 6.0;
/// The shortest note an edit can make, in ticks.
pub const MIN_NOTE_TICKS: i64 = 30;

/// Where on a note the pointer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotePart {
    /// The body: dragging moves the note.
    Body,
    /// The right edge: dragging resizes it.
    End,
}

/// The note under (`ticks`, `pitch`), preferring the one that starts
/// latest. `edge_ticks` is how many ticks [`EDGE_GRAB_PX`] covers.
pub fn hit_note(
    notes: &[MidiNote],
    ticks: f64,
    pitch: u8,
    edge_ticks: f64,
) -> Option<(usize, NotePart)> {
    notes
        .iter()
        .enumerate()
        .filter(|(_, note)| note.pitch == pitch)
        .filter(|(_, note)| {
            let start = note.start.get() as f64;
            let end = (note.start + note.length).get() as f64;
            ticks >= start && ticks < end
        })
        .max_by_key(|(_, note)| note.start)
        .map(|(index, note)| {
            let end = (note.start + note.length).get() as f64;
            let part = if end - ticks <= edge_ticks {
                NotePart::End
            } else {
                NotePart::Body
            };
            (index, part)
        })
}

/// A new note at `start` (clamped into the clip).
pub fn add_note(
    notes: &[MidiNote],
    start: i64,
    length: i64,
    pitch: u8,
    velocity: u8,
    clip_length: i64,
) -> Vec<MidiNote> {
    let start = start.clamp(0, (clip_length - 1).max(0));
    let length = length.max(MIN_NOTE_TICKS).min(clip_length - start).max(1);
    let mut out = notes.to_vec();
    out.push(MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(length),
        pitch: pitch.min(127),
        velocity: velocity.clamp(1, 127),
    });
    sort(&mut out);
    out
}

/// Moves note `index` by `dt` ticks and `dp` semitones, kept inside the
/// clip and the MIDI range.
pub fn move_note(
    notes: &[MidiNote],
    index: usize,
    dt: i64,
    dp: i32,
    clip_length: i64,
) -> Vec<MidiNote> {
    let mut out = notes.to_vec();
    if let Some(note) = out.get_mut(index) {
        let length = note.length.get();
        let start = (note.start.get() + dt).clamp(0, (clip_length - length).max(0));
        note.start = Ticks::new(start);
        note.pitch = (i32::from(note.pitch) + dp).clamp(0, 127) as u8;
        note.length = Ticks::new(length.min(clip_length - start).max(1));
    }
    sort(&mut out);
    out
}

/// Sets note `index`'s length so it ends at `end` ticks (at least
/// [`MIN_NOTE_TICKS`], at most the clip end).
pub fn resize_note(notes: &[MidiNote], index: usize, end: i64, clip_length: i64) -> Vec<MidiNote> {
    let mut out = notes.to_vec();
    if let Some(note) = out.get_mut(index) {
        let start = note.start.get();
        let length = (end - start)
            .max(MIN_NOTE_TICKS)
            .min(clip_length - start)
            .max(1);
        note.length = Ticks::new(length);
    }
    out
}

/// Removes note `index`.
pub fn delete_note(notes: &[MidiNote], index: usize) -> Vec<MidiNote> {
    notes
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != index)
        .map(|(_, note)| *note)
        .collect()
}

/// Sets note `index`'s velocity.
pub fn set_velocity(notes: &[MidiNote], index: usize, velocity: u8) -> Vec<MidiNote> {
    let mut out = notes.to_vec();
    if let Some(note) = out.get_mut(index) {
        note.velocity = velocity.clamp(1, 127);
    }
    out
}

fn sort(notes: &mut [MidiNote]) {
    notes.sort_by_key(|note| (note.start, note.pitch));
}

/// The command that replaces `clip`'s notes with `notes`.
pub fn rewrite(track: TrackId, clip: &Clip, notes: Vec<MidiNote>) -> WriteMidiClip {
    WriteMidiClip {
        track,
        clip: Some(clip.id),
        name: clip.name.clone(),
        start: clip.start,
        length: clip.length,
        notes,
    }
}

/// The chord tool's command: a chord rooted at `root` from `start`.
pub fn chord(
    clip: ClipId,
    start: i64,
    length: i64,
    root: u8,
    kind: ChordKind,
    velocity: u8,
) -> StampChord {
    StampChord {
        clip,
        start: Ticks::new(start.max(0)),
        length: Ticks::new(length.max(1)),
        root,
        chord: kind,
        inversion: 0,
        velocity: velocity.clamp(1, 127),
    }
}

/// Every chord the chord tool offers, with a label.
pub const CHORDS: [(ChordKind, &str); 9] = [
    (ChordKind::Major, "maj"),
    (ChordKind::Minor, "min"),
    (ChordKind::Major7, "maj7"),
    (ChordKind::Minor7, "min7"),
    (ChordKind::Dominant7, "7"),
    (ChordKind::Minor9, "min9"),
    (ChordKind::Sus2, "sus2"),
    (ChordKind::Sus4, "sus4"),
    (ChordKind::Power, "5"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::command::CreateTrack;
    use fourfloor_model::{ClipContent, CommandBus, Project, TrackKind};

    fn n(start: i64, length: i64, pitch: u8) -> MidiNote {
        MidiNote {
            start: Ticks::new(start),
            length: Ticks::new(length),
            pitch,
            velocity: 100,
        }
    }

    #[test]
    fn hit_testing_finds_bodies_and_edges() {
        let notes = [n(0, 960, 60), n(480, 960, 60), n(0, 240, 64)];
        assert_eq!(hit_note(&notes, 600.0, 60, 30.0), Some((1, NotePart::Body)));
        assert_eq!(hit_note(&notes, 100.0, 60, 30.0), Some((0, NotePart::Body)));
        assert_eq!(hit_note(&notes, 1430.0, 60, 30.0), Some((1, NotePart::End)));
        assert_eq!(hit_note(&notes, 100.0, 62, 30.0), None);
        assert_eq!(hit_note(&notes, 240.0, 64, 30.0), None, "end is exclusive");
    }

    #[test]
    fn edits_stay_inside_the_clip_and_the_range() {
        let notes = vec![n(0, 480, 60)];
        let added = add_note(&notes, 3800, 480, 64, 90, 3840);
        assert_eq!(added[1].start.get(), 3800);
        assert_eq!(added[1].length.get(), 40, "cut at the clip end");
        let moved = move_note(&notes, 0, -500, 100, 3840);
        assert_eq!((moved[0].start.get(), moved[0].pitch), (0, 127));
        let moved = move_note(&notes, 0, 5000, -1, 3840);
        assert_eq!(moved[0].start.get(), 3840 - 480);
        let resized = resize_note(&notes, 0, 10, 3840);
        assert_eq!(resized[0].length.get(), MIN_NOTE_TICKS);
        assert!(delete_note(&notes, 0).is_empty());
        assert_eq!(set_velocity(&notes, 0, 0)[0].velocity, 1);
    }

    #[test]
    fn rewrites_and_chords_go_through_the_bus() {
        let mut bus = CommandBus::new(Project::new("Keys"));
        let track = bus
            .execute(CreateTrack {
                name: "Keys".to_owned(),
                kind: TrackKind::Midi,
                index: None,
            })
            .expect("track")
            .track()
            .expect("id");
        let clip = bus
            .execute(WriteMidiClip {
                track,
                clip: None,
                name: "Keys".to_owned(),
                start: Ticks::new(3840),
                length: Ticks::new(3840),
                notes: vec![],
            })
            .expect("clip")
            .clip()
            .expect("clip id");
        let current = bus.project().find_clip(clip).expect("clip").1.clone();
        let notes = add_note(current.notes(), 960, 240, 57, 100, current.length.get());
        bus.execute(rewrite(track, &current, notes))
            .expect("rewrite");
        bus.execute(chord(clip, 0, 960, 57, ChordKind::Minor, 90))
            .expect("chord");
        let after = bus.project().find_clip(clip).expect("clip").1.clone();
        assert_eq!(after.start.get(), 3840, "the clip stays where it was");
        let ClipContent::Midi { notes } = &after.content else {
            panic!("midi clip");
        };
        let mut placed: Vec<(i64, u8)> = notes.iter().map(|n| (n.start.get(), n.pitch)).collect();
        placed.sort_unstable();
        assert_eq!(placed, vec![(0, 57), (0, 60), (0, 64), (960, 57)]);
    }
}
