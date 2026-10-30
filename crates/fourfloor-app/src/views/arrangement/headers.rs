//! Track headers: colour, name, mute/solo/arm, volume, pan, a meter and
//! the automation toggle; lane labels under an expanded track.

use std::collections::HashSet;

use egui::{pos2, vec2, Rect, Sense, Ui, UiBuilder};
use fourfloor_core::TrackId;
use fourfloor_model::command::DeleteTrack;
use fourfloor_model::Track;

use crate::theme;
use crate::viewmodel::arrange::{Row, LANE_HEIGHT, TRACK_HEIGHT};
use crate::widgets::buttons::toggle;
use crate::widgets::paint_horizontal_meter;

use super::super::controls::{gain_slider, mute_solo_arm, pan_slider, Previews};
use super::super::ViewCtx;

#[allow(clippy::too_many_arguments)]
pub(super) fn track_header(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    previews: &mut Previews,
    expanded: &mut HashSet<TrackId>,
    track: &Track,
    index: usize,
    row: &Row,
    rect: Rect,
) {
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), TRACK_HEIGHT - 2.0));
    let selected = ctx.ui.selection.track == Some(track.id);
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        head,
        3.0,
        if selected {
            theme::RAISED
        } else {
            theme::SURFACE
        },
    );
    painter.rect_filled(
        Rect::from_min_size(head.min, vec2(4.0, head.height())),
        1.0,
        theme::track_color(index),
    );
    let click = ui.interact(
        head,
        ui.scope_id().with(("header", track.id)),
        Sense::click(),
    );
    if click.clicked() {
        ctx.ui.selection.select_track(track.id);
    }
    click
        .on_hover_text("Right-click for track options")
        .context_menu(|ui| {
            if ui.button(format!("Delete {}", track.name)).clicked() {
                ctx.intents.command(DeleteTrack { track: track.id });
                ctx.ui.selection = Default::default();
                ui.close();
            }
        });
    let inner = head.shrink2(vec2(8.0, 3.0)).translate(vec2(2.0, 0.0));
    ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 2.0);
        ui.horizontal(|ui| {
            let name = egui::RichText::new(&track.name).strong();
            if ui
                .add(egui::Label::new(name).truncate().sense(Sense::click()))
                .clicked()
            {
                ctx.ui.selection.select_track(track.id);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let open = expanded.contains(&track.id);
                if toggle(
                    ui,
                    "A",
                    &format!("Automation {}", track.name),
                    open,
                    theme::ACCENT_2,
                    20.0,
                )
                .clicked()
                {
                    if open {
                        expanded.remove(&track.id);
                    } else {
                        expanded.insert(track.id);
                    }
                }
                ui.label(
                    egui::RichText::new(track.kind.to_string())
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
        });
        ui.horizontal(|ui| {
            mute_solo_arm(ui, ctx, track);
            gain_slider(ui, ctx, previews, track, 64.0);
        });
        ui.horizontal(|ui| {
            let (meter, _) = ui.allocate_exact_size(vec2(96.0, 7.0), Sense::hover());
            paint_horizontal_meter(ui, meter, ctx.live.meters.track(track.id));
            pan_slider(ui, ctx, previews, track, 48.0);
        });
    });
    for lane in &row.lanes {
        let lane_rect = Rect::from_min_size(
            pos2(rect.left() + 12.0, lane.top + 1.0),
            vec2(rect.width() - 12.0, LANE_HEIGHT - 2.0),
        );
        painter.rect_filled(lane_rect, 2.0, theme::BG);
        painter.text(
            lane_rect.left_center() + vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &lane.label,
            egui::FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
    }
}
