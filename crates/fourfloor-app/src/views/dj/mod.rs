//! The DJ view: two to four decks, the mixer (crossfader, master,
//! headphones, recording) and the "what mixes next" list.
//!
//! The decks play through the app's audio callback on top of the project
//! (headphone cue on outputs 3 and 4 when the device has them).

pub mod deck;
pub mod waveform;

use egui::{vec2, Ui};
use fourfloor_dj::{CrossfaderCurve, DjCommand, MAX_DECKS};

use crate::state::{DjIntent, LibraryIntent, PathPurpose, ViewId};
use crate::theme;
use crate::viewmodel::browser::{bpm_label, key_label};
use crate::views::shell::dialogs::choose_path;
use crate::widgets::buttons::pad;
use crate::widgets::meter::paint_stereo_meter;

use super::{View, ViewCtx};
use deck::{deck_panel, DeckUi, DECK_NAMES};

/// The DJ view's own UI state.
#[derive(Debug, Default)]
pub struct DjView {
    decks: [DeckUi; MAX_DECKS],
    asked_to_start: bool,
    /// The deck the suggestion list follows.
    pub follow: usize,
}

impl View for DjView {
    fn id(&self) -> ViewId {
        ViewId::DJ
    }

    fn title(&self) -> &str {
        "DJ"
    }

    fn ui(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
        let Some(dj) = ctx.features.dj.as_ref() else {
            if !self.asked_to_start {
                ctx.intents.push(DjIntent::Start);
                self.asked_to_start = true;
            }
            ui.centered_and_justified(|ui| {
                if ui.button("Start the DJ decks").clicked() {
                    ctx.intents.push(DjIntent::Start);
                }
            });
            return;
        };
        let state = dj.state().clone();
        self.top_bar(
            ui,
            ctx,
            state.decks.len(),
            state.master_bpm,
            &state.key_lock_engine,
        );
        ui.add_space(4.0);
        // Decks (and mixer) on the left, suggestions on the right, as two
        // explicit top-down regions so neither can push the other away.
        let full = ui.available_rect_before_wrap();
        let suggestions_width = 300.0_f32.min(full.width() * 0.28);
        let decks_rect = egui::Rect::from_min_max(
            full.min,
            egui::pos2(full.right() - suggestions_width - 12.0, full.bottom()),
        );
        let side_rect = egui::Rect::from_min_max(
            egui::pos2(full.right() - suggestions_width, full.top()),
            full.max,
        );
        let top_down = egui::Layout::top_down(egui::Align::LEFT);
        ui.allocate_rect(full, egui::Sense::hover());
        let x = decks_rect.right() + 6.0;
        ui.painter()
            .vline(x, full.y_range(), egui::Stroke::new(1.0, theme::GRID_BEAT));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(side_rect).layout(top_down),
            |ui| {
                self.suggestions(ui, ctx, state.decks.len());
            },
        );
        {
            let ui =
                &mut ui.new_child(egui::UiBuilder::new().max_rect(decks_rect).layout(top_down));
            {
                egui::ScrollArea::vertical()
                    .id_salt("dj-decks")
                    .max_width(decks_rect.width())
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // Two decks per row; one per row when narrow.
                        // Sized from the region, not from what the scroll
                        // area remembers of last frame's content.
                        let gap = 8.0;
                        let inner = decks_rect.width() - 14.0;
                        ui.set_max_width(inner);
                        let columns = if inner < 900.0 { 1 } else { 2 };
                        let column_width =
                            ((inner - gap * (columns as f32 - 1.0)) / columns as f32).max(1.0);
                        for row in state.decks.chunks(columns) {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = gap;
                                for deck_state in row {
                                    let index = deck_state.index;
                                    let media = dj.media(index);
                                    let loading = ctx.features.loading_decks.contains(&index);
                                    let layout = egui::Layout::top_down(egui::Align::LEFT);
                                    ui.allocate_ui_with_layout(
                                        vec2(column_width, 0.0),
                                        layout,
                                        |ui| {
                                            ui.set_max_width(column_width);
                                            deck_panel(
                                                ui,
                                                ctx,
                                                deck_state,
                                                media,
                                                &mut self.decks[index],
                                                loading,
                                            );
                                        },
                                    );
                                }
                            });
                            ui.add_space(6.0);
                        }
                        mixer_bar(ui, ctx, &state, dj.peaks());
                    });
            }
        }
    }
}

impl DjView {
    fn top_bar(
        &mut self,
        ui: &mut Ui,
        ctx: &mut ViewCtx<'_>,
        decks: usize,
        master_bpm: Option<f64>,
        engine: &str,
    ) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Decks").strong());
            for count in 2..=MAX_DECKS {
                let label = format!("{count} decks");
                let size = vec2(22.0, 20.0);
                if pad(
                    ui,
                    &count.to_string(),
                    &label,
                    decks == count,
                    theme::ACCENT,
                    size,
                )
                .clicked()
                    && decks != count
                {
                    ctx.intents.push(DjIntent::SetDecks(count));
                }
            }
            ui.separator();
            let tempo =
                master_bpm.map_or("no master".to_owned(), |bpm| format!("master {bpm:.2} BPM"));
            ui.label(egui::RichText::new(tempo).monospace().color(theme::ACCENT));
            ui.label(
                egui::RichText::new(format!("key lock: {engine}"))
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let recording = ctx
                    .features
                    .dj
                    .as_ref()
                    .and_then(|dj| dj.state().recording.clone());
                match recording {
                    Some(path) => {
                        if ui
                            .add(egui::Button::new("Stop recording").selected(true))
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            ctx.intents
                                .push(DjIntent::Command(DjCommand::StopRecording));
                        }
                    }
                    None => {
                        if ui.button("Record mix…").clicked() {
                            choose_path(ctx, PathPurpose::RecordMix);
                        }
                    }
                }
                if ui.button("Sample browser").clicked() {
                    ctx.ui.panels.library = !ctx.ui.panels.library;
                }
            });
        });
    }

    fn suggestions(&mut self, ui: &mut Ui, ctx: &mut ViewCtx<'_>, decks: usize) {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new("What mixes next").strong());
            ui.horizontal(|ui| {
                ui.label("after deck");
                for (deck, name) in DECK_NAMES.iter().copied().enumerate().take(decks) {
                    if ui.selectable_label(self.follow == deck, name).clicked() {
                        self.follow = deck;
                        ctx.intents.push(DjIntent::Suggest(deck));
                    }
                }
                if ui.small_button("refresh").clicked() {
                    ctx.intents.push(DjIntent::Suggest(self.follow));
                }
            });
            if !ctx.features.library.is_open() {
                ui.label(
                    egui::RichText::new(
                        "Open the sample browser and scan your music to get suggestions.",
                    )
                    .color(theme::TEXT_DIM),
                );
                if ui.button("Open sample browser").clicked() {
                    ctx.ui.panels.library = true;
                    ctx.intents.push(LibraryIntent::Open);
                }
                return;
            }
            if ctx.features.suggestions_for != Some(self.follow)
                || ctx.features.suggestions.is_empty()
            {
                ui.label(
                    egui::RichText::new("Load a track with a known tempo or key, then refresh.")
                        .color(theme::TEXT_DIM),
                );
                return;
            }
            egui::ScrollArea::vertical()
                .id_salt("dj-next")
                .show(ui, |ui| {
                    for next in &ctx.features.suggestions {
                        egui::Frame::new()
                            .fill(theme::SURFACE)
                            .corner_radius(4.0)
                            .inner_margin(6.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.label(
                                    egui::RichText::new(crate::widgets::knob::truncate(
                                        &next.title,
                                        34,
                                    ))
                                    .strong(),
                                );
                                let tempo = next
                                    .tempo_change_percent
                                    .map_or(String::new(), |p| format!("  pitch {p:+.1}%"));
                                let sound = next.similarity.map_or(String::new(), |s| {
                                    format!("  sound {:.0}%", s * 100.0)
                                });
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} BPM · {} · {:?}{tempo}{sound}",
                                        bpm_label(next.bpm),
                                        key_label(next.key),
                                        next.key_match
                                    ))
                                    .small()
                                    .color(theme::TEXT_DIM),
                                );
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::ProgressBar::new(next.score as f32)
                                            .desired_width(90.0)
                                            .text(format!("{:.0}", next.score * 100.0)),
                                    );
                                    for deck in (0..decks).filter(|d| *d != self.follow) {
                                        if ui
                                            .small_button(format!("Load {}", DECK_NAMES[deck]))
                                            .clicked()
                                        {
                                            ctx.intents
                                                .push(LibraryIntent::LoadDeck(next.file, deck));
                                        }
                                    }
                                });
                            });
                        ui.add_space(3.0);
                    }
                });
        });
    }
}

fn mixer_bar(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    state: &fourfloor_dj::SessionState,
    peaks: [[f32; 2]; 4],
) {
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(6.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Mixer").strong());
                for (deck, peak) in peaks.iter().enumerate().take(state.decks.len()) {
                    ui.label(DECK_NAMES[deck]);
                    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 34.0), egui::Sense::hover());
                    paint_stereo_meter(ui, rect, *peak);
                }
                ui.separator();
                let mixer = &state.mixer;
                let mut position = mixer.crossfader;
                ui.label("A");
                if ui
                    .add_sized(
                        vec2(220.0, 20.0),
                        egui::Slider::new(&mut position, -1.0..=1.0).show_value(false),
                    )
                    .on_hover_text("Crossfader")
                    .changed()
                {
                    ctx.intents
                        .push(DjIntent::Command(DjCommand::SetCrossfader { position }));
                }
                ui.label("B");
                let mut curve = mixer.curve;
                egui::ComboBox::new("xfader-curve", "curve")
                    .selected_text(format!("{curve:?}"))
                    .show_ui(ui, |ui| {
                        for option in [
                            CrossfaderCurve::Linear,
                            CrossfaderCurve::ConstantPower,
                            CrossfaderCurve::Sharp,
                        ] {
                            ui.selectable_value(&mut curve, option, format!("{option:?}"));
                        }
                    });
                if curve != mixer.curve {
                    ctx.intents
                        .push(DjIntent::Command(DjCommand::SetCrossfaderCurve { curve }));
                }
            });
            ui.horizontal(|ui| {
                let mixer = &state.mixer;
                let mut master = mixer.master_db;
                if ui
                    .add(
                        egui::Slider::new(&mut master, -60.0..=6.0)
                            .text("master dB")
                            .fixed_decimals(1),
                    )
                    .changed()
                {
                    ctx.intents
                        .push(DjIntent::Command(DjCommand::SetMasterGain { db: master }));
                }
                let mut cue_mix = mixer.cue_mix;
                if ui
                    .add(
                        egui::Slider::new(&mut cue_mix, 0.0..=1.0)
                            .text("cue/master")
                            .show_value(false),
                    )
                    .on_hover_text(
                        "Headphones: cued decks at the left end, the master mix at the right",
                    )
                    .changed()
                {
                    ctx.intents
                        .push(DjIntent::Command(DjCommand::SetCueMix { value: cue_mix }));
                }
                let mut phones = mixer.headphone_db;
                if ui
                    .add(
                        egui::Slider::new(&mut phones, -60.0..=6.0)
                            .text("phones dB")
                            .fixed_decimals(1),
                    )
                    .changed()
                {
                    ctx.intents
                        .push(DjIntent::Command(DjCommand::SetHeadphoneGain {
                            db: phones,
                        }));
                }
            });
        });
}
