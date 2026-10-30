//! The result table: sortable column headers and virtualised rows.

use egui::{Align, Layout, RichText, Ui};
use fourfloor_library::{FileRecord, SortKey};

use crate::services::library::ResultSource;
use crate::state::LibraryIntent;
use crate::theme;
use crate::viewmodel::browser::{bpm_label, duration_label, kind_label, tags_label};

use super::LibraryPanel;
use crate::views::ViewCtx;

/// Height of one row.
pub const ROW_HEIGHT: f32 = 20.0;

/// Columns: header, width, sort key (`None`: not sortable).
const COLUMNS: [(&str, f32, Option<SortKey>); 6] = [
    ("Name", 150.0, Some(SortKey::Title)),
    ("Kind", 58.0, None),
    ("BPM", 46.0, Some(SortKey::Bpm)),
    ("Key", 44.0, Some(SortKey::Key)),
    ("Length", 46.0, Some(SortKey::Duration)),
    ("Tags", 120.0, None),
];

fn header(panel: &mut LibraryPanel, ui: &mut Ui, sortable: bool) {
    ui.horizontal(|ui| {
        for (title, width, key) in COLUMNS {
            let arrow = match key {
                Some(key) if sortable && panel.filters.sort == key => {
                    if panel.filters.descending {
                        " v"
                    } else {
                        " ^"
                    }
                }
                _ => "",
            };
            let text = RichText::new(format!("{title}{arrow}")).strong();
            let button = egui::Button::new(text).frame(false);
            let response = ui.add_sized([width, ROW_HEIGHT], button);
            if let (Some(key), true) = (key, sortable) {
                if response
                    .on_hover_text(format!("Sort by {}", title.to_lowercase()))
                    .clicked()
                {
                    panel.filters.click_column(key);
                }
            }
        }
    });
}

fn cell(ui: &mut Ui, width: f32, text: String, dim: bool) {
    let colour = if dim { theme::TEXT_DIM } else { theme::TEXT };
    ui.allocate_ui_with_layout(
        egui::vec2(width, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.set_width(width);
            ui.add(egui::Label::new(RichText::new(text).color(colour)).truncate());
        },
    );
}

fn row(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    panel: &LibraryPanel,
    record: &FileRecord,
    score: Option<f32>,
) {
    let selected = ctx.features.library.selected == Some(record.id);
    ui.horizontal(|ui| {
        let name = ui
            .add_sized(
                [COLUMNS[0].1, ROW_HEIGHT],
                egui::Button::selectable(selected, &record.title)
                    .right_text("")
                    .truncate(),
            )
            .on_hover_text(record.path.display().to_string());
        // The empty right text left-aligns the name; keep the plain title
        // as the accessible name.
        name.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, selected, &record.title)
        });
        if name.clicked() {
            ctx.intents.push(LibraryIntent::Select(record.id));
        }
        if name.double_clicked() {
            ctx.intents
                .push(LibraryIntent::Audition(record.id, panel.mode, panel.key));
        }
        cell(ui, COLUMNS[1].1, kind_label(record.kind).to_owned(), true);
        cell(ui, COLUMNS[2].1, bpm_label(record.bpm), false);
        let key = record.key.map_or_else(|| "-".to_owned(), |k| k.camelot());
        cell(ui, COLUMNS[3].1, key, false);
        cell(
            ui,
            COLUMNS[4].1,
            duration_label(record.duration_seconds),
            true,
        );
        let tags = match score {
            Some(score) => format!("{:.0} %  {}", score * 100.0, tags_label(record)),
            None => tags_label(record),
        };
        cell(ui, COLUMNS[5].1, tags, true);
    });
}

/// Draws the table of current results.
pub fn table(panel: &mut LibraryPanel, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    let sortable = matches!(
        ctx.features.library.source,
        Some(ResultSource::Query) | None
    );
    egui::ScrollArea::horizontal()
        .id_salt("library-table-h")
        .show(ui, |ui| {
            ui.vertical(|ui| {
                header(panel, ui, sortable);
                let rows = ctx.features.library.rows.clone();
                if rows.is_empty() {
                    ui.label(
                        RichText::new("Nothing here yet: add a folder or the factory sounds.")
                            .color(theme::TEXT_DIM),
                    );
                    return;
                }
                let spacing = ui.spacing().item_spacing.y;
                egui::ScrollArea::vertical()
                    .id_salt("library-table")
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW_HEIGHT + spacing, rows.len(), |ui, range| {
                        for found in &rows[range] {
                            row(ui, ctx, panel, &found.record, found.score);
                        }
                    });
            });
        });
}
