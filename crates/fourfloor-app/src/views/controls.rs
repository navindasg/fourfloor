//! Track controls shared by the arrangement headers and the mixer strips:
//! volume and pan that move the engine live while dragged and commit one
//! undoable command on release, plus mute, solo and arm.

use std::collections::HashMap;

use egui::Ui;
use fourfloor_core::TrackId;
use fourfloor_model::command::{SetTrackGain, SetTrackMute, SetTrackPan, SetTrackSolo};
use fourfloor_model::{Track, TrackKind};

use crate::state::Intent;
use crate::theme;
use crate::viewmodel::mixer::{fader_gain, fader_position, gain_label};
use crate::widgets::buttons::toggle;

use super::ViewCtx;

/// Values shown while a control is being dragged, before the command.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Previews {
    gain: HashMap<TrackId, f32>,
    pan: HashMap<TrackId, f32>,
}

impl Previews {
    /// Gain shown for `track`.
    pub fn gain(&self, track: &Track) -> f32 {
        self.gain.get(&track.id).copied().unwrap_or(track.gain_db)
    }

    /// Pan shown for `track`.
    pub fn pan(&self, track: &Track) -> f32 {
        self.pan.get(&track.id).copied().unwrap_or(track.pan)
    }
}

fn commit_or_preview(
    response: &egui::Response,
    value: f32,
    previews: &mut HashMap<TrackId, f32>,
    track: TrackId,
) -> Option<f32> {
    let finished = response.drag_stopped() || (response.changed() && !response.dragged());
    if finished {
        previews.remove(&track);
        return Some(value);
    }
    if response.changed() {
        previews.insert(track, value);
    }
    None
}

/// A horizontal volume slider for `track`.
pub fn gain_slider(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    previews: &mut Previews,
    track: &Track,
    width: f32,
) {
    let mut position = fader_position(previews.gain(track));
    let label = gain_label(fader_gain(position));
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = width;
            ui.add(
                egui::Slider::new(&mut position, 0.0..=1.0)
                    .show_value(false)
                    .text(label),
            )
        })
        .inner
        .on_hover_text(format!("Volume {}", track.name));
    let gain_db = fader_gain(position);
    if response.changed() {
        ctx.intents.push(Intent::LiveTrackGain {
            track: track.id,
            gain_db,
        });
    }
    if let Some(gain_db) = commit_or_preview(&response, gain_db, &mut previews.gain, track.id) {
        if (gain_db - track.gain_db).abs() > 1e-4 {
            ctx.intents.command(SetTrackGain {
                track: track.id,
                gain_db,
            });
        }
    }
}

/// A horizontal pan slider for `track`.
pub fn pan_slider(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    previews: &mut Previews,
    track: &Track,
    width: f32,
) {
    let mut pan = previews.pan(track);
    let text = pan_label(pan);
    let response = ui
        .scope(|ui| {
            ui.spacing_mut().slider_width = width;
            ui.add(
                egui::Slider::new(&mut pan, -1.0..=1.0)
                    .show_value(false)
                    .text(text),
            )
        })
        .inner
        .on_hover_text(format!("Pan {}", track.name));
    if response.double_clicked() {
        pan = 0.0;
    }
    if response.changed() {
        ctx.intents.push(Intent::LiveTrackPan {
            track: track.id,
            pan,
        });
    }
    if let Some(pan) = commit_or_preview(&response, pan, &mut previews.pan, track.id) {
        if (pan - track.pan).abs() > 1e-4 {
            ctx.intents.command(SetTrackPan {
                track: track.id,
                pan,
            });
        }
    }
}

/// `C`, `L40`, `R12` style pan text.
pub fn pan_label(pan: f32) -> String {
    let percent = (pan * 100.0).round() as i32;
    match percent {
        0 => "C".to_owned(),
        p if p < 0 => format!("L{}", -p),
        p => format!("R{p}"),
    }
}

/// Mute, solo and (for MIDI tracks) arm buttons.
pub fn mute_solo_arm(ui: &mut Ui, ctx: &mut ViewCtx<'_>, track: &Track) {
    let size = 20.0;
    if toggle(
        ui,
        "M",
        &format!("Mute {}", track.name),
        track.mute,
        theme::MUTE,
        size,
    )
    .clicked()
    {
        ctx.intents.command(SetTrackMute {
            track: track.id,
            mute: !track.mute,
        });
    }
    if toggle(
        ui,
        "S",
        &format!("Solo {}", track.name),
        track.solo,
        theme::SOLO,
        size,
    )
    .clicked()
    {
        ctx.intents.command(SetTrackSolo {
            track: track.id,
            solo: !track.solo,
        });
    }
    if track.kind == TrackKind::Midi {
        let armed = ctx.ui.armed == Some(track.id);
        if toggle(
            ui,
            "R",
            &format!("Arm {}", track.name),
            armed,
            theme::ARM,
            size,
        )
        .clicked()
        {
            ctx.ui.armed = if armed { None } else { Some(track.id) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pan_reads_as_left_centre_right() {
        assert_eq!(pan_label(0.0), "C");
        assert_eq!(pan_label(-0.4), "L40");
        assert_eq!(pan_label(0.12), "R12");
    }
}
