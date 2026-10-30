use fourfloor_core::{
    Device, Event, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, SamplePos,
    Tempo, Ticks, TransportContext, TICKS_PER_BEAT,
};

use super::clip_launcher::tick_at_or_after;

/// The order an [`Arpeggiator`] plays its held notes in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ArpMode {
    /// Lowest to highest, then again from the lowest.
    #[default]
    Up,
    /// Highest to lowest.
    Down,
    /// Up then down, without repeating the top and bottom notes.
    UpDown,
    /// The order the notes were played in.
    AsPlayed,
}

impl ArpMode {
    fn from_index(index: f32) -> Self {
        match index.round() as i32 {
            i32::MIN..=0 => Self::Up,
            1 => Self::Down,
            2 => Self::UpDown,
            _ => Self::AsPlayed,
        }
    }

    /// The value of [`Arpeggiator::MODE`] that selects this order.
    pub fn param_value(self) -> f32 {
        match self {
            Self::Up => 0.0,
            Self::Down => 1.0,
            Self::UpDown => 2.0,
            Self::AsPlayed => 3.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Held {
    pitch: u8,
    velocity: u8,
    channel: u8,
}

#[derive(Clone, Copy, Debug)]
struct Sounding {
    pitch: u8,
    channel: u8,
    /// Absolute sample position of the note-off.
    off_at: SamplePos,
}

/// A note effect that plays held notes one at a time, on a tempo grid.
///
/// It sits on event edges before a track's instrument. Notes that arrive
/// are held, not passed on; while any are held and the transport rolls,
/// one note sounds on every multiple of [`Arpeggiator::RATE`] beats on the
/// timeline, for [`Arpeggiator::GATE`] of the step. A note that starts on
/// a step sample is part of that step. The pattern restarts whenever the
/// first note of a new chord arrives. Events that are not notes pass
/// through. It is silent while the transport is stopped, and its audio
/// output is silence.
#[derive(Clone, Debug)]
pub struct Arpeggiator {
    rate_beats: f32,
    gate: f32,
    mode: ArpMode,
    octaves: u8,
    held: [Held; Arpeggiator::MAX_HELD],
    held_count: usize,
    step: u32,
    sounding: Option<Sounding>,
    expected: Option<(SamplePos, Tempo)>,
}

static PARAMS: [ParamInfo; 4] = [
    ParamInfo::new(
        Arpeggiator::RATE,
        "Rate",
        0.0625,
        4.0,
        0.25,
        ParamUnit::Beats,
    ),
    ParamInfo::new(Arpeggiator::GATE, "Gate", 0.05, 1.0, 0.5, ParamUnit::Ratio),
    ParamInfo::new(Arpeggiator::MODE, "Mode", 0.0, 3.0, 0.0, ParamUnit::Integer),
    ParamInfo::new(
        Arpeggiator::OCTAVES,
        "Octaves",
        1.0,
        4.0,
        1.0,
        ParamUnit::Integer,
    ),
];

impl Default for Arpeggiator {
    fn default() -> Self {
        Self::new(0.25, ArpMode::Up)
    }
}

impl Arpeggiator {
    /// Notes that can be held at once; further notes are ignored.
    pub const MAX_HELD: usize = 32;
    /// Step length in beats (0.25 is a sixteenth note).
    pub const RATE: ParamId = ParamId(0);
    /// Note length as a fraction of the step, 0.05 to 1.
    pub const GATE: ParamId = ParamId(1);
    /// Order: 0 up, 1 down, 2 up-down, 3 as played.
    pub const MODE: ParamId = ParamId(2);
    /// Octaves the pattern spans, 1 to 4.
    pub const OCTAVES: ParamId = ParamId(3);

    /// An arpeggiator stepping every `rate_beats` beats, half-step gate,
    /// one octave.
    pub fn new(rate_beats: f32, mode: ArpMode) -> Self {
        let mut arp = Self {
            rate_beats: 0.25,
            gate: 0.5,
            mode,
            octaves: 1,
            held: [Held::default(); Self::MAX_HELD],
            held_count: 0,
            step: 0,
            sounding: None,
            expected: None,
        };
        arp.set_param(Self::RATE, rate_beats);
        arp
    }

    /// The current order.
    pub fn mode(&self) -> ArpMode {
        self.mode
    }

    fn step_ticks(&self) -> i64 {
        ((f64::from(self.rate_beats) * TICKS_PER_BEAT as f64).round() as i64).max(1)
    }

    fn hold(&mut self, note: Held) {
        let held = &mut self.held[..self.held_count];
        if let Some(existing) = held
            .iter_mut()
            .find(|h| h.pitch == note.pitch && h.channel == note.channel)
        {
            existing.velocity = note.velocity;
            return;
        }
        if self.held_count == 0 {
            self.step = 0;
        }
        if self.held_count < Self::MAX_HELD {
            self.held[self.held_count] = note;
            self.held_count += 1;
        }
    }

    fn unhold(&mut self, pitch: u8, channel: u8) {
        let count = self.held_count;
        if let Some(at) = self.held[..count]
            .iter()
            .position(|h| h.pitch == pitch && h.channel == channel)
        {
            self.held.copy_within(at + 1..count, at);
            self.held_count -= 1;
        }
    }

    /// The note the current step plays; `None` when nothing is held or the
    /// octave shift leaves the MIDI range.
    fn pick(&self) -> Option<Held> {
        let n = self.held_count;
        if n == 0 {
            return None;
        }
        let mut notes = self.held;
        let notes = &mut notes[..n];
        if self.mode != ArpMode::AsPlayed {
            notes.sort_unstable_by_key(|h| (h.pitch, h.channel));
        }
        let total = n * usize::from(self.octaves.max(1));
        let step = self.step as usize;
        let index = match self.mode {
            ArpMode::Up | ArpMode::AsPlayed => step % total,
            ArpMode::Down => total - 1 - step % total,
            ArpMode::UpDown => {
                let period = (2 * total).saturating_sub(2).max(1);
                let j = step % period;
                if j < total {
                    j
                } else {
                    period - j
                }
            }
        };
        let note = notes[index % n];
        let pitch = usize::from(note.pitch) + 12 * (index / n);
        (pitch <= 127).then_some(Held {
            pitch: pitch as u8,
            ..note
        })
    }

    fn take_input(&mut self, event: &Event, io: &mut ProcessIo<'_, '_>) {
        match event.kind {
            EventKind::NoteOn {
                pitch,
                velocity,
                channel,
            } => self.hold(Held {
                pitch,
                velocity,
                channel,
            }),
            EventKind::NoteOff { pitch, channel, .. } => self.unhold(pitch, channel),
            EventKind::AllNotesOff => {
                self.held_count = 0;
                self.sounding = None;
                io.events_out.push(*event);
            }
            _ => {
                io.events_out.push(*event);
            }
        }
    }

    fn note_off(note: Sounding, frame: u32) -> Event {
        Event::new(
            frame,
            EventKind::NoteOff {
                pitch: note.pitch,
                velocity: 0,
                channel: note.channel,
            },
        )
    }

    /// Ends the sounding note if its note-off falls before `before` (or on
    /// it, when `inclusive`).
    fn finish(
        &mut self,
        before: SamplePos,
        inclusive: bool,
        start: SamplePos,
        io: &mut ProcessIo<'_, '_>,
    ) {
        if let Some(note) = self.sounding {
            if note.off_at < before || (inclusive && note.off_at == before) {
                let frame = note.off_at.saturating_sub(start) as u32;
                io.events_out.push(Self::note_off(note, frame));
                self.sounding = None;
            }
        }
    }

    fn release_now(&mut self, io: &mut ProcessIo<'_, '_>) {
        if let Some(note) = self.sounding.take() {
            io.events_out.push(Self::note_off(note, 0));
        }
    }
}

impl Device for Arpeggiator {
    fn prepare(&mut self, _config: &PrepareConfig) {
        self.reset();
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        io.silence_outputs();
        let events = io.events;
        if !ctx.playing {
            self.release_now(io);
            self.expected = None;
            for event in events {
                self.take_input(event, io);
            }
            return;
        }
        let (tempo, rate) = (ctx.tempo, ctx.sample_rate);
        let start = ctx.position_samples;
        let end = start.saturating_add(io.frames() as u64);
        if self
            .expected
            .is_some_and(|(at, t)| t == tempo && at != start)
        {
            self.release_now(io);
        }
        self.expected = Some((end, tempo));

        let step = self.step_ticks();
        let first = tick_at_or_after(start, tempo, rate);
        let on_grid = first.rem_euclid(step) == 0;
        let mut tick = first.div_euclid(step) * step + if on_grid { 0 } else { step };
        let mut next_event = 0usize;
        loop {
            let at = Ticks::new(tick).to_sample_pos(tempo, rate);
            if at >= end {
                break;
            }
            // Notes that change on or before this step count for it.
            while let Some(event) = events.get(next_event) {
                if start + u64::from(event.frame) > at {
                    break;
                }
                self.take_input(event, io);
                next_event += 1;
            }
            self.finish(at, true, start, io);
            if let Some(note) = self.pick() {
                let next_at = Ticks::new(tick + step).to_sample_pos(tempo, rate);
                let length = ((next_at - at) as f64 * f64::from(self.gate)).round() as u64;
                io.events_out.push(Event::new(
                    (at - start) as u32,
                    EventKind::NoteOn {
                        pitch: note.pitch,
                        velocity: note.velocity,
                        channel: note.channel,
                    },
                ));
                self.sounding = Some(Sounding {
                    pitch: note.pitch,
                    channel: note.channel,
                    off_at: at + length.max(1),
                });
            }
            if self.held_count > 0 {
                self.step = self.step.wrapping_add(1);
            }
            tick += step;
        }
        for event in &events[next_event..] {
            self.take_input(event, io);
        }
        self.finish(end, false, start, io);
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if !value.is_finite() {
            return;
        }
        match param {
            Self::RATE => self.rate_beats = value.clamp(0.0625, 4.0),
            Self::GATE => self.gate = value.clamp(0.05, 1.0),
            Self::MODE => self.mode = ArpMode::from_index(value),
            Self::OCTAVES => self.octaves = value.round().clamp(1.0, 4.0) as u8,
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.held_count = 0;
        self.sounding = None;
        self.step = 0;
        self.expected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, RATE};

    type Timed = (u64, EventKind);

    /// Plays from sample 0 at 120 BPM (a sixteenth is 6 000 samples),
    /// feeding `input` (absolute sample, kind). Returns every event out.
    fn play(arp: &mut Arpeggiator, frames: u64, block: u64, input: &[Timed]) -> Vec<Timed> {
        let mut out = Vec::new();
        let mut position = 0;
        while position < frames {
            let n = block.min(frames - position);
            let events: Vec<Event> = input
                .iter()
                .filter(|(at, _)| (position..position + n).contains(at))
                .map(|(at, kind)| Event::new((at - position) as u32, *kind))
                .collect();
            let mut audio = vec![9.0f32; n as usize];
            let ctx = TransportContext::playing_at(RATE, Tempo::DEFAULT, position);
            let emitted = run(arp, &ctx, &[], None, &mut [&mut audio], &events);
            assert!(audio.iter().all(|s| *s == 0.0));
            out.extend(
                emitted
                    .iter()
                    .map(|e| (position + u64::from(e.frame), e.kind)),
            );
            position += n;
        }
        out
    }

    fn ons(events: &[Timed]) -> Vec<(u64, u8)> {
        events
            .iter()
            .filter_map(|(at, kind)| match kind {
                EventKind::NoteOn { pitch, .. } => Some((*at, *pitch)),
                _ => None,
            })
            .collect()
    }

    const CHORD: [Timed; 3] = [
        (0, EventKind::note_on(64, 100)),
        (0, EventKind::note_on(60, 100)),
        (0, EventKind::note_on(67, 100)),
    ];

    #[test]
    fn a_held_chord_steps_upwards_on_the_sixteenth_grid_with_half_gate() {
        let mut arp = Arpeggiator::default();
        let out = play(&mut arp, 30_000, 512, &CHORD);
        assert_eq!(
            ons(&out),
            [
                (0, 60),
                (6_000, 64),
                (12_000, 67),
                (18_000, 60),
                (24_000, 64)
            ]
        );
        assert_eq!(
            out[1],
            (3_000, EventKind::note_off(60)),
            "half of a 6 000 sample step"
        );
        assert_eq!(out.len(), 10, "every note-on has its note-off");
    }

    #[test]
    fn modes_and_octaves_change_the_order() {
        let order = |mode: ArpMode, octaves: f32| {
            let mut arp = Arpeggiator::new(0.25, mode);
            arp.set_param(Arpeggiator::OCTAVES, octaves);
            assert_eq!(arp.mode(), mode);
            let out = play(&mut arp, 48_000, 700, &CHORD);
            ons(&out)
                .iter()
                .map(|(_, pitch)| *pitch)
                .collect::<Vec<_>>()
        };
        assert_eq!(order(ArpMode::Down, 1.0), [67, 64, 60, 67, 64, 60, 67, 64]);
        assert_eq!(
            order(ArpMode::UpDown, 1.0),
            [60, 64, 67, 64, 60, 64, 67, 64]
        );
        assert_eq!(
            order(ArpMode::AsPlayed, 1.0),
            [64, 60, 67, 64, 60, 67, 64, 60]
        );
        assert_eq!(order(ArpMode::Up, 2.0), [60, 64, 67, 72, 76, 79, 60, 64]);
        let mut arp = Arpeggiator::default();
        arp.set_param(Arpeggiator::MODE, ArpMode::Down.param_value());
        assert_eq!(arp.mode(), ArpMode::Down);
    }

    #[test]
    fn output_does_not_depend_on_block_size_and_release_stops_the_pattern() {
        let input = [
            (0, EventKind::note_on(60, 90)),
            (0, EventKind::note_on(63, 90)),
            (13_000, EventKind::note_off(60)),
            (13_000, EventKind::note_off(63)),
            (24_000, EventKind::note_on(72, 80)),
        ];
        let reference = play(&mut Arpeggiator::default(), 40_000, 40_000, &input);
        assert_eq!(
            ons(&reference),
            [
                (0, 60),
                (6_000, 63),
                (12_000, 60),
                (24_000, 72),
                (30_000, 72),
                (36_000, 72)
            ],
            "nothing sounds between the release and the next note"
        );
        for block in [1, 17, 64, 513, 6_000] {
            assert_eq!(
                play(&mut Arpeggiator::default(), 40_000, block, &input),
                reference,
                "block size {block}"
            );
        }
    }

    #[test]
    fn full_gate_ends_each_note_as_the_next_starts_and_other_events_pass() {
        let mut arp = Arpeggiator::new(0.5, ArpMode::Up);
        arp.set_param(Arpeggiator::GATE, 1.0);
        let bend = EventKind::PitchBend {
            value: 5,
            channel: 0,
        };
        let input = [
            (0, EventKind::note_on(60, 100)),
            (100, bend),
            (20_000, EventKind::AllNotesOff),
        ];
        let out = play(&mut arp, 30_000, 256, &input);
        assert_eq!(
            out,
            [
                (0, EventKind::note_on(60, 100)),
                (100, bend),
                (12_000, EventKind::note_off(60)),
                (12_000, EventKind::note_on(60, 100)),
                (20_000, EventKind::AllNotesOff),
            ]
        );
    }

    #[test]
    fn stopping_releases_the_sounding_note() {
        let mut arp = Arpeggiator::default();
        play(&mut arp, 1_000, 500, &[(0, EventKind::note_on(60, 100))]);
        let stopped = TransportContext::stopped(RATE, Tempo::DEFAULT);
        let mut audio = [0.0f32; 8];
        let out = run(&mut arp, &stopped, &[], None, &mut [&mut audio], &[]);
        assert_eq!(out, [Event::new(0, EventKind::note_off(60))]);
    }
}
