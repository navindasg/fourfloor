//! Transport: play/stop/seek, tempo, sample-accurate position and a loop
//! region. Lives on the audio thread; every method is allocation-free.

use fourfloor_core::{Beats, SamplePos, SampleRate, Tempo, TransportContext};

/// A loop region in beats. Playback reaching `end` jumps back to `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopRegion {
    /// First beat of the loop (inclusive).
    pub start: Beats,
    /// End of the loop (exclusive).
    pub end: Beats,
}

impl LoopRegion {
    /// Creates a region from `start` to `end` beats.
    pub const fn new(start: Beats, end: Beats) -> Self {
        Self { start, end }
    }
}

/// The engine's clock.
///
/// The position is held in whole samples, which is what makes it
/// sample-accurate; beats are derived from it at the current tempo. There is
/// one constant tempo (no tempo map yet): changing the tempo keeps the beat
/// position and recomputes the sample position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    sample_rate: SampleRate,
    tempo: Tempo,
    playing: bool,
    position: SamplePos,
    loop_region: Option<LoopRegion>,
    /// The loop in samples at the current tempo: `(start, end)`, `end > start`.
    loop_samples: Option<(SamplePos, SamplePos)>,
}

impl Transport {
    /// A stopped transport at the start of the timeline.
    pub fn new(sample_rate: SampleRate, tempo: Tempo) -> Self {
        Self {
            sample_rate,
            tempo,
            playing: false,
            position: 0,
            loop_region: None,
            loop_samples: None,
        }
    }

    /// Starts rolling from the current position.
    pub fn play(&mut self) {
        self.playing = true;
    }

    /// Stops rolling and keeps the position.
    pub fn stop(&mut self) {
        self.playing = false;
    }

    /// Whether the transport is rolling.
    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Jumps to a beat position. Negative positions clamp to zero.
    pub fn seek(&mut self, position: Beats) {
        self.position = position.to_sample_pos(self.tempo, self.sample_rate);
    }

    /// Jumps to a sample position.
    pub fn seek_samples(&mut self, position: SamplePos) {
        self.position = position;
    }

    /// Changes the tempo, keeping the beat position (and the loop's beat
    /// boundaries) fixed.
    pub fn set_tempo(&mut self, tempo: Tempo) {
        if tempo == self.tempo {
            return;
        }
        let was_inside = self.loop_samples.is_some_and(|(_, e)| self.position < e);
        let beats = self.position_beats();
        self.tempo = tempo;
        self.position = beats.to_sample_pos(tempo, self.sample_rate);
        self.loop_samples = self
            .loop_region
            .and_then(|r| Self::region_samples(r, tempo, self.sample_rate));
        // Rounding may land exactly on the new loop end; keep the playhead
        // inside the loop it was in.
        if let (true, Some((start, end))) = (was_inside, self.loop_samples) {
            if self.position >= end {
                self.position = start;
            }
        }
    }

    /// Sets the loop region. Returns `false` and leaves the loop unchanged
    /// when the region is empty or reversed (less than one sample long).
    pub fn set_loop(&mut self, region: LoopRegion) -> bool {
        match Self::region_samples(region, self.tempo, self.sample_rate) {
            Some(samples) => {
                self.loop_region = Some(region);
                self.loop_samples = Some(samples);
                true
            }
            None => false,
        }
    }

    /// Removes the loop region.
    pub fn clear_loop(&mut self) {
        self.loop_region = None;
        self.loop_samples = None;
    }

    /// The loop region, if one is set.
    pub fn loop_region(&self) -> Option<LoopRegion> {
        self.loop_region
    }

    /// Current position in samples.
    pub fn position_samples(&self) -> SamplePos {
        self.position
    }

    /// Current position in beats at the current tempo.
    pub fn position_beats(&self) -> Beats {
        Beats::from_sample_pos(self.position, self.tempo, self.sample_rate)
    }

    /// Current tempo.
    pub fn tempo(&self) -> Tempo {
        self.tempo
    }

    /// Sample rate the transport counts in.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// The context handed to devices for a block starting at the current
    /// position.
    pub fn context(&self) -> TransportContext {
        TransportContext {
            sample_rate: self.sample_rate,
            tempo: self.tempo,
            playing: self.playing,
            position_samples: self.position,
            position_beats: self.position_beats(),
        }
    }

    /// Frames left before the playhead jumps (the loop end), or `None` when
    /// it will not jump: stopped, no loop, or already past the loop end.
    /// The engine never lets a block cross this boundary, so positions
    /// inside one block are always contiguous.
    pub fn frames_until_jump(&self) -> Option<u64> {
        match self.loop_samples {
            Some((_, end)) if self.playing && self.position < end => Some(end - self.position),
            _ => None,
        }
    }

    /// Moves the playhead on by `frames` if rolling, wrapping at the loop
    /// end. A playhead already past the loop end keeps going.
    pub fn advance(&mut self, frames: usize) {
        if !self.playing {
            return;
        }
        let frames = frames as u64;
        match self.loop_samples {
            Some((start, end)) if self.position < end => {
                let next = self.position.saturating_add(frames);
                self.position = if next >= end {
                    start + (next - end) % (end - start)
                } else {
                    next
                };
            }
            _ => self.position = self.position.saturating_add(frames),
        }
    }

    fn region_samples(
        region: LoopRegion,
        tempo: Tempo,
        rate: SampleRate,
    ) -> Option<(SamplePos, SamplePos)> {
        let start = region.start.to_sample_pos(tempo, rate);
        let end = region.end.to_sample_pos(tempo, rate);
        (end > start).then_some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transport(bpm: f64) -> Transport {
        Transport::new(SampleRate::HZ_48000, Tempo::new(bpm).unwrap())
    }

    #[test]
    fn stopped_transport_does_not_advance() {
        let mut t = transport(120.0);
        t.advance(512);
        assert_eq!(t.position_samples(), 0);
        assert!(!t.context().playing);
    }

    #[test]
    fn playing_advances_sample_accurately() {
        let mut t = transport(120.0);
        t.play();
        for _ in 0..375 {
            t.advance(64);
        }
        assert_eq!(t.position_samples(), 24_000);
        assert_eq!(t.position_beats(), Beats(1.0));
        t.stop();
        t.advance(64);
        assert_eq!(t.position_samples(), 24_000, "stop keeps position");
    }

    #[test]
    fn seek_converts_beats_to_samples() {
        let mut t = transport(125.0);
        t.seek(Beats(4.0));
        // 48000 * 60 / 125 = 23040 samples per beat.
        assert_eq!(t.position_samples(), 92_160);
        t.seek(Beats(-3.0));
        assert_eq!(t.position_samples(), 0);
        let ctx = t.context();
        assert_eq!(ctx.position_samples, 0);
        assert_eq!(ctx.position_beats, Beats::ZERO);
    }

    #[test]
    fn tempo_change_keeps_beat_position() {
        let mut t = transport(120.0);
        t.seek(Beats(8.0));
        assert_eq!(t.position_samples(), 192_000);
        t.set_tempo(Tempo::new(60.0).unwrap());
        assert_eq!(t.position_beats(), Beats(8.0));
        assert_eq!(t.position_samples(), 384_000);
    }

    #[test]
    fn loop_wraps_exactly_at_end() {
        let mut t = transport(120.0);
        assert!(t.set_loop(LoopRegion::new(Beats(1.0), Beats(2.0))));
        t.seek(Beats(1.0));
        t.play();
        assert_eq!(t.frames_until_jump(), Some(24_000));
        t.advance(23_999);
        assert_eq!(t.position_samples(), 47_999);
        assert_eq!(t.frames_until_jump(), Some(1));
        t.advance(1);
        assert_eq!(t.position_samples(), 24_000);
    }

    #[test]
    fn advancing_past_the_loop_end_wraps_modulo_the_loop() {
        let mut t = transport(120.0);
        t.set_loop(LoopRegion::new(Beats(0.0), Beats(1.0)));
        t.play();
        t.advance(24_000 * 2 + 10);
        assert_eq!(t.position_samples(), 10);
    }

    #[test]
    fn playhead_past_loop_end_keeps_going() {
        let mut t = transport(120.0);
        t.set_loop(LoopRegion::new(Beats(0.0), Beats(1.0)));
        t.seek(Beats(2.0));
        t.play();
        assert_eq!(t.frames_until_jump(), None);
        t.advance(100);
        assert_eq!(t.position_samples(), 48_100);
    }

    #[test]
    fn empty_or_reversed_loop_is_rejected() {
        let mut t = transport(120.0);
        assert!(!t.set_loop(LoopRegion::new(Beats(2.0), Beats(2.0))));
        assert!(!t.set_loop(LoopRegion::new(Beats(2.0), Beats(1.0))));
        assert_eq!(t.loop_region(), None);
        t.set_loop(LoopRegion::new(Beats(0.0), Beats(4.0)));
        t.clear_loop();
        t.play();
        assert_eq!(t.frames_until_jump(), None);
    }

    #[test]
    fn loop_follows_tempo_changes() {
        let mut t = transport(120.0);
        t.set_loop(LoopRegion::new(Beats(0.0), Beats(4.0)));
        t.play();
        t.set_tempo(Tempo::new(60.0).unwrap());
        assert_eq!(t.frames_until_jump(), Some(192_000));
    }

    #[test]
    fn stopped_transport_reports_no_jump() {
        let mut t = transport(120.0);
        t.set_loop(LoopRegion::new(Beats(0.0), Beats(1.0)));
        assert_eq!(t.frames_until_jump(), None);
    }
}
