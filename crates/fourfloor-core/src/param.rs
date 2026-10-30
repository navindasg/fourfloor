//! Parameter metadata: how a device describes its parameters so the UI,
//! automation and the agent API can discover them.

use std::borrow::Cow;

use crate::id::ParamId;

/// What a parameter's plain value measures. Used for display and for
/// choosing sensible editing steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ParamUnit {
    /// A plain number with no unit (a linear gain factor, a mix amount
    /// from 0 to 1).
    #[default]
    Generic,
    /// Decibels.
    Decibels,
    /// Hertz.
    Hertz,
    /// Seconds.
    Seconds,
    /// Milliseconds.
    Milliseconds,
    /// Percent, 0 to 100.
    Percent,
    /// Semitones.
    Semitones,
    /// Cents (hundredths of a semitone).
    Cents,
    /// A ratio such as a compressor's 4:1, stored as 4.0.
    Ratio,
    /// Musical beats (quarter notes).
    Beats,
    /// Samples.
    Samples,
    /// Off (0.0) or on (1.0).
    Toggle,
    /// A whole number, for example a choice index or a step count.
    Integer,
}

impl ParamUnit {
    /// Short display suffix, for example `"dB"`. Empty for unitless values.
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Generic | Self::Toggle | Self::Integer => "",
            Self::Decibels => "dB",
            Self::Hertz => "Hz",
            Self::Seconds => "s",
            Self::Milliseconds => "ms",
            Self::Percent => "%",
            Self::Semitones => "st",
            Self::Cents => "ct",
            Self::Ratio => ":1",
            Self::Beats => "beats",
            Self::Samples => "samples",
        }
    }
}

/// How a control should map its travel onto the parameter's range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ParamScale {
    /// Even steps across the range.
    #[default]
    Linear,
    /// Even ratios across the range (frequencies, times). Needs `min > 0`.
    Logarithmic,
}

/// Describes one parameter of a device.
///
/// Values are in the device's plain units, the same units
/// [`Device::set_param`](crate::Device::set_param) takes.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParamInfo {
    /// Id passed to [`Device::set_param`](crate::Device::set_param). Unique
    /// within the device and stable across versions: projects store it.
    pub id: ParamId,
    /// Display name, for example `"Cutoff"`.
    pub name: Cow<'static, str>,
    /// Lowest allowed value.
    pub min: f32,
    /// Highest allowed value.
    pub max: f32,
    /// Value a new device starts with.
    pub default: f32,
    /// What the value measures.
    pub unit: ParamUnit,
    /// How a control should sweep the range.
    pub scale: ParamScale,
}

impl ParamInfo {
    /// A linear parameter. `const`, so a device can keep its descriptors
    /// in a `static` table and return `&TABLE` from
    /// [`Device::params`](crate::Device::params). The table must be a
    /// `static`, not a `const`: a reference to a `const` of this type does
    /// not live long enough.
    pub const fn new(
        id: ParamId,
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
        unit: ParamUnit,
    ) -> Self {
        Self {
            id,
            name: Cow::Borrowed(name),
            min,
            max,
            default,
            unit,
            scale: ParamScale::Linear,
        }
    }

    /// An on/off parameter: 0.0 is off, 1.0 is on.
    pub const fn toggle(id: ParamId, name: &'static str, default_on: bool) -> Self {
        Self::new(
            id,
            name,
            0.0,
            1.0,
            if default_on { 1.0 } else { 0.0 },
            ParamUnit::Toggle,
        )
    }

    /// The same parameter with a logarithmic control scale.
    pub const fn logarithmic(mut self) -> Self {
        self.scale = ParamScale::Logarithmic;
        self
    }

    /// The same parameter with an owned name, for parameters discovered at
    /// run time (hosted plugins).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Cow::Owned(name.into());
        self
    }

    /// `value` limited to `min..=max`. A value that is not a number becomes
    /// the default.
    pub fn clamp(&self, value: f32) -> f32 {
        if value.is_nan() {
            self.default
        } else {
            value.clamp(self.min, self.max)
        }
    }

    /// Whether `value` is a number inside `min..=max`.
    pub fn contains(&self, value: f32) -> bool {
        value >= self.min && value <= self.max
    }

    /// Maps a control position in `0.0..=1.0` to a plain value, following
    /// [`scale`](ParamInfo::scale).
    pub fn from_normalized(&self, position: f32) -> f32 {
        let t = if position.is_nan() {
            0.0
        } else {
            position.clamp(0.0, 1.0)
        };
        match self.scale {
            ParamScale::Logarithmic if self.min > 0.0 && self.max > self.min => {
                self.min * (self.max / self.min).powf(t)
            }
            _ => self.min + (self.max - self.min) * t,
        }
    }

    /// Maps a plain value to a control position in `0.0..=1.0`; the inverse
    /// of [`from_normalized`](ParamInfo::from_normalized).
    pub fn to_normalized(&self, value: f32) -> f32 {
        let value = self.clamp(value);
        if self.max <= self.min {
            return 0.0;
        }
        match self.scale {
            ParamScale::Logarithmic if self.min > 0.0 => {
                (value / self.min).ln() / (self.max / self.min).ln()
            }
            _ => (value - self.min) / (self.max - self.min),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUTOFF: ParamInfo = ParamInfo::new(
        ParamId(0),
        "Cutoff",
        20.0,
        20_000.0,
        1_000.0,
        ParamUnit::Hertz,
    )
    .logarithmic();
    static TABLE: [ParamInfo; 2] = [CUTOFF, ParamInfo::toggle(ParamId(1), "Sync", true)];

    #[test]
    fn descriptors_can_live_in_a_static_table() {
        assert_eq!(TABLE[0].name, "Cutoff");
        assert_eq!(TABLE[0].scale, ParamScale::Logarithmic);
        assert_eq!(TABLE[1].unit, ParamUnit::Toggle);
        assert_eq!(TABLE[1].default, 1.0);
        assert_eq!((TABLE[1].min, TABLE[1].max), (0.0, 1.0));
    }

    #[test]
    fn clamp_limits_and_replaces_nan_with_the_default() {
        assert_eq!(CUTOFF.clamp(5.0), 20.0);
        assert_eq!(CUTOFF.clamp(1e9), 20_000.0);
        assert_eq!(CUTOFF.clamp(440.0), 440.0);
        assert_eq!(CUTOFF.clamp(f32::NAN), 1_000.0);
        assert!(CUTOFF.contains(20.0) && CUTOFF.contains(20_000.0));
        assert!(!CUTOFF.contains(19.9) && !CUTOFF.contains(f32::NAN));
    }

    #[test]
    fn logarithmic_scale_puts_the_geometric_mean_in_the_middle() {
        let mid = CUTOFF.from_normalized(0.5);
        assert!((mid - (20.0f32 * 20_000.0).sqrt()).abs() < 0.5, "{mid}");
        assert_eq!(CUTOFF.from_normalized(0.0), 20.0);
        assert!((CUTOFF.from_normalized(1.0) - 20_000.0).abs() < 0.5);
        assert!((CUTOFF.to_normalized(mid) - 0.5).abs() < 1e-5);
        assert_eq!(CUTOFF.from_normalized(f32::NAN), 20.0);
    }

    #[test]
    fn linear_scale_round_trips() {
        let gain = ParamInfo::new(ParamId(2), "Gain", -60.0, 12.0, 0.0, ParamUnit::Decibels);
        assert_eq!(gain.from_normalized(0.5), -24.0);
        assert!((gain.to_normalized(-24.0) - 0.5).abs() < 1e-6);
        assert_eq!(gain.to_normalized(100.0), 1.0);
        assert_eq!(gain.unit.suffix(), "dB");
        let flat = ParamInfo::new(ParamId(3), "Flat", 1.0, 1.0, 1.0, ParamUnit::Generic);
        assert_eq!(flat.to_normalized(1.0), 0.0);
    }

    #[test]
    fn runtime_names_are_owned() {
        let info = CUTOFF.with_name(format!("Macro {}", 3));
        assert_eq!(info.name, "Macro 3");
        assert!(matches!(info.name, Cow::Owned(_)));
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serializes_for_the_agent_api() {
        let json = serde_json::to_value(&CUTOFF).unwrap();
        assert_eq!(json["id"], 0);
        assert_eq!(json["name"], "Cutoff");
        assert_eq!(json["unit"], "hertz");
        assert_eq!(json["scale"], "logarithmic");
        let back: ParamInfo = serde_json::from_value(json).unwrap();
        assert_eq!(back, CUTOFF);
    }
}
