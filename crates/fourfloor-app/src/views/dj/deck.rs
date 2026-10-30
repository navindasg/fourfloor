//! One deck: readouts, waveforms, transport, pitch, cues, loops, beat
//! jump, channel strip and effects.

use egui::{vec2, Color32, Sense, Ui};
use fourfloor_dj::{CrossfaderSide, DeckState, DjCommand, EqBand, SyncMode, HOT_CUES};

use crate::geometry::deck::overview_seconds;
use crate::services::dj::DeckMedia;
use crate::state::{DjIntent, PathPurpose};
use crate::theme;
use crate::viewmodel::dj::{
    clock, hot_cue_command, loop_command, pitch_command, pitch_fader_position, JUMP_BEATS,
    LOOP_BEATS, PITCH_RANGES,
};
use crate::views::shell::dialogs::choose_path;
use crate::views::ViewCtx;
use crate::widgets::buttons::pad;
use crate::widgets::knob::knob;

use super::waveform::{paint_overview, paint_scrolling};

/// Per-deck UI settings that are not part of the session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeckUi {
    /// Index into [`PITCH_RANGES`].
    pub pitch_range: usize,
}

/// Deck letters as printed on the decks.
pub const DECK_NAMES: [&str; 4] = ["A", "B", "C", "D"];

/// The colour of deck `index`.
pub fn deck_color(index: usize) -> Color32 {
    [
        theme::ACCENT,
        theme::ACCENT_2,
        theme::SOLO,
        theme::METER_HOT,
    ][index % 4]
}

fn send(ctx: &mut ViewCtx<'_>, command: DjCommand) {
    ctx.intents.push(DjIntent::Command(command));
}

/// Draws deck `state` (with `media` when a track is loaded).
pub fn deck_panel(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    state: &DeckState,
    media: Option<&DeckMedia>,
    deck_ui: &mut DeckUi,
    loading: bool,
) {
    let index = state.index;
    let name = DECK_NAMES.get(index).copied().unwrap_or("?");
    let color = deck_color(index);
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(6.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            header(ui, ctx, state, name, color, loading);
            match media {
                Some(media) => waveforms(ui, ctx, state, media, color),
                None => {
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 104.0), Sense::hover());
                    ui.painter().rect_filled(rect, 3.0, theme::CANVAS);
                    let text = if loading {
                        "Loading and analysing…"
                    } else {
                        "No track. Load one, or pick a file in the sample browser."
                    };
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(12.0),
                        theme::TEXT_DIM,
                    );
                }
            }
            ui.add_space(4.0);
            transport(ui, ctx, state, name, color, deck_ui);
            ui.add_space(2.0);
            cues_and_loops(ui, ctx, state, name);
            ui.add_space(2.0);
            strip(ui, ctx, state, name);
        });
}

fn header(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    state: &DeckState,
    name: &str,
    color: Color32,
    loading: bool,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("Deck {name}"))
                .strong()
                .color(color),
        );
        let title = state
            .track
            .as_ref()
            .map_or("(empty)".to_owned(), |t| t.name.clone());
        ui.label(egui::RichText::new(crate::widgets::knob::truncate(&title, 36)).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add_enabled(state.track.is_some(), egui::Button::new("Eject"))
                .on_hover_text(format!("Unload deck {name}"))
                .clicked()
            {
                send(ctx, DjCommand::Eject { deck: state.index });
            }
            if ui
                .add_enabled(!loading, egui::Button::new("Load…"))
                .on_hover_text(format!("Load a file into deck {name}"))
                .clicked()
            {
                choose_path(ctx, PathPurpose::LoadDeck(state.index));
            }
        });
    });
    ui.horizontal(|ui| {
        let mono = |text: String| egui::RichText::new(text).monospace();
        let bpm = state
            .bpm
            .map_or("--- BPM".to_owned(), |bpm| format!("{bpm:6.2} BPM"));
        ui.label(mono(bpm).size(15.0).color(theme::TEXT));
        let key = state.track.as_ref().and_then(|t| {
            t.key
                .as_ref()
                .map(|k| format!("{k} ({})", t.camelot.clone().unwrap_or_default()))
        });
        ui.label(mono(key.unwrap_or_else(|| "key ?".to_owned())).color(theme::ACCENT_2));
        let duration = state.track.as_ref().map_or(0.0, |t| t.duration_seconds);
        ui.label(mono(clock(state.position_seconds)));
        ui.label(
            mono(format!("-{}", clock(duration - state.position_seconds))).color(theme::TEXT_DIM),
        );
        ui.label(mono(format!("{:+.2}%", state.pitch_percent)).color(theme::TEXT_DIM));
        if state.at_end {
            ui.label(egui::RichText::new("END").color(theme::METER_CLIP));
        }
    });
}

fn waveforms(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    state: &DeckState,
    media: &DeckMedia,
    color: Color32,
) {
    let width = ui.available_width();
    let (overview, response) = ui.allocate_exact_size(vec2(width, 26.0), Sense::click());
    paint_overview(ui.painter(), overview, media, state, color);
    if let Some(pos) = response
        .interact_pointer_pos()
        .filter(|_| response.clicked())
    {
        let seconds = overview_seconds(
            pos.x,
            media.track.seconds(),
            overview.left(),
            overview.width(),
        );
        send(
            ctx,
            DjCommand::Seek {
                deck: state.index,
                seconds,
            },
        );
    }
    ui.add_space(3.0);
    let (scroll, _) = ui.allocate_exact_size(vec2(width, 74.0), Sense::hover());
    paint_scrolling(&ui.painter_at(scroll), scroll, media, state, color);
}

fn transport(
    ui: &mut Ui,
    ctx: &mut ViewCtx<'_>,
    state: &DeckState,
    name: &str,
    color: Color32,
    deck_ui: &mut DeckUi,
) {
    let deck = state.index;
    let size = vec2(54.0, 26.0);
    ui.horizontal(|ui| {
        let play = if state.playing { "Pause" } else { "Play" };
        if pad(
            ui,
            play,
            &format!("Play deck {name}"),
            state.playing,
            color,
            size,
        )
        .clicked()
        {
            send(ctx, DjCommand::TogglePlay { deck });
        }
        if pad(ui, "Cue", &format!("Cue deck {name}"), false, color, size).clicked() {
            send(ctx, DjCommand::Cue { deck });
        }
        let next = match state.sync {
            SyncMode::Off => SyncMode::Beat,
            SyncMode::Beat => SyncMode::Tempo,
            SyncMode::Tempo => SyncMode::Off,
        };
        let sync_text = match state.sync {
            SyncMode::Off => "Sync",
            SyncMode::Beat => "Beat",
            SyncMode::Tempo => "Tempo",
        };
        let synced = state.sync != SyncMode::Off;
        if pad(
            ui,
            sync_text,
            &format!("Sync deck {name}"),
            synced,
            theme::ACCENT_2,
            size,
        )
        .on_hover_text("Click: off, beat sync, tempo only")
        .clicked()
        {
            send(ctx, DjCommand::SetSync { deck, mode: next });
        }
        let master = ctx.features.dj.as_ref().and_then(|dj| dj.state().master) == Some(deck);
        if pad(
            ui,
            "Master",
            &format!("Tempo master deck {name}"),
            master,
            theme::METER_HOT,
            size,
        )
        .clicked()
        {
            send(
                ctx,
                DjCommand::SetMaster {
                    deck: (!master).then_some(deck),
                },
            );
        }
        let lock = state.key_lock;
        if pad(
            ui,
            "Key",
            &format!("Key lock deck {name}"),
            lock,
            theme::SOLO,
            size,
        )
        .on_hover_text("Key lock: tempo changes keep the pitch")
        .clicked()
        {
            send(ctx, DjCommand::SetKeyLock { deck, on: !lock });
        }
        let quantize = state.quantize;
        if pad(
            ui,
            "Q",
            &format!("Quantize deck {name}"),
            quantize,
            theme::GRID_BAR,
            vec2(26.0, 26.0),
        )
        .on_hover_text("Quantize cues and loops to the beat")
        .clicked()
        {
            send(
                ctx,
                DjCommand::SetQuantize {
                    deck,
                    on: !quantize,
                },
            );
        }
    });
    ui.horizontal(|ui| {
        let range = PITCH_RANGES[deck_ui.pitch_range.min(PITCH_RANGES.len() - 1)];
        let mut position = pitch_fader_position(state.pitch_percent, range);
        let slider = egui::Slider::new(&mut position, -1.0..=1.0)
            .show_value(false)
            .text(format!("pitch ±{range:.0}%"));
        if ui
            .add_sized(vec2(ui.available_width().min(260.0), 18.0), slider)
            .on_hover_text(format!("Pitch deck {name} (double-click resets)"))
            .changed()
        {
            send(ctx, pitch_command(deck, position, range));
        }
        if ui.small_button("0").on_hover_text("Reset pitch").clicked() {
            send(ctx, DjCommand::SetPitch { deck, percent: 0.0 });
        }
        if ui.small_button("range").clicked() {
            deck_ui.pitch_range = (deck_ui.pitch_range + 1) % PITCH_RANGES.len();
        }
        for (text, percent) in [("−", -4.0), ("+", 4.0)] {
            let bend = ui
                .add(egui::Button::new(text).sense(Sense::click_and_drag()))
                .on_hover_text("Hold to bend (temporary pitch)");
            if bend.is_pointer_button_down_on() {
                send(ctx, DjCommand::Bend { deck, percent });
            } else if bend.drag_stopped() || bend.clicked() {
                send(ctx, DjCommand::Bend { deck, percent: 0.0 });
            }
        }
        for (text, ms) in [("«", -10.0), ("»", 10.0)] {
            if ui.small_button(text).on_hover_text("Nudge 10 ms").clicked() {
                send(
                    ctx,
                    DjCommand::Nudge {
                        deck,
                        milliseconds: ms,
                    },
                );
            }
        }
    });
}

fn cues_and_loops(ui: &mut Ui, ctx: &mut ViewCtx<'_>, state: &DeckState, name: &str) {
    let deck = state.index;
    let clear = ui.input(|i| i.modifiers.shift);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("Hot cues")
                .small()
                .color(theme::TEXT_DIM),
        );
        for slot in 0..HOT_CUES {
            let set = state.hot_cues.get(slot).copied().flatten().is_some();
            let label = format!("Hot cue {} deck {name}", slot + 1);
            if pad(
                ui,
                &(slot + 1).to_string(),
                &label,
                set,
                theme::track_color(slot),
                vec2(26.0, 22.0),
            )
            .on_hover_text("Click: set or jump. Shift-click: clear")
            .clicked()
            {
                send(ctx, hot_cue_command(deck, slot, clear));
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Loop").small().color(theme::TEXT_DIM));
        let active = crate::viewmodel::dj::active_loop_beats(state);
        for beats in LOOP_BEATS {
            let text = if beats < 1.0 {
                "1/2".to_owned()
            } else {
                format!("{beats:.0}")
            };
            let on = active.is_some_and(|b| (b - beats).abs() < 1e-3);
            if pad(
                ui,
                &text,
                &format!("Loop {text} deck {name}"),
                on,
                theme::ACCENT_2,
                vec2(28.0, 22.0),
            )
            .on_hover_text("Shift-click: loop roll")
            .clicked()
            {
                let command = if clear {
                    DjCommand::LoopRoll { deck, beats }
                } else {
                    loop_command(state, beats)
                };
                send(ctx, command);
            }
        }
        if ui.small_button("Exit").clicked() {
            send(ctx, DjCommand::LoopExit { deck });
        }
        if ui.small_button("Reloop").clicked() {
            send(ctx, DjCommand::Reloop { deck });
        }
    });
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Jump").small().color(theme::TEXT_DIM));
        for beats in JUMP_BEATS.iter().rev().map(|b| -b).chain(JUMP_BEATS) {
            let text = if beats < 0.0 {
                format!("−{:.0}", -beats)
            } else {
                format!("+{beats:.0}")
            };
            if ui
                .small_button(text)
                .on_hover_text(format!("Beat jump {beats:+.0} beats"))
                .clicked()
            {
                send(ctx, DjCommand::BeatJump { deck, beats });
            }
        }
    });
}

/// Plain value from a knob's travel `t` (0..1) over `low..high`.
fn lerp(low: f32, high: f32, t: f32) -> f32 {
    low + (high - low) * t
}

fn strip(ui: &mut Ui, ctx: &mut ViewCtx<'_>, state: &DeckState, name: &str) {
    let deck = state.index;
    ui.horizontal(|ui| {
        for (band, text) in [
            (EqBand::High, "High"),
            (EqBand::Mid, "Mid"),
            (EqBand::Low, "Low"),
        ] {
            let i = band.index();
            let db = state.eq_db[i];
            let travel = (db + 26.0) / 32.0;
            let out = knob(
                ui,
                &format!("{text} {name}"),
                &format!("{db:+.0} dB"),
                travel,
                26.0 / 32.0,
                26.0,
            );
            if let Some(t) = out.changed {
                send(
                    ctx,
                    DjCommand::SetEq {
                        deck,
                        band,
                        db: lerp(-26.0, 6.0, t).round(),
                    },
                );
            }
            let kill = state.eq_kill[i];
            ui.vertical(|ui| {
                ui.add_space(14.0);
                if pad(
                    ui,
                    "K",
                    &format!("Kill {text} deck {name}"),
                    kill,
                    theme::METER_CLIP,
                    vec2(20.0, 18.0),
                )
                .on_hover_text(format!("Kill the {text} band"))
                .clicked()
                {
                    send(
                        ctx,
                        DjCommand::SetEqKill {
                            deck,
                            band,
                            on: !kill,
                        },
                    );
                }
            });
        }
        let filter = state.filter;
        let out = knob(
            ui,
            &format!("Filter {name}"),
            &filter_text(filter),
            (filter + 1.0) / 2.0,
            0.5,
            26.0,
        );
        if let Some(t) = out.changed {
            let amount = lerp(-1.0, 1.0, t);
            let amount = if amount.abs() < 0.03 { 0.0 } else { amount };
            send(ctx, DjCommand::SetFilter { deck, amount });
        }
        let trim = state.trim_db;
        let out = knob(
            ui,
            &format!("Trim {name}"),
            &format!("{trim:+.0} dB"),
            (trim + 24.0) / 36.0,
            24.0 / 36.0,
            26.0,
        );
        if let Some(t) = out.changed {
            send(
                ctx,
                DjCommand::SetTrim {
                    deck,
                    db: lerp(-24.0, 12.0, t).round(),
                },
            );
        }
    });
    ui.horizontal(|ui| {
        let mut fader = state.fader;
        ui.spacing_mut().slider_width = 90.0;
        if ui
            .add(
                egui::Slider::new(&mut fader, 0.0..=1.0)
                    .show_value(false)
                    .text("fader"),
            )
            .on_hover_text(format!("Channel fader deck {name}"))
            .changed()
        {
            send(ctx, DjCommand::SetFader { deck, value: fader });
        }
        let cue = state.cue_monitor;
        if pad(
            ui,
            "Cue",
            &format!("Headphone cue deck {name}"),
            cue,
            theme::METER_HOT,
            vec2(34.0, 20.0),
        )
        .on_hover_text("Send this deck to the headphones (outputs 3-4)")
        .clicked()
        {
            send(ctx, DjCommand::SetCueMonitor { deck, on: !cue });
        }
        for (side, text) in [
            (CrossfaderSide::A, "A"),
            (CrossfaderSide::Thru, "Thru"),
            (CrossfaderSide::B, "B"),
        ] {
            let on = state.crossfader_side == side;
            let label = format!("Crossfader {text} deck {name}");
            if pad(ui, text, &label, on, theme::GRID_BAR, vec2(30.0, 20.0)).clicked() {
                send(ctx, DjCommand::SetCrossfaderSide { deck, side });
            }
        }
    });
    effects(ui, ctx, state, name);
}

fn filter_text(amount: f32) -> String {
    if amount.abs() < 0.01 {
        "off".to_owned()
    } else if amount < 0.0 {
        format!("LP {:.0}%", -amount * 100.0)
    } else {
        format!("HP {:.0}%", amount * 100.0)
    }
}

fn effects(ui: &mut Ui, ctx: &mut ViewCtx<'_>, state: &DeckState, name: &str) {
    let deck = state.index;
    ui.horizontal(|ui| {
        for (slot, effect) in state.effects.iter().enumerate() {
            let short = effect.kind.trim_start_matches("fx.");
            let label = format!("FX {short} deck {name}");
            if pad(
                ui,
                short,
                &label,
                effect.enabled,
                theme::ACCENT,
                vec2(52.0, 20.0),
            )
            .on_hover_text(format!("{} on deck {name}", effect.kind))
            .clicked()
            {
                send(
                    ctx,
                    DjCommand::SetDeckEffect {
                        deck,
                        slot,
                        on: !effect.enabled,
                    },
                );
            }
            if let Some(mix) = effect.params.get("Mix").copied() {
                let mut percent = mix * 100.0;
                if ui
                    .add(
                        egui::DragValue::new(&mut percent)
                            .range(0.0..=100.0)
                            .speed(0.5)
                            .fixed_decimals(0)
                            .suffix("%"),
                    )
                    .on_hover_text(format!("{short} mix"))
                    .changed()
                {
                    let value = percent / 100.0;
                    send(
                        ctx,
                        DjCommand::SetDeckEffectParam {
                            deck,
                            slot,
                            param: "Mix".to_owned(),
                            value,
                        },
                    );
                }
            }
        }
    });
}
