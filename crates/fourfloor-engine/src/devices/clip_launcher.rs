use fourfloor_core::{
    Device, Event, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, SamplePos,
    SampleRate, Tempo, Ticks, TransportContext,
};

use super::PlayerNote;

/// One launchable clip for a [`ClipLauncher`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LauncherClip {
    /// Loop length in ticks. The clip repeats for as long as it plays.
    pub length: Ticks,
    /// The notes, with starts relative to the start of the clip. Notes are
    /// cut at the clip end; notes that start outside the clip are dropped.
    pub notes: Vec<PlayerNote>,
}

#[derive(Clone, Copy, Debug)]
struct Scheduled {
    tick: i64,
    kind: EventKind,
}

#[derive(Clone, Debug)]
struct Slot {
    length: i64,
    /// Sorted by tick; at one tick, note-offs come before note-ons.
    schedule: Vec<Scheduled>,
}

/// A track's clip launcher: the session-grid half of a MIDI track.
///
/// It sits between the track's [`NotePlayer`](super::NotePlayer) and its
/// instrument, on event edges. While no slot is playing it passes the
/// arrangement's events through untouched. Setting [`ClipLauncher::LAUNCH`]
/// to `n` asks for slot `n - 1`; the launcher waits for the next multiple
/// of its launch quantization on the timeline, then silences the
/// arrangement on that exact sample and loops the slot's clip from there.
/// Setting it to `0` hands the track back to the arrangement, again on the
/// next boundary.
///
/// A launch only happens while the transport rolls. When the playhead
/// jumps or the transport stops, sounding notes are released; a playing
/// slot keeps its place in its loop relative to the timeline.
#[derive(Clone, Debug)]
pub struct ClipLauncher {
    slots: Vec<Slot>,
    quantize: i64,
    params: [ParamInfo; 1],
    /// The slot asked for and not yet started (`None` inside is "stop").
    pending: Option<Option<usize>>,
    /// The playing slot and the tick its loop is anchored to.
    active: Option<(usize, i64)>,
    /// Whether a note-on went out since the last all-notes-off.
    sounding: bool,
    expected: Option<(SamplePos, Tempo)>,
}

/// The first tick whose sample position is at or after `sample`.
pub(super) fn tick_at_or_after(sample: SamplePos, tempo: Tempo, rate: SampleRate) -> i64 {
    let mut tick = Ticks::from_sample_pos(sample, tempo, rate).get();
    while Ticks::new(tick).to_sample_pos(tempo, rate) < sample {
        tick += 1;
    }
    while tick > 0 && Ticks::new(tick - 1).to_sample_pos(tempo, rate) >= sample {
        tick -= 1;
    }
    tick
}

impl ClipLauncher {
    /// Which slot plays: `0` for the arrangement, `n` for slot `n - 1`.
    /// Every write is a new launch request, even with an unchanged value.
    pub const LAUNCH: ParamId = ParamId(0);

    /// A launcher for `clips` that starts them on multiples of `quantize`
    /// ticks (zero or less starts them at once).
    pub fn new(clips: &[LauncherClip], quantize: Ticks) -> Self {
        let slots: Vec<Slot> = clips
            .iter()
            .map(|clip| {
                let length = clip.length.get().max(1);
                let mut schedule = Vec::with_capacity(clip.notes.len() * 2);
                for note in &clip.notes {
                    let start = note.start.get();
                    if !(0..length).contains(&start) {
                        continue;
                    }
                    let (pitch, channel) = (note.pitch.min(127), note.channel.min(15));
                    schedule.push(Scheduled {
                        tick: start,
                        kind: EventKind::NoteOn {
                            pitch,
                            velocity: note.velocity.clamp(1, 127),
                            channel,
                        },
                    });
                    schedule.push(Scheduled {
                        tick: (start + note.length.get().max(1)).min(length),
                        kind: EventKind::NoteOff {
                            pitch,
                            velocity: 0,
                            channel,
                        },
                    });
                }
                schedule.sort_by_key(|event| (event.tick, event.kind.is_note_on()));
                Slot { length, schedule }
            })
            .collect();
        let mut launch = ParamInfo::new(Self::LAUNCH, "Launch", 0.0, 1.0, 0.0, ParamUnit::Integer);
        launch.max = slots.len() as f32;
        Self {
            slots,
            quantize: quantize.get().max(0),
            params: [launch],
            pending: None,
            active: None,
            sounding: false,
            expected: None,
        }
    }

    /// Number of slots.
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// The slot that is playing, if any.
    pub fn playing_slot(&self) -> Option<usize> {
        self.active.map(|(slot, _)| slot)
    }

    fn release(&mut self, frame: u32, io: &mut ProcessIo<'_, '_>) {
        if self.sounding {
            io.events_out
                .push(Event::new(frame, EventKind::AllNotesOff));
            self.sounding = false;
        }
    }

    /// The tick of the next launch boundary at or after `sample`.
    fn boundary(&self, sample: SamplePos, tempo: Tempo, rate: SampleRate) -> i64 {
        let tick = tick_at_or_after(sample, tempo, rate);
        if self.quantize == 0 {
            return tick;
        }
        // A boundary just before `tick` may still land on `sample` after
        // rounding; `tick_at_or_after` already returned the earliest tick
        // on that sample, so rounding `tick` up to the grid is exact.
        tick.div_euclid(self.quantize) * self.quantize
            + if tick.rem_euclid(self.quantize) == 0 {
                0
            } else {
                self.quantize
            }
    }

    /// Emits the active slot's events that fall in `[from, end)`.
    fn play_slot(
        &mut self,
        from: SamplePos,
        end: SamplePos,
        block_start: SamplePos,
        ctx: &TransportContext,
        io: &mut ProcessIo<'_, '_>,
    ) {
        let Some((index, origin)) = self.active else {
            return;
        };
        let slot = &self.slots[index];
        let (tempo, rate) = (ctx.tempo, ctx.sample_rate);
        let first_tick = tick_at_or_after(from, tempo, rate);
        let last_tick = tick_at_or_after(end, tempo, rate);
        let first_pass = (first_tick - origin).div_euclid(slot.length) - 1;
        let last_pass = (last_tick - origin).div_euclid(slot.length);
        let mut sounding = self.sounding;
        for pass in first_pass.max(0)..=last_pass {
            let base = origin + pass * slot.length;
            for event in &slot.schedule {
                let at = Ticks::new(base + event.tick).to_sample_pos(tempo, rate);
                if at >= end {
                    break;
                }
                // The note-off on the loop end belongs to the pass before
                // `from`; everything else before `from` is already out.
                if at < from {
                    continue;
                }
                if io
                    .events_out
                    .push(Event::new((at - block_start) as u32, event.kind))
                {
                    sounding |= event.kind.is_note_on();
                }
            }
        }
        self.sounding = sounding;
    }
}

impl Device for ClipLauncher {
    fn prepare(&mut self, _config: &PrepareConfig) {
        self.reset();
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        io.silence_outputs();
        let frames = io.frames() as u64;
        if !ctx.playing {
            self.release(0, io);
            self.expected = None;
            if self.active.is_none() {
                io.events_out.extend_from(io.events);
            }
            return;
        }
        let (tempo, rate) = (ctx.tempo, ctx.sample_rate);
        let start = ctx.position_samples;
        let end = start.saturating_add(frames);

        if let Some((expected, at_tempo)) = self.expected {
            if at_tempo == tempo && expected != start {
                self.release(0, io);
            }
        }
        // Keep the slot's phase after a jump backwards: move its anchor
        // back by whole loops until it is at or before the playhead.
        if let Some((index, origin)) = self.active {
            let now = tick_at_or_after(start, tempo, rate);
            let length = self.slots[index].length;
            if origin > now {
                let loops = (origin - now + length - 1).div_euclid(length);
                self.active = Some((index, origin - loops * length));
            }
        }
        self.expected = Some((end, tempo));

        // At most one switch per block, on the launch boundary.
        let switch = self.pending.and_then(|request| {
            let tick = self.boundary(start, tempo, rate);
            let at = Ticks::new(tick).to_sample_pos(tempo, rate);
            (at < end).then_some((request, tick, at.max(start)))
        });
        let switch_at = switch.map_or(end, |(_, _, at)| at);

        // Before the switch: the old state.
        let passes_before = self.active.is_none();
        if passes_before {
            for event in io
                .events
                .iter()
                .filter(|e| u64::from(e.frame) < switch_at - start)
            {
                if io.events_out.push(*event) {
                    self.sounding |= event.kind.is_note_on();
                }
            }
        } else {
            self.play_slot(start, switch_at, start, ctx, io);
        }

        let Some((request, tick, at)) = switch else {
            return;
        };
        self.pending = None;
        let frame = (at - start) as u32;
        self.sounding = true; // The instrument may hold notes we did not see.
        self.release(frame, io);
        self.active = request
            .filter(|slot| *slot < self.slots.len())
            .map(|slot| (slot, tick));
        if self.active.is_none() {
            for event in io.events.iter().filter(|e| e.frame >= frame) {
                if io.events_out.push(*event) {
                    self.sounding |= event.kind.is_note_on();
                }
            }
        } else {
            self.play_slot(at, end, start, ctx, io);
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &self.params
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if param != Self::LAUNCH || !value.is_finite() {
            return;
        }
        let slot = value.round().max(0.0) as usize;
        self.pending = Some(slot.checked_sub(1).filter(|slot| *slot < self.slots.len()));
    }

    fn reset(&mut self) {
        self.sounding = false;
        self.expected = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, RATE};

    fn clip(length: i64, notes: &[(i64, i64, u8)]) -> LauncherClip {
        LauncherClip {
            length: Ticks::new(length),
            notes: notes
                .iter()
                .map(|&(start, len, pitch)| {
                    PlayerNote::new(Ticks::new(start), Ticks::new(len), pitch, 100)
                })
                .collect(),
        }
    }

    /// Plays `frames` from `from` at 120 BPM (25 samples per tick), feeding
    /// `upstream` (absolute sample, kind) as arrangement events. Returns
    /// `(absolute sample, kind)` for every event out.
    fn play(
        launcher: &mut ClipLauncher,
        from: u64,
        frames: usize,
        block: usize,
        upstream: &[(u64, EventKind)],
    ) -> Vec<(u64, EventKind)> {
        let mut out = Vec::new();
        let mut position = from;
        let end = from + frames as u64;
        while position < end {
            let n = block.min((end - position) as usize);
            let events: Vec<Event> = upstream
                .iter()
                .filter(|(at, _)| (position..position + n as u64).contains(at))
                .map(|(at, kind)| Event::new((at - position) as u32, *kind))
                .collect();
            let mut audio = vec![9.0f32; n];
            let ctx = TransportContext::playing_at(RATE, Tempo::DEFAULT, position);
            let emitted = run(launcher, &ctx, &[], None, &mut [&mut audio], &events);
            assert!(audio.iter().all(|s| *s == 0.0));
            out.extend(
                emitted
                    .iter()
                    .map(|e| (position + u64::from(e.frame), e.kind)),
            );
            position += n as u64;
        }
        out
    }

    #[test]
    fn the_arrangement_passes_through_until_a_slot_is_launched() {
        let mut launcher = ClipLauncher::new(&[clip(960, &[(0, 240, 60)])], Ticks::new(960));
        assert_eq!(launcher.slot_count(), 1);
        assert_eq!(launcher.params()[0].max, 1.0);
        let upstream = [
            (100, EventKind::note_on(36, 90)),
            (700, EventKind::note_off(36)),
        ];
        let out = play(&mut launcher, 0, 1_000, 64, &upstream);
        assert_eq!(out, upstream);
        assert_eq!(launcher.playing_slot(), None);
    }

    #[test]
    fn a_launch_waits_for_the_quantize_boundary_and_then_loops() {
        // One beat is 24 000 samples. Launch quantization: one beat.
        let mut launcher = ClipLauncher::new(
            &[clip(480, &[(0, 120, 60), (240, 480, 64)])],
            Ticks::new(960),
        );
        let upstream = [
            (10_000, EventKind::note_on(36, 90)),
            (30_000, EventKind::note_on(38, 90)),
        ];
        let mut out = play(&mut launcher, 0, 5_000, 256, &upstream);
        assert!(out.is_empty());
        launcher.set_param(ClipLauncher::LAUNCH, 1.0);
        out = play(&mut launcher, 5_000, 43_000, 256, &upstream);
        assert_eq!(
            out,
            [
                (10_000, EventKind::note_on(36, 90)),
                (24_000, EventKind::AllNotesOff),
                (24_000, EventKind::note_on(60, 100)),
                (27_000, EventKind::note_off(60)),
                (30_000, EventKind::note_on(64, 100)),
                (36_000, EventKind::note_off(64)),
                (36_000, EventKind::note_on(60, 100)),
                (39_000, EventKind::note_off(60)),
                (42_000, EventKind::note_on(64, 100)),
            ],
            "the arrangement's note at 30 000 is blocked; the clip loops every 12 000"
        );
        assert_eq!(launcher.playing_slot(), Some(0));
    }

    #[test]
    fn launch_timing_does_not_depend_on_block_size() {
        let clips = [
            clip(960, &[(0, 240, 60), (480, 240, 62)]),
            clip(240, &[(0, 60, 72)]),
        ];
        let run_with = |block: usize| {
            let mut launcher = ClipLauncher::new(&clips, Ticks::new(480));
            let mut out = play(&mut launcher, 0, 7_001, block, &[]);
            launcher.set_param(ClipLauncher::LAUNCH, 1.0);
            out.extend(play(&mut launcher, 7_001, 40_000, block, &[]));
            launcher.set_param(ClipLauncher::LAUNCH, 2.0);
            out.extend(play(&mut launcher, 47_001, 30_000, block, &[]));
            out
        };
        let reference = run_with(77_001);
        assert_eq!(reference[0], (12_000, EventKind::AllNotesOff));
        assert!(reference.contains(&(48_000, EventKind::note_on(72, 100))));
        for block in [1, 13, 64, 512, 4_096] {
            assert_eq!(run_with(block), reference, "block size {block}");
        }
    }

    #[test]
    fn zero_quantize_launches_at_once_and_stop_returns_to_the_arrangement() {
        let mut launcher = ClipLauncher::new(&[clip(960, &[(0, 240, 60)])], Ticks::ZERO);
        launcher.set_param(ClipLauncher::LAUNCH, 1.0);
        let out = play(&mut launcher, 1_001, 500, 64, &[]);
        assert_eq!(
            out[0],
            (1_025, EventKind::AllNotesOff),
            "the next whole tick"
        );
        assert_eq!(out[1], (1_025, EventKind::note_on(60, 100)));

        launcher.set_param(ClipLauncher::LAUNCH, 0.0);
        let upstream = [(2_000, EventKind::note_on(36, 90))];
        let out = play(&mut launcher, 1_501, 1_000, 64, &upstream);
        assert_eq!(
            out,
            [
                (1_525, EventKind::AllNotesOff),
                (2_000, EventKind::note_on(36, 90))
            ]
        );
        assert_eq!(launcher.playing_slot(), None);

        launcher.set_param(ClipLauncher::LAUNCH, 9.0);
        play(&mut launcher, 2_501, 100, 64, &[]);
        assert_eq!(launcher.playing_slot(), None, "an unknown slot is a stop");
    }

    #[test]
    fn stopping_and_jumping_release_notes_and_the_loop_keeps_its_phase() {
        let mut launcher =
            ClipLauncher::new(&[clip(960, &[(0, 900, 60), (480, 100, 62)])], Ticks::ZERO);
        launcher.set_param(ClipLauncher::LAUNCH, 1.0);
        let out = play(&mut launcher, 48_000, 100, 100, &[]);
        assert_eq!(out[1], (48_000, EventKind::note_on(60, 100)));

        let stopped = TransportContext::stopped(RATE, Tempo::DEFAULT);
        let mut audio = [0.0f32; 16];
        let off = run(&mut launcher, &stopped, &[], None, &mut [&mut audio], &[]);
        assert_eq!(off, [Event::new(0, EventKind::AllNotesOff)]);

        // Jump back to the start: the loop is anchored at beat 2, so beat 0
        // is also a loop start, and the note half a beat in still plays.
        let out = play(&mut launcher, 0, 13_000, 512, &[]);
        assert_eq!(
            out,
            [
                (0, EventKind::note_on(60, 100)),
                (12_000, EventKind::note_on(62, 100))
            ]
        );
    }
}
