//! The compiler itself: project in, graph out.

use std::collections::HashMap;

use fourfloor_core::{DeviceId, DeviceRegistry, Ticks, TrackId};
use fourfloor_engine::devices::{
    AudioClipPlayer, ClipLauncher, Fader, Gain, LauncherClip, NotePlayer, PlayerNote,
};
use fourfloor_engine::GraphBuilder;
use fourfloor_model::{Project, Track, TrackKind};

use crate::audio::AudioLoader;
use crate::build::{Built, Chain, Compiler};
use crate::control::control_plan;
use crate::options::{CompileError, CompileOptions, CompiledProject, CompiledTrack, CHANNELS};
use crate::routing::{
    audible, db_to_linear, fader_closed, output_bus, project_length, send_bus, track_notes,
};

/// What one track contributes to the graph before it is wired.
struct Strip<'a> {
    track: &'a Track,
    /// Note player (MIDI track) or audio clip player (audio track).
    player: Option<Built>,
    launcher: Option<Built>,
    chain: Chain,
    /// Bus summing node.
    input: Option<Built>,
    fader: DeviceId,
    note_count: usize,
    audio_clips: usize,
}

impl Strip<'_> {
    /// The node whose audio starts the strip's effect chain: the
    /// instrument, the bus input or the audio clip player.
    fn head(&self) -> Option<DeviceId> {
        self.chain
            .instrument
            .as_ref()
            .or(self.input.as_ref())
            .or(match self.track.kind {
                TrackKind::Audio => self.player.as_ref(),
                _ => None,
            })
            .map(|built| built.id)
    }
}

/// The launcher clips of a track, one per project scene, top to bottom. A
/// scene the track has no MIDI clip in gets an empty clip, so launching
/// that scene silences the track. `None` when the track has no MIDI slot.
fn launcher_clips(compiler: &mut Compiler<'_>, track: &Track) -> Option<Vec<LauncherClip>> {
    let audio_slots = track
        .slots
        .iter()
        .filter(|slot| !slot.clip.is_midi())
        .count();
    if audio_slots > 0 {
        compiler.warn(
            track,
            format_args!("{audio_slots} audio clip(s) in launcher slots are not played yet"),
        );
    }
    if !track.slots.iter().any(|slot| slot.clip.is_midi()) {
        return None;
    }
    let clips = compiler
        .project
        .scenes
        .iter()
        .map(|scene| match track.slot(scene.id) {
            Some(slot) if slot.clip.is_midi() => LauncherClip {
                length: slot.clip.length,
                notes: slot
                    .clip
                    .sounding_notes()
                    .iter()
                    .map(|note| PlayerNote::new(note.start, note.length, note.pitch, note.velocity))
                    .collect(),
            },
            _ => LauncherClip {
                length: Ticks::BEAT,
                notes: Vec::new(),
            },
        })
        .collect();
    Some(clips)
}

fn midi_strip<'a>(
    compiler: &mut Compiler<'_>,
    track: &'a Track,
) -> Result<Option<Strip<'a>>, CompileError> {
    let notes = track_notes(track);
    if notes.is_empty() && track.devices.is_empty() && track.slots.is_empty() {
        return Ok(None);
    }
    let mut chain = compiler.chain(track);
    if chain.instrument.is_none() {
        chain.instrument = compiler.default_instrument(track)?;
    }
    if chain.instrument.is_none() {
        return Ok(None);
    }
    let player = if notes.is_empty() {
        None
    } else {
        Some(Built {
            id: compiler.fresh_id()?,
            device: Box::new(NotePlayer::new(&notes)),
        })
    };
    let quantize = compiler.project.launch_quantize;
    let launcher = match launcher_clips(compiler, track) {
        Some(clips) => Some(Built {
            id: compiler.fresh_id()?,
            device: Box::new(ClipLauncher::new(&clips, quantize)),
        }),
        None => None,
    };
    Ok(Some(Strip {
        track,
        player,
        launcher,
        chain,
        input: None,
        fader: compiler.fresh_id()?,
        note_count: notes.len(),
        audio_clips: 0,
    }))
}

fn audio_strip<'a>(
    compiler: &mut Compiler<'_>,
    loader: &mut AudioLoader,
    track: &'a Track,
) -> Result<Option<Strip<'a>>, CompileError> {
    let mut regions = Vec::new();
    for clip in &track.clips {
        match loader.region(clip) {
            Some(Ok(region)) => regions.push(region),
            Some(Err(reason)) => compiler.warn(
                track,
                format_args!("clip \"{}\" is left out: {reason}", clip.name),
            ),
            None => compiler.warn(
                track,
                format_args!(
                    "clip \"{}\" holds notes and cannot play on an audio track",
                    clip.name
                ),
            ),
        }
    }
    if regions.is_empty() {
        return Ok(None);
    }
    let audio_clips = regions.len();
    let chain = compiler.chain(track);
    Ok(Some(Strip {
        track,
        player: Some(Built {
            id: compiler.fresh_id()?,
            device: Box::new(AudioClipPlayer::new(regions)),
        }),
        launcher: None,
        chain,
        input: None,
        fader: compiler.fresh_id()?,
        note_count: 0,
        audio_clips,
    }))
}

/// Adds every bus a strip feeds, by its output or a send, until no more
/// are found, so a bus feeding a bus is picked up too.
fn add_buses<'a>(
    compiler: &mut Compiler<'_>,
    project: &'a Project,
    strips: &mut Vec<Strip<'a>>,
) -> Result<(), CompileError> {
    loop {
        let wanted = strips
            .iter()
            .flat_map(|strip| {
                output_bus(project, strip.track).into_iter().chain(
                    strip
                        .track
                        .sends
                        .iter()
                        .filter_map(|send| send_bus(project, strip.track, send)),
                )
            })
            .find(|bus| !strips.iter().any(|strip| strip.track.id == bus.id));
        let Some(bus) = wanted else {
            return Ok(());
        };
        let chain = compiler.chain(bus);
        let input = Built {
            id: compiler.fresh_id()?,
            device: Box::new(Gain::new(1.0)),
        };
        strips.push(Strip {
            track: bus,
            player: None,
            launcher: None,
            chain,
            input: Some(input),
            fader: compiler.fresh_id()?,
            note_count: 0,
            audio_clips: 0,
        });
    }
}

/// Edges collected while nodes are added; a node may feed one that comes
/// later in display order, so edges are connected last.
#[derive(Default)]
struct Edges {
    events: Vec<(DeviceId, DeviceId)>,
    audio: Vec<(DeviceId, DeviceId)>,
    sidechain: Vec<(DeviceId, DeviceId)>,
}

/// Compiles `project` into a stereo graph, building devices through
/// `registry`. Allocates and reads audio files; call it off the audio
/// thread.
///
/// See the [crate docs](crate) for how tracks and device chains map to
/// graph nodes.
pub fn compile_project(
    project: &Project,
    registry: &DeviceRegistry,
    options: &CompileOptions,
) -> Result<CompiledProject, CompileError> {
    let mut compiler = Compiler::new(project, registry, options);
    let mut loader = AudioLoader::new(
        options.project_dir.clone(),
        options.sample_rate,
        project.tempo,
    );
    let master = compiler.fresh_id()?;

    // Pass 1: tracks that have something to play or to play through.
    let mut strips: Vec<Strip<'_>> = Vec::new();
    for track in project.tracks.iter().map(|track| track.as_ref()) {
        let strip = match track.kind {
            TrackKind::Midi => midi_strip(&mut compiler, track)?,
            TrackKind::Audio => audio_strip(&mut compiler, &mut loader, track)?,
            TrackKind::Bus => None,
        };
        strips.extend(strip);
    }
    // Pass 2: the buses they reach. Then back to display order.
    add_buses(&mut compiler, project, &mut strips)?;
    strips.sort_by_key(|strip| project.track_index(strip.track.id));
    compiler.warnings.append(&mut loader.notes);

    let sounding: Vec<&Strip<'_>> = strips
        .iter()
        .filter(|strip| strip.note_count + strip.audio_clips > 0 && audible(project, strip.track))
        .collect();
    let sounding_tracks = sounding.len();
    let notes = sounding.iter().map(|strip| strip.note_count).sum();

    // Where each strip's fader goes: its bus's summing node, or the master.
    let inputs: HashMap<TrackId, DeviceId> = strips
        .iter()
        .filter_map(|strip| Some((strip.track.id, strip.input.as_ref()?.id)))
        .collect();
    let destinations: Vec<DeviceId> = strips
        .iter()
        .map(|strip| {
            output_bus(project, strip.track)
                .and_then(|bus| inputs.get(&bus.id).copied())
                .unwrap_or(master)
        })
        .collect();

    let mut builder = GraphBuilder::new(CHANNELS);
    builder.add_node(master, Box::new(Gain::new(1.0)), 1)?;
    builder.set_output(master)?;

    let mut edges = Edges::default();
    let mut tracks = Vec::with_capacity(strips.len());
    let mut sidechains = Vec::new();
    for (strip, destination) in strips.into_iter().zip(destinations) {
        let Some(head) = strip.head() else {
            continue; // Every strip has an instrument, a player or an input.
        };
        let track = strip.track;
        let fader = Fader::new(track.gain_db, track.pan).with_mute(fader_closed(project, track));
        let is_audio = track.kind == TrackKind::Audio;
        let mut compiled = CompiledTrack {
            track: track.id,
            player: strip.player.as_ref().map(|built| built.id),
            launcher: strip.launcher.as_ref().map(|built| built.id),
            instrument: strip.chain.instrument.as_ref().map(|built| built.id),
            input: strip.input.as_ref().map(|built| built.id),
            devices: Vec::new(),
            pre_fader: head,
            fader: strip.fader,
            sends: Vec::new(),
            to_master: destination == master,
        };

        // Events: player -> launcher -> note effects -> instrument.
        let mut event_source = None;
        if let Some(player) = strip.player {
            builder.add_node(player.id, player.device, 0)?;
            if !is_audio {
                event_source = Some(player.id);
            }
        }
        if let Some(launcher) = strip.launcher {
            builder.add_node(launcher.id, launcher.device, 0)?;
            edges
                .events
                .extend(event_source.map(|source| (source, launcher.id)));
            event_source = Some(launcher.id);
        }
        for effect in strip.chain.note_effects {
            builder.add_node(effect.id, effect.device, 0)?;
            edges
                .events
                .extend(event_source.map(|source| (source, effect.id)));
            event_source = Some(effect.id);
            compiled.devices.push(effect.id);
        }
        if let Some(instrument) = strip.chain.instrument {
            builder.add_node(instrument.id, instrument.device, 0)?;
            edges
                .events
                .extend(event_source.map(|source| (source, instrument.id)));
            if track.device(instrument.id).is_some() {
                compiled.devices.push(instrument.id);
            }
        }
        if let Some(input) = strip.input {
            builder.add_node(input.id, input.device, 1)?;
        }

        // Audio: head -> effects -> fader -> destination.
        let mut audio_source = head;
        for effect in strip.chain.effects {
            builder.add_node(effect.id, effect.device, 1)?;
            edges.audio.push((audio_source, effect.id));
            audio_source = effect.id;
            compiled.devices.push(effect.id);
        }
        compiled.pre_fader = audio_source;
        builder.add_node(strip.fader, Box::new(fader), 1)?;
        edges.audio.push((audio_source, strip.fader));
        edges.audio.push((strip.fader, destination));

        // Sends: a gain node per send, from before or after the fader.
        for send in &track.sends {
            let target = send_bus(project, track, send).and_then(|bus| inputs.get(&bus.id));
            let Some(&target) = target else {
                compiler.warn(
                    track,
                    format_args!(
                        "a send goes to track {}, which is not a bus; it is left out",
                        send.target
                    ),
                );
                continue;
            };
            let level = if send.gain_db <= Fader::MIN_DB {
                0.0
            } else {
                db_to_linear(send.gain_db)
            };
            let node = compiler.fresh_id()?;
            builder.add_node(node, Box::new(Gain::new(level)), 1)?;
            let tap = if send.pre_fader {
                audio_source
            } else {
                strip.fader
            };
            edges.audio.push((tap, node));
            edges.audio.push((node, target));
            compiled.sends.push((send.target, node));
        }
        sidechains.extend(
            strip
                .chain
                .sidechains
                .into_iter()
                .map(|(device, source)| (track, device, source)),
        );
        tracks.push(compiled);
    }

    // Sidechains: from the keying track's pre- or post-fader signal.
    for (track, device, source) in sidechains {
        let key = tracks.iter().find(|other| other.track == source.track);
        match key {
            Some(key) if key.track != track.id => {
                let tap = if source.pre_fader {
                    key.pre_fader
                } else {
                    key.fader
                };
                edges.sidechain.push((tap, device));
            }
            Some(_) => compiler.warn(track, "a device is keyed by its own track; that is ignored"),
            None => compiler.warn(
                track,
                format_args!(
                    "a device is keyed by track {}, which plays nothing; that is ignored",
                    source.track
                ),
            ),
        }
    }

    for (from, to) in edges.events {
        builder.connect_events(from, to)?;
    }
    for (from, to) in edges.audio {
        builder.connect(from, to, 0)?;
    }
    for (from, to) in edges.sidechain {
        builder.connect_sidechain(from, to)?;
    }

    let faders: HashMap<TrackId, DeviceId> = tracks
        .iter()
        .map(|compiled| (compiled.track, compiled.fader))
        .collect();
    let plan = control_plan(project, &compiler.ranges, &faders, &mut compiler.warnings);
    if !plan.is_empty() {
        builder.set_control(plan);
    }

    Ok(CompiledProject {
        graph: builder.compile(options.sample_rate, options.max_block)?,
        master,
        length: project_length(project),
        sounding_tracks,
        notes,
        tracks,
        scenes: project.scenes.iter().map(|scene| scene.id).collect(),
        warnings: compiler.warnings,
    })
}
