//! MIDI clock: 24 pulses per quarter note, locked to the timeline.

use fourfloor_core::{SamplePos, SampleRate, Tempo};

/// MIDI clock pulses per quarter note.
pub const PULSES_PER_BEAT: u64 = 24;
/// Timing clock status byte.
pub const CLOCK: u8 = 0xF8;
/// Start status byte.
pub const START: u8 = 0xFA;
/// Continue status byte.
pub const CONTINUE: u8 = 0xFB;
/// Stop status byte.
pub const STOP: u8 = 0xFC;

/// The sample position of clock pulse `index`, counted from the start of
/// the timeline.
pub fn pulse_position(index: u64, tempo: Tempo, rate: SampleRate) -> SamplePos {
    let per_pulse = tempo.samples_per_beat(rate) / PULSES_PER_BEAT as f64;
    (index as f64 * per_pulse).round() as SamplePos
}

/// The index of the first clock pulse at or after `sample`.
pub fn first_pulse_at_or_after(sample: SamplePos, tempo: Tempo, rate: SampleRate) -> u64 {
    let per_pulse = tempo.samples_per_beat(rate) / PULSES_PER_BEAT as f64;
    let mut index = (sample as f64 / per_pulse).floor() as u64;
    while pulse_position(index, tempo, rate) < sample {
        index += 1;
    }
    while index > 0 && pulse_position(index - 1, tempo, rate) >= sample {
        index -= 1;
    }
    index
}

/// Generates the MIDI clock stream for a transport, block by block.
///
/// Feed it every block the engine processes; it calls `emit(frame, byte)`
/// for each realtime byte that falls in the block: [`START`] (or
/// [`CONTINUE`] when playback starts away from zero) when the transport
/// starts, [`CLOCK`] on every pulse while it rolls, [`STOP`] when it
/// stops. It never allocates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockOut {
    running: bool,
}

impl ClockOut {
    /// A generator for a stopped transport.
    pub const fn new() -> Self {
        Self { running: false }
    }

    /// Whether a start has been sent without a stop.
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Covers the block of `frames` frames that starts at `position`.
    pub fn advance(
        &mut self,
        playing: bool,
        position: SamplePos,
        frames: usize,
        tempo: Tempo,
        rate: SampleRate,
        mut emit: impl FnMut(u32, u8),
    ) {
        if !playing {
            if self.running {
                emit(0, STOP);
                self.running = false;
            }
            return;
        }
        if !self.running {
            emit(0, if position == 0 { START } else { CONTINUE });
            self.running = true;
        }
        let end = position.saturating_add(frames as u64);
        let mut index = first_pulse_at_or_after(position, tempo, rate);
        loop {
            let at = pulse_position(index, tempo, rate);
            if at >= end {
                break;
            }
            emit((at - position) as u32, CLOCK);
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;

    /// Runs the clock over `frames` frames from `from` in blocks of
    /// `block`, returning `(absolute sample, byte)`.
    fn run(
        clock: &mut ClockOut,
        from: u64,
        frames: u64,
        block: u64,
        tempo: Tempo,
    ) -> Vec<(u64, u8)> {
        let mut out = Vec::new();
        let mut position = from;
        while position < from + frames {
            let n = block.min(from + frames - position);
            clock.advance(true, position, n as usize, tempo, RATE, |frame, byte| {
                out.push((position + u64::from(frame), byte));
            });
            position += n;
        }
        out
    }

    #[test]
    fn a_beat_carries_24_evenly_spaced_pulses_after_a_start() {
        let mut clock = ClockOut::new();
        // One beat at 120 BPM is 24 000 samples: a pulse every 1 000.
        let out = run(&mut clock, 0, 24_000, 512, Tempo::DEFAULT);
        assert_eq!(out[0], (0, START));
        let pulses: Vec<u64> = out[1..].iter().map(|(at, _)| *at).collect();
        assert!(out[1..].iter().all(|(_, byte)| *byte == CLOCK));
        let expected: Vec<u64> = (0..24).map(|n| n * 1_000).collect();
        assert_eq!(pulses, expected);
        assert!(clock.is_running());
    }

    #[test]
    fn pulses_follow_the_tempo_and_do_not_depend_on_block_size() {
        let tempo = Tempo::new(127.3).unwrap();
        let reference = run(&mut ClockOut::new(), 0, 200_000, 200_000, tempo);
        let pulses = reference.len() - 1;
        // 200 000 samples at 127.3 BPM is 8.84 beats: 212.2 pulses.
        assert_eq!(pulses, 213);
        for block in [1, 7, 64, 480, 4_096] {
            assert_eq!(
                run(&mut ClockOut::new(), 0, 200_000, block, tempo),
                reference,
                "{block}"
            );
        }
        // No drift: pulse 2 400 is exactly 100 beats in.
        let at = pulse_position(2_400, tempo, RATE) as f64;
        assert!((at - 100.0 * tempo.samples_per_beat(RATE)).abs() <= 0.5);
        assert_eq!(first_pulse_at_or_after(at as u64, tempo, RATE), 2_400);
    }

    #[test]
    fn starting_mid_song_continues_and_stopping_sends_stop_once() {
        let mut clock = ClockOut::new();
        let out = run(&mut clock, 12_500, 1_000, 256, Tempo::DEFAULT);
        assert_eq!(out, [(12_500, CONTINUE), (13_000, CLOCK)]);
        let mut stopped = Vec::new();
        for _ in 0..3 {
            clock.advance(false, 13_500, 256, Tempo::DEFAULT, RATE, |frame, byte| {
                stopped.push((frame, byte));
            });
        }
        assert_eq!(stopped, [(0, STOP)]);
        assert!(!clock.is_running());
    }
}
