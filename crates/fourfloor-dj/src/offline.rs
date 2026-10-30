//! Driving a DJ session without an audio device: for tests, offline mix
//! renders and tools.

use fourfloor_core::{DeviceRegistry, Tempo, TransportContext};
use fourfloor_engine::CompiledGraph;

use crate::command::DjCommand;
use crate::error::DjError;
use crate::recorder::RecordingSummary;
use crate::session::{DjSession, SessionConfig, DJ_CHANNELS};
use crate::track::DeckTrack;

/// Stereo audio as two channels.
pub type Stereo = [Vec<f32>; 2];

/// What [`DjRenderer::render`] produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DjOutput {
    /// The master mix.
    pub master: Stereo,
    /// The headphone (cue) mix.
    pub cue: Stereo,
    /// Each deck's own output, before its channel strip.
    pub decks: Vec<Stereo>,
    /// Each deck's channel strip output (after EQ, filter and effects),
    /// before the mixer.
    pub strips: Vec<Stereo>,
    /// Recordings that closed during the render.
    pub recordings: Vec<RecordingSummary>,
}

impl DjOutput {
    /// Frames rendered.
    pub fn frames(&self) -> usize {
        self.master[0].len()
    }
}

/// A session and its graph, processed block by block in the calling
/// thread.
pub struct DjRenderer {
    session: DjSession,
    graph: CompiledGraph,
    block: usize,
    position: u64,
}

fn append(target: &mut Stereo, graph: &CompiledGraph, id: fourfloor_core::DeviceId, first: usize) {
    for (channel, buffer) in target.iter_mut().enumerate() {
        match graph.node_output(id, first + channel) {
            Some(samples) => buffer.extend_from_slice(samples),
            None => buffer.resize(buffer.len() + graph_frames(graph), 0.0),
        }
    }
}

fn graph_frames(graph: &CompiledGraph) -> usize {
    graph.output(0).map_or(0, <[f32]>::len)
}

impl DjRenderer {
    /// Builds a session and renders it in blocks of `config.max_block`.
    pub fn new(config: &SessionConfig, registry: &DeviceRegistry) -> Result<Self, DjError> {
        let (session, graph) = DjSession::new(config, registry)?;
        Ok(Self {
            session,
            graph,
            block: config.max_block,
            position: 0,
        })
    }

    /// The session.
    pub fn session(&self) -> &DjSession {
        &self.session
    }

    /// The session, for calls that do not need the graph.
    pub fn session_mut(&mut self) -> &mut DjSession {
        &mut self.session
    }

    /// The graph, for taps of other nodes.
    pub fn graph(&self) -> &CompiledGraph {
        &self.graph
    }

    /// Frames rendered so far.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// Carries out a command; it takes effect at the next block.
    pub fn apply(&mut self, command: &DjCommand) -> Result<(), DjError> {
        self.session.apply(command, &mut self.graph)
    }

    /// Carries out several commands in order.
    pub fn apply_all(&mut self, commands: &[DjCommand]) -> Result<(), DjError> {
        commands.iter().try_for_each(|command| self.apply(command))
    }

    /// Loads a prepared track.
    pub fn load(&mut self, deck: usize, track: DeckTrack) -> Result<(), DjError> {
        self.session.load_track(deck, track)
    }

    /// Renders `frames` frames, pumping the session after every block.
    pub fn render(&mut self, frames: usize) -> Result<DjOutput, DjError> {
        let decks = self.session.deck_count();
        let mut output = DjOutput {
            master: [Vec::with_capacity(frames), Vec::with_capacity(frames)],
            cue: [Vec::with_capacity(frames), Vec::with_capacity(frames)],
            decks: vec![[Vec::with_capacity(frames), Vec::with_capacity(frames)]; decks],
            strips: vec![[Vec::with_capacity(frames), Vec::with_capacity(frames)]; decks],
            recordings: Vec::new(),
        };
        let mut left = frames;
        while left > 0 {
            let n = left.min(self.block);
            let tempo = self.session.tempo().unwrap_or(Tempo::DEFAULT);
            let ctx =
                TransportContext::playing_at(self.session.sample_rate(), tempo, self.position);
            self.graph.process(&ctx, n);
            let out = crate::session::RECORDER_ID;
            append(&mut output.master, &self.graph, out, 0);
            if DJ_CHANNELS >= 4 {
                append(&mut output.cue, &self.graph, out, 2);
            }
            for deck in 0..decks {
                if let Some(ids) = self.session.deck_ids(deck) {
                    let (deck_id, strip_id) = (ids.deck, ids.strip_output());
                    append(&mut output.decks[deck], &self.graph, deck_id, 0);
                    append(&mut output.strips[deck], &self.graph, strip_id, 0);
                }
            }
            if let Some(summary) = self.session.pump()? {
                output.recordings.push(summary);
            }
            self.position += n as u64;
            left -= n;
        }
        Ok(output)
    }

    /// Renders `seconds` of audio.
    pub fn render_seconds(&mut self, seconds: f64) -> Result<DjOutput, DjError> {
        let frames = (seconds.max(0.0) * self.session.sample_rate().as_f64()).round() as usize;
        self.render(frames)
    }
}
