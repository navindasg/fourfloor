//! The panel's buttons: generate drums or a bassline, separate the
//! selected audio clip into stems, transcribe it to MIDI, report the mix.

use egui::Ui;

use crate::app::assistant::labels;
use crate::state::{AssistantIntent, MlIntent};
use crate::theme;
use crate::viewmodel::assistant::{
    bass_args, drum_args, selected_audio_clip, BASS_STYLES, DRUM_STYLES, KEYS,
};
use crate::views::ViewCtx;

use super::{hint, AssistantPanel};

/// Width of the style and key pickers.
const PICKER_WIDTH: f32 = 92.0;

/// Draws the buttons.
pub fn actions(panel: &mut AssistantPanel, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    generate(panel, ui, ctx);
    ui.add_space(4.0);
    clip_tools(panel, ui, ctx);
    ui.add_space(4.0);
    report(ui, ctx);
}

fn picker(ui: &mut Ui, id: &str, value: &mut &'static str, options: &[&'static str]) {
    egui::ComboBox::from_id_salt(id)
        .width(PICKER_WIDTH)
        .selected_text(*value)
        .show_ui(ui, |ui| {
            for option in options {
                ui.selectable_value(value, *option, *option);
            }
        })
        .response
        .on_hover_text(id.replace('-', " "));
}

fn generate(panel: &mut AssistantPanel, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    let busy = ctx.features.assistant_busy;
    ui.horizontal(|ui| {
        ui.label("Bars");
        ui.add(
            egui::DragValue::new(&mut panel.bars)
                .range(1..=64)
                .speed(0.2),
        )
        .on_hover_text("Length of generated parts");
        hint(ui, format!("seed {}", panel.seed));
    });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy,
                egui::Button::new("Generate drums").min_size(egui::vec2(116.0, 0.0)),
            )
            .on_hover_text("Write a house drum pattern: kick track plus kit")
            .clicked()
        {
            let args = drum_args(panel.bars, panel.drum_style, panel.seed);
            ctx.intents.push(AssistantIntent::Tool(
                "add_house_drum_pattern".to_owned(),
                args,
            ));
            panel.seed += 1;
        }
        picker(ui, "drum-style", &mut panel.drum_style, &DRUM_STYLES);
    });
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !busy,
                egui::Button::new("Generate bass").min_size(egui::vec2(116.0, 0.0)),
            )
            .on_hover_text("Write a bassline on a new acid bass track")
            .clicked()
        {
            let args = bass_args(panel.bars, panel.key, panel.bass_style, panel.seed);
            ctx.intents
                .push(AssistantIntent::Tool("add_bassline".to_owned(), args));
            panel.seed += 1;
        }
        picker(ui, "bass-style", &mut panel.bass_style, &BASS_STYLES);
        picker(ui, "bass-key", &mut panel.key, &KEYS);
    });
}

fn clip_tools(panel: &mut AssistantPanel, ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    let clip = selected_audio_clip(ctx.project, &ctx.ui.selection);
    let busy = ctx.features.ml_busy;
    let saved = ctx.live.project_dir.is_some();
    let ready = clip.is_some() && saved && !busy;
    let why = if busy {
        "A separation or transcription is running"
    } else if clip.is_none() {
        "Select an audio clip in the arrangement first"
    } else if !saved {
        "Save the project first: clip audio lives in its folder"
    } else {
        ""
    };
    ui.horizontal(|ui| {
        let stems = ui
            .add_enabled(ready, egui::Button::new("Stem-separate clip"))
            .on_hover_text("Split the selected audio clip into stems on new tracks")
            .on_disabled_hover_text(why);
        if let (true, Some(clip)) = (stems.clicked(), clip) {
            ctx.intents.push(MlIntent::Separate {
                clip,
                prefer_model: panel.prefer_model,
            });
        }
        let midi = ui
            .add_enabled(ready, egui::Button::new("Audio to MIDI"))
            .on_hover_text("Transcribe the selected audio clip to a new MIDI track (Basic Pitch)")
            .on_disabled_hover_text(why);
        if let (true, Some(clip)) = (midi.clicked(), clip) {
            ctx.intents.push(MlIntent::Transcribe(clip));
        }
    });
    ui.checkbox(&mut panel.prefer_model, "Use Spleeter when downloaded");
    if busy {
        progress_line(ui, ctx, &[labels::STEMS, labels::MIDI]);
    } else if !why.is_empty() {
        hint(ui, why);
    }
}

fn report(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    let busy = ctx.features.report_busy;
    let against = ctx.ui.reference.name.clone();
    ui.horizontal(|ui| {
        let text = if against.is_some() {
            "Mix report vs reference"
        } else {
            "Mix report"
        };
        if ui
            .add_enabled(!busy, egui::Button::new(text))
            .on_hover_text("Render the project, measure it and suggest changes")
            .clicked()
        {
            ctx.intents.push(AssistantIntent::MixReport);
        }
        match &against {
            Some(name) => hint(ui, name.clone()),
            None => hint(ui, "no reference loaded"),
        }
    });
    if busy {
        progress_line(ui, ctx, &[labels::REPORT]);
    }
}

fn progress_line(ui: &mut Ui, ctx: &ViewCtx<'_>, wanted: &[&str]) {
    if let Some((label, fraction)) = ctx
        .live
        .jobs
        .iter()
        .find(|(label, _)| wanted.contains(&label.as_str()))
    {
        ui.add(
            egui::ProgressBar::new(*fraction)
                .desired_width(ui.available_width().min(260.0))
                .text(egui::RichText::new(label.as_str()).color(theme::TEXT)),
        );
    }
}
