//! Tools that write generated parts: drums, basslines and chords.

use fourfloor_core::Ticks;
use fourfloor_model::command::{WriteMidiClip, WriteStepPattern};
use fourfloor_model::sequencer::BAR_TICKS;
use serde_json::json;

use super::args::{AddBasslineArgs, AddChordsArgs, AddHouseDrumPatternArgs};
use super::edit::{bar_tick, midi_track, swing_notes};
use super::ToolOutput;
use crate::agent::Agent;
use crate::error::AgentError;
use crate::generate::{
    bassline, chord_part, house_drums, BassOptions, ChordOptions, ChordStyle, DrumOptions,
    DrumVoice, Key, MAX_BARS,
};
use crate::rng::SeededRng;

/// Instrument of a new kick track.
pub const KICK_INSTRUMENT: &str = "drum.kick909";
/// Instrument of a new drum track.
pub const KIT_INSTRUMENT: &str = "drum.kit";
/// Instrument of a new bass track.
pub const BASS_INSTRUMENT: &str = "synth.acid303";
/// Instrument of a new organ or stab chord track.
pub const ORGAN_INSTRUMENT: &str = "fm.organ";
/// Instrument of a new pad or pulse chord track.
pub const PAD_INSTRUMENT: &str = "poly.pad";

fn check_bars(tool: &str, bars: u32) -> Result<(), AgentError> {
    if (1..=MAX_BARS).contains(&bars) {
        Ok(())
    } else {
        Err(AgentError::invalid(
            tool,
            format!("{bars} bars is outside 1 to {MAX_BARS}"),
        ))
    }
}

/// `add_house_drum_pattern`.
pub(crate) fn add_house_drum_pattern(
    agent: &mut Agent,
    args: AddHouseDrumPatternArgs,
) -> Result<ToolOutput, AgentError> {
    check_bars("add_house_drum_pattern", args.bars)?;
    let options = DrumOptions {
        bars: args.bars,
        style: args.style,
        density: args.density,
        fills: args.fills,
    };
    let pattern = house_drums(&options, &mut SeededRng::new(args.seed))?;
    let registry = agent.registry_arc();
    let start = bar_tick(args.start_bar);
    let written = agent.transaction("add_house_drum_pattern", |tx| {
        let (kick_track, kick_created) = midi_track(
            tx,
            &registry,
            args.kick_track.as_ref(),
            "Kick",
            KICK_INSTRUMENT,
        )?;
        let (drum_track, drum_created) = midi_track(
            tx,
            &registry,
            args.drum_track.as_ref(),
            "Drums",
            KIT_INSTRUMENT,
        )?;
        let lanes = |kick: bool| -> Vec<_> {
            pattern
                .lanes
                .iter()
                .filter(|lane| (lane.voice == DrumVoice::Kick) == kick)
                .map(|lane| lane.to_step_lane(None))
                .collect()
        };
        let mut write = |track, name: &str, lanes| {
            tx.execute(WriteStepPattern {
                track,
                clip: None,
                name: name.to_owned(),
                start,
                bars: args.bars,
                lanes,
                swing: None,
                groove: None,
                groove_amount: 1.0,
            })
            .map(|outcome| outcome.clip().map(|clip| clip.get()))
        };
        let kick_clip = write(kick_track, "Kick", lanes(true))?;
        let drum_clip = write(drum_track, "Drums", lanes(false))?;
        Ok(json!({
            "kick_track": kick_track.get(),
            "kick_track_created": kick_created,
            "kick_clip": kick_clip,
            "drum_track": drum_track.get(),
            "drum_track_created": drum_created,
            "drum_clip": drum_clip,
        }))
    })?;
    let hits: serde_json::Map<String, serde_json::Value> = pattern
        .lanes
        .iter()
        .map(|lane| (lane.voice.name().to_owned(), json!(lane.hits().len())))
        .collect();
    let mut data = written;
    data["bars"] = json!(args.bars);
    data["style"] = json!(args.style);
    data["seed"] = json!(args.seed);
    data["hits"] = serde_json::Value::Object(hits);
    Ok(ToolOutput::changed(
        format!(
            "wrote a {}-bar {:?} house drum pattern (seed {})",
            args.bars, args.style, args.seed
        ),
        data,
    ))
}

/// `add_bassline`.
pub(crate) fn add_bassline(
    agent: &mut Agent,
    args: AddBasslineArgs,
) -> Result<ToolOutput, AgentError> {
    check_bars("add_bassline", args.bars)?;
    let key: Key = args.key.parse()?;
    let options = BassOptions {
        bars: args.bars,
        key,
        style: args.style,
        octave: args.octave,
        progression: args.progression.clone(),
        density: args.density,
    };
    let notes = bassline(&options, &mut SeededRng::new(args.seed))?;
    let notes = swing_notes(&notes, &agent.project().groove);
    let registry = agent.registry_arc();
    let instrument = args.instrument.as_deref().unwrap_or(BASS_INSTRUMENT);
    let count = notes.len();
    let data = agent.transaction("add_bassline", |tx| {
        let (track, created) = midi_track(tx, &registry, args.track.as_ref(), "Bass", instrument)?;
        let clip = tx
            .execute(WriteMidiClip {
                track,
                clip: None,
                name: "Bassline".to_owned(),
                start: bar_tick(args.start_bar),
                length: Ticks::new(i64::from(args.bars) * BAR_TICKS),
                notes,
            })?
            .clip()
            .map(|clip| clip.get());
        Ok(json!({ "track": track.get(), "track_created": created, "clip": clip }))
    })?;
    let mut data = data;
    data["key"] = json!(key.name());
    data["notes"] = json!(count);
    data["style"] = json!(args.style);
    data["seed"] = json!(args.seed);
    Ok(ToolOutput::changed(
        format!(
            "wrote a {}-bar {:?} bassline in {key} ({count} notes)",
            args.bars, args.style
        ),
        data,
    ))
}

/// `add_chords`.
pub(crate) fn add_chords(agent: &mut Agent, args: AddChordsArgs) -> Result<ToolOutput, AgentError> {
    check_bars("add_chords", args.bars)?;
    let key: Key = args.key.parse()?;
    let defaults = ChordOptions::default();
    let options = ChordOptions {
        bars: args.bars,
        key,
        style: args.style,
        octave: args.octave.unwrap_or(defaults.octave),
        voices: args.voices.unwrap_or(defaults.voices),
        progression: args.progression.clone(),
        density: args.density,
    };
    let part = chord_part(&options, &mut SeededRng::new(args.seed))?;
    let notes = swing_notes(&part.notes, &agent.project().groove);
    let registry = agent.registry_arc();
    let instrument = args.instrument.as_deref().unwrap_or(match args.style {
        ChordStyle::Organ | ChordStyle::Stab => ORGAN_INSTRUMENT,
        ChordStyle::Sustained | ChordStyle::Pulse => PAD_INSTRUMENT,
    });
    let count = notes.len();
    let data = agent.transaction("add_chords", |tx| {
        let (track, created) =
            midi_track(tx, &registry, args.track.as_ref(), "Chords", instrument)?;
        let clip = tx
            .execute(WriteMidiClip {
                track,
                clip: None,
                name: "Chords".to_owned(),
                start: bar_tick(args.start_bar),
                length: Ticks::new(i64::from(args.bars) * BAR_TICKS),
                notes,
            })?
            .clip()
            .map(|clip| clip.get());
        Ok(json!({ "track": track.get(), "track_created": created, "clip": clip }))
    })?;
    let mut data = data;
    data["key"] = json!(key.name());
    data["progression"] = json!(part.progression);
    data["notes"] = json!(count);
    data["style"] = json!(args.style);
    Ok(ToolOutput::changed(
        format!(
            "wrote a {}-bar {:?} chord part in {key}, progression {:?}",
            args.bars, args.style, part.progression
        ),
        data,
    ))
}
