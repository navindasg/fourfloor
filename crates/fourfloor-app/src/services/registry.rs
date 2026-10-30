//! The device registry the app builds graphs from.

use fourfloor_core::{DeviceRegistry, ParamId, RegistryError};
use fourfloor_model::Param;

/// Engine built-ins, fourfloor's instruments and effects and, when
/// `with_plugins` is set, hosted plugins from the scan cache (the app never
/// scans on start-up; see `fourfloor_host::register`).
pub fn build_registry(with_plugins: bool) -> Result<DeviceRegistry, RegistryError> {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry)?;
    fourfloor_instruments::register(&mut registry)?;
    fourfloor_effects::register(&mut registry)?;
    if with_plugins {
        fourfloor_host::register(&mut registry)?;
    }
    Ok(registry)
}

/// A kind the registry does not know.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("device kind \"{0}\" is not registered")]
pub struct UnknownKind(pub String);

/// The full parameter list for a new device of `kind`: every descriptor
/// parameter at its default, except those in `values` (clamped to range).
pub fn device_params(
    registry: &DeviceRegistry,
    kind: &str,
    values: &[(ParamId, f32)],
) -> Result<Vec<Param>, UnknownKind> {
    let descriptor = registry
        .descriptor(kind)
        .ok_or_else(|| UnknownKind(kind.to_owned()))?;
    Ok(descriptor
        .params
        .iter()
        .map(|info| Param {
            id: info.id,
            name: info.name.to_string(),
            value: values
                .iter()
                .find(|(id, _)| *id == info.id)
                .map_or(info.default, |(_, value)| info.clamp(*value)),
            min: info.min,
            max: info.max,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_has_instruments_effects_and_built_ins() {
        let registry = build_registry(false).expect("registry builds");
        for kind in [
            "builtin.gain",
            "drum.kick909",
            "synth.acid303",
            "poly.pad",
            "fx.compressor",
            "fx.reverb",
            "fx.limiter",
            "note.arp",
        ] {
            assert!(registry.contains(kind), "{kind}");
        }
        let params =
            device_params(&registry, "fx.reverb", &[(ParamId(6), 7.0)]).expect("reverb params");
        let mix = params.iter().find(|p| p.name == "Mix").expect("mix param");
        assert_eq!(mix.value, 1.0, "clamped into range");
        assert!(device_params(&registry, "fx.nothing", &[]).is_err());
    }
}
