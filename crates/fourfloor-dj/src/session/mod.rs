//! A DJ session: the decks, their channel strips and the mixer as one
//! engine graph, and the control side that carries out [`DjCommand`]s.
//!
//! The graph, per deck: [`Deck`] → `fx.isolator` ([`Isolator`]) →
//! [`DjFilter`] → one [`FxSlot`] per configured effect kind → a port of
//! the [`DjMixer`]. The mixer feeds the [`Recorder`], which is the graph
//! output. Every bus has [`DJ_CHANNELS`] channels: the master on 0 and 1,
//! the headphone cue mix on 2 and 3 (decks and effects use 0 and 1).
//!
//! [`DjSession::new`] returns the session and the compiled graph. Hand the
//! graph to an engine (or to a [`DjRenderer`](crate::DjRenderer) offline)
//! and pass the matching [`ParamSink`] to [`DjSession::apply`]: deck
//! commands travel through each deck's own queue, mixer and effect
//! settings as parameter changes through the sink.

mod apply;
mod load;

use std::path::PathBuf;
use std::sync::Arc;

use fourfloor_analysis::Key;
use fourfloor_core::{
    DeviceCategory, DeviceId, DeviceRegistry, ParamId, ParamInfo, RegistryError, SampleRate, Tempo,
};
use fourfloor_effects::Isolator;
use fourfloor_engine::{CompiledGraph, EngineController, GraphBuilder};

use crate::command::DjCommand;
use crate::deck::{Deck, DeckHandle, SyncMode, HOT_CUES};
use crate::error::DjError;
use crate::filter::DjFilter;
use crate::fxslot::FxSlot;
use crate::harmonic::{rank_compatible, LibraryTrack, MatchOptions, Suggestion};
use crate::info::AnalysisCache;
use crate::mixer::{CrossfaderSide, DjMixer};
use crate::recorder::{Recorder, RecorderHandle, RecordingSummary};
use crate::state::{
    DeckState, EffectState, LoopState, MixerState, SessionState, StemState, TrackSummary,
};
use crate::sync::SyncBus;
use crate::track::DeckGrid;

/// Channels of every bus in a DJ graph: master stereo plus cue stereo.
pub const DJ_CHANNELS: usize = 4;
/// Node id of the mixer.
pub const MIXER_ID: DeviceId = DeviceId(10);
/// Node id of the recorder (the graph output).
pub const RECORDER_ID: DeviceId = DeviceId(11);

/// How to build a session.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionConfig {
    /// Number of decks, 2 to 4.
    pub decks: usize,
    /// Engine sample rate.
    pub sample_rate: SampleRate,
    /// Largest block the engine processes.
    pub max_block: usize,
    /// Effect kinds every deck gets a slot for, in chain order (e.g.
    /// `["fx.delay", "fx.reverb"]`).
    pub deck_effects: Vec<String>,
    /// Seconds of audio the recording queue holds between pumps.
    pub record_buffer_seconds: f64,
    /// Where analysis results are cached; `None` analyses every load.
    pub cache_dir: Option<PathBuf>,
}

impl SessionConfig {
    /// `decks` decks at `sample_rate` in blocks of up to `max_block`
    /// frames, no deck effects, a 10 s recording queue, no cache.
    pub fn new(decks: usize, sample_rate: SampleRate, max_block: usize) -> Self {
        Self {
            decks,
            sample_rate,
            max_block,
            deck_effects: Vec::new(),
            record_buffer_seconds: 10.0,
            cache_dir: None,
        }
    }

    /// The same config with these deck effect kinds.
    pub fn with_deck_effects(mut self, kinds: &[&str]) -> Self {
        self.deck_effects = kinds.iter().map(|kind| (*kind).to_owned()).collect();
        self
    }

    /// The same config with an analysis cache directory.
    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    /// The same config with a recording queue of `seconds`.
    pub fn with_record_buffer(mut self, seconds: f64) -> Self {
        self.record_buffer_seconds = seconds;
        self
    }
}

/// Graph node ids of one deck's strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeckIds {
    /// The deck itself.
    pub deck: DeviceId,
    /// Its isolator.
    pub isolator: DeviceId,
    /// Its DJ filter.
    pub filter: DeviceId,
    /// Its effect slots, in chain order.
    pub effects: Vec<DeviceId>,
}

impl DeckIds {
    fn new(deck: usize, effects: usize) -> Self {
        let base = 1_000 + 100 * deck as u64;
        Self {
            deck: DeviceId(base),
            isolator: DeviceId(base + 1),
            filter: DeviceId(base + 2),
            effects: (0..effects as u64)
                .map(|i| DeviceId(base + 10 + i))
                .collect(),
        }
    }

    /// The last node of the strip (what feeds the mixer).
    pub fn strip_output(&self) -> DeviceId {
        self.effects.last().copied().unwrap_or(self.filter)
    }
}

/// Where parameter changes go: a compiled graph driven offline, or a
/// running engine.
pub trait ParamSink {
    /// Sets a parameter of a graph node.
    fn set_param(&mut self, device: DeviceId, param: ParamId, value: f32) -> Result<(), DjError>;
}

impl ParamSink for CompiledGraph {
    fn set_param(&mut self, device: DeviceId, param: ParamId, value: f32) -> Result<(), DjError> {
        if CompiledGraph::set_param(self, device, param, value) {
            Ok(())
        } else {
            Err(DjError::NoSuchNode(device.get()))
        }
    }
}

impl ParamSink for EngineController {
    fn set_param(&mut self, device: DeviceId, param: ParamId, value: f32) -> Result<(), DjError> {
        EngineController::set_param(self, device, param, value)
            .map_err(|_| DjError::QueueFull("the engine"))
    }
}

/// A track as the control side remembers it.
#[derive(Clone, Debug)]
pub(crate) struct LoadedTrack {
    summary: TrackSummary,
    sample_rate: SampleRate,
    grid: Option<DeckGrid>,
    key: Option<Key>,
}

/// One effect slot as the control side remembers it.
#[derive(Clone, Debug)]
struct EffectControl {
    kind: String,
    enabled: bool,
    params: Vec<ParamInfo>,
    values: Vec<f32>,
}

/// One deck's settings on the control side.
struct DeckControl {
    handle: DeckHandle,
    ids: DeckIds,
    track: Option<LoadedTrack>,
    pitch_percent: f64,
    key_lock: bool,
    sync: SyncMode,
    quantize: bool,
    eq_db: [f32; 3],
    eq_kill: [bool; 3],
    filter: f32,
    fader: f32,
    trim_db: f32,
    cue_monitor: bool,
    side: CrossfaderSide,
    effects: Vec<EffectControl>,
    stems: Vec<StemState>,
}

/// A DJ session (see the [module docs](self)).
pub struct DjSession {
    sample_rate: SampleRate,
    decks: Vec<DeckControl>,
    mixer: MixerState,
    master: Option<usize>,
    bus: Arc<SyncBus>,
    recorder: RecorderHandle,
    cache: Option<AnalysisCache>,
}

/// A registry with everything a DJ session can use: the engine built-ins
/// and the `fourfloor-effects` devices.
pub fn default_registry() -> Result<DeviceRegistry, RegistryError> {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry)?;
    fourfloor_effects::register(&mut registry)?;
    Ok(registry)
}

fn effect_slot(registry: &DeviceRegistry, kind: &str) -> Result<(FxSlot, EffectControl), DjError> {
    let descriptor = registry
        .descriptor(kind)
        .ok_or_else(|| RegistryError::UnknownKind(kind.to_owned()))?;
    if descriptor.category != DeviceCategory::Effect {
        return Err(DjError::NotAnEffect(kind.to_owned()));
    }
    let params = descriptor.params.clone();
    let device = registry.create_default(kind)?;
    let control = EffectControl {
        kind: kind.to_owned(),
        enabled: false,
        values: params.iter().map(|info| info.default).collect(),
        params,
    };
    Ok((FxSlot::new(kind, device), control))
}

impl DjSession {
    /// Builds the session and its graph. Effect kinds are looked up in
    /// `registry` (see [`default_registry`]).
    pub fn new(
        config: &SessionConfig,
        registry: &DeviceRegistry,
    ) -> Result<(Self, CompiledGraph), DjError> {
        if !(2..=crate::sync::MAX_DECKS).contains(&config.decks) {
            return Err(DjError::DeckCount(config.decks));
        }
        let bus = Arc::new(SyncBus::new());
        let mut builder = GraphBuilder::new(DJ_CHANNELS);
        builder.add_node(MIXER_ID, Box::new(DjMixer::new(config.decks)), config.decks)?;
        let (recorder, recorder_handle) =
            Recorder::new(config.sample_rate, config.record_buffer_seconds);
        builder.add_node(RECORDER_ID, Box::new(recorder), 1)?;
        builder.connect(MIXER_ID, RECORDER_ID, 0)?;
        builder.set_output(RECORDER_ID)?;

        let mut decks = Vec::with_capacity(config.decks);
        for index in 0..config.decks {
            let ids = DeckIds::new(index, config.deck_effects.len());
            let (deck, handle) = Deck::new(index, Arc::clone(&bus));
            builder.add_node(ids.deck, Box::new(deck), 0)?;
            builder.add_node(ids.isolator, Box::new(Isolator::new()), 1)?;
            builder.add_node(ids.filter, Box::new(DjFilter::new()), 1)?;
            builder.connect(ids.deck, ids.isolator, 0)?;
            builder.connect(ids.isolator, ids.filter, 0)?;
            let mut effects = Vec::with_capacity(config.deck_effects.len());
            let mut previous = ids.filter;
            for (kind, &id) in config.deck_effects.iter().zip(&ids.effects) {
                let (slot, control) = effect_slot(registry, kind)?;
                builder.add_node(id, Box::new(slot), 1)?;
                builder.connect(previous, id, 0)?;
                previous = id;
                effects.push(control);
            }
            builder.connect(previous, MIXER_ID, index)?;
            decks.push(DeckControl {
                handle,
                ids,
                track: None,
                pitch_percent: 0.0,
                key_lock: false,
                sync: SyncMode::Off,
                quantize: true,
                eq_db: [0.0; 3],
                eq_kill: [false; 3],
                filter: 0.0,
                fader: 1.0,
                trim_db: 0.0,
                cue_monitor: false,
                side: match index {
                    0 => CrossfaderSide::A,
                    1 => CrossfaderSide::B,
                    _ => CrossfaderSide::Thru,
                },
                effects,
                stems: Vec::new(),
            });
        }
        let graph = builder.compile(config.sample_rate, config.max_block)?;
        let session = Self {
            sample_rate: config.sample_rate,
            decks,
            mixer: MixerState::default(),
            master: None,
            bus,
            recorder: recorder_handle,
            cache: config.cache_dir.as_ref().map(AnalysisCache::new),
        };
        Ok((session, graph))
    }

    /// Engine sample rate.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// Number of decks.
    pub fn deck_count(&self) -> usize {
        self.decks.len()
    }

    /// Graph node ids of deck `deck`'s strip.
    pub fn deck_ids(&self, deck: usize) -> Option<&DeckIds> {
        self.decks.get(deck).map(|control| &control.ids)
    }

    /// The sync bus the decks share.
    pub fn sync_bus(&self) -> &Arc<SyncBus> {
        &self.bus
    }

    /// The tempo master.
    pub fn master(&self) -> Option<usize> {
        self.master
    }

    /// Tempo of the master as it sounds now: give it to the engine
    /// transport so tempo-synced deck effects follow the mix.
    pub fn tempo(&self) -> Option<Tempo> {
        let clock = self.bus.read(self.master?)?;
        Tempo::new(clock.bpm).ok()
    }

    /// Frees replaced tracks and writes queued recording audio. Call it
    /// regularly (every UI frame). Returns the summary when a stopped
    /// recording was closed.
    pub fn pump(&mut self) -> Result<Option<RecordingSummary>, DjError> {
        for deck in &mut self.decks {
            deck.handle.collect();
        }
        self.recorder.pump()
    }

    /// Closes a recording at once, for an engine that has stopped.
    pub fn finish_recording(&mut self) -> Result<RecordingSummary, DjError> {
        self.recorder.stop_now()
    }

    /// Library tracks ranked by how well they mix with deck `deck`'s
    /// track, at its current tempo.
    pub fn suggest(
        &self,
        deck: usize,
        library: &[LibraryTrack],
        options: &MatchOptions,
    ) -> Result<Vec<Suggestion>, DjError> {
        let control = self.decks.get(deck).ok_or(DjError::NoSuchDeck(deck))?;
        let track = control.track.as_ref().ok_or(DjError::NoTrack(deck))?;
        let bpm = control.handle.snapshot().bpm.or(track.summary.bpm);
        Ok(rank_compatible(bpm, track.key, library, options))
    }

    /// The whole session as plain data.
    pub fn state(&self) -> SessionState {
        SessionState {
            sample_rate: self.sample_rate.hz(),
            decks: self
                .decks
                .iter()
                .enumerate()
                .map(|(index, deck)| deck_state(index, deck))
                .collect(),
            mixer: self.mixer.clone(),
            master: self.master,
            master_bpm: self.tempo().map(|tempo| tempo.bpm()),
            recording: self.recorder.path().map(|path| path.to_path_buf()),
            key_lock_engine: Deck::key_lock_engine().to_owned(),
        }
    }

    /// Carries out a command (see [`DjCommand`]). Mixer and effect
    /// changes go to `sink`; deck commands to the deck's queue.
    pub fn apply(&mut self, command: &DjCommand, sink: &mut dyn ParamSink) -> Result<(), DjError> {
        apply::apply(self, command, sink)
    }
}

fn deck_state(index: usize, deck: &DeckControl) -> DeckState {
    let live = deck.handle.snapshot();
    let rate = deck
        .track
        .as_ref()
        .map_or(1.0, |track| track.sample_rate.as_f64());
    let seconds = |samples: f64| samples / rate;
    let grid = deck.track.as_ref().and_then(|track| track.grid);
    DeckState {
        index,
        track: deck.track.as_ref().map(|track| track.summary.clone()),
        playing: live.playing,
        at_end: live.at_end,
        position_seconds: seconds(live.position),
        beat: grid.map(|grid| grid.beat_at(live.position)),
        pitch_percent: deck.pitch_percent,
        rate: live.rate,
        bpm: grid.map(|grid| grid.bpm * live.rate),
        key_lock: deck.key_lock,
        sync: deck.sync,
        quantize: deck.quantize,
        cue_seconds: seconds(live.cue_point),
        hot_cues: live.hot_cues[..HOT_CUES]
            .iter()
            .map(|cue| cue.map(seconds))
            .collect(),
        loop_state: live.loop_region.map(|(start, end)| LoopState {
            start_seconds: seconds(start),
            end_seconds: seconds(end),
            roll: live.roll,
        }),
        eq_db: deck.eq_db,
        eq_kill: deck.eq_kill,
        filter: deck.filter,
        fader: deck.fader,
        trim_db: deck.trim_db,
        cue_monitor: deck.cue_monitor,
        crossfader_side: deck.side,
        effects: deck
            .effects
            .iter()
            .map(|effect| EffectState {
                kind: effect.kind.clone(),
                enabled: effect.enabled,
                params: effect
                    .params
                    .iter()
                    .zip(&effect.values)
                    .map(|(info, &value)| (info.name.to_string(), value))
                    .collect(),
            })
            .collect(),
        stems: deck.stems.clone(),
    }
}
