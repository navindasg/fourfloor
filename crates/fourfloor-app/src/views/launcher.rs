//! The clip launcher: scenes down the side, tracks across, a slot in each
//! cell. Launching waits for the launch-quantize boundary; queued and
//! playing slots are shown.

use egui::{vec2, Color32, Rect, Sense, Stroke, StrokeKind, Ui};
use fourfloor_core::{Ticks, TrackId, TICKS_PER_BEAT};
use fourfloor_model::command::{
    AddScene, ClearSlot, ClipToSlot, RemoveScene, SetLaunchQuantize, WriteMidiSlot,
};
use fourfloor_model::{SceneId, TrackKind};

use crate::geometry::BAR_TICKS;
use crate::services::engine_host::LaunchState;
use crate::state::{Intent, ViewId};
use crate::theme;

use super::{View, ViewCtx};

/// Launch quantize choices: label and ticks.
pub const QUANTIZE: [(&str, i64); 5] = [
    ("None", 0),
    ("1 beat", TICKS_PER_BEAT),
    ("1 bar", BAR_TICKS),
    ("2 bars", BAR_TICKS * 2),
    ("4 bars", BAR_TICKS * 4),
];

/// How a slot is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    /// Nothing launched here.
    Idle,
    /// Launched; starts at the next boundary.
    Queued,
    /// Playing.
    Playing,
}

/// The state of `track`'s slot in `scene` given what was launched and
/// where the playhead is.
pub fn slot_state(launch: Option<LaunchState>, scene: SceneId, playhead: f64) -> SlotState {
    match launch {
        Some(state) if state.scene == Some(scene) => {
            if playhead < state.at as f64 {
                SlotState::Queued
            } else {
                SlotState::Playing
            }
        }
        _ => SlotState::Idle,
    }
}

/// The launcher view.
#[derive(Debug, Default)]
pub struct LauncherView;

const CELL: egui::Vec2 = vec2(118.0, 34.0);

impl View for LauncherView {
    fn id(&self) -> ViewId {
        ViewId::LAUNCHER
    }

    fn title(&self) -> &str {
        "Launcher"
    }

    fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        ui.horizontal_wrapped(|ui| {
            let current = ctx.project.launch_quantize.get();
            let label = QUANTIZE
                .iter()
                .find(|(_, ticks)| *ticks == current)
                .map_or_else(
                    || format!("{current} ticks"),
                    |(label, _)| (*label).to_owned(),
                );
            egui::ComboBox::new("launch-quantize", "launch quantize")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (label, ticks) in QUANTIZE {
                        if ui.selectable_label(ticks == current, label).clicked()
                            && ticks != current
                        {
                            ctx.intents.command(SetLaunchQuantize {
                                ticks: Ticks::new(ticks),
                            });
                        }
                    }
                });
            if ui.button("+ Scene").clicked() {
                ctx.intents.command(AddScene {
                    name: format!("Scene {}", ctx.project.scenes.len() + 1),
                    index: None,
                });
            }
            if ui
                .button("Stop all")
                .on_hover_text("Every launcher back to the arrangement")
                .clicked()
            {
                ctx.intents.push(Intent::LaunchScene(None));
            }
            if !ctx.live.transport.playing {
                ui.label(
                    egui::RichText::new("Launches start when the transport plays")
                        .color(theme::TEXT_DIM),
                );
            }
        });
        ui.add_space(6.0);
        egui::ScrollArea::both()
            .id_salt("launcher")
            .show(ui, |ui| self.grid(ui, ctx));
    }
}

impl LauncherView {
    fn grid(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        let tracks: Vec<(TrackId, String, TrackKind, usize)> = ctx
            .project
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind != TrackKind::Bus)
            .map(|(i, t)| (t.id, t.name.clone(), t.kind, i))
            .collect();
        let scenes: Vec<(SceneId, String)> = ctx
            .project
            .scenes
            .iter()
            .map(|s| (s.id, s.name.clone()))
            .collect();
        egui::Grid::new("launcher-grid")
            .spacing(vec2(4.0, 4.0))
            .show(ui, |ui| {
                ui.label("");
                for (_, name, _, _) in &tracks {
                    ui.add_sized(
                        CELL,
                        egui::Label::new(egui::RichText::new(name).strong()).truncate(),
                    );
                }
                ui.end_row();
                for (scene, scene_name) in &scenes {
                    ui.horizontal(|ui| {
                        if ui
                            .button(">")
                            .on_hover_text(format!("Launch scene {scene_name}"))
                            .clicked()
                        {
                            ctx.intents.push(Intent::LaunchScene(Some(*scene)));
                        }
                        ui.label(scene_name);
                        if ui.small_button("x").on_hover_text("Remove scene").clicked() {
                            ctx.intents.command(RemoveScene { scene: *scene });
                        }
                    });
                    for (track, track_name, kind, index) in &tracks {
                        self.slot(ui, ctx, *track, track_name, *kind, *index, *scene);
                    }
                    ui.end_row();
                }
                ui.label("");
                for (track, name, _, _) in &tracks {
                    let launch = ctx.live.launches.get(track).copied();
                    let back = matches!(launch, Some(LaunchState { scene: Some(_), .. }));
                    if ui
                        .add_enabled(back, egui::Button::new("To arrangement").min_size(CELL))
                        .on_hover_text(format!("Return {name} to the arrangement"))
                        .clicked()
                    {
                        ctx.intents.push(Intent::Launch {
                            track: *track,
                            scene: None,
                        });
                    }
                }
                ui.end_row();
            });
        if scenes.is_empty() {
            super::empty::empty_state(
                ui,
                "No scenes yet",
                "Add one with + Scene, then copy clips into its slots.",
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn slot(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        track: TrackId,
        track_name: &str,
        kind: TrackKind,
        index: usize,
        scene: SceneId,
    ) {
        let (rect, response) = ui.allocate_exact_size(CELL, Sense::click());
        let clip = ctx
            .project
            .track(track)
            .and_then(|t| t.slot(scene))
            .map(|slot| slot.clip.clone());
        let state = slot_state(
            ctx.live.launches.get(&track).copied(),
            scene,
            ctx.live.transport.position,
        );
        let label = format!(
            "Slot {track_name} {}",
            ctx.project.scene(scene).map_or("", |s| s.name.as_str())
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &label));
        let painter = ui.painter_at(rect);
        let color = theme::track_color(index);
        let (fill, text): (Color32, String) = match &clip {
            Some(clip) => (theme::dim(color, 0.55), clip.name.clone()),
            None => (theme::SURFACE, String::new()),
        };
        painter.rect_filled(rect, 3.0, fill);
        let border = match state {
            SlotState::Idle => Stroke::new(1.0, theme::GRID_BAR),
            SlotState::Queued => {
                // Blink while waiting for the boundary.
                let on = (ui.input(|i| i.time) * 4.0) as i64 % 2 == 0;
                ui.ctx().request_repaint();
                Stroke::new(2.0, if on { theme::ACCENT_2 } else { theme::GRID_BAR })
            }
            SlotState::Playing => Stroke::new(2.0, theme::ACCENT_2),
        };
        painter.rect_stroke(rect, 3.0, border, StrokeKind::Inside);
        let icon = match (state, clip.is_some()) {
            (SlotState::Playing, _) => ">",
            (SlotState::Queued, _) => "...",
            (_, true) => "-",
            (_, false) => "",
        };
        painter.text(
            rect.left_center() + vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            icon,
            egui::FontId::proportional(12.0),
            theme::TEXT,
        );
        painter.text(
            rect.left_center() + vec2(20.0, 0.0),
            egui::Align2::LEFT_CENTER,
            crate::widgets::truncate(&text, 14),
            egui::FontId::proportional(11.0),
            theme::TEXT,
        );
        if response.clicked() {
            ctx.ui.selection.select_track(track);
            match &clip {
                Some(clip) => {
                    ctx.ui.selection.clip = Some(clip.id);
                    ctx.intents.push(Intent::Launch {
                        track,
                        scene: Some(scene),
                    });
                }
                None if kind == TrackKind::Midi => {
                    ctx.intents.command(WriteMidiSlot {
                        track,
                        scene,
                        name: format!("{track_name} slot"),
                        length: Ticks::new(BAR_TICKS),
                        notes: Vec::new(),
                    });
                }
                None => {}
            }
        }
        let selected_clip = ctx.ui.selection.clip.filter(|id| {
            ctx.project
                .track(track)
                .is_some_and(|t| t.clip(*id).is_some())
        });
        response.context_menu(|ui| {
            if let Some(clip) = &clip {
                if ui.button("Edit in piano roll").clicked() {
                    ctx.ui.selection.clip = Some(clip.id);
                    ctx.ui.open_view(ViewId::PIANO_ROLL);
                    ui.close();
                }
                if ui.button("Clear slot").clicked() {
                    ctx.intents.command(ClearSlot { track, scene });
                    ui.close();
                }
            }
            if let Some(timeline_clip) = selected_clip {
                if ui.button("Copy selected timeline clip here").clicked() {
                    ctx.intents.command(ClipToSlot {
                        clip: timeline_clip,
                        scene,
                    });
                    ui.close();
                }
            }
        });
        let _ = Rect::NOTHING;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_queued_until_the_boundary_then_play() {
        let scene = SceneId(4);
        let launch = Some(LaunchState {
            scene: Some(scene),
            at: 3840,
        });
        assert_eq!(slot_state(launch, scene, 100.0), SlotState::Queued);
        assert_eq!(slot_state(launch, scene, 3840.0), SlotState::Playing);
        assert_eq!(slot_state(launch, SceneId(5), 5000.0), SlotState::Idle);
        assert_eq!(slot_state(None, scene, 0.0), SlotState::Idle);
    }
}
