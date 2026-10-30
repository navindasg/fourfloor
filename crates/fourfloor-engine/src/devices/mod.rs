//! Built-in devices: the plumbing every project graph needs (gain, fader,
//! mixer, note player) plus test sources and a placeholder instrument.
//!
//! All of them follow the graph's bus convention: every bus is
//! `PrepareConfig::channels` wide and input ports are laid out as
//! `inputs[port * channels + channel]`.
//!
//! [`register`] adds the ones a project can name to a
//! [`DeviceRegistry`]:
//!
//! | Kind | Device | Category |
//! | --- | --- | --- |
//! | `builtin.gain` | [`Gain`] | effect |
//! | `builtin.fader` | [`Fader`] | effect |
//! | `builtin.sine` | [`SineOsc`] | instrument (free-running test tone) |
//! | `builtin.metronome` | [`Metronome`] | instrument (follows the transport) |
//! | `builtin.placeholder` | [`PlaceholderSynth`] | instrument |
//! | `note.arp` | [`Arpeggiator`] | note effect |
//! | `note.chord` | [`Chord`] | note effect |
//!
//! [`Mixer`], [`NotePlayer`] and [`LatentDelay`] take constructor arguments
//! and are built directly by the project compiler and by tests.

mod arpeggiator;
mod audio_clip_player;
mod chord;
mod clip_launcher;
mod fader;
mod gain;
mod latent_delay;
mod metronome;
mod mixer;
mod note_player;
mod placeholder_synth;
mod sine;

use fourfloor_core::{DeviceCategory, DeviceRegistry, RegistryError};

pub use arpeggiator::{ArpMode, Arpeggiator};
pub use audio_clip_player::{AudioClipPlayer, AudioRegion, AudioSource};
pub use chord::Chord;
pub use clip_launcher::{ClipLauncher, LauncherClip};
pub use fader::Fader;
pub use gain::Gain;
pub use latent_delay::LatentDelay;
pub use metronome::Metronome;
pub use mixer::Mixer;
pub use note_player::{NotePlayer, PlayerNote};
pub use placeholder_synth::PlaceholderSynth;
pub use sine::SineOsc;

/// Registry kind of [`Gain`].
pub const KIND_GAIN: &str = "builtin.gain";
/// Registry kind of [`Fader`].
pub const KIND_FADER: &str = "builtin.fader";
/// Registry kind of [`SineOsc`].
pub const KIND_SINE: &str = "builtin.sine";
/// Registry kind of [`Metronome`].
pub const KIND_METRONOME: &str = "builtin.metronome";
/// Registry kind of [`PlaceholderSynth`].
pub const KIND_PLACEHOLDER: &str = "builtin.placeholder";
/// Registry kind of [`Arpeggiator`].
pub const KIND_ARP: &str = "note.arp";
/// Registry kind of [`Chord`].
pub const KIND_CHORD: &str = "note.chord";

/// Adds the engine's built-in devices to `registry`.
pub fn register(registry: &mut DeviceRegistry) -> Result<(), RegistryError> {
    registry.register_device(KIND_GAIN, "Gain", DeviceCategory::Effect, || Gain::new(1.0))?;
    registry.register_device(KIND_FADER, "Fader", DeviceCategory::Effect, Fader::default)?;
    registry.register_device(
        KIND_ARP,
        "Arpeggiator",
        DeviceCategory::NoteEffect,
        Arpeggiator::default,
    )?;
    registry.register_device(
        KIND_CHORD,
        "Chord",
        DeviceCategory::NoteEffect,
        Chord::default,
    )?;
    registry.register_device(KIND_SINE, "Sine", DeviceCategory::Instrument, || {
        SineOsc::new(440.0, 0.5)
    })?;
    registry.register_device(
        KIND_METRONOME,
        "Metronome",
        DeviceCategory::Instrument,
        || Metronome::new(0.5),
    )?;
    registry.register_device(
        KIND_PLACEHOLDER,
        "Placeholder Synth",
        DeviceCategory::Instrument,
        || PlaceholderSynth::new(PlaceholderSynth::DEFAULT_LEVEL),
    )
}

#[cfg(test)]
pub(crate) mod test_util {
    use fourfloor_core::{
        Device, Event, EventBuffer, ProcessIo, SampleRate, Tempo, TransportContext,
    };

    pub const RATE: SampleRate = SampleRate::HZ_48000;

    pub fn stopped() -> TransportContext {
        TransportContext::stopped(RATE, Tempo::DEFAULT)
    }

    /// Runs one block and returns the events the device emitted.
    pub fn run(
        device: &mut dyn Device,
        ctx: &TransportContext,
        inputs: &[&[f32]],
        sidechain: Option<&[&[f32]]>,
        outputs: &mut [&mut [f32]],
        events: &[Event],
    ) -> Vec<Event> {
        let mut events_out = EventBuffer::with_capacity(256);
        device.process(
            ctx,
            &mut ProcessIo {
                inputs,
                sidechain,
                outputs,
                events,
                events_out: &mut events_out,
            },
        );
        events_out.as_slice().to_vec()
    }

    /// Runs one block of a device with no inputs and no events.
    pub fn run_source(device: &mut dyn Device, ctx: &TransportContext, outputs: &mut [&mut [f32]]) {
        run(device, ctx, &[], None, outputs, &[]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_adds_every_builtin_with_its_parameters() {
        let mut registry = DeviceRegistry::new();
        register(&mut registry).unwrap();
        let kinds: Vec<&str> = registry.kinds().collect();
        assert_eq!(
            kinds,
            [
                KIND_FADER,
                KIND_GAIN,
                KIND_METRONOME,
                KIND_PLACEHOLDER,
                KIND_SINE,
                KIND_ARP,
                KIND_CHORD
            ]
        );
        let fader = registry.descriptor(KIND_FADER).unwrap();
        assert_eq!(fader.category, DeviceCategory::Effect);
        assert_eq!(fader.param_by_name("pan").unwrap().id, Fader::PAN);
        assert_eq!(
            registry.descriptor(KIND_PLACEHOLDER).unwrap().category,
            DeviceCategory::Instrument
        );
        for kind in kinds {
            assert!(registry.create_default(kind).is_ok(), "{kind}");
        }
        assert!(register(&mut registry).is_err(), "registering twice fails");
    }
}
