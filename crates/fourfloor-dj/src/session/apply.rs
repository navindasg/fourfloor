//! Carrying out [`DjCommand`]s: checking them, then sending deck commands
//! and parameter changes.

use fourfloor_effects::Isolator;

use super::{DeckControl, DjSession, ParamSink, MIXER_ID};
use crate::command::{DjCommand, EqBand};
use crate::deck::{DeckCommand, SyncMode, HOT_CUES};
use crate::error::DjError;
use crate::filter::DjFilter;
use crate::fxslot::FxSlot;
use crate::mixer::DjMixer;

const MIN_LOOP_BEATS: f64 = 1.0 / 32.0;
const MAX_LOOP_BEATS: f64 = 32.0;

fn check(what: &'static str, value: f64, low: f64, high: f64) -> Result<(), DjError> {
    if value.is_finite() && (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(DjError::InvalidValue { what, value })
    }
}

fn check32(what: &'static str, value: f32, low: f32, high: f32) -> Result<(), DjError> {
    check(what, f64::from(value), f64::from(low), f64::from(high))
}

fn band_param(band: EqBand) -> fourfloor_core::ParamId {
    match band {
        EqBand::Low => Isolator::LOW,
        EqBand::Mid => Isolator::MID,
        EqBand::High => Isolator::HIGH,
    }
}

fn on(value: bool) -> f32 {
    if value {
        1.0
    } else {
        0.0
    }
}

impl DeckControl {
    /// Track samples per second, and per beat, of the loaded track.
    fn track_rate(&self, deck: usize) -> Result<f64, DjError> {
        self.track
            .as_ref()
            .map(|track| track.sample_rate.as_f64())
            .ok_or(DjError::NoTrack(deck))
    }

    fn need_grid(&self, deck: usize) -> Result<(), DjError> {
        let track = self.track.as_ref().ok_or(DjError::NoTrack(deck))?;
        track.grid.map(|_| ()).ok_or(DjError::NoGrid(deck))
    }

    fn send_eq(&self, band: EqBand, sink: &mut dyn ParamSink) -> Result<(), DjError> {
        let index = band.index();
        let db = if self.eq_kill[index] {
            Isolator::KILL_DB
        } else {
            self.eq_db[index]
        };
        sink.set_param(self.ids.isolator, band_param(band), db)
    }

    fn stem_index(&self, deck: usize, stem: &str) -> Result<usize, DjError> {
        self.stems
            .iter()
            .position(|state| state.name == stem)
            .ok_or_else(|| DjError::NoSuchStem {
                deck,
                stem: stem.to_owned(),
            })
    }
}

/// Picks a tempo master for a deck that wants to sync: a playing deck with
/// a grid, else any other deck with a grid.
fn choose_master(session: &DjSession, deck: usize) -> Option<usize> {
    let candidates = || {
        session
            .decks
            .iter()
            .enumerate()
            .filter(move |(index, control)| {
                *index != deck && control.track.as_ref().is_some_and(|t| t.grid.is_some())
            })
    };
    candidates()
        .find(|(_, control)| control.handle.snapshot().playing)
        .or_else(|| candidates().next())
        .map(|(index, _)| index)
}

fn deck_command(session: &mut DjSession, deck: usize, command: DeckCommand) -> Result<(), DjError> {
    let control = session
        .decks
        .get_mut(deck)
        .ok_or(DjError::NoSuchDeck(deck))?;
    control.handle.send(command)
}

fn with_track(session: &mut DjSession, deck: usize, command: DeckCommand) -> Result<(), DjError> {
    let control = session.decks.get(deck).ok_or(DjError::NoSuchDeck(deck))?;
    control.track_rate(deck)?;
    deck_command(session, deck, command)
}

fn with_grid(session: &mut DjSession, deck: usize, command: DeckCommand) -> Result<(), DjError> {
    session
        .decks
        .get(deck)
        .ok_or(DjError::NoSuchDeck(deck))?
        .need_grid(deck)?;
    deck_command(session, deck, command)
}

fn loop_beats(beats: f64) -> Result<(), DjError> {
    check(
        "loop length in beats",
        beats,
        MIN_LOOP_BEATS,
        MAX_LOOP_BEATS,
    )
}

fn hot_cue_slot(slot: usize) -> Result<(), DjError> {
    if slot < HOT_CUES {
        Ok(())
    } else {
        Err(DjError::OutOfRange {
            what: "hot cue",
            index: slot,
            limit: HOT_CUES,
        })
    }
}

pub(super) fn apply(
    session: &mut DjSession,
    command: &DjCommand,
    sink: &mut dyn ParamSink,
) -> Result<(), DjError> {
    match command {
        DjCommand::LoadTrack {
            deck,
            path,
            analyze,
            stems,
        } => session.load_file(*deck, path, *analyze, stems),
        DjCommand::Eject { deck } => session.eject(*deck),
        &DjCommand::Play { deck } => with_track(session, deck, DeckCommand::Play),
        &DjCommand::Pause { deck } => with_track(session, deck, DeckCommand::Pause),
        &DjCommand::TogglePlay { deck } => {
            let control = session.decks.get(deck).ok_or(DjError::NoSuchDeck(deck))?;
            let command = if control.handle.snapshot().playing {
                DeckCommand::Pause
            } else {
                DeckCommand::Play
            };
            with_track(session, deck, command)
        }
        &DjCommand::Cue { deck } => with_track(session, deck, DeckCommand::Cue),
        &DjCommand::SetCuePoint { deck } => with_track(session, deck, DeckCommand::SetCuePoint),
        &DjCommand::Seek { deck, seconds } => {
            let control = session.decks.get(deck).ok_or(DjError::NoSuchDeck(deck))?;
            let rate = control.track_rate(deck)?;
            let length = control
                .track
                .as_ref()
                .map_or(0.0, |track| track.summary.duration_seconds);
            check("seek position in seconds", seconds, 0.0, length)?;
            deck_command(session, deck, DeckCommand::Seek(seconds * rate))
        }
        &DjCommand::SetPitch { deck, percent } => {
            check("pitch in percent", percent, -50.0, 100.0)?;
            let control = session
                .decks
                .get_mut(deck)
                .ok_or(DjError::NoSuchDeck(deck))?;
            control
                .handle
                .send(DeckCommand::SetPitch(1.0 + percent / 100.0))?;
            control.pitch_percent = percent;
            Ok(())
        }
        &DjCommand::SetKeyLock { deck, on } => {
            let control = session
                .decks
                .get_mut(deck)
                .ok_or(DjError::NoSuchDeck(deck))?;
            control.handle.send(DeckCommand::SetKeyLock(on))?;
            control.key_lock = on;
            Ok(())
        }
        &DjCommand::SetSync { deck, mode } => {
            if deck >= session.decks.len() {
                return Err(DjError::NoSuchDeck(deck));
            }
            if mode != SyncMode::Off && session.master.is_none() {
                if let Some(master) = choose_master(session, deck) {
                    session.master = Some(master);
                    session.bus.set_master(Some(master));
                }
            }
            let control = &mut session.decks[deck];
            control.handle.send(DeckCommand::SetSync(mode))?;
            control.sync = mode;
            Ok(())
        }
        &DjCommand::SetMaster { deck } => {
            if let Some(deck) = deck {
                if deck >= session.decks.len() {
                    return Err(DjError::NoSuchDeck(deck));
                }
            }
            session.master = deck;
            session.bus.set_master(deck);
            Ok(())
        }
        &DjCommand::Bend { deck, percent } => {
            check("bend in percent", percent, -50.0, 50.0)?;
            deck_command(session, deck, DeckCommand::Bend(percent / 100.0))
        }
        &DjCommand::Nudge { deck, milliseconds } => {
            check("nudge in milliseconds", milliseconds, -10_000.0, 10_000.0)?;
            let rate = session
                .decks
                .get(deck)
                .ok_or(DjError::NoSuchDeck(deck))?
                .track_rate(deck)?;
            deck_command(
                session,
                deck,
                DeckCommand::Nudge(milliseconds / 1000.0 * rate),
            )
        }
        &DjCommand::SetQuantize { deck, on } => {
            let control = session
                .decks
                .get_mut(deck)
                .ok_or(DjError::NoSuchDeck(deck))?;
            control.handle.send(DeckCommand::SetQuantize(on))?;
            control.quantize = on;
            Ok(())
        }
        &DjCommand::BeatLoop { deck, beats } => {
            loop_beats(beats)?;
            with_grid(session, deck, DeckCommand::BeatLoop(beats))
        }
        &DjCommand::LoopRoll { deck, beats } => {
            loop_beats(beats)?;
            with_grid(session, deck, DeckCommand::LoopRoll(beats))
        }
        &DjCommand::LoopExit { deck } => with_track(session, deck, DeckCommand::LoopExit),
        &DjCommand::Reloop { deck } => with_track(session, deck, DeckCommand::Reloop),
        &DjCommand::HotCue { deck, slot } => {
            hot_cue_slot(slot)?;
            with_track(session, deck, DeckCommand::HotCue(slot))
        }
        &DjCommand::ClearHotCue { deck, slot } => {
            hot_cue_slot(slot)?;
            with_track(session, deck, DeckCommand::ClearHotCue(slot))
        }
        &DjCommand::BeatJump { deck, beats } => {
            check("beat jump in beats", beats, -1024.0, 1024.0)?;
            with_grid(session, deck, DeckCommand::BeatJump(beats))
        }
        _ => apply_mixer(session, command, sink),
    }
}

fn apply_mixer(
    session: &mut DjSession,
    command: &DjCommand,
    sink: &mut dyn ParamSink,
) -> Result<(), DjError> {
    let deck_of = |session: &mut DjSession, deck: usize| {
        if deck < session.decks.len() {
            Ok(())
        } else {
            Err(DjError::NoSuchDeck(deck))
        }
    };
    match command {
        &DjCommand::SetEq { deck, band, db } => {
            check32("EQ gain in dB", db, Isolator::KILL_DB, 6.0)?;
            deck_of(session, deck)?;
            let control = &mut session.decks[deck];
            control.eq_db[band.index()] = db;
            control.send_eq(band, sink)
        }
        &DjCommand::SetEqKill { deck, band, on } => {
            deck_of(session, deck)?;
            let control = &mut session.decks[deck];
            control.eq_kill[band.index()] = on;
            control.send_eq(band, sink)
        }
        &DjCommand::SetFilter { deck, amount } => {
            check32("filter amount", amount, -1.0, 1.0)?;
            deck_of(session, deck)?;
            let control = &mut session.decks[deck];
            sink.set_param(control.ids.filter, DjFilter::AMOUNT, amount)?;
            control.filter = amount;
            Ok(())
        }
        &DjCommand::SetFader { deck, value } => {
            check32("fader", value, 0.0, 1.0)?;
            deck_of(session, deck)?;
            sink.set_param(MIXER_ID, DjMixer::channel(deck).fader, value)?;
            session.decks[deck].fader = value;
            Ok(())
        }
        &DjCommand::SetTrim { deck, db } => {
            check32("trim in dB", db, -24.0, 12.0)?;
            deck_of(session, deck)?;
            sink.set_param(MIXER_ID, DjMixer::channel(deck).trim, db)?;
            session.decks[deck].trim_db = db;
            Ok(())
        }
        &DjCommand::SetCueMonitor { deck, on: value } => {
            deck_of(session, deck)?;
            sink.set_param(MIXER_ID, DjMixer::channel(deck).cue, on(value))?;
            session.decks[deck].cue_monitor = value;
            Ok(())
        }
        &DjCommand::SetCrossfaderSide { deck, side } => {
            deck_of(session, deck)?;
            sink.set_param(MIXER_ID, DjMixer::channel(deck).side, side.param_value())?;
            session.decks[deck].side = side;
            Ok(())
        }
        &DjCommand::SetCrossfader { position } => {
            check32("crossfader position", position, -1.0, 1.0)?;
            sink.set_param(MIXER_ID, DjMixer::CROSSFADER, position)?;
            session.mixer.crossfader = position;
            Ok(())
        }
        &DjCommand::SetCrossfaderCurve { curve } => {
            sink.set_param(MIXER_ID, DjMixer::CURVE, curve.param_value())?;
            session.mixer.curve = curve;
            Ok(())
        }
        &DjCommand::SetMasterGain { db } => {
            check32("master gain in dB", db, -60.0, 6.0)?;
            sink.set_param(MIXER_ID, DjMixer::MASTER, db)?;
            session.mixer.master_db = db;
            Ok(())
        }
        &DjCommand::SetCueMix { value } => {
            check32("cue mix", value, 0.0, 1.0)?;
            sink.set_param(MIXER_ID, DjMixer::CUE_MIX, value)?;
            session.mixer.cue_mix = value;
            Ok(())
        }
        &DjCommand::SetHeadphoneGain { db } => {
            check32("headphone gain in dB", db, -60.0, 6.0)?;
            sink.set_param(MIXER_ID, DjMixer::HEADPHONE, db)?;
            session.mixer.headphone_db = db;
            Ok(())
        }
        &DjCommand::SetDeckEffect {
            deck,
            slot,
            on: value,
        } => {
            deck_of(session, deck)?;
            let control = &mut session.decks[deck];
            let limit = control.effects.len();
            let effect = control.effects.get_mut(slot).ok_or(DjError::OutOfRange {
                what: "effect slot",
                index: slot,
                limit,
            })?;
            sink.set_param(control.ids.effects[slot], FxSlot::ENABLED, on(value))?;
            effect.enabled = value;
            Ok(())
        }
        DjCommand::SetDeckEffectParam {
            deck,
            slot,
            param,
            value,
        } => {
            let (deck, slot) = (*deck, *slot);
            deck_of(session, deck)?;
            let control = &mut session.decks[deck];
            let limit = control.effects.len();
            let effect = control.effects.get_mut(slot).ok_or(DjError::OutOfRange {
                what: "effect slot",
                index: slot,
                limit,
            })?;
            let index = effect
                .params
                .iter()
                .position(|info| info.name == param.as_str())
                .ok_or_else(|| DjError::NoSuchParam {
                    deck,
                    param: param.clone(),
                })?;
            let info = &effect.params[index];
            if !value.is_finite() {
                return Err(DjError::InvalidValue {
                    what: "effect parameter",
                    value: f64::from(*value),
                });
            }
            let value = info.clamp(*value);
            sink.set_param(control.ids.effects[slot], info.id, value)?;
            effect.values[index] = value;
            Ok(())
        }
        DjCommand::SetStemGain { deck, stem, gain } => {
            let deck = *deck;
            check32("stem gain", *gain, 0.0, 4.0)?;
            deck_of(session, deck)?;
            let control = &mut session.decks[deck];
            let index = control.stem_index(deck, stem)?;
            let muted = control.stems[index].muted;
            let sounding = if muted { 0.0 } else { *gain };
            control
                .handle
                .send(DeckCommand::SetStemGain(index, sounding))?;
            control.stems[index].gain = *gain;
            Ok(())
        }
        DjCommand::SetStemMute { deck, stem, muted } => {
            let deck = *deck;
            deck_of(session, deck)?;
            let control = &mut session.decks[deck];
            let index = control.stem_index(deck, stem)?;
            let gain = control.stems[index].gain;
            let sounding = if *muted { 0.0 } else { gain };
            control
                .handle
                .send(DeckCommand::SetStemGain(index, sounding))?;
            control.stems[index].muted = *muted;
            Ok(())
        }
        DjCommand::StartRecording { path } => session.recorder.start(path),
        DjCommand::StopRecording => session.recorder.stop(),
        // Deck commands are handled by `apply`.
        _ => Ok(()),
    }
}
