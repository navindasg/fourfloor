//! Instruments: devices that turn note events into audio.
//!
//! Every instrument implements [`fourfloor_core::Device`], reads
//! `ProcessIo::events` and is added to the registry by [`register`] under
//! a stable kind:
//!
//! | Kind | Device |
//! | --- | --- |
//! | `drum.kick909`, `drum.kick808` | [`Kick909`], [`Kick808`] |
//! | `drum.snare909`, `drum.clap909` | [`Snare909`], [`Clap909`] |
//! | `drum.hat909` | [`Hat909`] (closed chokes open) |
//! | `drum.rim909`, `drum.tom909`, `drum.cowbell808` | [`Rim909`], [`Tom909`], [`Cowbell808`] |
//! | `drum.kit` | [`DrumKit`] (General MIDI drum pitches) |
//! | `synth.acid303` | [`Acid303`] |
//! | `fm.organ`, `fm.stab` | [`FmSynth`] |
//! | `poly.pad` | [`PolyPad`] |
//! | `sampler` | [`Sampler`] |
//!
//! Nothing here allocates, locks or logs while processing.

#![warn(missing_docs)]

pub mod acid;
pub mod drums;
mod dsp;
pub mod fm;
pub mod pad;
mod runtime;
pub mod sampler;

pub use fourfloor_core as core;

pub use acid::Acid303;
pub use drums::{
    Clap909, Cowbell808, Drum, DrumKit, DrumVoice, Hat909, Kick808, Kick909, KitSlot, Rim909,
    Snare909, Tom909,
};
pub use fm::FmSynth;
pub use pad::PolyPad;
pub use sampler::{detect_slices, SampleData, SampleError, SampleLoader, Sampler, SliceOptions};

use std::sync::Arc;

use fourfloor_core::{
    Device, DeviceCategory, DeviceDescriptor, DeviceError, DeviceRegistry, RegistryError,
};

/// Kind of the sampler in the registry.
pub const SAMPLER_KIND: &str = "sampler";

/// Every kind [`register`] adds, in the order it adds them.
pub const KINDS: [&str; 14] = [
    "drum.kick909",
    "drum.kick808",
    "drum.snare909",
    "drum.clap909",
    "drum.hat909",
    "drum.rim909",
    "drum.tom909",
    "drum.cowbell808",
    "drum.kit",
    "synth.acid303",
    "fm.organ",
    "fm.stab",
    "poly.pad",
    SAMPLER_KIND,
];

/// Adds this crate's devices to `registry`. The command line and the app
/// call this at start-up.
///
/// The sampler takes its sample from the device's saved state: the bytes
/// of an audio file (WAV, FLAC, AIFF and whatever else `fourfloor-audio`
/// decodes). With no state it is built empty and silent; state that does
/// not decode fails the build. Its slices (for slice mode) are the onsets
/// found by [`detect_slices`] with the default [`SliceOptions`].
pub fn register(registry: &mut DeviceRegistry) -> Result<(), RegistryError> {
    use DeviceCategory::Instrument;
    registry.register_device("drum.kick909", "Kick 909", Instrument, Kick909::new)?;
    registry.register_device("drum.kick808", "Kick 808", Instrument, Kick808::new)?;
    registry.register_device("drum.snare909", "Snare 909", Instrument, Snare909::new)?;
    registry.register_device("drum.clap909", "Clap 909", Instrument, Clap909::new)?;
    registry.register_device("drum.hat909", "Hat 909", Instrument, Hat909::new)?;
    registry.register_device("drum.rim909", "Rim 909", Instrument, Rim909::new)?;
    registry.register_device("drum.tom909", "Tom 909", Instrument, Tom909::new)?;
    registry.register_device(
        "drum.cowbell808",
        "Cowbell 808",
        Instrument,
        Cowbell808::new,
    )?;
    registry.register_device("drum.kit", "Drum Kit", Instrument, DrumKit::new)?;
    registry.register_device("synth.acid303", "Acid 303", Instrument, Acid303::new)?;
    registry.register_device("fm.organ", "FM Organ Bass", Instrument, FmSynth::organ)?;
    registry.register_device("fm.stab", "FM Stab", Instrument, FmSynth::stab)?;
    registry.register_device("poly.pad", "Poly Pad", Instrument, PolyPad::new)?;
    registry.register(
        DeviceDescriptor::describe(SAMPLER_KIND, "Sampler", Instrument, &Sampler::new()),
        |init| {
            let device: Box<dyn Device> = match init.state {
                None => Box::new(Sampler::new()),
                Some(bytes) => {
                    let sample = SampleData::decode(bytes.to_vec(), None)
                        .map_err(|error| DeviceError::new(error.to_string()))?
                        .with_detected_slices(&SliceOptions::default());
                    Box::new(Sampler::with_sample(Arc::new(sample)))
                }
            };
            Ok(device)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_every_kind_with_parameters() {
        let mut registry = DeviceRegistry::new();
        register(&mut registry).unwrap();
        assert_eq!(registry.len(), KINDS.len());
        for kind in KINDS {
            let descriptor = registry.descriptor(kind).unwrap();
            assert_eq!(descriptor.category, DeviceCategory::Instrument);
            assert!(!descriptor.params.is_empty(), "{kind} has no parameters");
            for info in &descriptor.params {
                assert!(info.min < info.max, "{kind} {}", info.name);
                assert!(info.contains(info.default), "{kind} {}", info.name);
            }
            registry.create_default(kind).unwrap();
        }
    }

    #[test]
    fn registering_twice_reports_the_duplicate() {
        let mut registry = DeviceRegistry::new();
        register(&mut registry).unwrap();
        assert!(matches!(
            register(&mut registry),
            Err(RegistryError::DuplicateKind(_))
        ));
    }

    #[test]
    fn the_sampler_rejects_state_that_is_not_audio() {
        let mut registry = DeviceRegistry::new();
        register(&mut registry).unwrap();
        let init = fourfloor_core::DeviceInit::new().with_state(&[1, 2, 3]);
        assert!(matches!(
            registry.create(SAMPLER_KIND, &init),
            Err(RegistryError::Build { .. })
        ));
    }
}
