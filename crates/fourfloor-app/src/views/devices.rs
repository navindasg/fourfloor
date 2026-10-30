//! The device dock: the selected track's chain as cards whose knobs are
//! generated from parameter descriptors (any device, hosted plugins too),
//! macro knobs, and modulation, macro, automation and MIDI-learn
//! assignment from each knob's context menu.

use std::collections::HashMap;

use egui::{vec2, Ui};
use fourfloor_core::{DeviceId, ParamId, Ticks};
use fourfloor_model::command::{
    AddMacro, RemoveDevice, RemoveMacro, SetDeviceBypass, SetDeviceSidechain, SetMacro,
    WriteAutomation,
};
use fourfloor_model::{AutomationPoint, DeviceInstance, Track};

use crate::state::Intent;
use crate::theme;
use crate::viewmodel::mixer::{macro_target, modulate, new_lfo};
use crate::viewmodel::params::{controls_for, ParamControl};
use crate::widgets::knob;

use super::ViewCtx;

/// Knobs shown per card before "more".
pub const KNOBS_PER_CARD: usize = 16;

/// The device dock's gesture state.
#[derive(Debug, Default)]
pub struct DevicePanel {
    previews: HashMap<(DeviceId, ParamId), f32>,
    macro_previews: HashMap<fourfloor_model::MacroId, f32>,
    expanded: HashMap<DeviceId, bool>,
}

impl DevicePanel {
    /// Draws the dock for the selected track.
    pub fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        ui.horizontal(|ui| {
            self.macros(ui, ctx);
            ui.separator();
            let Some(track) = ctx.selected_track().cloned() else {
                ui.label("Select a track to see its devices.");
                return;
            };
            egui::ScrollArea::horizontal()
                .id_salt("device-chain")
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for device in &track.devices {
                            self.card(ui, ctx, &track, device);
                        }
                        if ui.button("+ Add device").clicked() {
                            ctx.ui.panels.browser = true;
                        }
                    });
                });
        });
    }

    fn macros(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("Macros").small().color(theme::TEXT_DIM));
            ui.horizontal_wrapped(|ui| {
                ui.set_max_width(220.0);
                let macros: Vec<_> = ctx
                    .project
                    .macros
                    .iter()
                    .map(|m| (m.id, m.name.clone(), m.value))
                    .collect();
                for (id, name, value) in macros {
                    let shown = self.macro_previews.get(&id).copied().unwrap_or(value);
                    let output = knob(
                        ui,
                        &name,
                        &format!("{:.0}%", shown * 100.0),
                        shown,
                        0.0,
                        30.0,
                    );
                    if let Some(t) = output.changed {
                        self.macro_previews.insert(id, t);
                    }
                    if output.committed {
                        let value = self.macro_previews.remove(&id).unwrap_or(shown);
                        ctx.intents.command(SetMacro {
                            macro_id: id,
                            value,
                        });
                    }
                    output
                        .response
                        .on_hover_text("Right-click to remove")
                        .context_menu(|ui| {
                            if ui.button(format!("Remove {name}")).clicked() {
                                ctx.intents.command(RemoveMacro { macro_id: id });
                                ui.close();
                            }
                        });
                }
                if ui.small_button("+").on_hover_text("Add a macro").clicked() {
                    ctx.intents.command(AddMacro {
                        name: format!("Macro {}", ctx.project.macros.len() + 1),
                        value: 0.0,
                    });
                }
            });
        });
    }

    fn card(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, track: &Track, device: &DeviceInstance) {
        let descriptor = ctx.registry.descriptor(&device.kind);
        let controls = controls_for(device, descriptor);
        let selected = ctx.ui.selection.device == Some(device.id);
        let frame = egui::Frame::new()
            .fill(if selected {
                theme::RAISED
            } else {
                theme::SURFACE
            })
            .stroke(egui::Stroke::new(
                1.0,
                if selected {
                    theme::ACCENT
                } else {
                    theme::GRID_BEAT
                },
            ))
            .corner_radius(4.0)
            .inner_margin(6.0);
        frame.show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let title = egui::RichText::new(&device.name).strong();
                    if ui
                        .add(egui::Label::new(title).sense(egui::Sense::click()))
                        .clicked()
                    {
                        ctx.ui.selection.device = Some(device.id);
                    }
                    ui.label(
                        egui::RichText::new(&device.kind)
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                    let bypass = if device.bypassed { "Off" } else { "On" };
                    if ui.small_button(bypass).on_hover_text("Bypass").clicked() {
                        ctx.intents.command(SetDeviceBypass {
                            device: device.id,
                            bypassed: !device.bypassed,
                        });
                    }
                    if ui
                        .small_button("x")
                        .on_hover_text(format!("Remove {}", device.name))
                        .clicked()
                    {
                        ctx.intents.command(RemoveDevice { device: device.id });
                    }
                });
                if descriptor.is_some_and(|d| d.sidechain) {
                    sidechain_combo(ui, ctx, track, device);
                }
                let all = *self.expanded.get(&device.id).unwrap_or(&false);
                let shown = if all {
                    controls.len()
                } else {
                    controls.len().min(KNOBS_PER_CARD)
                };
                egui::Grid::new(("knobs", device.id))
                    .spacing(vec2(2.0, 2.0))
                    .show(ui, |ui| {
                        for (index, control) in controls.iter().take(shown).enumerate() {
                            self.param_knob(ui, ctx, track, device, control);
                            if index % 8 == 7 {
                                ui.end_row();
                            }
                        }
                    });
                if controls.len() > KNOBS_PER_CARD {
                    let label = if all { "fewer" } else { "more…" };
                    if ui.small_button(label).clicked() {
                        self.expanded.insert(device.id, !all);
                    }
                }
                if controls.is_empty() {
                    ui.label(egui::RichText::new("no parameters").color(theme::TEXT_DIM));
                }
            });
        });
    }

    fn param_knob(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        track: &Track,
        device: &DeviceInstance,
        control: &ParamControl,
    ) {
        let key = (device.id, control.id());
        let mut shown = control.clone();
        if let Some(value) = self.previews.get(&key) {
            shown.value = *value;
        }
        let modulated = ctx
            .project
            .modulators
            .iter()
            .any(|m| m.target(device.id, control.id()).is_some())
            || ctx
                .project
                .macros
                .iter()
                .any(|m| m.target(device.id, control.id()).is_some());
        let label = if modulated {
            format!("{}•", control.info.name)
        } else {
            control.info.name.to_string()
        };
        let default = control.info.to_normalized(control.info.default);
        let output = knob(
            ui,
            &label,
            &shown.display(),
            shown.normalized(),
            default,
            30.0,
        );
        if let Some(t) = output.changed {
            let value = control.value_at(t);
            self.previews.insert(key, value);
            ctx.intents.push(Intent::LiveParam {
                device: device.id,
                param: control.id(),
                value,
            });
        }
        if output.committed {
            let value = self.previews.remove(&key).unwrap_or(shown.value);
            ctx.intents.command(control.command(device.id, value));
        }
        output
            .response
            .context_menu(|ui| self.param_menu(ui, ctx, track, device, control));
    }

    fn param_menu(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        track: &Track,
        device: &DeviceInstance,
        control: &ParamControl,
    ) {
        ui.label(egui::RichText::new(control.info.name.as_ref()).strong());
        if ui.button("Reset to default").clicked() {
            ctx.intents
                .command(control.command(device.id, control.info.default));
            ui.close();
        }
        if ui.button("Automate (add lane)").clicked() {
            if track.lane(device.id, control.id()).is_none() {
                ctx.intents.command(WriteAutomation {
                    device: device.id,
                    param: control.id(),
                    points: vec![AutomationPoint {
                        time: Ticks::ZERO,
                        value: control.value,
                    }],
                });
            }
            ctx.ui
                .info("Lane added: open the track's automation (A) in the arrangement");
            ui.close();
        }
        ui.menu_button("Map to macro", |ui| {
            for m in &ctx.project.macros {
                if ui.button(&m.name).clicked() {
                    ctx.intents.command(macro_target(
                        m.id,
                        device.id,
                        control.id(),
                        control.info.min,
                        control.info.max,
                    ));
                    ui.close();
                }
            }
            if ctx.project.macros.is_empty() {
                ui.label("No macros yet");
            }
        });
        ui.menu_button("Modulate", |ui| {
            for m in &ctx.project.modulators {
                if ui.button(format!("{} (25%)", m.name)).clicked() {
                    ctx.intents
                        .command(modulate(m.id, device.id, control.id(), 0.25));
                    ui.close();
                }
            }
            if ui.button("New LFO (1 bar)").clicked() {
                ctx.intents.command(new_lfo(
                    &format!("LFO {}", ctx.project.modulators.len() + 1),
                    4.0,
                ));
                ctx.ui.info("LFO added: open Modulate again to assign it");
                ui.close();
            }
        });
        if ui.button("MIDI learn").clicked() {
            ctx.intents.push(Intent::MidiLearn {
                device: device.id,
                param: control.id(),
                min: control.info.min,
                max: control.info.max,
            });
            ui.close();
        }
    }
}

fn sidechain_combo(ui: &mut Ui, ctx: &mut ViewCtx<'_>, track: &Track, device: &DeviceInstance) {
    let current = device
        .sidechain
        .and_then(|s| ctx.project.track(s.track))
        .map_or("none".to_owned(), |t| t.name.clone());
    egui::ComboBox::new(("sidechain", device.id), "sidechain key")
        .selected_text(current)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(device.sidechain.is_none(), "none")
                .clicked()
            {
                ctx.intents.command(SetDeviceSidechain {
                    device: device.id,
                    source: None,
                    pre_fader: false,
                });
            }
            for other in ctx.project.tracks.iter().filter(|t| t.id != track.id) {
                let chosen = device.sidechain.is_some_and(|s| s.track == other.id);
                if ui.selectable_label(chosen, &other.name).clicked() {
                    ctx.intents.command(SetDeviceSidechain {
                        device: device.id,
                        source: Some(other.id),
                        pre_fader: true,
                    });
                }
            }
        });
}
