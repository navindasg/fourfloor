//! The browser's search box, description search and filter bar.

use egui::Ui;
use fourfloor_analysis::Key;
use fourfloor_library::FileKind;

use crate::services::embed::text_search_available;
use crate::state::LibraryIntent;
use crate::viewmodel::browser::kind_label;

use super::LibraryPanel;
use crate::views::ViewCtx;

/// Draws the search box and filters into `panel`.
pub fn filter_bar(panel: &mut LibraryPanel, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.add(
        egui::TextEdit::singleline(&mut panel.filters.text)
            .hint_text("search names, folders and tags")
            .desired_width(f32::INFINITY),
    );
    description_search(panel, ui, ctx);
    ui.horizontal_wrapped(|ui| {
        kind_choice(panel, ui);
        key_choice(ui, "filter-key", "key", &mut panel.filters.key);
        if panel.filters.key.is_some() {
            ui.checkbox(&mut panel.filters.compatible, "compatible")
                .on_hover_text("Also keys a fifth away and the relative key");
        }
    });
    ui.horizontal_wrapped(|ui| {
        let toggle = ui.checkbox(&mut panel.bpm_on, "BPM");
        let (low, high) = &mut panel.bpm_range;
        ui.add_enabled(
            panel.bpm_on,
            egui::DragValue::new(low).range(40.0..=250.0).speed(0.5),
        )
        .labelled_by(toggle.id);
        let to = ui.label("to");
        ui.add_enabled(
            panel.bpm_on,
            egui::DragValue::new(high).range(40.0..=250.0).speed(0.5),
        )
        .labelled_by(to.id);
        ui.add_enabled(
            panel.bpm_on,
            egui::Checkbox::new(&mut panel.filters.half_double, "½/2×"),
        )
        .on_hover_text("Count half and double tempo as a match");
        tag_choice(panel, ui, ctx);
    });
}

fn description_search(panel: &mut LibraryPanel, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.horizontal(|ui| {
        let busy = ctx.features.library.embedding;
        let response = ui.add(
            egui::TextEdit::singleline(&mut panel.description)
                .hint_text("describe a sound: \"dusty vinyl hat\"")
                .desired_width(250.0),
        );
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let find = ui
            .add_enabled(!busy, egui::Button::new("Find by sound"))
            .on_hover_text(if text_search_available() {
                "Rank files against the description with CLAP audio-text embeddings"
            } else {
                "This build has no audio-text model (feature clap-search): names and tags are searched"
            });
        if (find.clicked() || enter) && !panel.description.trim().is_empty() {
            ctx.intents
                .push(LibraryIntent::TextSearch(panel.description.clone()));
        }
        if text_search_available()
            && ui
                .add_enabled(!busy, egui::Button::new("Embed"))
                .on_hover_text("Compute CLAP embeddings for files that have none")
                .clicked()
        {
            ctx.intents.push(LibraryIntent::Embed);
        }
        if busy {
            ui.spinner();
        }
    });
}

fn kind_choice(panel: &mut LibraryPanel, ui: &mut Ui) {
    let text = panel.filters.kind.map_or("any kind", kind_label);
    egui::ComboBox::new("filter-kind", "kind")
        .selected_text(text)
        .width(90.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut panel.filters.kind, None, "any kind");
            for kind in [FileKind::OneShot, FileKind::Loop, FileKind::Track] {
                ui.selectable_value(&mut panel.filters.kind, Some(kind), kind_label(kind));
            }
        });
}

/// A key picker with an "any" entry; `id` keeps several apart.
pub fn key_choice(ui: &mut Ui, id: &str, label: &str, key: &mut Option<Key>) {
    let text = key.map_or_else(
        || "any".to_owned(),
        |k| format!("{} · {}", k.short_name(), k.camelot()),
    );
    egui::ComboBox::new(id, label)
        .selected_text(text)
        .width(80.0)
        .height(320.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(key, None, "any");
            for choice in Key::all() {
                let name = format!("{} · {}", choice.short_name(), choice.camelot());
                ui.selectable_value(key, Some(choice), name);
            }
        });
}

fn tag_choice(panel: &mut LibraryPanel, ui: &mut Ui, ctx: &ViewCtx<'_>) {
    let tags: Vec<String> = ctx
        .features
        .library
        .library()
        .and_then(|library| library.all_tags().ok())
        .map(|tags| tags.into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default();
    let shown = if panel.tag.is_empty() {
        "any tag".to_owned()
    } else {
        panel.tag.clone()
    };
    egui::ComboBox::new("filter-tag", "tag")
        .selected_text(shown)
        .width(90.0)
        .height(320.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut panel.tag, String::new(), "any tag");
            for tag in tags {
                let label = tag.clone();
                ui.selectable_value(&mut panel.tag, tag, label);
            }
        });
}
