//! Device panel controls generated from parameter descriptors, for any
//! device: fourfloor's instruments and effects, built-ins and hosted
//! plugins. The registry descriptor gives units and scaling; the project
//! gives current values. A parameter the descriptor does not list (a
//! plugin whose cache is gone) still gets a plain linear control from the
//! range saved in the project.

use fourfloor_core::{DeviceDescriptor, DeviceId, ParamId, ParamInfo, ParamScale, ParamUnit};
use fourfloor_model::command::SetParam;
use fourfloor_model::DeviceInstance;

/// How a parameter is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlKind {
    /// An on/off button.
    Toggle,
    /// A knob that clicks between whole numbers.
    Stepped,
    /// A smooth knob.
    Continuous,
}

/// One control in the device panel.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamControl {
    /// Range, default, unit and scaling.
    pub info: ParamInfo,
    /// Current value in plain units.
    pub value: f32,
    /// How to draw it.
    pub kind: ControlKind,
}

impl ParamControl {
    /// The parameter id.
    pub fn id(&self) -> ParamId {
        self.info.id
    }

    /// Position 0..1 of the current value along the control's travel.
    pub fn normalized(&self) -> f32 {
        self.info.to_normalized(self.value)
    }

    /// The plain value at travel position `t`, rounded for stepped
    /// controls and snapped to 0/1 for toggles.
    pub fn value_at(&self, t: f32) -> f32 {
        let value = self.info.from_normalized(t.clamp(0.0, 1.0));
        match self.kind {
            ControlKind::Continuous => value,
            ControlKind::Stepped => self.info.clamp(value.round()),
            ControlKind::Toggle => {
                if t >= 0.5 {
                    self.info.max
                } else {
                    self.info.min
                }
            }
        }
    }

    /// The value moved by `delta` of the full travel (a knob drag).
    pub fn dragged(&self, delta: f32) -> f32 {
        self.value_at(self.normalized() + delta)
    }

    /// The command that sets `value`.
    pub fn command(&self, device: DeviceId, value: f32) -> SetParam {
        SetParam {
            device,
            param: self.info.id,
            value: self.info.clamp(value),
        }
    }

    /// Readable value with its unit.
    pub fn display(&self) -> String {
        format_value(&self.info, self.value)
    }
}

/// Formats a value in `info`'s units.
pub fn format_value(info: &ParamInfo, value: f32) -> String {
    match info.unit {
        ParamUnit::Toggle => if value >= 0.5 { "On" } else { "Off" }.to_owned(),
        ParamUnit::Integer => format!("{value:.0}"),
        ParamUnit::Hertz if value.abs() >= 1000.0 => format!("{:.2} kHz", value / 1000.0),
        ParamUnit::Hertz => format!("{value:.0} Hz"),
        ParamUnit::Decibels => format!("{value:.1} dB"),
        ParamUnit::Milliseconds => format!("{value:.0} ms"),
        ParamUnit::Seconds => format!("{value:.2} s"),
        // Some devices give percentages as 0..1, others as 0..100.
        ParamUnit::Percent if info.max <= 1.0 => format!("{:.0} %", value * 100.0),
        ParamUnit::Percent => format!("{value:.0} %"),
        ParamUnit::Ratio => format!("{value:.1}:1"),
        ParamUnit::Beats => format!("{value:.3} beats"),
        ParamUnit::Semitones => format!("{value:+.1} st"),
        ParamUnit::Cents => format!("{value:+.0} ct"),
        ParamUnit::Samples => format!("{value:.0} smp"),
        ParamUnit::Generic => format!("{value:.2}"),
    }
}

fn kind_of(info: &ParamInfo) -> ControlKind {
    match info.unit {
        ParamUnit::Toggle => ControlKind::Toggle,
        ParamUnit::Integer => ControlKind::Stepped,
        _ => ControlKind::Continuous,
    }
}

/// Controls for every parameter of `device`, in descriptor order, then any
/// saved parameter the descriptor lacks.
pub fn controls_for(
    device: &DeviceInstance,
    descriptor: Option<&DeviceDescriptor>,
) -> Vec<ParamControl> {
    let described: Vec<&ParamInfo> = descriptor
        .map(|d| d.params.iter().collect())
        .unwrap_or_default();
    let mut controls: Vec<ParamControl> = described
        .iter()
        .map(|info| ParamControl {
            info: (*info).clone(),
            value: device
                .param(info.id)
                .map_or(info.default, |param| param.value),
            kind: kind_of(info),
        })
        .collect();
    for param in &device.params {
        if described.iter().any(|info| info.id == param.id) {
            continue;
        }
        let info = ParamInfo::new(
            param.id,
            "",
            param.min,
            param.max,
            param.value,
            ParamUnit::Generic,
        )
        .with_name(param.name.clone());
        controls.push(ParamControl {
            kind: ControlKind::Continuous,
            value: param.value,
            info,
        });
    }
    controls
}

/// `true` if the control's travel is logarithmic.
pub fn is_logarithmic(control: &ParamControl) -> bool {
    control.info.scale == ParamScale::Logarithmic
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::registry::{build_registry, device_params};
    use fourfloor_effects::{Compressor, Reverb};
    use fourfloor_model::Param;

    fn instance(kind: &str, params: Vec<Param>) -> DeviceInstance {
        DeviceInstance {
            id: DeviceId(5),
            kind: kind.to_owned(),
            name: kind.to_owned(),
            bypassed: false,
            params,
            state_blob: None,
            sidechain: None,
        }
    }

    #[test]
    fn controls_follow_the_descriptor_for_every_registered_device() {
        let registry = build_registry(false).expect("registry");
        for descriptor in registry.descriptors() {
            let params = device_params(&registry, &descriptor.kind, &[]).expect("params");
            let device = instance(&descriptor.kind, params);
            let controls = controls_for(&device, Some(descriptor));
            assert_eq!(
                controls.len(),
                descriptor.params.len(),
                "{}",
                descriptor.kind
            );
            for control in &controls {
                assert!(control.value.is_finite());
                let t = control.normalized();
                assert!((0.0..=1.0).contains(&t), "{} {t}", control.info.name);
                assert!(!control.display().is_empty());
            }
        }
    }

    #[test]
    fn toggles_steps_and_log_knobs_behave() {
        let registry = build_registry(false).expect("registry");
        let descriptor = registry.descriptor("fx.compressor").expect("compressor");
        let device = instance(
            "fx.compressor",
            device_params(&registry, "fx.compressor", &[]).expect("p"),
        );
        let controls = controls_for(&device, Some(descriptor));
        let key = controls
            .iter()
            .find(|c| c.id() == Compressor::EXTERNAL_KEY)
            .expect("toggle");
        assert_eq!(key.kind, ControlKind::Toggle);
        assert_eq!(key.value_at(0.2), key.info.min);
        assert_eq!(key.value_at(0.8), key.info.max);
        let reverb = registry.descriptor("fx.reverb").expect("reverb");
        let device = instance(
            "fx.reverb",
            device_params(&registry, "fx.reverb", &[]).expect("p"),
        );
        let controls = controls_for(&device, Some(reverb));
        let mode = controls
            .iter()
            .find(|c| c.id() == Reverb::MODE)
            .expect("mode");
        assert_eq!(mode.kind, ControlKind::Stepped);
        assert_eq!(mode.value_at(0.7), 1.0);
        let decay = controls
            .iter()
            .find(|c| c.id() == Reverb::DECAY)
            .expect("decay");
        assert!(is_logarithmic(decay));
        let up = decay.dragged(0.1);
        assert!(up > decay.value);
        assert_eq!(decay.command(DeviceId(5), 1e9).value, decay.info.max);
    }

    #[test]
    fn unknown_parameters_still_get_a_control() {
        let device = instance(
            "plugin.clap:gone",
            vec![Param {
                id: ParamId(77),
                name: "Cutoff".to_owned(),
                value: 0.25,
                min: 0.0,
                max: 1.0,
            }],
        );
        let controls = controls_for(&device, None);
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].info.name, "Cutoff");
        assert_eq!(controls[0].normalized(), 0.25);
        assert_eq!(format_value(&controls[0].info, 0.25), "0.25");
        let hz = ParamInfo::new(ParamId(0), "f", 20.0, 20000.0, 1000.0, ParamUnit::Hertz);
        assert_eq!(format_value(&hz, 1500.0), "1.50 kHz");
        assert_eq!(format_value(&hz, 150.0), "150 Hz");
    }
}
