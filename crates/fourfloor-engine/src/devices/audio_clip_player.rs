use std::sync::Arc;

use fourfloor_core::{Device, PrepareConfig, ProcessIo, SamplePos, Ticks, TransportContext};

/// Decoded audio shared between clips: planar channels at the graph's
/// sample rate. Build it off the audio thread.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioSource {
    channels: Vec<Vec<f32>>,
    frames: usize,
}

impl AudioSource {
    /// Wraps planar channels. Channels shorter than the longest one are
    /// treated as ending in silence.
    pub fn new(channels: Vec<Vec<f32>>) -> Self {
        let frames = channels.iter().map(Vec::len).max().unwrap_or(0);
        Self { channels, frames }
    }

    /// A one-channel source; it plays on every output channel.
    pub fn mono(samples: Vec<f32>) -> Self {
        Self::new(vec![samples])
    }

    /// Frames in the source.
    pub fn frames(&self) -> usize {
        self.frames
    }

    /// Channels in the source.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// One channel's samples.
    pub fn channel(&self, index: usize) -> Option<&[f32]> {
        self.channels.get(index).map(Vec::as_slice)
    }

    /// The sample for output channel `channel` at `frame`: a mono source
    /// feeds every channel, and anything out of range is silence.
    #[inline]
    fn sample(&self, channel: usize, frame: u64) -> f32 {
        let index = if self.channels.len() == 1 { 0 } else { channel };
        self.channels
            .get(index)
            .and_then(|samples| samples.get(usize::try_from(frame).ok()?))
            .copied()
            .unwrap_or(0.0)
    }
}

/// One audio clip for an [`AudioClipPlayer`], positioned on the timeline.
#[derive(Clone, Debug)]
pub struct AudioRegion {
    /// The audio.
    pub source: Arc<AudioSource>,
    /// Clip start in ticks from the start of the timeline.
    pub start: Ticks,
    /// Clip length in ticks.
    pub length: Ticks,
    /// Frames of the source to skip: the source frame that content time
    /// zero plays.
    pub source_offset_frames: u64,
    /// Content time at the clip start, in ticks (a split clip's right half
    /// has one).
    pub content_offset: Ticks,
    /// Length of the repeating part of the content, if the clip loops.
    pub loop_length: Option<Ticks>,
    /// Linear gain.
    pub gain: f32,
    /// Length of the linear fade-in at the clip start, in ticks.
    pub fade_in: Ticks,
    /// Length of the linear fade-out at the clip end, in ticks.
    pub fade_out: Ticks,
}

impl AudioRegion {
    /// A region that plays `source` from its first frame at unity gain,
    /// with no fades and no loop.
    pub fn new(source: Arc<AudioSource>, start: Ticks, length: Ticks) -> Self {
        Self {
            source,
            start,
            length,
            source_offset_frames: 0,
            content_offset: Ticks::ZERO,
            loop_length: None,
            gain: 1.0,
            fade_in: Ticks::ZERO,
            fade_out: Ticks::ZERO,
        }
    }
}

/// A track's audio clip player: plays its regions as the transport rolls,
/// sample-accurately, and is silent while the transport is stopped.
///
/// It is an audio source with no inputs. A clip's start, length, fades and
/// loop are musical (ticks) and are turned into samples at the block's
/// tempo with the transport's own rounding, so a clip starts on the exact
/// sample its tick maps to. The audio itself plays at its recorded speed:
/// one source frame per output frame. To follow the tempo, hand the player
/// a source that was stretched beforehand.
///
/// Overlapping regions are summed. The region list is fixed; changing it
/// means compiling a new graph.
#[derive(Clone, Debug)]
pub struct AudioClipPlayer {
    regions: Vec<AudioRegion>,
}

impl AudioClipPlayer {
    /// A player for `regions`, in any order.
    pub fn new(mut regions: Vec<AudioRegion>) -> Self {
        regions.retain(|region| region.length > Ticks::ZERO);
        regions.sort_by_key(|region| region.start);
        Self { regions }
    }

    /// Number of regions the player holds.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    fn render(region: &AudioRegion, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let (tempo, rate) = (ctx.tempo, ctx.sample_rate);
        let samples = |ticks: Ticks| ticks.to_sample_pos(tempo, rate);
        let block_start = ctx.position_samples;
        let block_end = block_start.saturating_add(io.frames() as u64);
        let clip_start = samples(region.start);
        let clip_end = samples(region.start + region.length);
        let (from, to) = (clip_start.max(block_start), clip_end.min(block_end));
        if from >= to {
            return;
        }
        let offset = samples(region.content_offset);
        let period: Option<SamplePos> =
            region.loop_length.map(samples).filter(|period| *period > 0);
        let fade_in = samples(region.fade_in);
        let fade_out = samples(region.fade_out);

        for (channel, output) in io.outputs.iter_mut().enumerate() {
            for position in from..to {
                let since_start = position - clip_start;
                let content = match period {
                    Some(period) => (offset + since_start) % period,
                    None => offset + since_start,
                };
                let mut gain = region.gain;
                if since_start < fade_in {
                    gain *= since_start as f32 / fade_in as f32;
                }
                let until_end = clip_end - position;
                if until_end <= fade_out {
                    gain *= until_end as f32 / fade_out as f32;
                }
                let frame = region.source_offset_frames.saturating_add(content);
                output[(position - block_start) as usize] +=
                    region.source.sample(channel, frame) * gain;
            }
        }
    }
}

impl Device for AudioClipPlayer {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        io.silence_outputs();
        if !ctx.playing {
            return;
        }
        for region in &self.regions {
            Self::render(region, ctx, io);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, stopped, RATE};
    use fourfloor_core::Tempo;

    /// A source whose sample `n` is `n + 1` on the left and its negative on
    /// the right, so every frame is identifiable.
    fn ramp(frames: usize) -> Arc<AudioSource> {
        let left: Vec<f32> = (0..frames).map(|n| (n + 1) as f32).collect();
        let right = left.iter().map(|s| -s).collect();
        Arc::new(AudioSource::new(vec![left, right]))
    }

    /// Plays `frames` from `from` in blocks of `block`. 120 BPM at 48 kHz is
    /// 25 samples per tick.
    fn play(player: &mut AudioClipPlayer, from: u64, frames: usize, block: usize) -> [Vec<f32>; 2] {
        let mut out = [Vec::new(), Vec::new()];
        let mut position = from;
        while out[0].len() < frames {
            let n = block.min(frames - out[0].len());
            let (mut l, mut r) = (vec![9.0f32; n], vec![9.0f32; n]);
            let ctx = TransportContext::playing_at(RATE, Tempo::DEFAULT, position);
            run(player, &ctx, &[], None, &mut [&mut l, &mut r], &[]);
            out[0].extend(l);
            out[1].extend(r);
            position += n as u64;
        }
        out
    }

    #[test]
    fn a_region_plays_on_its_exact_samples_in_stereo() {
        let region = AudioRegion::new(ramp(1_000), Ticks::new(4), Ticks::new(8));
        let mut player = AudioClipPlayer::new(vec![region]);
        assert_eq!(player.region_count(), 1);
        let [left, right] = play(&mut player, 0, 400, 64);
        assert!(
            left[..100].iter().all(|s| *s == 0.0),
            "silent before the clip"
        );
        assert_eq!(left[100], 1.0, "first source frame on tick 4");
        assert_eq!(left[299], 200.0);
        assert_eq!(right[299], -200.0);
        assert!(
            left[300..].iter().all(|s| *s == 0.0),
            "silent after 8 ticks"
        );
    }

    #[test]
    fn output_does_not_depend_on_block_size_and_is_silent_when_stopped() {
        let regions = vec![
            AudioRegion::new(ramp(500), Ticks::new(2), Ticks::new(10)),
            AudioRegion::new(ramp(500), Ticks::new(6), Ticks::new(10)),
        ];
        let reference = play(&mut AudioClipPlayer::new(regions.clone()), 0, 600, 600);
        assert_eq!(reference[0][150], 101.0 + 1.0, "overlapping regions sum");
        for block in [1, 7, 64, 511] {
            let out = play(&mut AudioClipPlayer::new(regions.clone()), 0, 600, block);
            assert_eq!(out, reference, "block size {block}");
        }
        let mut player = AudioClipPlayer::new(regions);
        let mut l = [9.0f32; 32];
        run(&mut player, &stopped(), &[], None, &mut [&mut l], &[]);
        assert!(l.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn offsets_skip_into_the_source_and_a_seek_lands_mid_clip() {
        let region = AudioRegion {
            source_offset_frames: 10,
            content_offset: Ticks::new(2),
            ..AudioRegion::new(ramp(1_000), Ticks::ZERO, Ticks::new(8))
        };
        let mut player = AudioClipPlayer::new(vec![region]);
        let [left, _] = play(&mut player, 0, 8, 8);
        assert_eq!(left[0], 61.0, "10 frames + 2 ticks (50 frames) in");
        let [left, _] = play(&mut player, 75, 8, 8);
        assert_eq!(left[0], 136.0, "playing from sample 75 continues the file");
    }

    #[test]
    fn a_loop_repeats_the_content_and_past_the_source_end_is_silence() {
        let looped = AudioRegion {
            loop_length: Some(Ticks::new(2)),
            ..AudioRegion::new(ramp(1_000), Ticks::ZERO, Ticks::new(6))
        };
        let [left, _] = play(&mut AudioClipPlayer::new(vec![looped]), 0, 160, 33);
        assert_eq!(
            (left[0], left[49], left[50], left[149]),
            (1.0, 50.0, 1.0, 50.0)
        );
        assert_eq!(left[150], 0.0);

        let short = AudioRegion::new(ramp(20), Ticks::ZERO, Ticks::new(4));
        let [left, _] = play(&mut AudioClipPlayer::new(vec![short]), 0, 100, 100);
        assert_eq!(left[19], 20.0);
        assert!(left[20..].iter().all(|s| *s == 0.0));
    }

    #[test]
    fn gain_and_fades_shape_the_clip() {
        let ones = Arc::new(AudioSource::mono(vec![1.0; 1_000]));
        let region = AudioRegion {
            gain: 0.5,
            fade_in: Ticks::new(4),
            fade_out: Ticks::new(2),
            ..AudioRegion::new(ones, Ticks::ZERO, Ticks::new(16))
        };
        let [left, right] = play(&mut AudioClipPlayer::new(vec![region]), 0, 400, 128);
        assert_eq!(left, right, "a mono source feeds both channels");
        assert_eq!(left[0], 0.0);
        assert!((left[50] - 0.25).abs() < 1e-6, "half way up the fade-in");
        assert_eq!(left[100], 0.5);
        assert_eq!(left[349], 0.5);
        assert!(
            (left[375] - 0.25).abs() < 1e-6,
            "half way down the fade-out"
        );
        assert!(left[399] > 0.0 && left[399] < 0.02);
    }
}
