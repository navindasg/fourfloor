//! The device registry: maps a stable string *kind* (for example
//! `"drum.kick909"`) to a factory and a description of the device.
//!
//! The project file stores a kind for every device in a track's chain. The
//! project compiler looks the kind up here to build the running
//! [`Device`], and the UI and the agent API list the registry to find out
//! which devices exist and what parameters they have.
//!
//! # Registering devices
//!
//! A crate that defines devices exposes one function that adds them:
//!
//! ```
//! use fourfloor_core::{
//!     Device, DeviceCategory, DeviceRegistry, PrepareConfig, ProcessIo, RegistryError,
//!     TransportContext,
//! };
//!
//! #[derive(Default)]
//! struct Silence;
//!
//! impl Device for Silence {
//!     fn prepare(&mut self, _config: &PrepareConfig) {}
//!     fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
//!         io.silence_outputs();
//!     }
//! }
//!
//! /// Adds this crate's devices to `registry`.
//! pub fn register(registry: &mut DeviceRegistry) -> Result<(), RegistryError> {
//!     registry.register_device("example.silence", "Silence", DeviceCategory::Instrument, || {
//!         Silence
//!     })
//! }
//!
//! let mut registry = DeviceRegistry::new();
//! register(&mut registry).unwrap();
//! let device = registry.create_default("example.silence").unwrap();
//! assert!(device.params().is_empty());
//! ```
//!
//! # Kind names
//!
//! A kind is `<family>.<name>` in lowercase ASCII, for example
//! `drum.kick909`, `synth.acid303`, `fx.compressor`, `note.arp` or
//! `builtin.gain`. Hosted plugins use `<format>:<plugin id>`, for example
//! `clap:com.example.synth`. A kind is saved in project files, so it must
//! never change once released.

use std::collections::BTreeMap;
use std::fmt;

use crate::device::Device;
use crate::id::ParamId;
use crate::param::ParamInfo;

/// What role a device plays in a track's chain. The project compiler uses
/// it to decide how to wire the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum DeviceCategory {
    /// Turns note events into audio. Has no audio input.
    Instrument,
    /// Processes audio: reads its input, writes its output.
    Effect,
    /// Transforms note events (arpeggiator, chord). Sits before the
    /// instrument; its audio output is not used.
    NoteEffect,
}

impl fmt::Display for DeviceCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Instrument => "instrument",
            Self::Effect => "effect",
            Self::NoteEffect => "note effect",
        })
    }
}

/// Describes one registered device kind.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DeviceDescriptor {
    /// The stable kind string projects store.
    pub kind: String,
    /// Display name.
    pub name: String,
    /// The device's role in a chain.
    pub category: DeviceCategory,
    /// The device's parameters, in display order.
    pub params: Vec<ParamInfo>,
    /// Whether the device reads a sidechain input
    /// ([`ProcessIo::sidechain`](crate::ProcessIo::sidechain)).
    pub sidechain: bool,
}

impl DeviceDescriptor {
    /// A descriptor with no parameters and no sidechain.
    pub fn new(kind: impl Into<String>, name: impl Into<String>, category: DeviceCategory) -> Self {
        Self {
            kind: kind.into(),
            name: name.into(),
            category,
            params: Vec::new(),
            sidechain: false,
        }
    }

    /// A descriptor whose parameters are read from `device`.
    pub fn describe(
        kind: impl Into<String>,
        name: impl Into<String>,
        category: DeviceCategory,
        device: &dyn Device,
    ) -> Self {
        Self::new(kind, name, category).with_params(device.params().to_vec())
    }

    /// The same descriptor with this parameter list.
    pub fn with_params(mut self, params: Vec<ParamInfo>) -> Self {
        self.params = params;
        self
    }

    /// The same descriptor, marked as reading a sidechain input.
    pub fn with_sidechain(mut self) -> Self {
        self.sidechain = true;
        self
    }

    /// Looks up a parameter by id.
    pub fn param(&self, id: ParamId) -> Option<&ParamInfo> {
        self.params.iter().find(|param| param.id == id)
    }

    /// Looks up a parameter by display name, ignoring ASCII case.
    pub fn param_by_name(&self, name: &str) -> Option<&ParamInfo> {
        self.params
            .iter()
            .find(|param| param.name.eq_ignore_ascii_case(name))
    }
}

/// What a factory is given to build one device instance.
///
/// Build one with [`DeviceInit::new`]; the struct may grow fields.
#[derive(Clone, Copy, Debug, Default)]
#[non_exhaustive]
pub struct DeviceInit<'a> {
    /// Starting parameter values, in plain units. The registry applies
    /// these with [`Device::set_param`] after the factory returns, so a
    /// factory only needs to read them if construction depends on one.
    pub params: &'a [(ParamId, f32)],
    /// Opaque saved state (a hosted plugin's state chunk, a sampler's
    /// sample reference), if the project has one for this device.
    pub state: Option<&'a [u8]>,
}

impl<'a> DeviceInit<'a> {
    /// No parameter overrides and no state: the device's defaults.
    pub const fn new() -> Self {
        Self {
            params: &[],
            state: None,
        }
    }

    /// The same request with these starting parameter values.
    pub const fn with_params(mut self, params: &'a [(ParamId, f32)]) -> Self {
        self.params = params;
        self
    }

    /// The same request with this saved state.
    pub const fn with_state(mut self, state: &'a [u8]) -> Self {
        self.state = Some(state);
        self
    }
}

/// Why a factory could not build a device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceError {
    message: String,
}

impl DeviceError {
    /// An error with a message for the user.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DeviceError {}

/// Why a registry call failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegistryError {
    /// The kind is empty or contains whitespace.
    InvalidKind(String),
    /// A device with this kind is already registered.
    DuplicateKind(String),
    /// No device with this kind is registered.
    UnknownKind(String),
    /// Two parameters in the descriptor share an id.
    DuplicateParam {
        /// The kind being registered.
        kind: String,
        /// The repeated parameter id.
        param: ParamId,
    },
    /// The factory failed.
    Build {
        /// The kind being built.
        kind: String,
        /// What went wrong.
        source: DeviceError,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKind(kind) => {
                write!(f, "\"{kind}\" is not a valid device kind")
            }
            Self::DuplicateKind(kind) => {
                write!(f, "device kind \"{kind}\" is already registered")
            }
            Self::UnknownKind(kind) => write!(f, "unknown device kind \"{kind}\""),
            Self::DuplicateParam { kind, param } => {
                write!(f, "device kind \"{kind}\" lists parameter {param} twice")
            }
            Self::Build { kind, source } => {
                write!(f, "cannot build device \"{kind}\": {source}")
            }
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Build { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Builds one device instance. Runs off the audio thread and may allocate.
pub type DeviceFactory =
    Box<dyn Fn(&DeviceInit<'_>) -> Result<Box<dyn Device>, DeviceError> + Send + Sync>;

struct Entry {
    descriptor: DeviceDescriptor,
    factory: DeviceFactory,
}

/// The set of device kinds a program can instantiate.
///
/// It is an ordinary value, not a global: the program builds one at start
/// by calling each device crate's `register` function, then shares it
/// (it is `Send + Sync`, so an `Arc` works).
#[derive(Default)]
pub struct DeviceRegistry {
    entries: BTreeMap<String, Entry>,
}

impl fmt::Debug for DeviceRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceRegistry")
            .field("kinds", &self.entries.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl DeviceRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a device kind with an explicit descriptor and factory.
    ///
    /// Fails when the kind is empty, contains whitespace or is already
    /// registered, or when the descriptor repeats a parameter id.
    pub fn register<F>(
        &mut self,
        descriptor: DeviceDescriptor,
        factory: F,
    ) -> Result<(), RegistryError>
    where
        F: Fn(&DeviceInit<'_>) -> Result<Box<dyn Device>, DeviceError> + Send + Sync + 'static,
    {
        let kind = &descriptor.kind;
        if kind.is_empty() || kind.chars().any(char::is_whitespace) {
            return Err(RegistryError::InvalidKind(kind.clone()));
        }
        if self.entries.contains_key(kind) {
            return Err(RegistryError::DuplicateKind(kind.clone()));
        }
        for (index, param) in descriptor.params.iter().enumerate() {
            if descriptor.params[..index]
                .iter()
                .any(|earlier| earlier.id == param.id)
            {
                return Err(RegistryError::DuplicateParam {
                    kind: kind.clone(),
                    param: param.id,
                });
            }
        }
        self.entries.insert(
            kind.clone(),
            Entry {
                descriptor,
                factory: Box::new(factory),
            },
        );
        Ok(())
    }

    /// Adds a device kind from a constructor. The parameter list is read
    /// from one instance built with `make`. Use
    /// [`register`](DeviceRegistry::register) for a device that reads a
    /// sidechain, needs saved state or can fail to build.
    pub fn register_device<D, F>(
        &mut self,
        kind: &str,
        name: &str,
        category: DeviceCategory,
        make: F,
    ) -> Result<(), RegistryError>
    where
        D: Device + 'static,
        F: Fn() -> D + Send + Sync + 'static,
    {
        let descriptor = DeviceDescriptor::describe(kind, name, category, &make());
        self.register(descriptor, move |_init| Ok(Box::new(make())))
    }

    /// Whether `kind` is registered.
    pub fn contains(&self, kind: &str) -> bool {
        self.entries.contains_key(kind)
    }

    /// The descriptor for `kind`.
    pub fn descriptor(&self, kind: &str) -> Option<&DeviceDescriptor> {
        self.entries.get(kind).map(|entry| &entry.descriptor)
    }

    /// Every descriptor, sorted by kind.
    pub fn descriptors(&self) -> impl Iterator<Item = &DeviceDescriptor> {
        self.entries.values().map(|entry| &entry.descriptor)
    }

    /// Every registered kind, sorted.
    pub fn kinds(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Number of registered kinds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Builds a device of `kind`, then applies `init.params` with
    /// [`Device::set_param`]. The device is not prepared yet. Allocates;
    /// call it off the audio thread.
    pub fn create(
        &self,
        kind: &str,
        init: &DeviceInit<'_>,
    ) -> Result<Box<dyn Device>, RegistryError> {
        let entry = self
            .entries
            .get(kind)
            .ok_or_else(|| RegistryError::UnknownKind(kind.to_owned()))?;
        let mut device = (entry.factory)(init).map_err(|source| RegistryError::Build {
            kind: kind.to_owned(),
            source,
        })?;
        for &(param, value) in init.params {
            device.set_param(param, value);
        }
        Ok(device)
    }

    /// Builds a device of `kind` with its default parameters.
    pub fn create_default(&self, kind: &str) -> Result<Box<dyn Device>, RegistryError> {
        self.create(kind, &DeviceInit::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{PrepareConfig, ProcessIo, TransportContext};
    use crate::event::EventBuffer;
    use crate::param::ParamUnit;
    use crate::time::{SampleRate, Tempo};

    const LEVEL: ParamId = ParamId(0);
    static PARAMS: [ParamInfo; 1] = [ParamInfo::new(
        LEVEL,
        "Level",
        0.0,
        1.0,
        0.5,
        ParamUnit::Generic,
    )];

    /// Writes a constant level.
    struct Dc {
        level: f32,
    }

    impl Device for Dc {
        fn prepare(&mut self, _config: &PrepareConfig) {}

        fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
            for channel in io.outputs.iter_mut() {
                channel.fill(self.level);
            }
        }

        fn params(&self) -> &[ParamInfo] {
            &PARAMS
        }

        fn set_param(&mut self, param: ParamId, value: f32) {
            if param == LEVEL {
                self.level = PARAMS[0].clamp(value);
            }
        }
    }

    fn output_of(device: &mut dyn Device) -> f32 {
        device.prepare(&PrepareConfig::new(SampleRate::HZ_48000, 4, 1));
        let mut out = [9.0f32; 4];
        let mut events_out = EventBuffer::with_capacity(0);
        device.process(
            &TransportContext::stopped(SampleRate::HZ_48000, Tempo::DEFAULT),
            &mut ProcessIo {
                inputs: &[],
                sidechain: None,
                outputs: &mut [&mut out],
                events: &[],
                events_out: &mut events_out,
            },
        );
        out[0]
    }

    fn registry() -> DeviceRegistry {
        let mut registry = DeviceRegistry::new();
        registry
            .register_device("test.dc", "DC", DeviceCategory::Instrument, || Dc {
                level: 0.5,
            })
            .unwrap();
        registry
    }

    #[test]
    fn registered_devices_are_described_and_instantiated() {
        let registry = registry();
        assert!(registry.contains("test.dc"));
        assert_eq!(registry.len(), 1);
        assert!(!registry.is_empty());

        let descriptor = registry.descriptor("test.dc").unwrap();
        assert_eq!(descriptor.name, "DC");
        assert_eq!(descriptor.category, DeviceCategory::Instrument);
        assert_eq!(descriptor.params, PARAMS.to_vec());
        assert!(!descriptor.sidechain);
        assert_eq!(descriptor.param(LEVEL).unwrap().default, 0.5);
        assert_eq!(descriptor.param_by_name("level").unwrap().id, LEVEL);
        assert!(descriptor.param(ParamId(9)).is_none());

        let mut device = registry.create_default("test.dc").unwrap();
        assert_eq!(output_of(device.as_mut()), 0.5);
    }

    #[test]
    fn create_applies_starting_parameter_values() {
        let registry = registry();
        let params = [(LEVEL, 0.25), (ParamId(77), 3.0)];
        let mut device = registry
            .create("test.dc", &DeviceInit::new().with_params(&params))
            .unwrap();
        assert_eq!(output_of(device.as_mut()), 0.25);
    }

    #[test]
    fn factories_see_state_and_can_fail() {
        let mut registry = DeviceRegistry::new();
        let descriptor = DeviceDescriptor::new("test.stateful", "Stateful", DeviceCategory::Effect)
            .with_params(PARAMS.to_vec())
            .with_sidechain();
        registry
            .register(descriptor, |init| match init.state {
                Some([level]) => Ok(Box::new(Dc {
                    level: f32::from(*level) / 255.0,
                })),
                Some(_) => Err(DeviceError::new("state must be one byte")),
                None => Ok(Box::new(Dc { level: 0.0 })),
            })
            .unwrap();
        assert!(registry.descriptor("test.stateful").unwrap().sidechain);

        let mut device = registry
            .create("test.stateful", &DeviceInit::new().with_state(&[255]))
            .unwrap();
        assert_eq!(output_of(device.as_mut()), 1.0);

        let error = registry
            .create("test.stateful", &DeviceInit::new().with_state(&[1, 2]))
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "cannot build device \"test.stateful\": state must be one byte"
        );
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn bad_registrations_and_lookups_are_typed_errors() {
        let mut registry = registry();
        let again = registry.register_device("test.dc", "DC", DeviceCategory::Effect, || Dc {
            level: 0.0,
        });
        assert_eq!(
            again,
            Err(RegistryError::DuplicateKind("test.dc".to_owned()))
        );
        for bad in ["", "has space", "tab\tbed"] {
            let result =
                registry.register_device(bad, "Bad", DeviceCategory::Effect, || Dc { level: 0.0 });
            assert_eq!(result, Err(RegistryError::InvalidKind(bad.to_owned())));
        }
        let twice = DeviceDescriptor::new("test.twice", "Twice", DeviceCategory::Effect)
            .with_params(vec![PARAMS[0].clone(), PARAMS[0].clone()]);
        assert_eq!(
            registry.register(twice, |_| Ok(Box::new(Dc { level: 0.0 }))),
            Err(RegistryError::DuplicateParam {
                kind: "test.twice".to_owned(),
                param: LEVEL
            })
        );
        let missing = registry.create_default("nope").err().unwrap();
        assert_eq!(missing, RegistryError::UnknownKind("nope".to_owned()));
        assert_eq!(missing.to_string(), "unknown device kind \"nope\"");
        assert!(registry.descriptor("nope").is_none());
        assert_eq!(registry.len(), 1, "failed registrations add nothing");
    }

    #[test]
    fn listings_are_sorted_by_kind() {
        let mut registry = registry();
        registry
            .register_device("a.first", "First", DeviceCategory::NoteEffect, || Dc {
                level: 0.0,
            })
            .unwrap();
        assert_eq!(registry.kinds().collect::<Vec<_>>(), ["a.first", "test.dc"]);
        let names: Vec<&str> = registry.descriptors().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["First", "DC"]);
        assert!(format!("{registry:?}").contains("a.first"));
        assert_eq!(DeviceCategory::NoteEffect.to_string(), "note effect");
    }

    #[test]
    fn the_registry_can_be_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DeviceRegistry>();
        let registry = std::sync::Arc::new(registry());
        let shared = std::sync::Arc::clone(&registry);
        let built = std::thread::spawn(move || shared.create_default("test.dc").is_ok())
            .join()
            .unwrap();
        assert!(built);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn descriptors_serialize_for_the_agent_api() {
        let registry = registry();
        let json = serde_json::to_value(registry.descriptor("test.dc").unwrap()).unwrap();
        assert_eq!(json["kind"], "test.dc");
        assert_eq!(json["category"], "instrument");
        assert_eq!(json["params"][0]["name"], "Level");
        assert_eq!(json["sidechain"], false);
    }
}
