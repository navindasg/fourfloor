//! The [`Device`] DSP trait: the interface every instrument, effect and
//! hosted-plugin wrapper presents to the engine.

use crate::event::{Event, EventBuffer};
use crate::id::ParamId;
use crate::param::ParamInfo;
use crate::time::{Beats, SamplePos, SampleRate, Tempo};

/// Transport state for one processing block. Describes the first frame of
/// the block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportContext {
    /// Sample rate the block is processed at.
    pub sample_rate: SampleRate,
    /// Tempo in effect for the whole block.
    pub tempo: Tempo,
    /// Whether the transport is rolling. When `false` the position does not
    /// advance between blocks, but devices are still processed (tails,
    /// live input).
    pub playing: bool,
    /// Timeline position of the block's first frame, in samples.
    pub position_samples: SamplePos,
    /// Timeline position of the block's first frame, in beats.
    pub position_beats: Beats,
}

impl TransportContext {
    /// A stopped transport at the start of the timeline.
    pub fn stopped(sample_rate: SampleRate, tempo: Tempo) -> Self {
        Self {
            sample_rate,
            tempo,
            playing: false,
            position_samples: 0,
            position_beats: Beats::ZERO,
        }
    }

    /// A rolling transport whose block starts at `position_samples`.
    pub fn playing_at(sample_rate: SampleRate, tempo: Tempo, position_samples: SamplePos) -> Self {
        Self {
            sample_rate,
            tempo,
            playing: true,
            position_samples,
            position_beats: Beats::from_sample_pos(position_samples, tempo, sample_rate),
        }
    }

    /// Beat position of frame `frame` within this block. When the transport
    /// is stopped this is the block's start position for every frame.
    pub fn beats_at_frame(&self, frame: usize) -> Beats {
        if self.playing {
            Beats(
                self.position_beats.0
                    + frame as f64 / self.tempo.samples_per_beat(self.sample_rate),
            )
        } else {
            self.position_beats
        }
    }
}

/// What a device is told before it processes: the rate, the largest block
/// and the channel count of every bus.
///
/// Build one with [`PrepareConfig::new`] or [`PrepareConfig::stereo`]; the
/// struct may grow fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct PrepareConfig {
    /// Sample rate blocks are processed at.
    pub sample_rate: SampleRate,
    /// Most frames one [`Device::process`] call carries.
    pub max_block: usize,
    /// Channels in every bus the device sees: its output, each input port
    /// and the sidechain. 2 (stereo) unless the engine is configured
    /// otherwise; never 0.
    pub channels: usize,
}

impl PrepareConfig {
    /// A configuration with an explicit channel count.
    pub const fn new(sample_rate: SampleRate, max_block: usize, channels: usize) -> Self {
        Self {
            sample_rate,
            max_block,
            channels,
        }
    }

    /// The default: stereo.
    pub const fn stereo(sample_rate: SampleRate, max_block: usize) -> Self {
        Self::new(sample_rate, max_block, 2)
    }
}

/// Everything one [`Device::process`] call reads and writes.
///
/// Audio is planar: one slice per channel, every slice the same length (the
/// block's frame count, at most [`PrepareConfig::max_block`]). Each bus is
/// [`PrepareConfig::channels`] wide.
///
/// `'a` is the lifetime of the block's borrows and `'b` that of the output
/// sample buffers; a device writes `io: &mut ProcessIo<'_, '_>` and never
/// names either.
pub struct ProcessIo<'a, 'b> {
    /// Main audio input. Empty for instruments and other sources. A device
    /// with several input ports (a mixer) sees them one after another:
    /// `inputs[port * channels + channel]`.
    pub inputs: &'a [&'a [f32]],
    /// Sidechain input bus, `channels` wide: the key signal for a
    /// compressor or ducker. `None` when nothing is routed to the device's
    /// sidechain; such a device should key from its main input instead.
    pub sidechain: Option<&'a [&'a [f32]]>,
    /// Audio output, `channels` wide. Holds unspecified data on entry; the
    /// device must write every frame of every channel.
    pub outputs: &'a mut [&'b mut [f32]],
    /// Note and controller events for this block, sorted by
    /// [`Event::frame`]. Every frame is less than the block's frame count.
    pub events: &'a [Event],
    /// Where a device that produces events (a clip player, an
    /// arpeggiator) writes them, with frames inside this block. Empty on
    /// entry. Devices that produce no events leave it alone.
    pub events_out: &'a mut EventBuffer,
}

impl ProcessIo<'_, '_> {
    /// Frames in this block.
    pub fn frames(&self) -> usize {
        self.outputs.first().map_or(0, |channel| channel.len())
    }

    /// Channels in the output bus.
    pub fn channels(&self) -> usize {
        self.outputs.len()
    }

    /// Writes silence to every output channel.
    pub fn silence_outputs(&mut self) {
        for channel in self.outputs.iter_mut() {
            channel.fill(0.0);
        }
    }

    /// Copies the first input port to the output, channel for channel.
    /// Output channels with no matching input are silenced. What a bypassed
    /// effect does.
    pub fn pass_through(&mut self) {
        for (index, output) in self.outputs.iter_mut().enumerate() {
            match self.inputs.get(index) {
                Some(input) => output.copy_from_slice(input),
                None => output.fill(0.0),
            }
        }
    }
}

/// A unit of DSP in the audio graph: an instrument, an effect, a note
/// effect or a hosted-plugin wrapper.
///
/// # Threading
///
/// [`prepare`](Device::prepare) and [`params`](Device::params) are called
/// off the audio thread and may allocate. [`process`](Device::process),
/// [`reset`](Device::reset) and [`latency_samples`](Device::latency_samples)
/// are called on the audio thread and must not allocate, lock, log, block
/// or panic. [`set_param`](Device::set_param) is called from both: off the
/// audio thread before `prepare` to set starting values, and on the audio
/// thread afterwards. It must never allocate, and values set before
/// `prepare` must survive it.
///
/// # Buses
///
/// Every bus is [`PrepareConfig::channels`] wide (stereo by default). An
/// instrument has no audio input and plays the notes in
/// [`ProcessIo::events`]. An effect reads [`ProcessIo::inputs`]. A device
/// that wants a key signal reads [`ProcessIo::sidechain`].
pub trait Device: Send {
    /// Gets ready to process. Allocate everything here: voices, delay
    /// lines, scratch buffers sized from `config`. May be called again when
    /// the rate, block size or channel count changes.
    fn prepare(&mut self, config: &PrepareConfig);

    /// Processes one block. See [`ProcessIo`] for what each buffer holds.
    ///
    /// Events are sample-accurate: an event at frame `n` takes effect
    /// before output frame `n` is computed.
    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>);

    /// Describes the device's parameters, in display order. The list is
    /// fixed for the life of the device. Default: no parameters.
    fn params(&self) -> &[ParamInfo] {
        &[]
    }

    /// Applies a parameter change before the next block. `value` is in the
    /// parameter's plain units. Unknown ids and values that are not finite
    /// are ignored; out-of-range values are clamped. Default: no-op.
    fn set_param(&mut self, _param: ParamId, _value: f32) {}

    /// Clears internal state (delay lines, envelopes, voices) without
    /// reallocating and without changing parameters, e.g. after a seek.
    /// Default: no-op.
    fn reset(&mut self) {}

    /// Delay this device adds, in samples, for delay compensation. Read
    /// once after [`prepare`](Device::prepare). Default: 0.
    fn latency_samples(&self) -> u32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use crate::param::ParamUnit;

    /// Minimal effect used to exercise the trait: per-channel gain.
    struct Gain {
        gain: f32,
        prepared: Option<PrepareConfig>,
    }

    const GAIN: ParamId = ParamId(0);
    static GAIN_PARAMS: [ParamInfo; 1] = [ParamInfo::new(
        GAIN,
        "Gain",
        0.0,
        4.0,
        1.0,
        ParamUnit::Generic,
    )];

    impl Device for Gain {
        fn prepare(&mut self, config: &PrepareConfig) {
            self.prepared = Some(*config);
        }

        fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
            for (out, inp) in io.outputs.iter_mut().zip(io.inputs) {
                for (o, i) in out.iter_mut().zip(inp.iter()) {
                    *o = i * self.gain;
                }
            }
        }

        fn params(&self) -> &[ParamInfo] {
            &GAIN_PARAMS
        }

        fn set_param(&mut self, param: ParamId, value: f32) {
            if param == GAIN {
                self.gain = value;
            }
        }
    }

    /// Instrument: outputs 1.0 from the frame of each note-on, 0.0 from
    /// each note-off, and echoes its events. Reports a fixed latency.
    struct Gate {
        open: bool,
    }

    impl Device for Gate {
        fn prepare(&mut self, _config: &PrepareConfig) {}

        fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
            let mut events = io.events.iter().peekable();
            for frame in 0..io.frames() {
                while let Some(event) = events.next_if(|event| event.frame as usize <= frame) {
                    self.open = event.kind.is_note_on();
                    io.events_out.push(*event);
                }
                for channel in io.outputs.iter_mut() {
                    channel[frame] = if self.open { 1.0 } else { 0.0 };
                }
            }
        }

        fn latency_samples(&self) -> u32 {
            64
        }
    }

    /// Ducks its input to zero wherever the sidechain is non-zero.
    struct Ducker;

    impl Device for Ducker {
        fn prepare(&mut self, _config: &PrepareConfig) {}

        fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
            let Some(key) = io.sidechain else {
                io.pass_through();
                return;
            };
            for ((out, inp), key) in io.outputs.iter_mut().zip(io.inputs).zip(key) {
                for ((o, i), k) in out.iter_mut().zip(inp.iter()).zip(key.iter()) {
                    *o = if *k == 0.0 { *i } else { 0.0 };
                }
            }
        }
    }

    fn ctx() -> TransportContext {
        TransportContext::stopped(SampleRate::HZ_48000, Tempo::DEFAULT)
    }

    #[test]
    fn processes_planar_stereo_through_a_trait_object() {
        let mut dev: Box<dyn Device> = Box::new(Gain {
            gain: 1.0,
            prepared: None,
        });
        dev.prepare(&PrepareConfig::stereo(SampleRate::HZ_48000, 4));
        dev.set_param(GAIN, 0.5);
        dev.set_param(ParamId(99), 123.0); // unknown id is ignored

        let (in_l, in_r) = ([1.0f32, 2.0, 3.0, 4.0], [-1.0f32, -2.0, -3.0, -4.0]);
        let (mut out_l, mut out_r) = ([9.0f32; 4], [9.0f32; 4]);
        let mut events_out = EventBuffer::with_capacity(4);
        {
            let mut io = ProcessIo {
                inputs: &[&in_l, &in_r],
                sidechain: None,
                outputs: &mut [&mut out_l, &mut out_r],
                events: &[],
                events_out: &mut events_out,
            };
            assert_eq!((io.frames(), io.channels()), (4, 2));
            dev.process(&ctx(), &mut io);
        }

        assert_eq!(out_l, [0.5, 1.0, 1.5, 2.0]);
        assert_eq!(out_r, [-0.5, -1.0, -1.5, -2.0]);
        assert_eq!(dev.latency_samples(), 0);
        assert_eq!(dev.params().len(), 1);
        assert_eq!(dev.params()[0].name, "Gain");
        dev.reset(); // default no-op must be callable
    }

    #[test]
    fn prepare_receives_rate_block_size_and_channels() {
        let mut gain = Gain {
            gain: 1.0,
            prepared: None,
        };
        gain.prepare(&PrepareConfig::stereo(SampleRate::HZ_44100, 512));
        let config = gain.prepared.unwrap();
        assert_eq!(config.sample_rate, SampleRate::HZ_44100);
        assert_eq!((config.max_block, config.channels), (512, 2));
        assert_eq!(PrepareConfig::new(SampleRate::HZ_48000, 64, 1).channels, 1);
    }

    #[test]
    fn instrument_follows_events_sample_accurately_and_can_emit_events() {
        let mut dev = Gate { open: false };
        dev.prepare(&PrepareConfig::new(SampleRate::HZ_48000, 8, 1));
        let mut out = [9.0f32; 8];
        let mut events_out = EventBuffer::with_capacity(4);
        let events = [Event::note_on(2, 60, 100), Event::note_off(5, 60)];
        dev.process(
            &ctx(),
            &mut ProcessIo {
                inputs: &[],
                sidechain: None,
                outputs: &mut [&mut out],
                events: &events,
                events_out: &mut events_out,
            },
        );
        assert_eq!(out, [0.0, 0.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
        assert_eq!(events_out.as_slice(), events);
        assert_eq!(events_out.as_slice()[0].kind, EventKind::note_on(60, 100));
        assert_eq!(dev.latency_samples(), 64);
        assert!(dev.params().is_empty());
    }

    #[test]
    fn sidechain_is_optional() {
        let mut dev = Ducker;
        let input = [1.0f32, 1.0, 1.0, 1.0];
        let key = [0.0f32, 0.5, 0.0, -0.5];
        let mut out = [9.0f32; 4];
        let mut events_out = EventBuffer::with_capacity(0);
        dev.process(
            &ctx(),
            &mut ProcessIo {
                inputs: &[&input],
                sidechain: Some(&[&key]),
                outputs: &mut [&mut out],
                events: &[],
                events_out: &mut events_out,
            },
        );
        assert_eq!(out, [1.0, 0.0, 1.0, 0.0]);

        dev.process(
            &ctx(),
            &mut ProcessIo {
                inputs: &[&input],
                sidechain: None,
                outputs: &mut [&mut out],
                events: &[],
                events_out: &mut events_out,
            },
        );
        assert_eq!(out, input);
    }

    #[test]
    fn io_helpers_silence_and_pass_through() {
        let input = [0.25f32; 3];
        let (mut l, mut r) = ([9.0f32; 3], [9.0f32; 3]);
        let mut events_out = EventBuffer::with_capacity(0);
        {
            let mut io = ProcessIo {
                inputs: &[&input],
                sidechain: None,
                outputs: &mut [&mut l, &mut r],
                events: &[],
                events_out: &mut events_out,
            };
            io.pass_through();
            io.silence_outputs();
            io.pass_through();
        }
        assert_eq!(l, [0.25; 3]);
        assert_eq!(r, [0.0; 3], "no matching input channel");
    }

    #[test]
    fn devices_are_send() {
        fn assert_send<T: Send + ?Sized>() {}
        assert_send::<dyn Device>();
        assert_send::<Box<dyn Device>>();
    }

    #[test]
    fn stopped_context_starts_at_zero() {
        let c = ctx();
        assert!(!c.playing);
        assert_eq!(c.position_samples, 0);
        assert_eq!(c.position_beats, Beats::ZERO);
        assert_eq!(c.beats_at_frame(1000), Beats::ZERO);
    }

    #[test]
    fn beats_at_frame_advances_while_playing() {
        // 120 BPM at 48 kHz: 24 000 samples per beat.
        let c = TransportContext {
            playing: true,
            position_samples: 48_000,
            position_beats: Beats(2.0),
            ..ctx()
        };
        assert_eq!(c.beats_at_frame(0), Beats(2.0));
        assert_eq!(c.beats_at_frame(12_000), Beats(2.5));
        let at = TransportContext::playing_at(SampleRate::HZ_48000, Tempo::DEFAULT, 48_000);
        assert_eq!(at, c);
    }
}
