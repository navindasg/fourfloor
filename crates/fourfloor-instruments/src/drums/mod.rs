//! Synthesized drum voices in the style of the TR-909 and TR-808, and the
//! [`DrumKit`] that plays them from General MIDI drum pitches.
//!
//! Every voice is synthesized: no samples are used. (The real 909 hats and
//! cymbals are samples; the hat here is the 808-style bank of six square
//! oscillators through filters, voiced short and bright like a 909.)

mod clap;
mod hat;
mod kick;
mod kit;
mod perc;
mod snare;

pub use clap::ClapVoice;
pub use hat::HatVoice;
pub use kick::{Kick808Voice, Kick909Voice};
pub use kit::{DrumKit, KitSlot};
pub use perc::{Cowbell808Voice, RimVoice, TomVoice};
pub use snare::Snare909Voice;

use fourfloor_core::{
    velocity_to_unit, Device, EventKind, ParamId, ParamInfo, PrepareConfig, ProcessIo,
    TransportContext,
};

use crate::runtime::{clamped, run, Synth};

/// One drum sound: triggered by a note-on, rings out by itself.
///
/// Voices are mono. Parameter ids are local to the voice and start at 0.
pub trait DrumVoice: Send + 'static {
    /// The voice's parameters, in display order.
    fn param_table() -> &'static [ParamInfo]
    where
        Self: Sized;
    /// Applies a parameter. `value` is already clamped to its range.
    fn set_param(&mut self, id: ParamId, value: f32);
    /// Sets the sample rate. Parameters keep their values.
    fn prepare(&mut self, sample_rate: f32);
    /// Starts the sound. `pitch` is the MIDI pitch that triggered it and
    /// `velocity` is 0 to 1.
    fn trigger(&mut self, pitch: u8, velocity: f32);
    /// The next sample.
    fn tick(&mut self) -> f32;
    /// Stops the sound at once and clears filters.
    fn reset(&mut self);
}

/// A [`Device`] that plays a single [`DrumVoice`] on every note-on.
/// Note-offs are ignored: drum voices ring out.
#[derive(Clone, Debug, Default)]
pub struct Drum<V> {
    voice: V,
}

impl<V: DrumVoice + Default> Drum<V> {
    /// A drum with default parameters.
    pub fn new() -> Self {
        Self::default()
    }
}

impl<V: DrumVoice> Synth for Drum<V> {
    fn handle_event(&mut self, kind: EventKind) {
        if let EventKind::NoteOn {
            pitch, velocity, ..
        } = kind
        {
            if velocity > 0 {
                self.voice.trigger(pitch, velocity_to_unit(velocity));
            }
        }
    }

    fn tick(&mut self) -> (f32, f32) {
        let sample = self.voice.tick();
        (sample, sample)
    }
}

impl<V: DrumVoice> Device for Drum<V> {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.voice.prepare(config.sample_rate.hz() as f32);
        self.voice.reset();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        run(self, io);
    }

    fn params(&self) -> &[ParamInfo] {
        V::param_table()
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if let Some(value) = clamped(V::param_table(), param, value) {
            self.voice.set_param(param, value);
        }
    }

    fn reset(&mut self) {
        self.voice.reset();
    }
}

/// 909-style kick drum. Kind `drum.kick909`.
pub type Kick909 = Drum<Kick909Voice>;
/// 808-style kick drum. Kind `drum.kick808`.
pub type Kick808 = Drum<Kick808Voice>;
/// 909-style snare. Kind `drum.snare909`.
pub type Snare909 = Drum<Snare909Voice>;
/// 909-style hand clap. Kind `drum.clap909`.
pub type Clap909 = Drum<ClapVoice>;
/// Closed and open hi-hat sharing one voice, so a closed hit chokes an
/// open one. Pitch 46 (and 26, its GM neighbour) plays open; every other
/// pitch plays closed. Kind `drum.hat909`.
pub type Hat909 = Drum<HatVoice>;
/// 909-style rim shot. Kind `drum.rim909`.
pub type Rim909 = Drum<RimVoice>;
/// 909-style tom. GM tom pitches (41, 43, 45, 47, 48, 50) play it at six
/// tunings; any other pitch plays the set tuning. Kind `drum.tom909`.
pub type Tom909 = Drum<TomVoice>;
/// 808-style cowbell. Kind `drum.cowbell808`.
pub type Cowbell808 = Drum<Cowbell808Voice>;

/// Default seed for drum noise sources. Each voice offsets it so two
/// voices never share a noise sequence.
pub(crate) const NOISE_SEED: u32 = 0x2545_F491;
