//! Settings tab: the machine-learning model registry, with each model's
//! code and weights licenses and whether it is downloaded.

use egui::{RichText, Ui};

use crate::state::MlIntent;
use crate::theme;
use crate::views::ViewCtx;

/// Megabytes, one decimal.
pub fn size_label(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{:.0} kB", bytes as f64 / 1e3)
    } else {
        format!("{:.1} MB", bytes as f64 / 1e6)
    }
}

/// A license with whether fourfloor's policy allows it.
fn license(ui: &mut Ui, spdx: &str) {
    let allowed = fourfloor_ml::is_allowed(spdx);
    let colour = if allowed {
        theme::METER_OK
    } else {
        theme::METER_CLIP
    };
    ui.label(RichText::new(spdx).color(colour))
        .on_hover_text(if allowed {
            "Open license that allows commercial use"
        } else {
            "Not allowed: the registry refuses it"
        });
}

/// Draws the tab.
pub fn models_tab(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    if ctx.features.models.is_empty() {
        ctx.intents.push(MlIntent::Refresh);
    }
    ui.label(
        RichText::new(format!(
            "Open-weights models only, run on this machine. Cache: {}",
            ctx.features.paths.models.display()
        ))
        .small()
        .color(theme::TEXT_DIM),
    );
    egui::Grid::new("models")
        .num_columns(6)
        .striped(true)
        .show(ui, |ui| {
            for title in ["Model", "Code", "Weights", "Size", "State", ""] {
                ui.label(RichText::new(title).strong());
            }
            ui.end_row();
            for model in &ctx.features.models {
                ui.label(&model.name)
                    .on_hover_text(format!("{}\n{}", model.description, model.homepage));
                license(ui, &model.code_license);
                license(ui, &model.weights_license);
                ui.label(size_label(model.size));
                let downloading = ctx.features.downloading.contains(&model.name);
                if model.cached {
                    ui.label(RichText::new("downloaded").color(theme::METER_OK));
                    ui.label("");
                } else if downloading {
                    let progress = ctx
                        .live
                        .jobs
                        .iter()
                        .find(|(label, _)| label.ends_with(&model.name))
                        .map_or(0.0, |(_, p)| *p);
                    ui.add(egui::ProgressBar::new(progress).desired_width(80.0));
                    ui.spinner();
                } else {
                    ui.label(RichText::new("not downloaded").color(theme::TEXT_DIM));
                    if ui
                        .small_button("Download")
                        .on_hover_text(format!(
                            "Download {} ({})",
                            model.name,
                            size_label(model.size)
                        ))
                        .clicked()
                    {
                        ctx.intents.push(MlIntent::Download(model.name.clone()));
                    }
                }
                ui.end_row();
            }
        });
    if ui.small_button("Refresh").clicked() {
        ctx.intents.push(MlIntent::Refresh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_well() {
        assert_eq!(size_label(230_000), "230 kB");
        assert_eq!(size_label(207_300_000), "207.3 MB");
    }
}
