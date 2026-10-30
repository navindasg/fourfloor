use fourfloor_core::{
    Device, Event, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo,
    TransportContext,
};

/// A note effect that turns every note into a chord.
///
/// It sits on event edges before a track's instrument. Each incoming note
/// passes through, followed by up to [`Chord::VOICES`] copies shifted by
/// the device's intervals. An interval of 0 switches that voice off, and a
/// copy that would leave the MIDI range is dropped. The note-off of a note
/// releases the same copies its note-on started, even if the intervals
/// were changed in between.
///
/// Its audio output is silence.
#[derive(Clone, Debug)]
pub struct Chord {
    intervals: [i8; Chord::VOICES],
    /// The intervals each sounding note was played with, per channel and
    /// pitch, so its note-off releases the same pitches.
    sounding: Box<[[i8; Chord::VOICES]; 16 * 128]>,
}

const fn interval_info(id: ParamId, name: &'static str, default: f32) -> ParamInfo {
    ParamInfo::new(id, name, -24.0, 24.0, default, ParamUnit::Semitones)
}

static PARAMS: [ParamInfo; Chord::VOICES] = [
    interval_info(Chord::INTERVAL_1, "Interval 1", 4.0),
    interval_info(Chord::INTERVAL_2, "Interval 2", 7.0),
    interval_info(Chord::INTERVAL_3, "Interval 3", 0.0),
    interval_info(Chord::INTERVAL_4, "Interval 4", 0.0),
];

impl Default for Chord {
    fn default() -> Self {
        Self::new(&[4, 7])
    }
}

impl Chord {
    /// Added voices a chord can have.
    pub const VOICES: usize = 4;
    /// First added voice, in semitones from the played note; 0 is off.
    pub const INTERVAL_1: ParamId = ParamId(0);
    /// Second added voice.
    pub const INTERVAL_2: ParamId = ParamId(1);
    /// Third added voice.
    pub const INTERVAL_3: ParamId = ParamId(2);
    /// Fourth added voice.
    pub const INTERVAL_4: ParamId = ParamId(3);

    /// A chord device adding the given intervals (semitones, clamped to
    /// -24..=24; only the first [`Chord::VOICES`] are used). The default is
    /// a major triad: `[4, 7]`.
    pub fn new(intervals: &[i8]) -> Self {
        let mut own = [0i8; Self::VOICES];
        for (slot, interval) in own.iter_mut().zip(intervals) {
            *slot = (*interval).clamp(-24, 24);
        }
        Self {
            intervals: own,
            sounding: Box::new([[0; Self::VOICES]; 16 * 128]),
        }
    }

    fn slot(channel: u8, pitch: u8) -> usize {
        usize::from(channel & 15) * 128 + usize::from(pitch & 127)
    }

    fn shifted(pitch: u8, interval: i8) -> Option<u8> {
        let moved = i16::from(pitch) + i16::from(interval);
        (interval != 0 && (0..=127).contains(&moved)).then_some(moved as u8)
    }
}

impl Device for Chord {
    fn prepare(&mut self, _config: &PrepareConfig) {
        self.reset();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        io.silence_outputs();
        for event in io.events {
            io.events_out.push(*event);
            match event.kind {
                EventKind::NoteOn {
                    pitch,
                    velocity,
                    channel,
                } => {
                    let intervals = self.intervals;
                    self.sounding[Self::slot(channel, pitch)] = intervals;
                    for interval in intervals {
                        if let Some(pitch) = Self::shifted(pitch, interval) {
                            io.events_out.push(Event::new(
                                event.frame,
                                EventKind::NoteOn {
                                    pitch,
                                    velocity,
                                    channel,
                                },
                            ));
                        }
                    }
                }
                EventKind::NoteOff {
                    pitch,
                    velocity,
                    channel,
                } => {
                    let slot = Self::slot(channel, pitch);
                    let intervals = self.sounding[slot];
                    self.sounding[slot] = [0; Self::VOICES];
                    for interval in intervals {
                        if let Some(pitch) = Self::shifted(pitch, interval) {
                            io.events_out.push(Event::new(
                                event.frame,
                                EventKind::NoteOff {
                                    pitch,
                                    velocity,
                                    channel,
                                },
                            ));
                        }
                    }
                }
                EventKind::AllNotesOff => self.reset(),
                _ => {}
            }
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if let Some(slot) = self.intervals.get_mut(param.0 as usize) {
            if value.is_finite() {
                *slot = value.round().clamp(-24.0, 24.0) as i8;
            }
        }
    }

    fn reset(&mut self) {
        for slot in self.sounding.iter_mut() {
            *slot = [0; Self::VOICES];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, stopped};

    fn play(chord: &mut Chord, events: &[Event]) -> Vec<Event> {
        let mut audio = [9.0f32; 64];
        let out = run(chord, &stopped(), &[], None, &mut [&mut audio], events);
        assert!(audio.iter().all(|s| *s == 0.0));
        out
    }

    #[test]
    fn a_note_becomes_a_triad_and_its_release_frees_the_same_pitches() {
        let mut chord = Chord::default();
        assert_eq!(chord.params().len(), Chord::VOICES);
        let out = play(&mut chord, &[Event::note_on(5, 60, 90)]);
        assert_eq!(
            out,
            [
                Event::note_on(5, 60, 90),
                Event::note_on(5, 64, 90),
                Event::note_on(5, 67, 90)
            ]
        );
        // Changing the chord while the note is held must not strand voices.
        chord.set_param(Chord::INTERVAL_1, 3.0);
        let out = play(&mut chord, &[Event::note_off(9, 60)]);
        assert_eq!(
            out,
            [
                Event::note_off(9, 60),
                Event::note_off(9, 64),
                Event::note_off(9, 67)
            ]
        );
        let out = play(&mut chord, &[Event::note_on(0, 60, 90)]);
        assert_eq!(out[1], Event::note_on(0, 63, 90), "now a minor third");
    }

    #[test]
    fn voices_outside_the_midi_range_are_dropped_and_other_events_pass() {
        let mut chord = Chord::new(&[12, -12, 0, 99]);
        let out = play(&mut chord, &[Event::note_on(0, 120, 100)]);
        assert_eq!(
            out,
            [Event::note_on(0, 120, 100), Event::note_on(0, 108, 100)],
            "+12 and +24 leave the range, 0 is off"
        );
        let bend = Event::new(
            3,
            EventKind::PitchBend {
                value: 100,
                channel: 0,
            },
        );
        let out = play(&mut chord, &[bend, Event::new(4, EventKind::AllNotesOff)]);
        assert_eq!(out, [bend, Event::new(4, EventKind::AllNotesOff)]);
        let out = play(&mut chord, &[Event::note_off(0, 120)]);
        assert_eq!(out, [Event::note_off(0, 120)], "all-notes-off forgot it");
    }
}
