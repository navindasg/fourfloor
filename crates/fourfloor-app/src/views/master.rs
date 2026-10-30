//! The master metering panel: loudness, true peak, correlation and a
//! spectrum, plus a reference track for A/B with loudness matching.

use egui::{vec2, Sense, Ui};

use crate::state::{Intent, PathPurpose};
use crate::theme;
use crate::widgets::{paint_correlation, paint_spectrum, paint_stereo_meter};

use super::mixer::{db_text, lufs_text};
use super::shell::dialogs::choose_path;
use super::ViewCtx;

/// Floor of the spectrum display, dB.
pub const SPECTRUM_FLOOR_DB: f32 = -100.0;

/// Draws the panel.
pub fn master_panel(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.heading("Master");
    let master = ctx.live.master.clone();
    ui.horizontal(|ui| {
        let (meter, _) = ui.allocate_exact_size(vec2(26.0, 120.0), Sense::hover());
        paint_stereo_meter(ui, meter, ctx.live.meters.master());
        ui.vertical(|ui| {
            egui::Grid::new("loudness").num_columns(2).show(ui, |ui| {
                for (label, value) in [
                    ("Momentary", lufs_text(master.momentary_lufs)),
                    ("Short-term", lufs_text(master.short_term_lufs)),
                    ("Integrated", lufs_text(master.integrated_lufs)),
                    ("True peak", db_text(master.true_peak_dbtp, "dBTP")),
                ] {
                    ui.label(egui::RichText::new(label).color(theme::TEXT_DIM));
                    let hot = label == "True peak" && master.true_peak_dbtp > -1.0;
                    ui.label(egui::RichText::new(value).monospace().color(if hot {
                        theme::METER_CLIP
                    } else {
                        theme::TEXT
                    }));
                    ui.end_row();
                }
            });
        });
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(format!("Correlation {:+.2}", master.correlation))
            .color(theme::TEXT_DIM),
    );
    let (corr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    paint_correlation(ui, corr, master.correlation);
    ui.add_space(4.0);
    ui.label(egui::RichText::new("Spectrum").color(theme::TEXT_DIM));
    let (spectrum, _) = ui.allocate_exact_size(vec2(ui.available_width(), 90.0), Sense::hover());
    paint_spectrum(ui, spectrum, &master.spectrum_db, SPECTRUM_FLOOR_DB);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("20 Hz").small().color(theme::TEXT_DIM));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new("20 kHz").small().color(theme::TEXT_DIM));
        });
    });
    ui.separator();
    reference(ui, ctx);
}

fn reference(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.label(egui::RichText::new("Reference A/B").strong());
    if ui.button("Load reference…").clicked() {
        choose_path(ctx, PathPurpose::Reference);
    }
    let reference = ctx.ui.reference.clone();
    let Some(name) = reference.name else {
        ui.label(
            egui::RichText::new("Load a finished track to compare against.").color(theme::TEXT_DIM),
        );
        return;
    };
    ui.label(crate::widgets::truncate(&name, 32));
    ui.horizontal(|ui| {
        if ui
            .selectable_label(!reference.monitoring, "A: mix")
            .clicked()
        {
            ctx.intents.push(Intent::MonitorReference(false));
        }
        if ui
            .selectable_label(reference.monitoring, "B: reference")
            .clicked()
        {
            ctx.intents.push(Intent::MonitorReference(true));
        }
    });
    let mut matched = reference.matched;
    if ui.checkbox(&mut matched, "Match loudness").changed() {
        ctx.intents.push(Intent::MatchReference(matched));
    }
    ui.label(format!(
        "Mix {}  ·  Ref {}",
        lufs_text(reference.mix_lufs),
        lufs_text(reference.reference_lufs)
    ));
    for note in reference.notes.iter().take(8) {
        ui.label(egui::RichText::new(note).small());
    }
}
