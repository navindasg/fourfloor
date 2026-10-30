//! The piano roll: draw, move and resize notes, a velocity lane, scale
//! highlighting with snap-to-scale, quantize and a chord tool.

use egui::{pos2, vec2, Rect, Sense, Stroke, StrokeKind, Ui};
use fourfloor_core::{ClipId, Ticks, TrackId};
use fourfloor_model::command::{QuantizeClip, SnapClipToScale};
use fourfloor_model::{ChordKind, Clip, MidiNote, Scale};

use crate::geometry::{is_black_key, pitch_name, GridKind, KeyMap, TimeMap};
use crate::state::{Intent, ViewId};
use crate::theme;
use crate::viewmodel::notes::{self, hit_note, NotePart, CHORDS, EDGE_GRAB_PX};

use super::{View, ViewCtx};

/// Width of the keyboard column.
pub const KEYS_WIDTH: f32 = 56.0;
/// Height of the velocity lane.
pub const VELOCITY_LANE: f32 = 80.0;

/// What a click on empty grid does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    /// Click draws a note; drag moves or resizes.
    Draw,
    /// Click stamps a chord.
    Chord,
}

/// Scales offered for highlighting.
pub const SCALES: [(Scale, &str); 6] = [
    (Scale::Minor, "minor"),
    (Scale::Major, "major"),
    (Scale::Dorian, "dorian"),
    (Scale::Phrygian, "phrygian"),
    (Scale::MinorPentatonic, "minor pent."),
    (Scale::Chromatic, "chromatic"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
struct NoteDrag {
    index: usize,
    part: NotePart,
    grab_ticks: f64,
    grab_pitch: u8,
}

/// The piano roll view.
#[derive(Debug)]
pub struct PianoRollView {
    /// Tick at the left edge (relative to the clip start).
    pub scroll: f64,
    /// Zoom.
    pub px_per_beat: f32,
    /// Vertical position and row height.
    pub keys: KeyMap,
    /// Current tool.
    pub tool: Tool,
    /// Chord for the chord tool.
    pub chord: ChordKind,
    /// Scale root (pitch class) and scale for highlighting.
    pub scale: (u8, Scale),
    /// Length of drawn notes, in ticks.
    pub note_length: i64,
    drag: Option<NoteDrag>,
    selected: Option<usize>,
    /// The clip last shown; a different one is scrolled into view.
    shown: Option<ClipId>,
}

impl Default for PianoRollView {
    fn default() -> Self {
        Self {
            scroll: 0.0,
            px_per_beat: 80.0,
            keys: KeyMap {
                top_y: 0.0,
                row_height: 12.0,
                top_pitch: 84,
            },
            tool: Tool::Draw,
            chord: ChordKind::Minor,
            scale: (9, Scale::Minor),
            note_length: 240,
            drag: None,
            selected: None,
            shown: None,
        }
    }
}

impl View for PianoRollView {
    fn id(&self) -> ViewId {
        ViewId::PIANO_ROLL
    }

    fn title(&self) -> &str {
        "Piano roll"
    }

    fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        let Some((track, clip)) = ctx.focused_midi_clip().map(|(t, c)| (t.id, c.clone())) else {
            super::empty::empty_state(
                ui,
                "No clip to edit",
                "Select a MIDI clip (click it in the arrangement or launcher) to edit its notes.",
            );
            return;
        };
        self.toolbar(ui, ctx, track, &clip);
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, Sense::hover());
        let grid_rect = Rect::from_min_max(
            pos2(area.left() + KEYS_WIDTH, area.top()),
            pos2(area.right(), area.bottom() - VELOCITY_LANE - 4.0),
        );
        let keys_rect =
            Rect::from_min_max(area.min, pos2(area.left() + KEYS_WIDTH, grid_rect.bottom()));
        let velocity_rect =
            Rect::from_min_max(pos2(grid_rect.left(), grid_rect.bottom() + 4.0), area.max);
        if self.shown != Some(clip.id) {
            self.shown = Some(clip.id);
            let pitches = clip.notes().iter().map(|note| note.pitch);
            if let (Some(low), Some(high)) = (pitches.clone().min(), pitches.max()) {
                self.keys = self.keys.centred_on(low, high, grid_rect.height());
            }
        }
        let keys = KeyMap {
            top_y: grid_rect.top(),
            ..self.keys
        };
        let map = TimeMap::new(grid_rect.left(), self.scroll, self.px_per_beat);
        self.paint_keys(ui, ctx, track, keys_rect, &keys);
        self.grid(ui, ctx, track, &clip, grid_rect, &keys, &map);
        self.velocity_lane(ui, ctx, track, &clip, velocity_rect, &map);
        self.wheel(ui, grid_rect);
    }
}

impl PianoRollView {
    fn toolbar(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, _track: TrackId, clip: &Clip) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&clip.name).strong());
            ui.separator();
            ui.selectable_value(&mut self.tool, Tool::Draw, "Draw");
            ui.selectable_value(&mut self.tool, Tool::Chord, "Chord");
            if self.tool == Tool::Chord {
                egui::ComboBox::new("chord-kind", "chord")
                    .selected_text(
                        CHORDS
                            .iter()
                            .find(|(k, _)| *k == self.chord)
                            .map_or("?", |(_, l)| l),
                    )
                    .width(60.0)
                    .show_ui(ui, |ui| {
                        for (kind, label) in CHORDS {
                            ui.selectable_value(&mut self.chord, kind, label);
                        }
                    });
            }
            ui.separator();
            ui.label("Length");
            for (label, ticks) in [
                ("1/16", 240i64),
                ("1/8", 480),
                ("1/4", 960),
                ("1 bar", 3840),
            ] {
                ui.selectable_value(&mut self.note_length, ticks, label);
            }
            ui.separator();
            egui::ComboBox::new("scale-root", "root")
                .selected_text(crate::geometry::keys::PITCH_NAMES[usize::from(self.scale.0)])
                .width(44.0)
                .show_ui(ui, |ui| {
                    for (pc, name) in crate::geometry::keys::PITCH_NAMES.iter().enumerate() {
                        ui.selectable_value(&mut self.scale.0, pc as u8, *name);
                    }
                });
            egui::ComboBox::new("scale-kind", "scale")
                .selected_text(
                    SCALES
                        .iter()
                        .find(|(s, _)| *s == self.scale.1)
                        .map_or("?", |(_, l)| l),
                )
                .show_ui(ui, |ui| {
                    for (scale, label) in SCALES {
                        ui.selectable_value(&mut self.scale.1, scale, label);
                    }
                });
            if ui.button("Snap to scale").clicked() {
                ctx.intents.command(SnapClipToScale {
                    clip: clip.id,
                    root: self.scale.0,
                    scale: self.scale.1,
                });
            }
            if ui
                .button("Quantize")
                .on_hover_text("Quantize to the snap grid")
                .clicked()
            {
                let grid = ctx.ui.snap.ticks().max(30);
                ctx.intents.command(QuantizeClip {
                    clip: clip.id,
                    grid: Ticks::new(grid),
                    strength: 1.0,
                    lengths: false,
                });
            }
        });
    }

    fn paint_keys(
        &self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        track: TrackId,
        rect: Rect,
        keys: &KeyMap,
    ) {
        let response = ui.interact(rect, ui.scope_id().with("keys"), Sense::click());
        let painter = ui.painter_at(rect);
        let rows = (rect.height() / keys.row_height).ceil() as i32 + 1;
        for row in 0..rows {
            let pitch = i32::from(keys.top_pitch) - row;
            if !(0..=127).contains(&pitch) {
                continue;
            }
            let pitch = pitch as u8;
            let y = keys.y_of(pitch);
            let key = Rect::from_min_size(
                pos2(rect.left(), y),
                vec2(rect.width(), keys.row_height - 1.0),
            );
            let fill = if is_black_key(pitch) {
                theme::CANVAS
            } else {
                theme::TEXT_DIM
            };
            painter.rect_filled(key, 1.0, fill);
            if pitch.is_multiple_of(12) {
                painter.text(
                    key.right_center() - vec2(3.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    pitch_name(pitch),
                    egui::FontId::proportional(9.0),
                    theme::CANVAS,
                );
            }
        }
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                ctx.intents.push(Intent::PreviewNote {
                    track,
                    pitch: keys.pitch_at(pos.y),
                    velocity: 100,
                });
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn grid(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        track: TrackId,
        clip: &Clip,
        rect: Rect,
        keys: &KeyMap,
        map: &TimeMap,
    ) {
        let response = ui.interact(rect, ui.scope_id().with("roll"), Sense::click_and_drag());
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "Piano roll grid"));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::CANVAS);
        let rows = (rect.height() / keys.row_height).ceil() as i32 + 1;
        for row in 0..rows {
            let pitch = i32::from(keys.top_pitch) - row;
            if !(0..=127).contains(&pitch) {
                continue;
            }
            let pitch = pitch as u8;
            let y = keys.y_of(pitch);
            let in_scale = self.scale.1.contains(self.scale.0, pitch);
            let fill = match (in_scale, pitch % 12 == self.scale.0) {
                (_, true) => theme::dim(theme::ACCENT, 0.88),
                (true, false) => theme::SURFACE,
                (false, false) => theme::CANVAS,
            };
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(rect.left(), y),
                    vec2(rect.width(), keys.row_height - 1.0),
                ),
                0.0,
                fill,
            );
        }
        for line in map.grid_lines(rect.left(), rect.right(), map.grid_step(12.0)) {
            let color = match line.kind {
                GridKind::Bar => theme::GRID_BAR,
                GridKind::Beat => theme::GRID_BEAT,
                GridKind::Sub => theme::GRID_SUB,
            };
            painter.line_segment(
                [pos2(line.x, rect.top()), pos2(line.x, rect.bottom())],
                Stroke::new(1.0, color),
            );
        }
        let end_x = map.x_of(clip.length.get() as f64);
        painter.rect_filled(
            Rect::from_min_max(pos2(end_x, rect.top()), rect.max),
            0.0,
            theme::dim(theme::BG, 0.3),
        );
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos());
        let live_notes = self.preview_notes(clip, pointer, keys, map, ctx);
        for (index, note) in live_notes.iter().enumerate() {
            let x0 = map.x_of(note.start.get() as f64);
            let x1 = map.x_of((note.start + note.length).get() as f64);
            let y = keys.y_of(note.pitch);
            let r = Rect::from_min_max(
                pos2(x0, y),
                pos2(x1.max(x0 + 2.0), y + keys.row_height - 1.0),
            );
            let alpha = 0.15 + 0.5 * (1.0 - f32::from(note.velocity) / 127.0);
            painter.rect_filled(r, 2.0, theme::dim(theme::ACCENT, alpha));
            let stroke = if self.selected == Some(index) {
                Stroke::new(1.5, theme::TEXT)
            } else {
                Stroke::new(1.0, theme::CANVAS)
            };
            painter.rect_stroke(r, 2.0, stroke, StrokeKind::Inside);
        }
        let playhead = ctx.live.transport.position - clip.start.get() as f64;
        if playhead >= 0.0 && playhead < clip.length.get() as f64 {
            let x = map.x_of(playhead);
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.5, theme::ACCENT),
            );
            ui.ctx().request_repaint();
        }
        self.gestures(ctx, track, clip, &response, keys, map);
    }

    fn edge_ticks(map: &TimeMap) -> f64 {
        f64::from(EDGE_GRAB_PX) / map.px_per_tick()
    }

    fn preview_notes(
        &self,
        clip: &Clip,
        pointer: Option<egui::Pos2>,
        keys: &KeyMap,
        map: &TimeMap,
        ctx: &ViewCtx<'_>,
    ) -> Vec<MidiNote> {
        let (Some(drag), Some(pos)) = (self.drag, pointer) else {
            return clip.notes().to_vec();
        };
        self.dragged(clip, drag, pos, keys, map, ctx)
    }

    fn dragged(
        &self,
        clip: &Clip,
        drag: NoteDrag,
        pos: egui::Pos2,
        keys: &KeyMap,
        map: &TimeMap,
        ctx: &ViewCtx<'_>,
    ) -> Vec<MidiNote> {
        let length = clip.length.get();
        let ticks = map.ticks_at(pos.x);
        match drag.part {
            NotePart::Body => {
                let dt = ctx.ui.snap.round((ticks - drag.grab_ticks).round() as i64);
                let dp = i32::from(keys.pitch_at(pos.y)) - i32::from(drag.grab_pitch);
                notes::move_note(clip.notes(), drag.index, dt, dp, length)
            }
            NotePart::End => {
                let end = ctx.ui.snap.round(ticks.round() as i64);
                notes::resize_note(clip.notes(), drag.index, end, length)
            }
        }
    }

    fn gestures(
        &mut self,
        ctx: &mut ViewCtx<'_>,
        track: TrackId,
        clip: &Clip,
        response: &egui::Response,
        keys: &KeyMap,
        map: &TimeMap,
    ) {
        let edge = Self::edge_ticks(map);
        if response.drag_started() {
            let origin = response
                .ctx
                .input(|i| i.pointer.press_origin())
                .or(response.interact_pointer_pos());
            if let Some(pos) = origin {
                let pitch = keys.pitch_at(pos.y);
                let ticks = map.ticks_at(pos.x);
                if let Some((index, part)) = hit_note(clip.notes(), ticks, pitch, edge) {
                    self.drag = Some(NoteDrag {
                        index,
                        part,
                        grab_ticks: ticks,
                        grab_pitch: pitch,
                    });
                    self.selected = Some(index);
                }
            }
        }
        if response.drag_stopped() {
            if let (Some(drag), Some(pos)) = (self.drag.take(), response.interact_pointer_pos()) {
                let notes = self.dragged(clip, drag, pos, keys, map, ctx);
                if notes != clip.notes() {
                    ctx.intents.command(notes::rewrite(track, clip, notes));
                }
            }
        }
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                self.click(ctx, track, clip, pos, keys, map);
            }
        }
        if response.secondary_clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some((index, _)) = hit_note(
                    clip.notes(),
                    map.ticks_at(pos.x),
                    keys.pitch_at(pos.y),
                    edge,
                ) {
                    ctx.intents.command(notes::rewrite(
                        track,
                        clip,
                        notes::delete_note(clip.notes(), index),
                    ));
                    self.selected = None;
                }
            }
        }
    }

    fn click(
        &mut self,
        ctx: &mut ViewCtx<'_>,
        track: TrackId,
        clip: &Clip,
        pos: egui::Pos2,
        keys: &KeyMap,
        map: &TimeMap,
    ) {
        let pitch = keys.pitch_at(pos.y);
        let ticks = map.ticks_at(pos.x);
        if let Some((index, _)) = hit_note(clip.notes(), ticks, pitch, Self::edge_ticks(map)) {
            self.selected = Some(index);
            return;
        }
        let start = ctx.ui.snap.floor(ticks.max(0.0) as i64);
        if start >= clip.length.get() {
            return;
        }
        match self.tool {
            Tool::Draw => {
                let added = notes::add_note(
                    clip.notes(),
                    start,
                    self.note_length,
                    pitch,
                    100,
                    clip.length.get(),
                );
                ctx.intents.command(notes::rewrite(track, clip, added));
                ctx.intents.push(Intent::PreviewNote {
                    track,
                    pitch,
                    velocity: 100,
                });
            }
            Tool::Chord => {
                ctx.intents.command(notes::chord(
                    clip.id,
                    start,
                    self.note_length.max(960),
                    pitch,
                    self.chord,
                    100,
                ));
            }
        }
    }

    fn velocity_lane(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        track: TrackId,
        clip: &Clip,
        rect: Rect,
        map: &TimeMap,
    ) {
        let response = ui.interact(
            rect,
            ui.scope_id().with("velocity-lane"),
            Sense::click_and_drag(),
        );
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Canvas, true, "Velocity lane"));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::SURFACE);
        for (index, note) in clip.notes().iter().enumerate() {
            let x = map.x_of(note.start.get() as f64);
            let h = rect.height() * f32::from(note.velocity) / 127.0;
            let color = if self.selected == Some(index) {
                theme::TEXT
            } else {
                theme::ACCENT
            };
            painter.line_segment(
                [
                    pos2(x + 2.0, rect.bottom()),
                    pos2(x + 2.0, rect.bottom() - h),
                ],
                Stroke::new(3.0, color),
            );
        }
        if response.clicked() || response.drag_stopped() {
            if let Some(pos) = response.interact_pointer_pos() {
                let ticks = map.ticks_at(pos.x);
                let nearest = clip
                    .notes()
                    .iter()
                    .enumerate()
                    .map(|(i, n)| (i, (n.start.get() as f64 - ticks).abs()))
                    .filter(|(_, d)| *d * map.px_per_tick() <= 8.0)
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((index, _)) = nearest {
                    let velocity = (((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0)
                        * 127.0)
                        .round() as u8;
                    ctx.intents.command(notes::rewrite(
                        track,
                        clip,
                        notes::set_velocity(clip.notes(), index, velocity),
                    ));
                }
            }
        }
    }

    fn wheel(&mut self, ui: &Ui, rect: Rect) {
        let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        if !rect.contains(pointer) {
            return;
        }
        let (zoom, scroll, shift) =
            ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta(), i.modifiers.shift));
        let map = TimeMap::new(rect.left(), self.scroll, self.px_per_beat);
        if zoom != 1.0 {
            let zoomed = map.zoomed_around(pointer.x, zoom);
            self.scroll = zoomed.scroll;
            self.px_per_beat = zoomed.px_per_beat;
        } else if shift || scroll.x != 0.0 {
            let dx = if shift { scroll.y } else { scroll.x };
            self.scroll = map.scrolled_by(-dx).scroll;
        } else if scroll.y != 0.0 {
            let rows = (scroll.y / self.keys.row_height).round() as i32;
            self.keys = self.keys.scrolled_rows(-rows);
        }
    }
}
