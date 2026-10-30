//! The sample browser: a panel on the left with folder scans, search and
//! filters, a sortable result table, and a preview of the selected file
//! with audition, "find similar" and buttons to put it on a track, into a
//! sampler or on a DJ deck.
//!
//! It reads the library through `ctx.features.library` and changes
//! nothing itself: every request is a [`LibraryIntent`].

mod filters;
mod preview;
mod table;

use egui::Ui;
use fourfloor_analysis::Key;
use fourfloor_library::{PreviewMode, Query};

use crate::app::library::labels;
use crate::services::library::ResultSource;
use crate::state::{LibraryIntent, PathPurpose};
use crate::theme;
use crate::viewmodel::browser::BrowserFilters;

use super::shell::dialogs::choose_path;
use super::ViewCtx;

/// Default width of the panel.
pub const PANEL_WIDTH: f32 = 460.0;

/// The browser's own state: filters and audition choices.
#[derive(Debug)]
pub struct LibraryPanel {
    /// The filter bar.
    pub filters: BrowserFilters,
    /// Tempo filter on.
    pub bpm_on: bool,
    /// Tempo filter range.
    pub bpm_range: (f64, f64),
    /// Tag filter text.
    pub tag: String,
    /// Description for audio-text search.
    pub description: String,
    /// How auditions follow the project.
    pub mode: PreviewMode,
    /// The key auditions are moved to (the project has none of its own).
    pub key: Option<Key>,
    requested_open: bool,
    sent: Option<Query>,
}

impl Default for LibraryPanel {
    fn default() -> Self {
        Self {
            filters: BrowserFilters::default(),
            bpm_on: false,
            bpm_range: (118.0, 130.0),
            tag: String::new(),
            description: String::new(),
            mode: PreviewMode::KeyLocked,
            key: None,
            requested_open: false,
            sent: None,
        }
    }
}

impl LibraryPanel {
    /// The query the filter bar makes now.
    pub fn query(&self) -> Query {
        let filters = BrowserFilters {
            bpm: self.bpm_on.then_some(self.bpm_range),
            tag: Some(self.tag.trim().to_owned()).filter(|t| !t.is_empty()),
            ..self.filters.clone()
        };
        filters.to_query()
    }

    /// Draws the panel.
    pub fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        ui.horizontal(|ui| {
            ui.heading("Samples");
            if ui.small_button("Close").clicked() {
                ctx.ui.panels.library = false;
            }
        });
        if ctx.features.paths.library.is_none() {
            ui.label(
                egui::RichText::new("The sample library is off in this run.")
                    .color(theme::TEXT_DIM),
            );
            return;
        }
        if !ctx.features.library.is_open() {
            if !self.requested_open {
                self.requested_open = true;
                ctx.intents.push(LibraryIntent::Open);
            }
            ui.label("Opening the library…");
            return;
        }
        self.scan_bar(ui, ctx);
        ui.separator();
        filters::filter_bar(self, ui, ctx);
        let query = self.query();
        if self.sent.as_ref() != Some(&query) {
            self.sent = Some(query.clone());
            ctx.intents.push(LibraryIntent::Search(query.clone()));
        }
        self.results_line(ui, ctx, &query);
        ui.separator();
        preview::preview(self, ui, ctx);
        ui.separator();
        table::table(self, ui, ctx);
    }

    fn scan_bar(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Add folder…").clicked() {
                choose_path(ctx, PathPurpose::ScanFolder);
            }
            let scanning = ctx.features.scan.running;
            if ui
                .add_enabled(!scanning, egui::Button::new("Rescan"))
                .clicked()
            {
                ctx.intents.push(LibraryIntent::Rescan);
            }
            if ui
                .add_enabled(!scanning, egui::Button::new("Factory sounds"))
                .on_hover_text("Write fourfloor's CC0 one-shots and loops and scan them")
                .clicked()
            {
                ctx.intents.push(LibraryIntent::InstallFactory);
            }
        });
        let progress = ctx
            .live
            .jobs
            .iter()
            .find(|(label, _)| label == labels::SCAN || label == labels::FACTORY)
            .map(|(_, fraction)| *fraction);
        if ctx.features.scan.running {
            ui.horizontal(|ui| {
                let fraction = progress.unwrap_or(0.0);
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(260.0)
                        .text(format!("Scanning {:.0} %", fraction * 100.0)),
                );
                if ui.small_button("Stop").clicked() {
                    ctx.intents.push(LibraryIntent::CancelScan);
                }
            });
        } else if let Some(last) = &ctx.features.scan.last {
            ui.label(
                egui::RichText::new(format!("Last scan: {last}"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
        }
    }

    fn results_line(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, query: &Query) {
        let library = &ctx.features.library;
        ui.horizontal(|ui| {
            let text = match &library.source {
                Some(ResultSource::Similar(file)) => {
                    let name = library
                        .record(*file)
                        .map_or_else(|| "that file".to_owned(), |r| r.title);
                    format!("{} files that sound like {name}", library.rows.len())
                }
                Some(ResultSource::Text(text)) => {
                    format!("{} files ranked against \"{text}\"", library.rows.len())
                }
                _ if library.total > library.rows.len() => {
                    format!("{} of {} files", library.rows.len(), library.total)
                }
                _ => format!("{} files", library.rows.len()),
            };
            ui.label(egui::RichText::new(text).color(theme::TEXT_DIM));
            if !matches!(library.source, Some(ResultSource::Query) | None)
                && ui.small_button("Back to filters").clicked()
            {
                ctx.intents.push(LibraryIntent::Search(query.clone()));
            }
        });
    }
}
