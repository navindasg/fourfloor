//! Offline rendering: drives an [`AudioProcessor`] without an audio device.
//! It goes through the same callback entry point as the realtime backend, so
//! what the tests render is what the sound card would get.

use crate::engine::AudioProcessor;

/// Renders `blocks` callbacks of `block_frames` frames each and returns the
/// interleaved result (`blocks * block_frames * processor.channels()`
/// samples).
///
/// The output buffer is allocated up front; each callback then runs under
/// the engine's allocation guard exactly as it would on the audio thread.
pub fn render(processor: &mut AudioProcessor, blocks: usize, block_frames: usize) -> Vec<f32> {
    render_frames(processor, blocks * block_frames, block_frames)
}

/// Renders exactly `frames` frames in callbacks of at most `block_frames`
/// frames (the last may be shorter). Returns interleaved samples.
pub fn render_frames(
    processor: &mut AudioProcessor,
    frames: usize,
    block_frames: usize,
) -> Vec<f32> {
    let channels = processor.channels();
    let mut out = vec![0.0f32; frames * channels];
    if block_frames > 0 && channels > 0 {
        for block in out.chunks_mut(block_frames * channels) {
            processor.process_interleaved(block, channels);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{Metronome, Mixer, SineOsc};
    use crate::engine::{engine, EngineConfig};
    use crate::graph::GraphBuilder;
    use fourfloor_core::{DeviceId, SampleRate};

    fn processor(max_block: usize) -> AudioProcessor {
        let config = EngineConfig {
            max_block,
            ..EngineConfig::new(SampleRate::HZ_44100)
        };
        let mut b = GraphBuilder::new(2);
        b.add_node(DeviceId(1), Box::new(SineOsc::new(220.0, 0.3)), 0)
            .unwrap();
        b.add_node(DeviceId(2), Box::new(Metronome::new(0.5)), 0)
            .unwrap();
        b.add_node(DeviceId(3), Box::new(Mixer::new(2)), 2).unwrap();
        b.connect(DeviceId(1), DeviceId(3), 0).unwrap();
        b.connect(DeviceId(2), DeviceId(3), 1).unwrap();
        b.set_output(DeviceId(3)).unwrap();
        let graph = b.compile(config.sample_rate, max_block).unwrap();
        let (mut controller, processor) = engine(config, graph).unwrap();
        controller.play().unwrap();
        processor
    }

    #[test]
    fn render_returns_blocks_times_frames_times_channels() {
        let out = render(&mut processor(64), 10, 100);
        assert_eq!(out.len(), 10 * 100 * 2);
        assert!(out.iter().any(|s| *s != 0.0));
        assert!(render(&mut processor(64), 3, 0).is_empty());
    }

    #[test]
    fn result_does_not_depend_on_callback_or_engine_block_size() {
        let reference = render_frames(&mut processor(512), 30_000, 30_000);
        assert_eq!(reference.len(), 60_000);
        for (max_block, callback) in [(512, 64), (64, 512), (1, 333), (100, 7)] {
            let out = render_frames(&mut processor(max_block), 30_000, callback);
            assert_eq!(out, reference, "max_block {max_block}, callback {callback}");
        }
    }
}
