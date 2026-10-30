//! The mixer: one strip per track with its device chain, sends, output
//! routing, pan, fader and meter, and a master strip with peak and LUFS.

use std::collections::HashMap;

use egui::{vec2, Sense, Ui};
use fourfloor_core::TrackId;
use fourfloor_model::command::{RemoveSend, SetSend, SetTrackGain, SetTrackOutput};
use fourfloor_model::{Track, TrackKind};

use crate::state::{Intent, ViewId};
use crate::theme;
use crate::viewmodel::mixer::{fader_gain, fader_position, gain_label, send_targets};
use crate::widgets::{fader, knob, paint_stereo_meter};

use super::controls::{mute_solo_arm, pan_slider, Previews};
use super::{View, ViewCtx};

/// Width of a channel strip.
pub const STRIP_WIDTH: f32 = 128.0;
/// Height of the device chain section (scrolls when longer).
pub const CHAIN_HEIGHT: f32 = 112.0;
/// Height of the sends section (scrolls when longer).
pub const SENDS_HEIGHT: f32 = 96.0;

/// Gives `add` exactly `height` pixels, scrolling when it needs more, so
/// every strip's fader lines up.
fn fixed_height(
    ui: &mut Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    height: f32,
    add: impl FnOnce(&mut Ui),
) {
    ui.allocate_ui(vec2(STRIP_WIDTH, height), |ui| {
        ui.set_min_height(height);
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, add);
    });
}

/// The mixer view.
#[derive(Debug, Default)]
pub struct MixerView {
    previews: Previews,
    gain_preview: HashMap<TrackId, f32>,
    send_preview: HashMap<(TrackId, TrackId), f32>,
}

impl View for MixerView {
    fn id(&self) -> ViewId {
        ViewId::MIXER
    }

    fn title(&self) -> &str {
        "Mixer"
    }

    fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        if ctx.project.tracks.is_empty() {
            super::empty::empty_state(
                ui,
                "No channels yet",
                "Each track gets a strip here once it exists.",
            );
            return;
        }
        egui::ScrollArea::horizontal()
            .id_salt("mixer")
            // Strips run off narrow windows: keep the bar visible so the
            // rest (and the master strip) can be found.
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    let tracks: Vec<Track> = ctx
                        .project
                        .tracks
                        .iter()
                        .map(|t| t.as_ref().clone())
                        .collect();
                    for (index, track) in tracks.iter().enumerate() {
                        self.strip(ui, ctx, track, index);
                    }
                    master_strip(ui, ctx);
                });
            });
    }
}

impl MixerView {
    fn strip(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, track: &Track, index: usize) {
        let selected = ctx.ui.selection.track == Some(track.id);
        let frame = egui::Frame::new()
            .fill(if selected {
                theme::RAISED
            } else {
                theme::SURFACE
            })
            .corner_radius(4.0)
            .inner_margin(6.0);
        frame.show(ui, |ui| {
            ui.set_width(STRIP_WIDTH);
            ui.vertical(|ui| {
                let (bar, _) = ui.allocate_exact_size(vec2(STRIP_WIDTH, 4.0), Sense::hover());
                ui.painter()
                    .rect_filled(bar, 1.0, theme::track_color(index));
                if ui
                    .add(
                        egui::Label::new(egui::RichText::new(&track.name).strong())
                            .truncate()
                            .sense(Sense::click()),
                    )
                    .clicked()
                {
                    ctx.ui.selection.select_track(track.id);
                }
                fixed_height(ui, ("chain", track.id), CHAIN_HEIGHT, |ui| {
                    self.chain(ui, ctx, track)
                });
                ui.separator();
                fixed_height(ui, ("sends", track.id), SENDS_HEIGHT, |ui| {
                    self.sends(ui, ctx, track)
                });
                ui.separator();
                output_combo(ui, ctx, track);
                pan_slider(ui, ctx, &mut self.previews, track, STRIP_WIDTH - 20.0);
                ui.horizontal(|ui| mute_solo_arm(ui, ctx, track));
                self.fader_and_meter(ui, ctx, track);
            });
        });
    }

    fn chain(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, track: &Track) {
        ui.label(
            egui::RichText::new("Devices")
                .small()
                .color(theme::TEXT_DIM),
        );
        for device in &track.devices {
            let chosen = ctx.ui.selection.device == Some(device.id);
            let text = if device.bypassed {
                egui::RichText::new(format!("(off) {}", device.name)).color(theme::TEXT_DIM)
            } else {
                egui::RichText::new(&device.name)
            };
            if ui
                .add_sized(
                    vec2(STRIP_WIDTH, 18.0),
                    egui::Button::selectable(chosen, text).truncate(),
                )
                .clicked()
            {
                ctx.ui.selection.select_track(track.id);
                ctx.ui.selection.device = Some(device.id);
                ctx.ui.panels.devices = true;
            }
        }
        if ui
            .add_sized(vec2(STRIP_WIDTH, 18.0), egui::Button::new("+ device"))
            .clicked()
        {
            ctx.ui.selection.select_track(track.id);
            ctx.ui.panels.browser = true;
        }
    }

    fn sends(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, track: &Track) {
        if track.sends.is_empty() && send_targets(ctx.project, track.id).is_empty() {
            return;
        }
        ui.label(egui::RichText::new("Sends").small().color(theme::TEXT_DIM));
        ui.horizontal_wrapped(|ui| {
            for send in &track.sends {
                let name = ctx
                    .project
                    .track(send.target)
                    .map_or("?".to_owned(), |t| t.name.clone());
                let key = (track.id, send.target);
                let shown = self.send_preview.get(&key).copied().unwrap_or(send.gain_db);
                let output = knob(
                    ui,
                    &name,
                    &gain_label(shown),
                    fader_position(shown),
                    fader_position(0.0),
                    26.0,
                );
                if let Some(t) = output.changed {
                    self.send_preview.insert(key, fader_gain(t));
                }
                if output.committed {
                    let gain_db = self.send_preview.remove(&key).unwrap_or(shown);
                    ctx.intents.command(SetSend {
                        track: track.id,
                        target: send.target,
                        gain_db,
                        pre_fader: send.pre_fader,
                    });
                }
                output.response.context_menu(|ui| {
                    let label = if send.pre_fader {
                        "Post fader"
                    } else {
                        "Pre fader"
                    };
                    if ui.button(label).clicked() {
                        ctx.intents.command(SetSend {
                            track: track.id,
                            target: send.target,
                            gain_db: send.gain_db,
                            pre_fader: !send.pre_fader,
                        });
                        ui.close();
                    }
                    if ui.button("Remove send").clicked() {
                        ctx.intents.command(RemoveSend {
                            track: track.id,
                            target: send.target,
                        });
                        ui.close();
                    }
                });
            }
        });
        let free: Vec<(TrackId, String)> = send_targets(ctx.project, track.id)
            .into_iter()
            .filter(|target| track.send(*target).is_none())
            .filter_map(|target| ctx.project.track(target).map(|t| (target, t.name.clone())))
            .collect();
        if !free.is_empty() {
            egui::ComboBox::new(("add-send", track.id), "send")
                .selected_text("+")
                .width(STRIP_WIDTH - 50.0)
                .show_ui(ui, |ui| {
                    for (target, name) in free {
                        if ui.button(name).clicked() {
                            ctx.intents.command(SetSend {
                                track: track.id,
                                target,
                                gain_db: -12.0,
                                pre_fader: false,
                            });
                        }
                    }
                });
        }
    }

    fn fader_and_meter(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, track: &Track) {
        let shown = self
            .gain_preview
            .get(&track.id)
            .copied()
            .unwrap_or(track.gain_db);
        ui.label(egui::RichText::new(format!("{} dB", gain_label(shown))).monospace());
        ui.horizontal(|ui| {
            let output = fader(
                ui,
                &track.name,
                fader_position(shown),
                fader_position(0.0),
                vec2(40.0, 150.0),
            );
            if let Some(position) = output.changed {
                let gain_db = fader_gain(position);
                self.gain_preview.insert(track.id, gain_db);
                ctx.intents.push(Intent::LiveTrackGain {
                    track: track.id,
                    gain_db,
                });
            }
            if output.committed {
                let gain_db = self.gain_preview.remove(&track.id).unwrap_or(shown);
                if (gain_db - track.gain_db).abs() > 1e-4 {
                    ctx.intents.command(SetTrackGain {
                        track: track.id,
                        gain_db,
                    });
                }
            }
            let (meter, _) = ui.allocate_exact_size(vec2(14.0, 150.0), Sense::hover());
            paint_stereo_meter(ui, meter, ctx.live.meters.track(track.id));
        });
    }
}

fn output_combo(ui: &mut Ui, ctx: &mut ViewCtx<'_>, track: &Track) {
    let current = track
        .output
        .and_then(|id| ctx.project.track(id))
        .map_or("Master".to_owned(), |t| t.name.clone());
    let buses: Vec<(TrackId, String)> = ctx
        .project
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Bus && t.id != track.id)
        .filter(|t| !crate::viewmodel::mixer::feeds(ctx.project, t.id, track.id))
        .map(|t| (t.id, t.name.clone()))
        .collect();
    egui::ComboBox::new(("output", track.id), "out")
        .selected_text(current)
        .width(STRIP_WIDTH - 40.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(track.output.is_none(), "Master")
                .clicked()
                && track.output.is_some()
            {
                ctx.intents.command(SetTrackOutput {
                    track: track.id,
                    output: None,
                });
            }
            for (bus, name) in buses {
                if ui
                    .selectable_label(track.output == Some(bus), name)
                    .clicked()
                    && track.output != Some(bus)
                {
                    ctx.intents.command(SetTrackOutput {
                        track: track.id,
                        output: Some(bus),
                    });
                }
            }
        });
}

fn master_strip(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(4.0)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.set_width(STRIP_WIDTH);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Master").strong().color(theme::ACCENT));
                let master = &ctx.live.master;
                ui.label(format!("M  {}", lufs_text(master.momentary_lufs)));
                ui.label(format!("S  {}", lufs_text(master.short_term_lufs)));
                ui.label(format!("I  {}", lufs_text(master.integrated_lufs)));
                ui.label(format!("TP {}", db_text(master.true_peak_dbtp, "dBTP")));
                let peaks = ctx.live.meters.master();
                let (meter, _) = ui.allocate_exact_size(vec2(28.0, 180.0), Sense::hover());
                paint_stereo_meter(ui, meter, peaks);
            });
        });
}

/// `-14.2 LUFS`, or a dash when there is nothing to measure.
pub fn lufs_text(lufs: f64) -> String {
    db_text(lufs, "LUFS")
}

/// Levels at or below this read as silence (loudness meters return huge
/// negative numbers rather than -inf for near-silence).
pub const SILENCE_DB: f64 = -100.0;

/// A level with its unit, or a dash for silence.
pub fn db_text(value: f64, unit: &str) -> String {
    if value.is_finite() && value > SILENCE_DB {
        format!("{value:.1} {unit}")
    } else {
        format!("– {unit}")
    }
}
