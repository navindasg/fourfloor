use fourfloor_core::{
    Device, Event, EventKind, PrepareConfig, ProcessIo, SamplePos, SampleRate, Tempo, Ticks,
    TransportContext,
};

/// One note for a [`NotePlayer`], positioned on the project timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerNote {
    /// Note start in ticks from the start of the timeline.
    pub start: Ticks,
    /// Note length in ticks. Lengths below one tick are played as one tick.
    pub length: Ticks,
    /// MIDI pitch, 0 to 127.
    pub pitch: u8,
    /// MIDI velocity, 1 to 127.
    pub velocity: u8,
    /// MIDI channel, 0 to 15.
    pub channel: u8,
}

impl PlayerNote {
    /// A note on channel 0.
    pub const fn new(start: Ticks, length: Ticks, pitch: u8, velocity: u8) -> Self {
        Self {
            start,
            length,
            pitch,
            velocity,
            channel: 0,
        }
    }
}

/// A note-on or note-off at a tick on the timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Scheduled {
    tick: Ticks,
    kind: EventKind,
}

/// A track's clip player: turns the notes of its MIDI clips into events,
/// sample-accurately, as the transport rolls.
///
/// It is an event source. Connect it to the track's instrument with
/// [`GraphBuilder::connect_events`](crate::GraphBuilder::connect_events);
/// its audio output is silence.
///
/// Each note-on and note-off is emitted on the exact sample its tick maps to
/// at the block's tempo: `round(beats * samples_per_beat)`, the same
/// rounding the transport uses. Nothing is emitted while the transport is
/// stopped.
///
/// When the playhead jumps (a loop wrap or a seek) or the transport stops
/// while notes are held, the player emits one
/// [`EventKind::AllNotesOff`] at frame 0 of the next block, so no note hangs.
/// Notes that started before the new position are not re-triggered.
///
/// The note list is fixed; changing it means compiling a new graph.
#[derive(Clone, Debug)]
pub struct NotePlayer {
    /// Sorted by tick; at one tick, note-offs come before note-ons.
    schedule: Vec<Scheduled>,
    /// Notes currently sounding: one bit per pitch, per channel.
    held: [u128; 16],
    /// Where the next block starts if the playhead does not jump, and the
    /// tempo that position was computed at.
    expected: Option<(SamplePos, Tempo)>,
}

impl NotePlayer {
    /// A player for `notes`, which may be in any order.
    pub fn new(notes: &[PlayerNote]) -> Self {
        let mut schedule = Vec::with_capacity(notes.len() * 2);
        for note in notes {
            let (pitch, channel) = (note.pitch.min(127), note.channel.min(15));
            let length = Ticks::new(note.length.get().max(1));
            schedule.push(Scheduled {
                tick: note.start,
                kind: EventKind::NoteOn {
                    pitch,
                    velocity: note.velocity.clamp(1, 127),
                    channel,
                },
            });
            schedule.push(Scheduled {
                tick: note.start + length,
                kind: EventKind::NoteOff {
                    pitch,
                    velocity: 0,
                    channel,
                },
            });
        }
        // Stable, so notes at one tick keep the order they were given in.
        schedule.sort_by_key(|event| (event.tick, event.kind.is_note_on()));
        Self {
            schedule,
            held: [0; 16],
            expected: None,
        }
    }

    /// Number of notes the player holds.
    pub fn note_count(&self) -> usize {
        self.schedule.len() / 2
    }

    fn any_held(&self) -> bool {
        self.held.iter().any(|bits| *bits != 0)
    }

    fn track(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn { pitch, channel, .. } => {
                self.held[usize::from(channel & 15)] |= 1u128 << (pitch & 127);
            }
            EventKind::NoteOff { pitch, channel, .. } => {
                self.held[usize::from(channel & 15)] &= !(1u128 << (pitch & 127));
            }
            _ => {}
        }
    }

    /// Emits an all-notes-off at the start of the block if anything is held.
    fn release_all(&mut self, io: &mut ProcessIo<'_, '_>) {
        if self.any_held() {
            io.events_out.push(Event::new(0, EventKind::AllNotesOff));
            self.held = [0; 16];
        }
    }

    fn position(event: &Scheduled, tempo: Tempo, rate: SampleRate) -> SamplePos {
        event.tick.to_sample_pos(tempo, rate)
    }
}

impl Device for NotePlayer {
    fn prepare(&mut self, _config: &PrepareConfig) {
        self.reset();
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        io.silence_outputs();
        if !ctx.playing {
            self.release_all(io);
            self.expected = None;
            return;
        }
        let (tempo, rate) = (ctx.tempo, ctx.sample_rate);
        let start = ctx.position_samples;
        let end = start.saturating_add(io.frames() as u64);

        // A tempo change moves the sample position without moving the
        // music, so only a jump at an unchanged tempo counts as one.
        if let Some((expected, at_tempo)) = self.expected {
            if at_tempo == tempo && expected != start {
                self.release_all(io);
            }
        }

        // Ticks are sorted, so their sample positions are too.
        let first = self
            .schedule
            .partition_point(|event| Self::position(event, tempo, rate) < start);
        for index in first..self.schedule.len() {
            let event = self.schedule[index];
            let at = Self::position(&event, tempo, rate);
            if at >= end {
                break;
            }
            if io
                .events_out
                .push(Event::new((at - start) as u32, event.kind))
            {
                self.track(event.kind);
            }
        }
        self.expected = Some((end, tempo));
    }

    fn reset(&mut self) {
        self.held = [0; 16];
        self.expected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, RATE};

    fn note(start: i64, length: i64, pitch: u8) -> PlayerNote {
        PlayerNote::new(Ticks::new(start), Ticks::new(length), pitch, 100)
    }

    fn rolling(position: SamplePos, bpm: f64) -> TransportContext {
        TransportContext::playing_at(RATE, Tempo::new(bpm).unwrap(), position)
    }

    /// Plays `frames` from `from` in blocks of `block`; returns
    /// `(absolute sample, kind)` for every event.
    fn play(
        player: &mut NotePlayer,
        from: SamplePos,
        frames: usize,
        block: usize,
    ) -> Vec<(u64, EventKind)> {
        let mut out = Vec::new();
        let mut position = from;
        let mut left = frames;
        while left > 0 {
            let n = left.min(block);
            let mut audio = vec![9.0f32; n];
            let events = run(
                player,
                &rolling(position, 120.0),
                &[],
                None,
                &mut [&mut audio],
                &[],
            );
            assert!(audio.iter().all(|s| *s == 0.0), "audio output is silence");
            for event in events {
                assert!((event.frame as usize) < n, "frame inside the block");
                out.push((position + u64::from(event.frame), event.kind));
            }
            position += n as u64;
            left -= n;
        }
        out
    }

    #[test]
    fn notes_become_on_and_off_events_on_the_exact_sample() {
        // 120 BPM at 48 kHz: 25 samples per tick.
        let mut player = NotePlayer::new(&[note(1440, 240, 60), note(0, 480, 36)]);
        assert_eq!(player.note_count(), 2);
        let events = play(&mut player, 0, 48_000, 512);
        assert_eq!(
            events,
            [
                (0, EventKind::note_on(36, 100)),
                (12_000, EventKind::note_off(36)),
                (36_000, EventKind::note_on(60, 100)),
                (42_000, EventKind::note_off(60)),
            ]
        );
    }

    #[test]
    fn events_do_not_depend_on_block_size() {
        let notes: Vec<PlayerNote> = (0..40)
            .map(|i| note(i * 97, 150, 30 + (i % 50) as u8))
            .collect();
        let reference = play(&mut NotePlayer::new(&notes), 0, 110_000, 110_000);
        assert_eq!(reference.len(), 80);
        for block in [1, 7, 64, 511, 4096] {
            let events = play(&mut NotePlayer::new(&notes), 0, 110_000, block);
            assert_eq!(events, reference, "block size {block}");
        }
    }

    #[test]
    fn a_retriggered_pitch_gets_its_note_off_before_the_next_note_on() {
        let mut player = NotePlayer::new(&[note(0, 480, 60), note(480, 480, 60)]);
        let events = play(&mut player, 0, 30_000, 256);
        let kinds: Vec<EventKind> = events.iter().map(|(_, kind)| *kind).collect();
        assert_eq!(
            kinds,
            [
                EventKind::note_on(60, 100),
                EventKind::note_off(60),
                EventKind::note_on(60, 100),
                EventKind::note_off(60),
            ]
        );
        assert_eq!(events[1].0, events[2].0, "both on sample 12 000");
    }

    #[test]
    fn nothing_plays_while_stopped_and_stopping_releases_held_notes() {
        let mut player = NotePlayer::new(&[note(0, 9_600, 60)]);
        let stopped = TransportContext::stopped(RATE, Tempo::DEFAULT);
        let mut audio = [0.0f32; 64];
        let idle = run(&mut player, &stopped, &[], None, &mut [&mut audio], &[]);
        assert!(idle.is_empty());

        let on = play(&mut player, 0, 64, 64);
        assert_eq!(on, [(0, EventKind::note_on(60, 100))]);
        let off = run(&mut player, &stopped, &[], None, &mut [&mut audio], &[]);
        assert_eq!(off, [Event::new(0, EventKind::AllNotesOff)]);
        let again = run(&mut player, &stopped, &[], None, &mut [&mut audio], &[]);
        assert!(again.is_empty(), "released once");
    }

    #[test]
    fn a_loop_wrap_releases_held_notes_and_replays_the_loop() {
        // A note that runs past the loop end at beat 1 (sample 24 000).
        let mut player = NotePlayer::new(&[note(480, 1_920, 60), note(0, 120, 36)]);
        let mut events = play(&mut player, 0, 24_000, 500);
        assert_eq!(events.len(), 3, "kick on, kick off, long note on");
        // The engine never lets a block cross the loop end; the next block
        // starts back at sample 0.
        events = play(&mut player, 0, 24_000, 500);
        assert_eq!(
            events,
            [
                (0, EventKind::AllNotesOff),
                (0, EventKind::note_on(36, 100)),
                (3_000, EventKind::note_off(36)),
                (12_000, EventKind::note_on(60, 100)),
            ]
        );
    }

    #[test]
    fn a_seek_into_the_middle_of_a_note_does_not_retrigger_it() {
        let mut player = NotePlayer::new(&[note(0, 960, 60), note(960, 240, 62)]);
        // Start at sample 12 000, half way through the first note.
        let events = play(&mut player, 12_000, 30_000, 512);
        assert_eq!(
            events,
            [
                (24_000, EventKind::note_off(60)),
                (24_000, EventKind::note_on(62, 100)),
                (30_000, EventKind::note_off(62)),
            ]
        );
    }

    #[test]
    fn a_tempo_change_is_not_treated_as_a_jump() {
        let mut player = NotePlayer::new(&[note(0, 9_600, 60)]);
        let mut audio = [0.0f32; 100];
        let first = run(
            &mut player,
            &rolling(0, 120.0),
            &[],
            None,
            &mut [&mut audio],
            &[],
        );
        assert_eq!(first.len(), 1);
        // The transport keeps the beat position, so the sample position moves.
        let second = run(
            &mut player,
            &rolling(200, 60.0),
            &[],
            None,
            &mut [&mut audio],
            &[],
        );
        assert!(second.is_empty(), "{second:?}");
    }

    #[test]
    fn reset_forgets_held_notes_and_odd_input_is_made_safe() {
        let mut player = NotePlayer::new(&[PlayerNote {
            start: Ticks::new(0),
            length: Ticks::new(-5),
            pitch: 200,
            velocity: 0,
            channel: 99,
        }]);
        let events = play(&mut player, 0, 100, 100);
        assert_eq!(
            events,
            [
                (
                    0,
                    EventKind::NoteOn {
                        pitch: 127,
                        velocity: 1,
                        channel: 15
                    }
                ),
                (
                    25,
                    EventKind::NoteOff {
                        pitch: 127,
                        velocity: 0,
                        channel: 15
                    }
                ),
            ]
        );
        let mut held = NotePlayer::new(&[note(0, 9_600, 60)]);
        play(&mut held, 0, 64, 64);
        assert!(held.any_held());
        held.reset();
        assert!(!held.any_held());
        held.prepare(&PrepareConfig::stereo(RATE, 64));
    }
}
