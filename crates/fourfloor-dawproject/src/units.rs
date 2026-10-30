//! Unit conversions between fourfloor's model and DAWproject, and the
//! mapping between device kinds and DAWproject device elements.

use fourfloor_core::{Tempo, Ticks, TICKS_PER_BEAT};

/// Lowest fader gain of the model; at or below this is silence.
pub(crate) const MIN_GAIN_DB: f32 = -96.0;
/// Highest fader gain of the model.
pub(crate) const MAX_GAIN_DB: f32 = 24.0;
/// Largest tick value the model accepts.
pub(crate) const MAX_TICKS: i64 = 1 << 40;

/// A DAWproject `timeUnit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeUnit {
    Beats,
    Seconds,
}

impl TimeUnit {
    pub(crate) fn parse(text: Option<&str>) -> Option<Self> {
        match text {
            Some("beats") => Some(Self::Beats),
            Some("seconds") => Some(Self::Seconds),
            _ => None,
        }
    }
}

/// Ticks as beats. Exact for every tick count the model allows.
pub(crate) fn ticks_to_beats(ticks: Ticks) -> f64 {
    ticks.get() as f64 / TICKS_PER_BEAT as f64
}

/// Beats as ticks, rounded to the nearest tick and kept in the model's range.
pub(crate) fn beats_to_ticks(beats: f64) -> Ticks {
    let ticks = (beats * TICKS_PER_BEAT as f64).round();
    Ticks::new(ticks.clamp(-(MAX_TICKS as f64), MAX_TICKS as f64) as i64)
}

/// Ticks as seconds at a tempo.
pub(crate) fn ticks_to_seconds(ticks: Ticks, tempo: Tempo) -> f64 {
    ticks_to_beats(ticks) * 60.0 / tempo.bpm()
}

/// Converts a time value between units at the project tempo.
pub(crate) fn convert_time(value: f64, from: TimeUnit, to: TimeUnit, tempo: Tempo) -> f64 {
    match (from, to) {
        (TimeUnit::Beats, TimeUnit::Seconds) => value * 60.0 / tempo.bpm(),
        (TimeUnit::Seconds, TimeUnit::Beats) => value * tempo.bpm() / 60.0,
        _ => value,
    }
}

/// A time value in `unit` as ticks at the project tempo.
#[cfg(test)]
pub(crate) fn time_to_ticks(value: f64, unit: TimeUnit, tempo: Tempo) -> Ticks {
    beats_to_ticks(convert_time(value, unit, TimeUnit::Beats, tempo))
}

/// Decibels as a linear gain factor, in `f64` so the value survives the
/// trip through text and back to the same `f32`.
pub(crate) fn db_to_linear(db: f32) -> f64 {
    if db <= MIN_GAIN_DB {
        0.0
    } else {
        10f64.powf(f64::from(db) / 20.0)
    }
}

/// A linear gain factor as decibels within the model's fader range.
pub(crate) fn linear_to_db(linear: f64) -> f32 {
    if linear <= 0.0 {
        return MIN_GAIN_DB;
    }
    clamp_db(20.0 * linear.log10())
}

/// Decibels kept in the model's fader range.
pub(crate) fn clamp_db(db: f64) -> f32 {
    (db as f32).clamp(MIN_GAIN_DB, MAX_GAIN_DB)
}

/// Pan from -1..1 as DAWproject's normalized 0..1.
pub(crate) fn pan_to_normalized(pan: f32) -> f64 {
    (f64::from(pan) + 1.0) / 2.0
}

/// Normalized 0..1 pan as -1..1.
pub(crate) fn normalized_to_pan(normalized: f64) -> f32 {
    ((normalized * 2.0 - 1.0) as f32).clamp(-1.0, 1.0)
}

/// MIDI velocity as DAWproject's 0..1.
pub(crate) fn velocity_to_unit(velocity: u8) -> f64 {
    f64::from(velocity) / 127.0
}

/// DAWproject's 0..1 velocity as MIDI 1..127.
pub(crate) fn unit_to_velocity(unit: f64) -> u8 {
    (unit * 127.0).round().clamp(1.0, 127.0) as u8
}

/// The DAWproject element a hosted plugin kind is written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PluginElement {
    Clap,
    Vst3,
    Vst2,
    Au,
}

impl PluginElement {
    pub(crate) const ALL: [Self; 4] = [Self::Clap, Self::Vst3, Self::Vst2, Self::Au];

    pub(crate) fn element(self) -> &'static str {
        match self {
            Self::Clap => "ClapPlugin",
            Self::Vst3 => "Vst3Plugin",
            Self::Vst2 => "Vst2Plugin",
            Self::Au => "AuPlugin",
        }
    }

    /// The prefix of fourfloor device kinds for this format, as
    /// `fourfloor-host` writes them.
    pub(crate) fn kind_prefix(self) -> &'static str {
        match self {
            Self::Clap => "plugin.clap:",
            Self::Vst3 => "plugin.vst3:",
            Self::Vst2 => "plugin.vst2:",
            Self::Au => "plugin.au:",
        }
    }
}

/// Splits a hosted-plugin kind into its element and the plugin's own id
/// (with the kind's `%XX` escapes undone). `None` for any other kind.
pub(crate) fn plugin_from_kind(kind: &str) -> Option<(PluginElement, String)> {
    PluginElement::ALL.into_iter().find_map(|element| {
        let escaped = kind.strip_prefix(element.kind_prefix())?;
        let id = unescape(escaped)?;
        (!id.is_empty()).then_some((element, id))
    })
}

/// Builds the device kind for a plugin id, escaping whitespace and `%`
/// exactly as `fourfloor_host::plugin_kind` does.
pub(crate) fn kind_from_plugin(element: PluginElement, id: &str) -> String {
    format!("{}{}", element.kind_prefix(), escape(id))
}

pub(crate) fn escape(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for c in id.chars() {
        if c.is_whitespace() || c == '%' || c.is_control() {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn unescape(escaped: &str) -> Option<String> {
    let raw = escaped.as_bytes();
    let mut bytes = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            let hex = escaped.get(index + 1..index + 3)?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fader_step_survives_linear_text() {
        // Every gain on a 0.01 dB grid, plus awkward values, goes through
        // linear gain printed as text and comes back as the same f32.
        let awkward = [-95.99999_f32, 0.1, -0.1, 23.999998, 1.0e-6, -6.0206];
        let grid = (-9_600..=2_400).map(|n| n as f32 / 100.0);
        for db in grid.chain(awkward) {
            let text = db_to_linear(db).to_string();
            let back = linear_to_db(text.parse().unwrap());
            assert_eq!(back, db, "{db} dB via {text}");
        }
        assert_eq!(linear_to_db(0.0), MIN_GAIN_DB);
        assert_eq!(linear_to_db(1.0e9), MAX_GAIN_DB);
    }

    #[test]
    fn pan_velocity_and_ticks_round_trip() {
        for n in -1_000..=1_000 {
            let pan = n as f32 / 1_000.0;
            let text = pan_to_normalized(pan).to_string();
            assert_eq!(normalized_to_pan(text.parse().unwrap()), pan);
        }
        for velocity in 1..=127u8 {
            let text = velocity_to_unit(velocity).to_string();
            assert_eq!(unit_to_velocity(text.parse().unwrap()), velocity);
        }
        for ticks in [0, 1, 7, 959, 960, 123_457, MAX_TICKS - 1, MAX_TICKS] {
            let text = ticks_to_beats(Ticks::new(ticks)).to_string();
            assert_eq!(beats_to_ticks(text.parse().unwrap()).get(), ticks);
        }
        let tempo = Tempo::new(126.5).unwrap();
        for ticks in [1, 333, 960 * 64 + 1] {
            let seconds = ticks_to_seconds(Ticks::new(ticks), tempo).to_string();
            let back = time_to_ticks(seconds.parse().unwrap(), TimeUnit::Seconds, tempo);
            assert_eq!(back.get(), ticks);
        }
    }

    #[test]
    fn plugin_kinds_map_to_elements_and_back() {
        let kind = "plugin.au:aumu.dls%20.appl";
        let (element, id) = plugin_from_kind(kind).unwrap();
        assert_eq!(
            (element, id.as_str()),
            (PluginElement::Au, "aumu.dls .appl")
        );
        assert_eq!(kind_from_plugin(element, &id), kind);
        let clap = plugin_from_kind("plugin.clap:org.surge-synth-team.surge-xt").unwrap();
        assert_eq!(clap.0.element(), "ClapPlugin");
        assert_eq!(plugin_from_kind("fx.delay"), None);
        assert_eq!(plugin_from_kind("plugin.clap:"), None);
    }
}
