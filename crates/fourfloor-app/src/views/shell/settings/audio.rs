//! Settings tab: audio output device and buffer, MIDI input port.

use egui::{RichText, Ui};

use crate::state::Intent;
use crate::theme;
use crate::views::ViewCtx;

/// Draws the tab.
pub fn audio_tab(ui: &mut Ui, ctx: &mut ViewCtx<'_>) {
    ui.label(RichText::new("Audio output").strong());
    ui.label(format!(
        "Now: {} at {} Hz",
        ctx.live.output_name, ctx.live.sample_rate
    ));
    let devices = ctx.ui.devices.clone();
    let mut chosen = devices.output.clone();
    egui::ComboBox::new("output-device", "output")
        .selected_text(
            chosen
                .clone()
                .unwrap_or_else(|| "System default".to_owned()),
        )
        .width(300.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut chosen, None, "System default");
            for name in &devices.outputs {
                ui.selectable_value(&mut chosen, Some(name.clone()), name);
            }
        });
    let mut buffer = devices.buffer_frames;
    egui::ComboBox::new("buffer-size", "buffer")
        .selected_text(buffer.map_or("Device default".to_owned(), |b| format!("{b} frames")))
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut buffer, None, "Device default");
            for frames in [64u32, 128, 256, 512, 1024] {
                ui.selectable_value(&mut buffer, Some(frames), format!("{frames} frames"));
            }
        });
    ctx.ui.devices.output = chosen.clone();
    ctx.ui.devices.buffer_frames = buffer;
    if ui.button("Apply (restarts audio)").clicked() {
        ctx.intents.push(Intent::RestartAudio {
            device: chosen,
            buffer_frames: buffer,
        });
    }
    ui.separator();
    ui.label(RichText::new("MIDI input").strong());
    let current = ctx.ui.devices.midi_input.clone();
    egui::ComboBox::new("midi-input", "input")
        .selected_text(current.clone().unwrap_or_else(|| "None".to_owned()))
        .width(300.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), "None").clicked() {
                ctx.intents.push(Intent::OpenMidiInput(None));
            }
            for name in &devices.midi_inputs {
                if ui
                    .selectable_label(current.as_deref() == Some(name), name)
                    .clicked()
                {
                    ctx.intents.push(Intent::OpenMidiInput(Some(name.clone())));
                }
            }
        });
    if devices.midi_inputs.is_empty() {
        ui.label(RichText::new("No MIDI inputs found.").color(theme::TEXT_DIM));
    }
    ui.label(
        RichText::new(
            "Arm a MIDI track (R) to play its instrument from the input; mapped controllers move their parameters.",
        )
        .small()
        .color(theme::TEXT_DIM),
    );
}
