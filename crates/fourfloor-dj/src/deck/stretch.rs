//! Key lock through Signalsmith Stretch (feature `stretch`).
//!
//! The stretcher is fed track audio ahead of the playhead by its own
//! latency (input latency plus output latency at the current rate), so the
//! audio that comes out of a block is the audio at the playhead, as with
//! varispeed. Buffers are allocated in [`StretchLock::prepare`]; the
//! library's `process` works in place on its own preallocated state. Its
//! `seek` resizes an internal buffer to a fixed size, which `prepare` does
//! once so later seeks do not allocate.

use fourfloor_core::SampleRate;
use signalsmith_stretch::Stretch;

use super::playhead::Playhead;
use super::source::frame;
use crate::track::{DeckTrack, MAX_STEMS};

/// Most track frames fed per output frame (the deck's fastest rate, with
/// room for a track at a higher sample rate than the engine).
const MAX_FEED_PER_FRAME: usize = 8;

/// Rates for one block.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Block {
    /// Track samples the playhead moves per output frame.
    pub step: f64,
    /// Track samples per output frame at normal speed (track rate over
    /// engine rate).
    pub ratio: f64,
}

pub(crate) struct StretchLock {
    stretch: Option<Stretch>,
    /// Interleaved stereo input for one block.
    input: Vec<f32>,
    /// Interleaved stereo output for one block.
    output: Vec<f32>,
    /// Interleaved stereo pre-roll for a seek.
    preroll: Vec<f32>,
    input_latency: f64,
    output_latency: f64,
    /// Next track position (unwrapped) to feed.
    feed: f64,
    ratio: f64,
    primed: bool,
}

impl StretchLock {
    pub fn new() -> Self {
        Self {
            stretch: None,
            input: Vec::new(),
            output: Vec::new(),
            preroll: Vec::new(),
            input_latency: 0.0,
            output_latency: 0.0,
            feed: 0.0,
            ratio: 1.0,
            primed: false,
        }
    }

    /// Builds the stretcher and its buffers. Allocates.
    pub fn prepare(&mut self, sample_rate: SampleRate, max_block: usize) {
        let mut stretch = Stretch::preset_default(2, sample_rate.hz());
        stretch.set_transpose_factor(self.ratio as f32, None);
        let input_latency = stretch.input_latency();
        let output_latency = stretch.output_latency();
        // One block plus one interval is what a seek reads; both latencies
        // together are at least that.
        let preroll_frames = 2 * (input_latency + output_latency).max(64);
        self.preroll = vec![0.0; 2 * preroll_frames];
        // Warm the library's seek buffer so later seeks reuse it.
        stretch.seek(&self.preroll, 1.0);
        stretch.reset();
        self.input = vec![0.0; 2 * (max_block.max(1) * MAX_FEED_PER_FRAME + preroll_frames)];
        self.output = vec![0.0; 2 * max_block.max(1)];
        self.input_latency = input_latency as f64;
        self.output_latency = output_latency as f64;
        self.stretch = Some(stretch);
        self.primed = false;
    }

    /// Starts over at the playhead on the next block (after a jump).
    pub fn restart(&mut self) {
        self.primed = false;
    }

    fn lead(&self, step: f64) -> f64 {
        self.input_latency + self.output_latency * step
    }

    /// Plays one block into `left` and `right` and moves the playhead.
    /// Returns true when the end of the track was reached.
    pub fn render(
        &mut self,
        track: &DeckTrack,
        head: &mut Playhead,
        gains: &[f32; MAX_STEMS],
        block: Block,
        left: &mut [f32],
        right: &mut [f32],
    ) -> bool {
        let frames = left.len().min(right.len());
        let capacity = self.output.len() / 2;
        let Some(stretch) = self.stretch.as_mut() else {
            left.fill(0.0);
            right.fill(0.0);
            return head.advance(block.step * frames as f64);
        };
        if frames > capacity {
            // Larger than the prepared block: the engine never does this.
            left.fill(0.0);
            right.fill(0.0);
            return head.advance(block.step * frames as f64);
        }
        if block.ratio.is_finite() && block.ratio > 0.0 && block.ratio != self.ratio {
            self.ratio = block.ratio;
            stretch.set_transpose_factor(self.ratio as f32, None);
        }
        let lead = self.input_latency + self.output_latency * block.step;
        if !self.primed {
            stretch.reset();
            self.feed = head.pos + lead;
            let preroll_frames = self.preroll.len() / 2;
            let first = self.feed.floor() as i64 - preroll_frames as i64;
            for (i, pair) in self.preroll.chunks_exact_mut(2).enumerate() {
                let (l, r) = frame(track, head.map(first + i as i64), gains);
                pair[0] = l;
                pair[1] = r;
            }
            stretch.seek(&self.preroll, block.step);
            self.primed = true;
        }
        let before = head.pos;
        let travelled = block.step * frames as f64;
        let ended = head.advance(travelled);
        // Feed up to where the playhead will be, plus the latency, so the
        // stretcher's output lines up with the playhead.
        let target = before + travelled + lead;
        let max_feed = self.input.len() / 2;
        let wanted = (target.floor() - self.feed.floor()).max(0.0) as usize;
        let count = wanted.min(max_feed);
        let first = self.feed.floor() as i64;
        for (i, pair) in self.input[..2 * count].chunks_exact_mut(2).enumerate() {
            let (l, r) = frame(track, head.map(first + i as i64), gains);
            pair[0] = l;
            pair[1] = r;
        }
        stretch.process(&self.input[..2 * count], &mut self.output[..2 * frames]);
        self.feed = if count == wanted {
            target
        } else {
            self.feed + count as f64
        };
        // A loop wrap moved the playhead back; keep the feed the same
        // distance ahead of it.
        let unwrapped = before + travelled;
        if head.pos < unwrapped - 1e-9 && !ended {
            self.feed -= unwrapped - head.pos;
        }
        for ((pair, l), r) in self
            .output
            .chunks_exact(2)
            .zip(left.iter_mut())
            .zip(right.iter_mut())
        {
            *l = pair[0];
            *r = pair[1];
        }
        let _ = self.lead(block.step);
        ended
    }
}
