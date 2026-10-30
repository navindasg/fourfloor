//! Import of automation lanes: `Points` timelines on track lanes become
//! device parameter lanes or fader gain and pan lanes.

use std::sync::Arc;

use fourfloor_core::Ticks;
use fourfloor_model::validate::{MAX_AUTOMATION_POINTS, MAX_GAIN_DB, MAX_TICKS, MIN_GAIN_DB};
use fourfloor_model::{AutomationLane, AutomationPoint, MixerLane, MixerParam};

use super::{Importer, Target};
use crate::report::Subject;
use crate::units::{beats_to_ticks, clamp_db, linear_to_db, normalized_to_pan};
use crate::xml::{boolean, child, elements, number, path, tag, Node};

/// A point read from the XML, in beats and in the file's value units.
struct RawPoint {
    time: f64,
    value: f64,
    hold: bool,
}

/// Turns held segments into steps: a held point keeps its value up to the
/// next point, where the value jumps.
fn with_steps(raw: &[RawPoint], mut convert: impl FnMut(f64) -> f32) -> Vec<AutomationPoint> {
    let mut points = Vec::with_capacity(raw.len() * 2);
    for (index, point) in raw.iter().enumerate() {
        let time = beats_to_ticks(point.time).clamp(Ticks::ZERO, Ticks::new(MAX_TICKS));
        let value = convert(point.value);
        points.push(AutomationPoint { time, value });
        if let Some(next) = raw.get(index + 1).filter(|_| point.hold) {
            let next_value = convert(next.value);
            if next_value != value {
                let next_time = beats_to_ticks(next.time).clamp(Ticks::ZERO, Ticks::new(MAX_TICKS));
                points.push(AutomationPoint {
                    time: next_time,
                    value,
                });
            }
        }
    }
    points
}

impl<'a, 'input> Importer<'a, 'input> {
    pub(super) fn automation_lane(&mut self, node: Node<'a, 'input>) {
        let at = path(node);
        let target_node = child(node, "Target");
        if target_node.is_some_and(|t| t.has_attribute("expression")) {
            self.report.dropped(
                Subject::Automation,
                at,
                "note expression and MIDI controller automation are not supported",
            );
            return;
        }
        let parameter = target_node.and_then(|t| t.attribute("parameter"));
        let Some(target) = parameter.and_then(|id| self.targets.get(id).copied()) else {
            self.report.dropped(
                Subject::Automation,
                at,
                format!(
                    "automation of `{}`, which fourfloor has no control for",
                    parameter.unwrap_or("nothing")
                ),
            );
            return;
        };
        let unit = node.attribute("unit");
        let time_unit = self.time_unit(node);
        let mut raw: Vec<RawPoint> = elements(node)
            .filter(|n| tag(*n) != "Target")
            .filter_map(|point| {
                let value = match tag(point) {
                    "BoolPoint" => boolean(point, "value").map(|b| if b { 1.0 } else { 0.0 }),
                    _ => number(point, "value"),
                }?;
                let hold =
                    tag(point) != "RealPoint" || point.attribute("interpolation") == Some("hold");
                Some(RawPoint {
                    time: self.beats(number(point, "time")?, time_unit),
                    value,
                    hold,
                })
            })
            .collect();
        raw.sort_by(|a, b| a.time.total_cmp(&b.time));
        if raw.len() > MAX_AUTOMATION_POINTS / 2 {
            self.report.dropped(
                Subject::Automation,
                at.clone(),
                "the lane is cut to the most points fourfloor holds",
            );
            raw.truncate(MAX_AUTOMATION_POINTS / 2);
        }
        match target {
            Target::Unsupported(what) => self.report.dropped(
                Subject::Automation,
                at,
                format!("automation of the {what} is not supported"),
            ),
            Target::Volume(track) => {
                let points = with_steps(&raw, |v| match unit {
                    Some("decibel") => clamp_db(v.max(f64::from(MIN_GAIN_DB))),
                    _ => linear_to_db(v),
                });
                self.mixer_lane(track, MixerParam::Gain, points, &at);
            }
            Target::Pan(track) => {
                let points = with_steps(&raw, |v| match unit {
                    Some("linear") => (v as f32).clamp(-1.0, 1.0),
                    Some("percent") => (v as f32 / 100.0).clamp(-1.0, 1.0),
                    _ => normalized_to_pan(v),
                });
                self.mixer_lane(track, MixerParam::Pan, points, &at);
            }
            Target::Device {
                track,
                device,
                param,
                xml_min,
                xml_max,
                normalized,
            } => {
                let scale = unit == Some("normalized") && !normalized;
                let (min, max) = (xml_min as f32, xml_max as f32);
                let mut clamped = false;
                let points = with_steps(&raw, |v| {
                    let plain = if scale {
                        xml_min + v * (xml_max - xml_min)
                    } else {
                        v
                    } as f32;
                    if plain < min || plain > max {
                        clamped = true;
                    }
                    plain.clamp(min, max)
                });
                if clamped || points.iter().any(|p| !p.value.is_finite()) {
                    self.report.approximated(
                        Subject::Automation,
                        at.clone(),
                        "values outside the parameter range are limited to it",
                    );
                }
                let lanes = &mut self.tracks[track].automation;
                if lanes.iter().any(|l| l.device == device && l.param == param) {
                    self.report.dropped(
                        Subject::Automation,
                        at,
                        "a second lane for the same parameter",
                    );
                    return;
                }
                lanes.push(Arc::new(AutomationLane {
                    device,
                    param,
                    points,
                }));
            }
        }
    }

    fn mixer_lane(
        &mut self,
        track: usize,
        param: MixerParam,
        points: Vec<AutomationPoint>,
        at: &str,
    ) {
        if points.is_empty() {
            return;
        }
        let points = points
            .into_iter()
            .map(|p| AutomationPoint {
                value: match param {
                    MixerParam::Gain => p.value.clamp(MIN_GAIN_DB, MAX_GAIN_DB),
                    MixerParam::Pan => p.value,
                },
                ..p
            })
            .collect();
        let lanes = &mut self.tracks[track].mixer_automation;
        if lanes.iter().any(|lane| lane.param == param) {
            self.report.dropped(
                Subject::Automation,
                at.to_owned(),
                "a second lane for the same mixer control",
            );
            return;
        }
        lanes.push(Arc::new(MixerLane { param, points }));
    }
}
