//! The ruler (seek, loop brace) and the clip area (clips, drags,
//! automation lanes, playhead).

use egui::{pos2, vec2, Rect, Sense, Stroke, Ui};
use fourfloor_core::TrackId;
use fourfloor_model::command::{ClipToSlot, SetClipLoop};
use fourfloor_model::{Clip, ClipContent, Project};

use crate::geometry::{GridKind, TimeMap};
use crate::state::{Intent, TransportIntent, ViewId};
use crate::theme;
use crate::viewmodel::arrange::{
    self, hit_clip, track_at, value_to_y, with_point, without_point, y_to_value, ClipDrag, LaneRow,
    Row, LANE_HEIGHT, TRACK_HEIGHT,
};
use crate::widgets::clip::{paint_clip, ClipStyle};

use super::super::ViewCtx;
use super::ArrangementView;

fn is_drum_track(project: &Project, track: TrackId) -> bool {
    project
        .track(track)
        .is_some_and(|t| t.devices.iter().any(|d| d.kind.starts_with("drum.")))
}

impl ArrangementView {
    pub(super) fn ruler(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, rect: Rect, map: &TimeMap) {
        let response = ui.interact(rect, ui.scope_id().with("ruler"), Sense::click_and_drag());
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, true, "Timeline ruler"));
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::SURFACE);
        let step = map.grid_step(40.0);
        for line in map.grid_lines(rect.left(), rect.right(), step) {
            let bar = line.ticks / crate::geometry::BAR_TICKS + 1;
            let strong = line.kind == GridKind::Bar;
            painter.line_segment(
                [
                    pos2(line.x, rect.bottom() - if strong { 10.0 } else { 5.0 }),
                    pos2(line.x, rect.bottom()),
                ],
                Stroke::new(1.0, theme::TEXT_DIM),
            );
            if strong {
                painter.text(
                    pos2(line.x + 3.0, rect.top() + 9.0),
                    egui::Align2::LEFT_CENTER,
                    bar.to_string(),
                    egui::FontId::proportional(11.0),
                    theme::TEXT_DIM,
                );
            }
        }
        // Loop brace.
        let (start, end) = ctx.ui.loop_range;
        let brace = Rect::from_min_max(
            pos2(map.x_of(start as f64), rect.bottom() - 6.0),
            pos2(map.x_of(end as f64), rect.bottom()),
        );
        let color = if ctx.ui.loop_on {
            theme::ACCENT_2
        } else {
            theme::GRID_BAR
        };
        painter.rect_filled(brace, 1.0, color);
        // Gestures: click seeks, drag draws a loop.
        let pointer = response.interact_pointer_pos();
        if response.drag_started() {
            if let Some(pos) = ui.input(|i| i.pointer.press_origin()).or(pointer) {
                self.loop_drag = Some(map.snapped_at(pos.x, ctx.ui.snap));
            }
        }
        if let (Some(anchor), Some(pos)) = (self.loop_drag, pointer) {
            let here = map.snapped_at(pos.x, ctx.ui.snap);
            let (a, b) = (anchor.min(here), anchor.max(here));
            let preview = Rect::from_min_max(
                pos2(map.x_of(a as f64), rect.top()),
                pos2(map.x_of(b as f64), rect.bottom()),
            );
            painter.rect_filled(preview, 0.0, theme::dim(theme::ACCENT_2, 0.75));
            if response.drag_stopped() {
                self.loop_drag = None;
                if b > a {
                    ctx.ui.loop_range = (a, b);
                    ctx.ui.loop_on = true;
                    ctx.intents
                        .push(Intent::Transport(TransportIntent::SetLoop(Some((a, b)))));
                }
            }
        }
        if response.clicked() {
            if let Some(pos) = pointer {
                self.seek(ctx, map.ticks_at(pos.x));
            }
        }
        let x = map.x_of(ctx.live.transport.position);
        if x >= rect.left() {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(2.0, theme::ACCENT),
            );
        }
    }

    pub(super) fn timeline(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        rect: Rect,
        rows: &[Row],
        map: &TimeMap,
    ) {
        let response = ui.interact(
            rect,
            ui.scope_id().with("timeline"),
            Sense::click_and_drag(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Canvas, true, "Arrangement timeline")
        });
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::CANVAS);
        for line in map.grid_lines(rect.left(), rect.right(), map.grid_step(10.0)) {
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
        if ctx.ui.loop_on {
            let (a, b) = ctx.ui.loop_range;
            let shade = Rect::from_min_max(
                pos2(map.x_of(a as f64), rect.top()),
                pos2(map.x_of(b as f64), rect.bottom()),
            );
            painter.rect_filled(
                shade.intersect(rect),
                0.0,
                theme::dim(theme::ACCENT_2, 0.93),
            );
        }
        for (index, row) in rows.iter().enumerate() {
            painter.line_segment(
                [
                    pos2(rect.left(), row.top + TRACK_HEIGHT - 1.0),
                    pos2(rect.right(), row.top + TRACK_HEIGHT - 1.0),
                ],
                Stroke::new(1.0, theme::GRID_BEAT),
            );
            self.paint_row_clips(ui, ctx, &painter, rect, row, index, map);
            for lane in &row.lanes {
                paint_lane(&painter, rect, lane, map);
            }
        }
        self.clip_gestures(ui, ctx, &response, rows, map);
        let x = map.x_of(ctx.live.transport.position);
        if x >= rect.left() {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.5, theme::ACCENT),
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_row_clips(
        &self,
        _ui: &Ui,
        ctx: &mut ViewCtx<'_>,
        painter: &egui::Painter,
        area: Rect,
        row: &Row,
        index: usize,
        map: &TimeMap,
    ) {
        let Some(track) = ctx.project.track(row.track) else {
            return;
        };
        let tempo = ctx.project.tempo.bpm();
        for clip in &track.clips {
            let (start, length) = match (&self.drag, self.drag_pointer) {
                (Some(drag), Some((ticks, _))) if drag.clip == clip.id => {
                    drag.preview(ticks, ctx.ui.snap)
                }
                _ => (clip.start.get(), clip.length.get()),
            };
            let target_row = match (&self.drag, self.drag_pointer) {
                (Some(drag), Some((_, Some(over))))
                    if drag.clip == clip.id && over != row.track =>
                {
                    None
                }
                _ => Some(row.top),
            };
            let Some(top) = target_row else {
                continue;
            };
            let rect = Rect::from_min_max(
                pos2(map.x_of(start as f64), top + 2.0),
                pos2(map.x_of((start + length) as f64), top + TRACK_HEIGHT - 3.0),
            );
            if !rect.intersects(area) {
                continue;
            }
            let shown = Clip {
                start: fourfloor_core::Ticks::new(start),
                length: fourfloor_core::Ticks::new(length),
                ..clip.as_ref().clone()
            };
            let waveform = match &clip.content {
                ClipContent::Audio { source, .. } => {
                    let key = source.file_name();
                    let found = ctx.assets.waveforms.get(&key).cloned();
                    if found.is_none() && !ctx.assets.requested.contains(&key) {
                        ctx.intents.push(Intent::NeedWaveform(source.clone()));
                    }
                    found
                }
                ClipContent::Midi { .. } => None,
            };
            // A warped clip plays its file at the recording's tempo.
            let source_bpm = match &clip.content {
                ClipContent::Audio {
                    warp: Some(warp), ..
                } => warp.source_bpm,
                _ => tempo,
            };
            let frames_per_tick = waveform.as_ref().map_or(0.0, |w| {
                f64::from(w.sample_rate) * 60.0 / (source_bpm * 960.0)
            });
            let selected = ctx.ui.selection.clip == Some(clip.id);
            let style = ClipStyle {
                color: theme::track_color(index),
                selected,
                waveform: waveform.as_deref(),
                frames_per_tick,
            };
            paint_clip(painter, rect, &shown, map, &style);
        }
        // A clip dragged onto this row from another.
        if let (Some(drag), Some((ticks, Some(over)))) = (&self.drag, self.drag_pointer) {
            if over == row.track && drag.track != row.track {
                let (start, length) = drag.preview(ticks, ctx.ui.snap);
                let rect = Rect::from_min_max(
                    pos2(map.x_of(start as f64), row.top + 2.0),
                    pos2(
                        map.x_of((start + length) as f64),
                        row.top + TRACK_HEIGHT - 3.0,
                    ),
                );
                painter.rect_filled(rect, 3.0, theme::dim(theme::track_color(index), 0.5));
            }
        }
    }

    fn clip_gestures(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        response: &egui::Response,
        rows: &[Row],
        map: &TimeMap,
    ) {
        let pointer = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos());
        if response.drag_started() {
            // Grab where the button went down, not where the drag was
            // recognised a few pixels later.
            let origin = ui
                .input(|i| i.pointer.press_origin())
                .or(response.interact_pointer_pos());
            if let Some(pos) = origin {
                if let Some((track, clip, part)) = hit_clip(ctx.project, rows, map, pos.x, pos.y) {
                    if let Some((_, found)) = ctx.project.find_clip(clip) {
                        self.drag = Some(ClipDrag {
                            clip,
                            track,
                            part,
                            start: found.start.get(),
                            length: found.length.get(),
                            grab: map.ticks_at(pos.x),
                        });
                        ctx.ui.selection.select_track(track);
                        ctx.ui.selection.clip = Some(clip);
                    }
                }
            }
        }
        if let (Some(_), Some(pos)) = (&self.drag, pointer) {
            self.drag_pointer = Some((map.ticks_at(pos.x), track_at(rows, pos.y)));
        }
        if response.drag_stopped() {
            if let (Some(drag), Some((ticks, over))) = (self.drag.take(), self.drag_pointer.take())
            {
                if let Some(command) = drag.finish(ctx.project, ticks, over, ctx.ui.snap) {
                    ctx.intents.push(command);
                }
            }
        }
        if response.clicked() {
            if let Some(pos) = pointer {
                self.click(ctx, rows, map, pos);
            }
        }
        if response.double_clicked() {
            if let Some(pos) = pointer {
                match hit_clip(ctx.project, rows, map, pos.x, pos.y) {
                    Some((track, clip, _)) => {
                        ctx.ui.selection.select_track(track);
                        ctx.ui.selection.clip = Some(clip);
                        ctx.ui.open_view(if is_drum_track(ctx.project, track) {
                            ViewId::SEQUENCER
                        } else {
                            ViewId::PIANO_ROLL
                        });
                    }
                    None => {
                        if let Some(track) = track_at(rows, pos.y) {
                            self.create_clip_at(ctx, track, map.ticks_at(pos.x));
                        }
                    }
                }
            }
        }
        if response.secondary_clicked() {
            if let Some(pos) = pointer {
                self.context_clip =
                    hit_clip(ctx.project, rows, map, pos.x, pos.y).map(|(_, clip, _)| clip);
                if let Some((lane, track)) = lane_at(rows, pos.y) {
                    let tolerance = f64::from(8.0 / map.px_per_tick() as f32);
                    if let Some(points) =
                        without_point(&lane.points, map.ticks_at(pos.x), tolerance)
                    {
                        ctx.intents
                            .push(arrange::write_lane(track, lane.target, points));
                    }
                }
            }
        }
        self.clip_menu(ui, ctx, response);
    }

    fn click(&mut self, ctx: &mut ViewCtx<'_>, rows: &[Row], map: &TimeMap, pos: egui::Pos2) {
        if let Some((lane, track)) = lane_at(rows, pos.y) {
            let time = map.snapped_at(pos.x, ctx.ui.snap);
            let value = y_to_value(lane, pos.y);
            ctx.intents.push(arrange::write_lane(
                track,
                lane.target,
                with_point(&lane.points, time, value),
            ));
            return;
        }
        match hit_clip(ctx.project, rows, map, pos.x, pos.y) {
            Some((track, clip, _)) => {
                ctx.ui.selection.select_track(track);
                ctx.ui.selection.clip = Some(clip);
            }
            None => {
                if let Some(track) = track_at(rows, pos.y) {
                    ctx.ui.selection.select_track(track);
                }
                ctx.ui.selection.clip = None;
                self.seek(ctx, map.ticks_at(pos.x));
            }
        }
    }

    fn clip_menu(&mut self, _ui: &mut Ui, ctx: &mut ViewCtx<'_>, response: &egui::Response) {
        let Some(clip_id) = self.context_clip else {
            return;
        };
        let Some((_, clip)) = ctx.project.find_clip(clip_id) else {
            self.context_clip = None;
            return;
        };
        let clip = clip.clone();
        let scenes: Vec<(fourfloor_model::SceneId, String)> = ctx
            .project
            .scenes
            .iter()
            .map(|s| (s.id, s.name.clone()))
            .collect();
        response.context_menu(|ui| {
            ui.label(egui::RichText::new(&clip.name).strong());
            if ui.button("Split at playhead").clicked() {
                ctx.ui.selection.clip = Some(clip_id);
                self.split_selected(ctx);
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                ctx.intents.push(arrange::duplicate(clip_id));
                ui.close();
            }
            let looping = clip.loop_length.is_some();
            if ui
                .button(if looping {
                    "Stop looping"
                } else {
                    "Loop contents"
                })
                .clicked()
            {
                let loop_length = if looping { None } else { Some(clip.length) };
                ctx.intents.command(SetClipLoop {
                    clip: clip_id,
                    loop_length,
                });
                ui.close();
            }
            if !scenes.is_empty() {
                ui.menu_button("Copy to launcher", |ui| {
                    for (scene, name) in &scenes {
                        if ui.button(name).clicked() {
                            ctx.intents.command(ClipToSlot {
                                clip: clip_id,
                                scene: *scene,
                            });
                            ui.close();
                        }
                    }
                });
            }
            if ui.button("Delete").clicked() {
                ctx.intents.push(arrange::delete(clip_id));
                ctx.ui.selection.clip = None;
                ui.close();
            }
        });
    }
}

fn lane_at(rows: &[Row], y: f32) -> Option<(&LaneRow, TrackId)> {
    rows.iter().find_map(|row| {
        row.lanes
            .iter()
            .find(|lane| y >= lane.top && y < lane.top + LANE_HEIGHT)
            .map(|lane| (lane, row.track))
    })
}

fn paint_lane(painter: &egui::Painter, area: Rect, lane: &LaneRow, map: &TimeMap) {
    let rect = Rect::from_min_size(
        pos2(area.left(), lane.top + 1.0),
        vec2(area.width(), LANE_HEIGHT - 2.0),
    );
    painter.rect_filled(rect, 0.0, theme::dim(theme::SURFACE, 0.3));
    if lane.points.is_empty() {
        painter.text(
            rect.left_center() + vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "click to add points, right-click to remove",
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
        return;
    }
    let mut points: Vec<egui::Pos2> = lane
        .points
        .iter()
        .map(|p| pos2(map.x_of(p.time.get() as f64), value_to_y(lane, p.value)))
        .collect();
    if let (Some(first), Some(last)) = (points.first().copied(), points.last().copied()) {
        points.insert(0, pos2(area.left(), first.y));
        points.push(pos2(area.right(), last.y));
    }
    painter.add(egui::Shape::line(
        points.clone(),
        Stroke::new(1.5, theme::ACCENT_2),
    ));
    for point in &points[1..points.len() - 1] {
        painter.circle_filled(*point, 3.5, theme::ACCENT_2);
    }
}
