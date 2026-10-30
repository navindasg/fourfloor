//! The arrangement: track headers on the left, a bars-and-beats timeline
//! with clips, automation lanes, the playhead and the loop brace.

mod headers;
mod timeline;

use std::collections::HashSet;

use egui::{pos2, vec2, Rect, Sense, Ui};
use fourfloor_core::{ClipId, Ticks, TrackId};
use fourfloor_model::command::{CreateTrack, DeleteTrack, WriteMidiClip};
use fourfloor_model::{Command, TrackKind};

use crate::geometry::{Snap, TimeMap, BAR_TICKS};
use crate::state::{Intent, TransportIntent, ViewId};
use crate::viewmodel::arrange::{self, layout_rows, ClipDrag};

use super::controls::Previews;
use super::{View, ViewCtx};

/// Width of the track header column.
pub const HEADER_WIDTH: f32 = 210.0;
/// Height of the ruler.
pub const RULER_HEIGHT: f32 = 26.0;

/// The arrangement view and its scroll, zoom and gesture state.
#[derive(Debug)]
pub struct ArrangementView {
    /// Tick at the left edge.
    pub scroll: f64,
    /// Zoom.
    pub px_per_beat: f32,
    /// Tracks whose automation lanes are open.
    pub expanded: HashSet<TrackId>,
    drag: Option<ClipDrag>,
    drag_pointer: Option<(f64, Option<TrackId>)>,
    loop_drag: Option<i64>,
    context_clip: Option<ClipId>,
    previews: Previews,
}

impl Default for ArrangementView {
    fn default() -> Self {
        Self {
            scroll: 0.0,
            px_per_beat: 28.0,
            expanded: HashSet::new(),
            drag: None,
            drag_pointer: None,
            loop_drag: None,
            context_clip: None,
            previews: Previews::default(),
        }
    }
}

impl View for ArrangementView {
    fn id(&self) -> ViewId {
        ViewId::ARRANGEMENT
    }

    fn title(&self) -> &str {
        "Arrangement"
    }

    fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        self.toolbar(ui, ctx);
        ui.add_space(2.0);
        if ctx.project.tracks.is_empty() {
            super::empty::empty_state(
                ui,
                "An empty project",
                "Add a track with + MIDI, + Audio or + Bus, open the demo from the File menu, or ask the assistant (AI) for a drum pattern.",
            );
            return;
        }
        let full = ui.available_rect_before_wrap();
        let timeline_left = full.left() + HEADER_WIDTH;
        let map = TimeMap::new(timeline_left, self.scroll, self.px_per_beat);
        self.follow_playhead(ctx, &map, full.right());
        let map = TimeMap::new(timeline_left, self.scroll, self.px_per_beat);
        let ruler = Rect::from_min_max(
            pos2(timeline_left, full.top()),
            pos2(full.right(), full.top() + RULER_HEIGHT),
        );
        ui.allocate_rect(
            Rect::from_min_max(full.min, pos2(full.right(), ruler.bottom())),
            Sense::hover(),
        );
        self.ruler(ui, ctx, ruler, &map);
        egui::ScrollArea::vertical()
            .id_salt("arrangement-rows")
            .auto_shrink([false, false])
            .show(ui, |ui| self.body(ui, ctx, &map));
        self.handle_wheel(ui, full, timeline_left);
        self.shortcuts(ui, ctx);
    }
}

impl ArrangementView {
    fn toolbar(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        ui.horizontal_wrapped(|ui| {
            for (label, kind) in [
                ("+ MIDI", TrackKind::Midi),
                ("+ Audio", TrackKind::Audio),
                ("+ Bus", TrackKind::Bus),
            ] {
                if ui
                    .button(label)
                    .on_hover_text(format!("Add a {kind} track"))
                    .clicked()
                {
                    let count = ctx.project.tracks.len() + 1;
                    ctx.intents.command(CreateTrack {
                        name: format!(
                            "{} {count}",
                            match kind {
                                TrackKind::Midi => "MIDI",
                                TrackKind::Audio => "Audio",
                                TrackKind::Bus => "Bus",
                            }
                        ),
                        kind,
                        index: None,
                    });
                    if kind == TrackKind::Midi {
                        // A new MIDI track is silent until it has an instrument.
                        ctx.ui.panels.browser = true;
                    }
                }
            }
            if let Some(track) = ctx.selected_track().map(|t| (t.id, t.name.clone())) {
                if ui
                    .button("Delete track")
                    .on_hover_text(format!(
                        "Delete {} (Delete key when no clip is selected)",
                        track.1
                    ))
                    .clicked()
                {
                    ctx.intents.command(DeleteTrack { track: track.0 });
                    ctx.ui.selection = Default::default();
                }
            }
            ui.separator();
            egui::ComboBox::new("arrangement-snap", "snap")
                .selected_text(ctx.ui.snap.label())
                .width(60.0)
                .show_ui(ui, |ui| {
                    for snap in Snap::ALL {
                        ui.selectable_value(&mut ctx.ui.snap, snap, snap.label());
                    }
                });
            if ui.button("-").on_hover_text("Zoom out").clicked() {
                self.px_per_beat =
                    (self.px_per_beat / 1.5).max(crate::geometry::timeline::MIN_PX_PER_BEAT);
            }
            if ui.button("+").on_hover_text("Zoom in").clicked() {
                self.px_per_beat =
                    (self.px_per_beat * 1.5).min(crate::geometry::timeline::MAX_PX_PER_BEAT);
            }
            ui.toggle_value(&mut ctx.ui.follow, "Follow");
            let audio_selected = ctx
                .selected_track()
                .is_some_and(|t| t.kind == TrackKind::Audio);
            if ui
                .add_enabled(audio_selected, egui::Button::new("Import audio..."))
                .on_hover_text("Place an audio file on the selected audio track at the playhead")
                .clicked()
            {
                super::shell::dialogs::choose_path(ctx, crate::state::PathPurpose::ImportAudio);
            }
            ui.separator();
            let selected = ctx.ui.selection.clip;
            ui.add_enabled_ui(selected.is_some(), |ui| {
                if ui
                    .button("Split")
                    .on_hover_text("Split the selected clip at the playhead (S)")
                    .clicked()
                {
                    self.split_selected(ctx);
                }
                if ui.button("Duplicate").on_hover_text("Ctrl+D").clicked() {
                    if let Some(clip) = selected {
                        ctx.intents.push(arrange::duplicate(clip));
                    }
                }
                if ui.button("Delete").on_hover_text("Delete key").clicked() {
                    if let Some(clip) = selected {
                        ctx.intents.push(arrange::delete(clip));
                        ctx.ui.selection.clip = None;
                    }
                }
            });
        });
    }

    fn split_selected(&self, ctx: &mut ViewCtx<'_>) {
        let Some(clip) = ctx.ui.selection.clip else {
            return;
        };
        let at = ctx
            .ui
            .snap
            .round(ctx.live.transport.position.round() as i64);
        match arrange::split(ctx.project, clip, at) {
            Some(command) => ctx.intents.push(command),
            None => ctx.ui.info("Move the playhead inside the clip to split it"),
        }
    }

    fn follow_playhead(&mut self, ctx: &ViewCtx<'_>, map: &TimeMap, right: f32) {
        if !(ctx.ui.follow && ctx.live.transport.playing) {
            return;
        }
        let x = map.x_of(ctx.live.transport.position);
        if x > right - 40.0 || x < map.origin_x {
            self.scroll = (ctx.live.transport.position - 960.0).max(0.0);
        }
    }

    fn handle_wheel(&mut self, ui: &Ui, full: Rect, timeline_left: f32) {
        let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        if !full.contains(pointer) || pointer.x < timeline_left {
            return;
        }
        let (zoom, scroll, shift) =
            ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta(), i.modifiers.shift));
        let map = TimeMap::new(timeline_left, self.scroll, self.px_per_beat);
        if zoom != 1.0 {
            let zoomed = map.zoomed_around(pointer.x, zoom);
            self.scroll = zoomed.scroll;
            self.px_per_beat = zoomed.px_per_beat;
        } else {
            let dx = if shift { scroll.y } else { scroll.x };
            if dx != 0.0 {
                self.scroll = map.scrolled_by(-dx).scroll;
            }
        }
    }

    fn shortcuts(&mut self, ui: &Ui, ctx: &mut ViewCtx<'_>) {
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (delete, duplicate, split) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                i.modifiers.command && i.key_pressed(egui::Key::D),
                !i.modifiers.command && i.key_pressed(egui::Key::S),
            )
        });
        let Some(clip) = ctx.ui.selection.clip else {
            if delete {
                if let Some(track) = ctx.ui.selection.track {
                    ctx.intents.command(DeleteTrack { track });
                    ctx.ui.selection = Default::default();
                }
            }
            return;
        };
        if delete {
            ctx.intents.push(arrange::delete(clip));
            ctx.ui.selection.clip = None;
        } else if duplicate {
            ctx.intents.push(arrange::duplicate(clip));
        } else if split {
            self.split_selected(ctx);
        }
    }

    fn body(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, map: &TimeMap) {
        let top = ui.cursor().top();
        let rows = layout_rows(ctx.project, top, &self.expanded);
        let bottom = rows.last().map_or(top, |row| row.bottom());
        let width = ui.available_width();
        let height = (bottom - top).max(ui.available_height());
        let area = Rect::from_min_size(pos2(ui.cursor().left(), top), vec2(width, height));
        ui.allocate_rect(area, Sense::hover());
        let header_column =
            Rect::from_min_max(area.min, pos2(area.left() + HEADER_WIDTH, area.bottom()));
        let lanes = Rect::from_min_max(pos2(area.left() + HEADER_WIDTH, area.top()), area.max);
        for (index, row) in rows.iter().enumerate() {
            if let Some(track) = ctx.project.track(row.track).cloned() {
                let rect = Rect::from_min_max(
                    pos2(header_column.left(), row.top),
                    pos2(header_column.right() - 2.0, row.bottom()),
                );
                headers::track_header(
                    ui,
                    ctx,
                    &mut self.previews,
                    &mut self.expanded,
                    &track,
                    index,
                    row,
                    rect,
                );
            }
        }
        self.timeline(ui, ctx, lanes, &rows, map);
    }

    fn create_clip_at(&self, ctx: &mut ViewCtx<'_>, track: TrackId, ticks: f64) {
        let Some(owner) = ctx.project.track(track) else {
            return;
        };
        if owner.kind != TrackKind::Midi {
            return;
        }
        let start = Snap::Bar.floor(ticks.max(0.0) as i64);
        ctx.intents.push(Command::WriteMidiClip(WriteMidiClip {
            track,
            clip: None,
            name: format!("{} clip", owner.name),
            start: Ticks::new(start),
            length: Ticks::new(BAR_TICKS),
            notes: Vec::new(),
        }));
    }

    fn seek(&self, ctx: &mut ViewCtx<'_>, ticks: f64) {
        let at = ctx.ui.snap.round(ticks.max(0.0).round() as i64);
        ctx.intents
            .push(Intent::Transport(TransportIntent::Seek(at)));
    }
}
