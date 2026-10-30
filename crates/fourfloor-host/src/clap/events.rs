//! Translation between fourfloor events and CLAP events, and the transport.
//!
//! Everything here runs on the audio thread. The CLAP event buffer is
//! preallocated and every push is checked against its capacity first, so a
//! flood of events is dropped (and counted) instead of growing the buffer.

use clack_host::events::event_types::{
    MidiEvent, NoteOffEvent, NoteOnEvent, TransportEvent, TransportFlags,
};
use clack_host::events::io::{OutputEventBuffer, TryPushError};
use clack_host::events::spaces::CoreEventSpace;
use clack_host::events::{EventFlags, EventHeader, Match};
use clack_host::prelude::*;
use clack_host::utils::{BeatTime, SecondsTime};
use fourfloor_core::{
    Event as CoreEvent, EventBuffer as CoreEventBuffer, EventKind, TransportContext,
};

use super::instance::NoteInput;

/// Beats per bar. The edit model has no time signature yet, so plugins are
/// told 4/4.
const BEATS_PER_BAR: f64 = 4.0;

/// Notes currently held on the plugin, one bit per pitch and channel, so
/// "all notes off" can release exactly the notes that are sounding.
#[derive(Clone, Debug, Default)]
pub(crate) struct HeldNotes {
    channels: [u128; 16],
}

impl HeldNotes {
    pub fn clear(&mut self) {
        self.channels = [0; 16];
    }

    /// Marks one note as held or released.
    pub fn set(&mut self, channel: u8, pitch: u8, held: bool) {
        let bit = 1u128 << (pitch & 0x7F);
        let slot = &mut self.channels[usize::from(channel & 0x0F)];
        if held {
            *slot |= bit;
        } else {
            *slot &= !bit;
        }
    }

    /// Calls `release(channel, pitch)` for every held note and forgets
    /// them all.
    pub fn drain(&mut self, mut release: impl FnMut(u8, u8)) {
        for channel in 0..16u8 {
            let mut bits = self.channels[usize::from(channel)];
            while bits != 0 {
                let pitch = bits.trailing_zeros() as u8;
                bits &= bits - 1;
                release(channel, pitch);
            }
        }
        self.clear();
    }

    #[cfg(test)]
    fn count(&self) -> u32 {
        self.channels.iter().map(|bits| bits.count_ones()).sum()
    }
}

/// A bounded writer over a clack event buffer.
pub(crate) struct EventWriter<'a> {
    buffer: &'a mut EventBuffer,
    capacity: u32,
    /// Events that did not fit.
    pub dropped: u32,
}

impl<'a> EventWriter<'a> {
    /// `capacity` must not exceed what `buffer` was created with.
    pub fn new(buffer: &'a mut EventBuffer, capacity: u32) -> Self {
        Self {
            buffer,
            capacity,
            dropped: 0,
        }
    }

    pub fn push<E: AsRef<UnknownEvent> + ?Sized>(&mut self, event: &E) {
        if self.buffer.len() < self.capacity {
            self.buffer.push(event);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn note_on(&mut self, input: NoteInput, time: u32, channel: u8, pitch: u8, velocity: u8) {
        match input {
            NoteInput::None => {}
            NoteInput::Clap => self.push(&NoteOnEvent::new(
                time,
                Pckn::new(0u16, channel & 0x0F, pitch & 0x7F, Match::All),
                f64::from(velocity.min(127)) / 127.0,
            )),
            NoteInput::Midi => self.push(&MidiEvent::new(
                time,
                0,
                [
                    0x90 | (channel & 0x0F),
                    pitch & 0x7F,
                    velocity.clamp(1, 127),
                ],
            )),
        }
    }

    fn note_off(&mut self, input: NoteInput, time: u32, channel: u8, pitch: u8, velocity: u8) {
        match input {
            NoteInput::None => {}
            NoteInput::Clap => self.push(&NoteOffEvent::new(
                time,
                Pckn::new(0u16, channel & 0x0F, pitch & 0x7F, Match::All),
                f64::from(velocity.min(127)) / 127.0,
            )),
            NoteInput::Midi => self.push(&MidiEvent::new(
                time,
                0,
                [0x80 | (channel & 0x0F), pitch & 0x7F, velocity.min(127)],
            )),
        }
    }

    /// Appends one fourfloor event, translated for the plugin.
    ///
    /// `frames` is the block length; an event frame past the block is
    /// clamped to its last frame. `midi` says whether the plugin also takes
    /// raw MIDI, which is the only way to send controllers and pitch bend.
    pub fn push_core(
        &mut self,
        event: &CoreEvent,
        frames: usize,
        input: NoteInput,
        midi: bool,
        held: &mut HeldNotes,
    ) {
        let last = u32::try_from(frames.saturating_sub(1)).unwrap_or(u32::MAX);
        let time = event.frame.min(last);
        match event.kind {
            EventKind::NoteOn {
                pitch,
                velocity,
                channel,
            } => {
                if velocity == 0 {
                    // MIDI's "note on with velocity 0" is a note off.
                    self.note_off(input, time, channel, pitch, 0);
                    held.set(channel, pitch, false);
                } else {
                    self.note_on(input, time, channel, pitch, velocity);
                    held.set(channel, pitch, input != NoteInput::None);
                }
            }
            EventKind::NoteOff {
                pitch,
                velocity,
                channel,
            } => {
                self.note_off(input, time, channel, pitch, velocity);
                held.set(channel, pitch, false);
            }
            EventKind::ControlChange {
                controller,
                value,
                channel,
            } => {
                if midi {
                    self.push(&MidiEvent::new(
                        time,
                        0,
                        [0xB0 | (channel & 0x0F), controller & 0x7F, value & 0x7F],
                    ));
                }
            }
            EventKind::PitchBend { value, channel } => {
                if midi {
                    let bend = (i32::from(value) + 8192).clamp(0, 16383) as u16;
                    self.push(&MidiEvent::new(
                        time,
                        0,
                        [
                            0xE0 | (channel & 0x0F),
                            (bend & 0x7F) as u8,
                            (bend >> 7) as u8,
                        ],
                    ));
                }
            }
            EventKind::AllNotesOff => {
                // Not every plugin honours a wildcard note-off, so release
                // each held note by name.
                held.drain(|channel, pitch| self.note_off(input, time, channel, pitch, 0));
            }
        }
    }
}

/// Collects the notes a plugin emits into a fourfloor event buffer.
pub(crate) struct NoteSink<'a> {
    pub out: &'a mut CoreEventBuffer,
    /// Last valid frame of the block.
    pub last_frame: u32,
}

impl OutputEventBuffer for NoteSink<'_> {
    fn try_push(&mut self, event: &UnknownEvent) -> Result<(), TryPushError> {
        let time = event.header().time().min(self.last_frame);
        let key = |key: Match<u16>| key.into_specific().map(|key| key.min(127) as u8);
        let channel = |channel: Match<u16>| channel.into_specific().map_or(0, |c| (c & 0x0F) as u8);
        let unit = |velocity: f64| (velocity.clamp(0.0, 1.0) * 127.0).round() as u8;
        let kind = match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(note)) => key(note.key()).map(|pitch| EventKind::NoteOn {
                pitch,
                velocity: unit(note.velocity()).max(1),
                channel: channel(note.channel()),
            }),
            Some(CoreEventSpace::NoteOff(note)) => {
                key(note.key()).map(|pitch| EventKind::NoteOff {
                    pitch,
                    velocity: unit(note.velocity()),
                    channel: channel(note.channel()),
                })
            }
            Some(CoreEventSpace::Midi(midi)) => {
                let [status, data1, data2] = midi.data();
                let channel = status & 0x0F;
                match status & 0xF0 {
                    0x90 if data2 > 0 => Some(EventKind::NoteOn {
                        pitch: data1 & 0x7F,
                        velocity: data2 & 0x7F,
                        channel,
                    }),
                    0x80 | 0x90 => Some(EventKind::NoteOff {
                        pitch: data1 & 0x7F,
                        velocity: data2 & 0x7F,
                        channel,
                    }),
                    0xB0 => Some(EventKind::ControlChange {
                        controller: data1 & 0x7F,
                        value: data2 & 0x7F,
                        channel,
                    }),
                    0xE0 => Some(EventKind::PitchBend {
                        value: ((i32::from(data2 & 0x7F) << 7 | i32::from(data1 & 0x7F)) - 8192)
                            as i16,
                        channel,
                    }),
                    _ => None,
                }
            }
            // Parameter changes made in the plugin's own editor, note-end
            // markers and anything else are not routed anywhere yet.
            _ => None,
        };
        if let Some(kind) = kind {
            // A full buffer counts the drop itself.
            self.out.push(CoreEvent::new(time, kind));
        }
        Ok(())
    }
}

/// Builds the CLAP transport for one block.
pub(crate) fn transport(ctx: &TransportContext) -> TransportEvent {
    let beats = ctx.position_beats.get();
    let seconds = ctx.position_samples as f64 / ctx.sample_rate.as_f64();
    let bar = (beats / BEATS_PER_BAR).floor();
    let mut flags = TransportFlags::HAS_TEMPO
        | TransportFlags::HAS_BEATS_TIMELINE
        | TransportFlags::HAS_SECONDS_TIMELINE
        | TransportFlags::HAS_TIME_SIGNATURE;
    if ctx.playing {
        flags |= TransportFlags::IS_PLAYING;
    }
    TransportEvent {
        header: EventHeader::new_core(0, EventFlags::empty()),
        flags,
        song_pos_beats: BeatTime::from_float(beats),
        song_pos_seconds: SecondsTime::from_float(seconds),
        tempo: ctx.tempo.bpm(),
        tempo_inc: 0.0,
        loop_start_beats: BeatTime::from_int(0),
        loop_end_beats: BeatTime::from_int(0),
        loop_start_seconds: SecondsTime::from_int(0),
        loop_end_seconds: SecondsTime::from_int(0),
        bar_start: BeatTime::from_float(bar * BEATS_PER_BAR),
        bar_number: bar.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
        time_signature_numerator: 4,
        time_signature_denominator: 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_core::{SampleRate, Tempo};

    fn collect(
        events: &[CoreEvent],
        input: NoteInput,
        midi: bool,
        held: &mut HeldNotes,
        capacity: u32,
    ) -> (EventBuffer, u32) {
        let mut buffer = EventBuffer::with_capacity(capacity as usize);
        let mut writer = EventWriter::new(&mut buffer, capacity);
        for event in events {
            writer.push_core(event, 64, input, midi, held);
        }
        let dropped = writer.dropped;
        (buffer, dropped)
    }

    #[test]
    fn notes_become_clap_note_events_with_unit_velocity() {
        let mut held = HeldNotes::default();
        let events = [CoreEvent::note_on(5, 60, 127), CoreEvent::note_off(40, 60)];
        let (buffer, dropped) = collect(&events, NoteInput::Clap, false, &mut held, 16);
        assert_eq!((buffer.len(), dropped), (2, 0));
        let Some(CoreEventSpace::NoteOn(on)) = buffer.get(0).and_then(|e| e.as_core_event()) else {
            panic!("first event is not a note on");
        };
        assert_eq!(on.header().time(), 5);
        assert_eq!(on.key().into_specific(), Some(60));
        assert!((on.velocity() - 1.0).abs() < 1e-9);
        assert!(matches!(
            buffer.get(1).and_then(|e| e.as_core_event()),
            Some(CoreEventSpace::NoteOff(_))
        ));
        assert_eq!(held.count(), 0);
    }

    #[test]
    fn midi_dialect_sends_raw_bytes_and_controllers() {
        let mut held = HeldNotes::default();
        let events = [
            CoreEvent::new(
                0,
                EventKind::NoteOn {
                    pitch: 64,
                    velocity: 100,
                    channel: 3,
                },
            ),
            CoreEvent::new(
                1,
                EventKind::ControlChange {
                    controller: 74,
                    value: 20,
                    channel: 3,
                },
            ),
            CoreEvent::new(
                2,
                EventKind::PitchBend {
                    value: 0,
                    channel: 3,
                },
            ),
        ];
        let (buffer, _) = collect(&events, NoteInput::Midi, true, &mut held, 16);
        let bytes: Vec<[u8; 3]> = buffer
            .iter()
            .filter_map(|e| match e.as_core_event() {
                Some(CoreEventSpace::Midi(midi)) => Some(midi.data()),
                _ => None,
            })
            .collect();
        assert_eq!(
            bytes,
            vec![[0x93, 64, 100], [0xB3, 74, 20], [0xE3, 0x00, 0x40]]
        );
    }

    #[test]
    fn controllers_are_dropped_without_a_midi_port() {
        let mut held = HeldNotes::default();
        let events = [CoreEvent::new(
            0,
            EventKind::ControlChange {
                controller: 1,
                value: 1,
                channel: 0,
            },
        )];
        let (buffer, dropped) = collect(&events, NoteInput::Clap, false, &mut held, 16);
        assert_eq!((buffer.len(), dropped), (0, 0));
    }

    #[test]
    fn all_notes_off_releases_exactly_the_held_notes() {
        let mut held = HeldNotes::default();
        let events = [
            CoreEvent::note_on(0, 60, 100),
            CoreEvent::note_on(0, 64, 100),
            CoreEvent::note_on(0, 67, 100),
            CoreEvent::note_off(10, 64),
            CoreEvent::new(20, EventKind::AllNotesOff),
        ];
        let (buffer, _) = collect(&events, NoteInput::Clap, false, &mut held, 16);
        let offs: Vec<(u32, u16)> = buffer
            .iter()
            .filter_map(|e| match e.as_core_event() {
                Some(CoreEventSpace::NoteOff(off)) => {
                    Some((off.header().time(), off.key().into_specific()?))
                }
                _ => None,
            })
            .collect();
        assert_eq!(offs, vec![(10, 64), (20, 60), (20, 67)]);
        assert_eq!(held.count(), 0);
    }

    #[test]
    fn a_full_buffer_drops_and_counts_instead_of_growing() {
        let mut held = HeldNotes::default();
        let events: Vec<CoreEvent> = (0..10).map(|i| CoreEvent::note_on(i, 60, 100)).collect();
        let (buffer, dropped) = collect(&events, NoteInput::Clap, false, &mut held, 4);
        assert_eq!((buffer.len(), dropped), (4, 6));
    }

    #[test]
    fn event_frames_are_clamped_into_the_block() {
        let mut held = HeldNotes::default();
        let (buffer, _) = collect(
            &[CoreEvent::note_on(9999, 60, 100)],
            NoteInput::Clap,
            false,
            &mut held,
            4,
        );
        assert_eq!(buffer.get(0).map(|e| e.header().time()), Some(63));
    }

    #[test]
    fn emitted_notes_are_translated_back() {
        let mut out = CoreEventBuffer::with_capacity(8);
        let mut sink = NoteSink {
            out: &mut out,
            last_frame: 63,
        };
        let on = NoteOnEvent::new(7, Pckn::new(0u16, 2u16, 72u16, Match::All), 0.5);
        let midi_off = MidiEvent::new(500, 0, [0x82, 72, 0]);
        sink.try_push(on.as_ref()).unwrap();
        sink.try_push(midi_off.as_ref()).unwrap();
        assert_eq!(
            out.as_slice(),
            &[
                CoreEvent::new(
                    7,
                    EventKind::NoteOn {
                        pitch: 72,
                        velocity: 64,
                        channel: 2
                    }
                ),
                CoreEvent::new(
                    63,
                    EventKind::NoteOff {
                        pitch: 72,
                        velocity: 0,
                        channel: 2
                    }
                ),
            ]
        );
    }

    #[test]
    fn transport_reports_position_tempo_and_bar() {
        let rate = SampleRate::HZ_48000;
        let tempo = Tempo::new(120.0).unwrap();
        // 5 beats in: 2.5 s at 120 BPM.
        let ctx = TransportContext::playing_at(rate, tempo, 120_000);
        let event = transport(&ctx);
        assert!(event.flags.contains(TransportFlags::IS_PLAYING));
        assert!((event.song_pos_beats.to_float() - 5.0).abs() < 1e-6);
        assert!((event.song_pos_seconds.to_float() - 2.5).abs() < 1e-6);
        assert_eq!(event.tempo, 120.0);
        assert_eq!(event.bar_number, 1);
        assert!((event.bar_start.to_float() - 4.0).abs() < 1e-6);

        let stopped = transport(&TransportContext::stopped(rate, tempo));
        assert!(!stopped.flags.contains(TransportFlags::IS_PLAYING));
    }
}
