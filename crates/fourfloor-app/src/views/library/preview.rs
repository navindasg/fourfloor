//! The selected file: waveform, details, audition and placement buttons.

use egui::{pos2, vec2, Color32, RichText, Sense, Stroke, Ui};
use fourfloor_core::SampleRate;
use fourfloor_library::{FileRecord, PreviewMode};

use crate::state::LibraryIntent;
use crate::theme;
use crate::viewmodel::browser::{
    audition_note, bpm_label, duration_label, key_label, kind_label, mode_label, waveform_columns,
};
use crate::views::dj::deck::DECK_NAMES;

use super::filters::key_choice;
use super::LibraryPanel;
use crate::views::ViewCtx;

/// Height of the waveform strip.
pub const WAVEFORM_HEIGHT: f32 = 56.0;

fn paint_waveform(ui: &mut Ui, ctx: &ViewCtx<'_>) {
    let width = ui.available_width().max(40.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, WAVEFORM_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3.0, theme::CANVAS);
    let Some(waveform) = &ctx.features.library.waveform else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no waveform stored",
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
        return;
    };
    let middle = rect.center().y;
    let half = rect.height() / 2.0 - 2.0;
    let columns = waveform_columns(&waveform.peaks, rect.width() as usize);
    let colour = if ctx.features.auditioning == ctx.features.library.selected {
        theme::ACCENT
    } else {
        theme::ACCENT_2
    };
    for (index, (low, high)) in columns.into_iter().enumerate() {
        let x = rect.left() + index as f32 + 0.5;
        let top = middle - high.clamp(-1.0, 1.0) * half;
        let bottom = (middle - low.clamp(-1.0, 1.0) * half).max(top + 1.0);
        painter.line_segment([pos2(x, top), pos2(x, bottom)], Stroke::new(1.0, colour));
    }
    painter.line_segment(
        [pos2(rect.left(), middle), pos2(rect.right(), middle)],
        Stroke::new(1.0, Color32::from_white_alpha(18)),
    );
}

fn details(ui: &mut Ui, record: &FileRecord) {
    ui.label(RichText::new(&record.title).strong());
    let loudness = record
        .loudness_lufs
        .map_or_else(|| "-".to_owned(), |l| format!("{l:.1} LUFS"));
    ui.label(
        RichText::new(format!(
            "{}  ·  {} BPM  ·  {}  ·  {}  ·  {loudness}",
            kind_label(record.kind),
            bpm_label(record.bpm),
            key_label(record.key),
            duration_label(record.duration_seconds),
        ))
        .small()
        .color(theme::TEXT_DIM),
    );
}

fn audition_controls(
    panel: &mut LibraryPanel,
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    record: &FileRecord,
) {
    ui.horizontal_wrapped(|ui| {
        let playing = ctx.features.auditioning == Some(record.id);
        if ui.button("Audition").clicked() {
            ctx.intents
                .push(LibraryIntent::Audition(record.id, panel.mode, panel.key));
        }
        if ui
            .add_enabled(
                ctx.features.auditioning.is_some(),
                egui::Button::new("Stop"),
            )
            .clicked()
        {
            ctx.intents.push(LibraryIntent::StopAudition);
        }
        egui::ComboBox::new("audition-mode", "")
            .selected_text(mode_label(panel.mode))
            .width(90.0)
            .show_ui(ui, |ui| {
                for mode in [
                    PreviewMode::KeyLocked,
                    PreviewMode::Varispeed,
                    PreviewMode::Original,
                ] {
                    ui.selectable_value(&mut panel.mode, mode, mode_label(mode));
                }
            });
        key_choice(ui, "audition-key", "to key", &mut panel.key);
        if playing {
            ui.label(RichText::new("playing").color(theme::ACCENT));
        }
    });
    let rate = SampleRate::new(ctx.live.sample_rate).unwrap_or(SampleRate::HZ_48000);
    let note = audition_note(record, ctx.project.tempo, panel.key, panel.mode, rate);
    ui.label(
        RichText::new(format!("At {:.1} BPM: {note}", ctx.project.tempo.bpm()))
            .small()
            .color(theme::TEXT_DIM),
    );
}

fn placement(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    record: &FileRecord,
    filter: &fourfloor_library::Query,
) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("Find similar")
            .on_hover_text("Files whose sound features are nearest this one")
            .clicked()
        {
            ctx.intents
                .push(LibraryIntent::FindSimilar(record.id, filter.clone()));
        }
        if ui
            .button("Add to track")
            .on_hover_text("On the selected audio track at the playhead, or on a new audio track")
            .clicked()
        {
            ctx.intents.push(LibraryIntent::AddToTrack(record.id));
        }
        if ui
            .button("Load into sampler")
            .on_hover_text("Into the selected MIDI track's sampler, or a new sampler track")
            .clicked()
        {
            ctx.intents.push(LibraryIntent::LoadSampler(record.id));
        }
        let decks = ctx
            .features
            .dj
            .as_ref()
            .map_or(2, crate::services::dj::DjService::deck_count);
        for (deck, name) in DECK_NAMES.iter().enumerate().take(decks) {
            if ui
                .button(format!("Deck {name}"))
                .on_hover_text(format!("Load into DJ deck {name}"))
                .clicked()
            {
                ctx.intents.push(LibraryIntent::LoadDeck(record.id, deck));
            }
        }
    });
}

/// Draws the preview of the selected file (or a hint).
pub fn preview(panel: &mut LibraryPanel, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    let Some(record) = ctx.features.library.selected_record() else {
        ui.label(RichText::new("Select a file to preview it.").color(theme::TEXT_DIM));
        return;
    };
    details(ui, &record);
    paint_waveform(ui, ctx);
    audition_controls(panel, ui, ctx, &record);
    let filter = panel.query();
    placement(ui, ctx, &record, &filter);
}
