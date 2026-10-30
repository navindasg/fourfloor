//! The step sequencer for drum tracks: lanes of sixteenth steps, per-step
//! velocity, and the pattern's swing and groove template.

use std::collections::HashMap;

use egui::{pos2, vec2, Rect, Sense, Stroke, StrokeKind, Ui};
use fourfloor_core::{ClipId, Ticks, TrackId};
use fourfloor_model::{Clip, GrooveTemplate, BUILTIN_GROOVES};

use crate::geometry::{pitch_name, CellGrid};
use crate::state::{Intent, ViewId};
use crate::theme;
use crate::viewmodel::steps::{drum_name, StepPattern, DEFAULT_LANES, KIT_PITCHES, STEPS_PER_BAR};

use super::{View, ViewCtx};

/// Width of the lane label column.
pub const LABEL_WIDTH: f32 = 110.0;
/// Height of one lane.
pub const LANE_HEIGHT: f32 = 34.0;
/// Height of the velocity editor.
pub const VELOCITY_HEIGHT: f32 = 110.0;

/// Per-pattern settings the model does not store (it bakes them into the
/// notes): swing and groove name, by clip.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatternFeel {
    /// Swing percent; `None` follows the project swing.
    pub swing: Option<f64>,
    /// Built-in groove template name.
    pub groove: Option<&'static str>,
}

/// The step sequencer view.
#[derive(Debug, Default)]
pub struct SequencerView {
    /// Feel per clip.
    pub feel: HashMap<ClipId, PatternFeel>,
    /// Lanes added by hand, by clip. A lane with no steps has no notes, so
    /// it cannot be read back from the clip and is kept here instead.
    pub extra_lanes: HashMap<ClipId, Vec<u8>>,
    /// The lane whose velocities are shown.
    pub velocity_lane: usize,
    /// Last laid-out grid, for tests and hit-testing.
    pub last_grid: Option<CellGrid>,
}

fn lane_label(pitch: u8) -> String {
    let name = drum_name(pitch);
    if name.is_empty() {
        pitch_name(pitch)
    } else {
        name.to_owned()
    }
}

impl View for SequencerView {
    fn id(&self) -> ViewId {
        ViewId::SEQUENCER
    }

    fn title(&self) -> &str {
        "Sequencer"
    }

    fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        let Some(track) = ctx.selected_track().map(|t| (t.id, t.name.clone(), t.kind)) else {
            super::empty::empty_state(
                ui,
                "No track selected",
                "Select a MIDI track (click its header in the arrangement) to edit its steps.",
            );
            return;
        };
        let (track_id, track_name, kind) = track;
        if kind != fourfloor_model::TrackKind::Midi {
            super::empty::empty_state(
                ui,
                &format!("{track_name} is not a MIDI track"),
                "The step sequencer edits drum patterns on MIDI tracks.",
            );
            return;
        }
        let Some((_, clip)) = ctx.focused_midi_clip().map(|(t, c)| (t.id, c.clone())) else {
            ui.horizontal(|ui| {
                ui.label(format!("{track_name} has no clip yet."));
                if ui.button("Create 1-bar pattern").clicked() {
                    let pattern = StepPattern::empty(1);
                    let at = Ticks::new(0);
                    ctx.intents.command(pattern.to_command(
                        track_id,
                        None,
                        &format!("{track_name} pattern"),
                        at,
                        None,
                        None,
                    ));
                }
            });
            return;
        };
        let read = StepPattern::from_clip(&clip);
        let groove = self
            .feel
            .get(&clip.id)
            .and_then(|feel| feel.groove)
            .and_then(GrooveTemplate::builtin);
        let pattern = match groove {
            Some(groove) => read.without_groove(&groove),
            None => read,
        };
        let pattern = match self.extra_lanes.get(&clip.id) {
            Some(pitches) => pattern.with_lanes(pitches),
            None => pattern,
        };
        self.header(ui, ctx, track_id, &track_name, &clip, &pattern);
        ui.add_space(6.0);
        egui::ScrollArea::both()
            .id_salt("sequencer")
            .show(ui, |ui| {
                self.grid(ui, ctx, track_id, &clip, pattern);
            });
    }
}

impl SequencerView {
    fn write(&self, ctx: &mut ViewCtx<'_>, track: TrackId, clip: &Clip, pattern: &StepPattern) {
        let feel = self.feel.get(&clip.id).cloned().unwrap_or_default();
        ctx.intents.command(pattern.to_command(
            track,
            Some(clip.id),
            &clip.name,
            clip.start,
            feel.swing,
            feel.groove,
        ));
    }

    fn header(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        track: TrackId,
        track_name: &str,
        clip: &Clip,
        pattern: &StepPattern,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(format!("{track_name} · {}", clip.name)).strong());
            ui.separator();
            ui.label("Bars");
            for bars in [1u32, 2, 4, 8] {
                if ui
                    .selectable_label(pattern.bars == bars, bars.to_string())
                    .clicked()
                    && pattern.bars != bars
                {
                    let resized = resize(pattern, bars);
                    self.write(ctx, track, clip, &resized);
                }
            }
            ui.separator();
            let project_swing = ctx.project.groove.swing_percent;
            let mut feel = self.feel.get(&clip.id).cloned().unwrap_or_default();
            let mut swing = feel.swing.unwrap_or(project_swing);
            let response = ui.add(
                egui::Slider::new(&mut swing, 50.0..=75.0)
                    .text("swing %")
                    .fixed_decimals(0),
            );
            if response.drag_stopped() || (response.changed() && !response.dragged()) {
                feel.swing = Some(swing);
                self.feel.insert(clip.id, feel.clone());
                self.write(ctx, track, clip, pattern);
            } else if response.changed() {
                feel.swing = Some(swing);
                self.feel.insert(clip.id, feel.clone());
            }
            let current = feel.groove.unwrap_or("none");
            egui::ComboBox::new("groove", "groove")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    let mut choice = None;
                    if ui.selectable_label(feel.groove.is_none(), "none").clicked() {
                        choice = Some(None);
                    }
                    for name in BUILTIN_GROOVES {
                        if ui
                            .selectable_label(feel.groove == Some(*name), *name)
                            .clicked()
                        {
                            choice = Some(Some(*name));
                        }
                    }
                    if let Some(groove) = choice {
                        let mut updated = feel.clone();
                        updated.groove = groove;
                        self.feel.insert(clip.id, updated);
                        self.write(ctx, track, clip, pattern);
                    }
                });
            ui.menu_button("+ Add sound", |ui| {
                let missing: Vec<u8> = KIT_PITCHES
                    .iter()
                    .copied()
                    .filter(|pitch| !pattern.lanes.iter().any(|l| l.pitch == *pitch))
                    .collect();
                if missing.is_empty() {
                    ui.label("Every drum kit sound is already a lane.");
                }
                for pitch in missing {
                    if ui.button(lane_label(pitch)).clicked() {
                        let lanes = self.extra_lanes.entry(clip.id).or_default();
                        if !lanes.contains(&pitch) {
                            lanes.push(pitch);
                        }
                        ctx.intents.push(Intent::PreviewNote {
                            track,
                            pitch,
                            velocity: 100,
                        });
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("Add a lane for another drum kit sound");
        });
    }

    fn grid(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        track: TrackId,
        clip: &Clip,
        pattern: StepPattern,
    ) {
        let steps = pattern.steps();
        let width = (ui.available_width() - LABEL_WIDTH - 8.0).max(steps as f32 * 14.0);
        let height = pattern.lanes.len() as f32 * (LANE_HEIGHT + 2.0) + VELOCITY_HEIGHT + 24.0;
        let (area, _) =
            ui.allocate_exact_size(vec2(LABEL_WIDTH + 8.0 + width, height), Sense::hover());
        let grid = CellGrid::fitted(
            pos2(area.left() + LABEL_WIDTH + 8.0, area.top()),
            width,
            LANE_HEIGHT,
            pattern.lanes.len(),
            steps,
            4,
        );
        self.last_grid = Some(grid);
        let playing_step = playhead_step(ctx.live.transport.position, clip, steps);
        let mut edited: Option<StepPattern> = None;
        for (lane_index, lane) in pattern.lanes.iter().enumerate() {
            let label_rect = Rect::from_min_size(
                pos2(area.left(), grid.rect(lane_index, 0).top()),
                vec2(LABEL_WIDTH, LANE_HEIGHT),
            );
            let label = ui.interact(
                label_rect,
                ui.scope_id().with(("lane", lane.pitch)),
                Sense::click(),
            );
            let painter = ui.painter();
            let chosen = self.velocity_lane == lane_index;
            painter.rect_filled(
                label_rect,
                2.0,
                if chosen {
                    theme::RAISED
                } else {
                    theme::SURFACE
                },
            );
            painter.text(
                label_rect.left_center() + vec2(6.0, 0.0),
                egui::Align2::LEFT_CENTER,
                lane_label(lane.pitch),
                egui::FontId::proportional(12.0),
                theme::TEXT,
            );
            if label.clicked() {
                self.velocity_lane = lane_index;
                ctx.intents.push(Intent::PreviewNote {
                    track,
                    pitch: lane.pitch,
                    velocity: 100,
                });
            }
            for step in 0..steps {
                let rect = grid.rect(lane_index, step);
                let on = pattern.is_on(lane_index, step);
                let id = ui.scope_id().with(("step", lane.pitch, step));
                let response = ui.interact(rect, id, Sense::click());
                let accessible = format!("Step {} {}", lane_label(lane.pitch), step + 1);
                response.widget_info(|| {
                    egui::WidgetInfo::selected(egui::Role::Button, true, on, &accessible)
                });
                let beat_shade = (step / 4) % 2 == 0;
                let fill = if on {
                    let velocity = f32::from(pattern.velocity(lane_index, step)) / 127.0;
                    theme::dim(theme::ACCENT, 0.6 * (1.0 - velocity))
                } else if beat_shade {
                    theme::RAISED
                } else {
                    theme::SURFACE
                };
                let painter = ui.painter();
                painter.rect_filled(rect, 2.0, fill);
                if Some(step) == playing_step {
                    painter.rect_stroke(
                        rect,
                        2.0,
                        Stroke::new(1.5, theme::ACCENT_2),
                        StrokeKind::Inside,
                    );
                } else if response.hovered() {
                    painter.rect_stroke(
                        rect,
                        2.0,
                        Stroke::new(1.0, theme::TEXT_DIM),
                        StrokeKind::Inside,
                    );
                }
                if response.clicked() {
                    let mut next = edited.clone().unwrap_or_else(|| pattern.clone());
                    next.toggle(lane_index, step);
                    self.velocity_lane = lane_index;
                    edited = Some(next);
                }
            }
        }
        let velocity_top = grid.rect(pattern.lanes.len().saturating_sub(1), 0).bottom() + 16.0;
        if let Some(next) = self.velocity_editor(ui, &pattern, &grid, velocity_top, area.left()) {
            edited = Some(next);
        }
        if playing_step.is_some() {
            ui.ctx().request_repaint();
        }
        if let Some(next) = edited {
            self.write(ctx, track, clip, &next);
        }
    }

    fn velocity_editor(
        &mut self,
        ui: &mut Ui,
        pattern: &StepPattern,
        grid: &CellGrid,
        top: f32,
        left: f32,
    ) -> Option<StepPattern> {
        let lane = self
            .velocity_lane
            .min(pattern.lanes.len().saturating_sub(1));
        let row = pattern.lanes.get(lane)?;
        ui.painter().text(
            pos2(left + 6.0, top + VELOCITY_HEIGHT / 2.0),
            egui::Align2::LEFT_CENTER,
            format!("Velocity\n{}", lane_label(row.pitch)),
            egui::FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
        let mut result = None;
        for step in 0..pattern.steps() {
            let column = grid.rect(0, step);
            let rect = Rect::from_min_max(
                pos2(column.left(), top),
                pos2(column.right(), top + VELOCITY_HEIGHT),
            );
            let response = ui.interact(
                rect,
                ui.scope_id().with(("velocity", step)),
                Sense::click_and_drag(),
            );
            let accessible = format!("Velocity {} {}", lane_label(row.pitch), step + 1);
            response.widget_info(|| {
                egui::WidgetInfo::slider(true, f64::from(pattern.velocity(lane, step)), &accessible)
            });
            let painter = ui.painter();
            painter.rect_filled(rect, 1.0, theme::CANVAS);
            let velocity = pattern.velocity(lane, step);
            if velocity > 0 {
                let h = rect.height() * f32::from(velocity) / 127.0;
                let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - h), rect.max);
                painter.rect_filled(bar, 1.0, theme::ACCENT);
            }
            if response.clicked() || response.drag_stopped() {
                if let Some(pos) = response.interact_pointer_pos() {
                    let t = ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0);
                    let value = (t * 127.0).round().max(1.0) as u8;
                    let mut next = pattern.clone();
                    next.set_velocity(lane, step, value);
                    result = Some(next);
                }
            }
        }
        result
    }
}

/// The step the playhead is on inside `clip`, if it is inside.
pub fn playhead_step(position: f64, clip: &Clip, steps: usize) -> Option<usize> {
    let offset = position - clip.start.get() as f64;
    if offset < 0.0 || offset >= clip.length.get() as f64 {
        return None;
    }
    let step = (offset / crate::geometry::SIXTEENTH_TICKS as f64) as usize;
    (step < steps).then_some(step)
}

/// The pattern at a new length: longer repeats the existing bars,
/// shorter cuts.
pub fn resize(pattern: &StepPattern, bars: u32) -> StepPattern {
    let old_steps = pattern.steps().max(1);
    let new_steps = bars as usize * STEPS_PER_BAR;
    let lanes = pattern
        .lanes
        .iter()
        .map(|lane| {
            let mut out = lane.clone();
            out.steps = (0..new_steps).map(|s| lane.steps[s % old_steps]).collect();
            out.velocities = (0..new_steps)
                .map(|s| lane.velocities.get(s % old_steps).copied().unwrap_or(0))
                .collect();
            out
        })
        .collect();
    StepPattern { bars, lanes }
}

/// Lanes a new drum clip starts with, for display.
pub fn default_lane_names() -> Vec<String> {
    DEFAULT_LANES.iter().map(|p| lane_label(*p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::{ClipContent, MidiNote};

    #[test]
    fn resizing_repeats_or_cuts_bars() {
        let mut one = StepPattern::empty(1);
        one.toggle(0, 0);
        one.toggle(0, 10);
        let two = resize(&one, 2);
        assert_eq!(two.steps(), 32);
        assert!(two.is_on(0, 16) && two.is_on(0, 26));
        let back = resize(&two, 1);
        assert_eq!(back, one);
        assert_eq!(
            default_lane_names(),
            ["Kick", "Clap", "Closed hat", "Open hat"]
        );
    }

    #[test]
    fn the_playhead_lands_on_a_step_inside_the_clip_only() {
        let clip = Clip {
            id: ClipId(1),
            name: "c".to_owned(),
            start: Ticks::new(3840),
            length: Ticks::new(3840),
            offset: Ticks::ZERO,
            loop_length: None,
            content: ClipContent::Midi {
                notes: Vec::<MidiNote>::new(),
            },
        };
        assert_eq!(playhead_step(3840.0 + 250.0, &clip, 16), Some(1));
        assert_eq!(playhead_step(100.0, &clip, 16), None);
        assert_eq!(playhead_step(7680.0, &clip, 16), None);
    }
}
