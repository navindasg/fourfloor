//! A deck effect slot: any audio effect from the device registry (the
//! `fourfloor-effects` devices) with an on/off switch that crossfades
//! between the dry signal and the effect.

use fourfloor_core::{
    Device, EventBuffer, ParamId, ParamInfo, PrepareConfig, ProcessIo, TransportContext,
};

use crate::inner::{at, process_into};

/// Crossfade time of the on/off switch, in seconds.
const SWITCH_SECONDS: f32 = 0.01;

/// An effect slot (see the [module docs](self)). The wrapped effect's
/// parameters keep their ids; the switch is [`FxSlot::ENABLED`].
pub struct FxSlot {
    kind: String,
    effect: Box<dyn Device>,
    params: Vec<ParamInfo>,
    enabled: bool,
    mix: f32,
    step: f32,
    stride: usize,
    wet: Vec<f32>,
    /// Dry signal delayed by the effect's latency, per channel.
    dry: [Vec<f32>; 2],
    dry_write: usize,
    latency: usize,
    events: EventBuffer,
}

impl FxSlot {
    /// The on/off switch (a toggle). Chosen above any effect's own ids.
    pub const ENABLED: ParamId = ParamId(10_000);

    /// A slot around `effect` (registry kind `kind`), switched off.
    pub fn new(kind: impl Into<String>, effect: Box<dyn Device>) -> Self {
        let mut params = effect.params().to_vec();
        params.push(ParamInfo::toggle(Self::ENABLED, "Enabled", false));
        Self {
            kind: kind.into(),
            effect,
            params,
            enabled: false,
            mix: 0.0,
            step: 1.0,
            stride: 0,
            wet: Vec::new(),
            dry: [Vec::new(), Vec::new()],
            dry_write: 0,
            latency: 0,
            events: EventBuffer::with_capacity(64),
        }
    }

    /// The registry kind of the wrapped effect.
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

impl Device for FxSlot {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.effect.prepare(config);
        self.stride = config.max_block;
        self.wet = vec![0.0; config.max_block * config.channels.max(2)];
        self.latency = self.effect.latency_samples() as usize;
        let length = self.latency + 1;
        self.dry = [vec![0.0; length], vec![0.0; length]];
        self.dry_write = 0;
        self.step = 1.0 / (SWITCH_SECONDS * config.sample_rate.as_f64() as f32).max(1.0);
        self.mix = if self.enabled { 1.0 } else { 0.0 };
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let frames = io.frames();
        if self.stride < frames || io.outputs.len() < 2 || io.inputs.len() < 2 {
            io.pass_through();
            return;
        }
        let channels = io.outputs.len().min(self.wet.len() / self.stride);
        let stride = self.stride;
        process_into(
            self.effect.as_mut(),
            ctx,
            &io.inputs[..io.inputs.len().min(channels)],
            &mut self.wet[..channels * stride],
            stride,
            frames,
            &mut self.events,
        );
        self.events.clear();
        let target = if self.enabled { 1.0 } else { 0.0 };
        let length = self.dry[0].len();
        for index in 0..frames {
            if self.mix < target {
                self.mix = (self.mix + self.step).min(target);
            } else if self.mix > target {
                self.mix = (self.mix - self.step).max(target);
            }
            let read = (self.dry_write + 1) % length;
            for channel in 0..2 {
                let line = &mut self.dry[channel];
                line[self.dry_write] = io.inputs[channel][index];
                let dry = line[read];
                let wet = at(&self.wet, stride, channel, index);
                io.outputs[channel][index] = dry + (wet - dry) * self.mix;
            }
            self.dry_write = read;
        }
        for channel in io.outputs.iter_mut().skip(2) {
            channel.fill(0.0);
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &self.params
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if param == Self::ENABLED {
            self.enabled = value.is_finite() && value >= 0.5;
        } else {
            self.effect.set_param(param, value);
        }
    }

    fn reset(&mut self) {
        self.effect.reset();
        for line in &mut self.dry {
            line.fill(0.0);
        }
        self.mix = if self.enabled { 1.0 } else { 0.0 };
    }

    fn latency_samples(&self) -> u32 {
        self.latency as u32
    }
}
