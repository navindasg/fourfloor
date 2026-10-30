//! The UI thread's handle on the running engine: transport, graph swaps
//! after edits, live parameter moves, launcher control and meters.
//!
//! The audio processor lives either in a cpal stream ([`Backend::Device`])
//! or here, pumped by the UI clock ([`Backend::Offline`], used without an
//! audio device, in `--smoke` runs and in tests).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fourfloor_compile::CompiledTrack;
use fourfloor_core::{
    AudioMessage, Beats, DeviceId, DeviceRegistry, ParamId, SampleRate, Tempo, Ticks, TrackId,
    TICKS_PER_BEAT,
};
use fourfloor_engine::devices::{ClipLauncher, Fader};
use fourfloor_engine::{engine, CompiledGraph, EngineConfig, EngineController};
use fourfloor_model::{Project, SceneId};

use super::callback::{AudioCallback, CallbackLink, CallbackMessage, Monitor, ReferenceAudio};
use super::compile_worker::{CompileRequest, CompileWorker};
use super::meters::{MeterBank, TapPlan};
#[cfg(feature = "audio")]
use super::output::{self, DeviceStream, OutputSettings};
use super::voices::{DeckTaps, MAX_DECK_METERS};

/// Where the audio goes.
pub enum Backend {
    /// Rendered here when [`EngineHost::pump`] is called; the output is
    /// discarded (meters and analysis still run).
    Offline(Box<AudioCallback>),
    /// A live device stream.
    #[cfg(feature = "audio")]
    Device(DeviceStream),
}

impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Offline(_) => f.write_str("Offline"),
            #[cfg(feature = "audio")]
            Self::Device(stream) => write!(f, "Device({})", stream.name),
        }
    }
}

/// How to start the engine.
#[derive(Clone, Debug, PartialEq)]
pub enum OutputMode {
    /// No audio device.
    Offline {
        /// Engine sample rate.
        sample_rate: SampleRate,
    },
    /// A cpal device (falls back to offline if it will not open).
    #[cfg(feature = "audio")]
    Device(OutputSettings),
}

/// Engine block size used by the app.
pub const MAX_BLOCK: usize = 512;

/// Per-track ids in the running graph.
#[derive(Clone, Debug, Default)]
pub struct GraphLayout {
    /// Each compiled track, by track id.
    pub tracks: HashMap<TrackId, CompiledTrack>,
    /// Meter slot of each track.
    pub tap_slots: HashMap<TrackId, usize>,
    /// Warnings from the compiler.
    pub warnings: Vec<String>,
    /// Revision of the project this graph came from.
    pub revision: u64,
}

/// A launch that has been sent and when it takes effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LaunchState {
    /// The scene playing (or queued); `None` is the arrangement.
    pub scene: Option<SceneId>,
    /// Tick at which it starts.
    pub at: i64,
}

/// What the transport bar shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransportReadout {
    /// Playing or stopped.
    pub playing: bool,
    /// Playhead in ticks.
    pub position: f64,
    /// Engine tempo.
    pub tempo: Tempo,
}

/// The engine as seen from the UI thread.
pub struct EngineHost {
    controller: EngineController,
    link: CallbackLink,
    meters: Arc<MeterBank>,
    backend: Backend,
    registry: Arc<DeviceRegistry>,
    worker: CompileWorker,
    layout: GraphLayout,
    requested: Option<u64>,
    installed: Option<u64>,
    monitor: Monitor,
    loop_region: Option<(i64, i64)>,
    launches: HashMap<TrackId, LaunchState>,
    sample_rate: SampleRate,
    scratch: Vec<f32>,
    /// Why the device output is not running, if it is not.
    pub output_error: Option<String>,
    /// The last compile failure, if the newest compile failed.
    pub compile_error: Option<String>,
}

impl std::fmt::Debug for EngineHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EngineHost")
            .field("backend", &self.backend)
            .field("sample_rate", &self.sample_rate)
            .finish_non_exhaustive()
    }
}

/// Starting the engine failed outright.
#[derive(Debug, thiserror::Error)]
#[error("cannot start the engine: {0}")]
pub struct HostError(pub String);

fn start_engine(
    sample_rate: SampleRate,
    meters: &Arc<MeterBank>,
) -> Result<(EngineController, AudioCallback, CallbackLink), HostError> {
    let mut config = EngineConfig::new(sample_rate);
    config.max_block = MAX_BLOCK;
    let graph = CompiledGraph::empty(config.channels, sample_rate, MAX_BLOCK)
        .map_err(|error| HostError(error.to_string()))?;
    let (controller, processor) =
        engine(config, graph).map_err(|error| HostError(error.to_string()))?;
    let (callback, link) = AudioCallback::new(processor, Arc::clone(meters));
    Ok((controller, callback, link))
}

impl EngineHost {
    /// Starts an engine for `mode`. A device that will not open leaves the
    /// host offline with [`output_error`](Self::output_error) set.
    pub fn new(registry: Arc<DeviceRegistry>, mode: &OutputMode) -> Result<Self, HostError> {
        let meters = Arc::new(MeterBank::new());
        let (sample_rate, output_error) = Self::rate_for(mode);
        let (controller, callback, link) = start_engine(sample_rate, &meters)?;
        let mut host = Self {
            controller,
            link,
            meters,
            backend: Backend::Offline(Box::new(callback)),
            worker: CompileWorker::spawn(Arc::clone(&registry), sample_rate, MAX_BLOCK),
            registry,
            layout: GraphLayout::default(),
            requested: None,
            installed: None,
            monitor: Monitor::default(),
            loop_region: None,
            launches: HashMap::new(),
            sample_rate,
            scratch: vec![0.0; MAX_BLOCK * 2],
            output_error,
            compile_error: None,
        };
        host.attach_output(mode);
        Ok(host)
    }

    fn rate_for(mode: &OutputMode) -> (SampleRate, Option<String>) {
        match mode {
            OutputMode::Offline { sample_rate } => (*sample_rate, None),
            #[cfg(feature = "audio")]
            OutputMode::Device(settings) => match output::device_format(settings) {
                Ok(format) => (format.sample_rate, None),
                Err(error) => (SampleRate::HZ_48000, Some(error.to_string())),
            },
        }
    }

    #[cfg(feature = "audio")]
    fn attach_output(&mut self, mode: &OutputMode) {
        let OutputMode::Device(settings) = mode else {
            return;
        };
        if self.output_error.is_some() {
            return;
        }
        let placeholder = match start_engine(self.sample_rate, &self.meters) {
            Ok((_, callback, _)) => Backend::Offline(Box::new(callback)),
            Err(error) => {
                self.output_error = Some(error.0);
                return;
            }
        };
        let Backend::Offline(callback) = std::mem::replace(&mut self.backend, placeholder) else {
            return;
        };
        match output::start_output(settings, *callback, MAX_BLOCK) {
            Ok(stream) => self.backend = Backend::Device(stream),
            Err(error) => {
                // The callback moved into the failed stream; rebuild.
                self.output_error = Some(error.to_string());
                if let Ok((controller, callback, link)) =
                    start_engine(self.sample_rate, &self.meters)
                {
                    self.controller = controller;
                    self.link = link;
                    self.backend = Backend::Offline(Box::new(callback));
                }
            }
        }
    }

    #[cfg(not(feature = "audio"))]
    fn attach_output(&mut self, _mode: &OutputMode) {}

    /// The engine's sample rate.
    pub fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    /// The device registry graphs are built from.
    pub fn registry(&self) -> &Arc<DeviceRegistry> {
        &self.registry
    }

    /// `true` when audio goes to a device.
    pub fn is_live(&self) -> bool {
        !matches!(self.backend, Backend::Offline(_))
    }

    /// A one-line description of the output.
    pub fn output_name(&self) -> String {
        match &self.backend {
            Backend::Offline(_) => "offline (no audio device)".to_owned(),
            #[cfg(feature = "audio")]
            Backend::Device(stream) => stream.name.clone(),
        }
    }

    /// The shared meters (load, dropouts, peaks).
    pub fn meters(&self) -> &Arc<MeterBank> {
        &self.meters
    }

    /// The running graph's layout.
    pub fn layout(&self) -> &GraphLayout {
        &self.layout
    }

    /// Asks the worker to compile `project` unless that revision is
    /// already running or queued.
    pub fn request_compile(&mut self, project: &Project, revision: u64, dir: Option<PathBuf>) {
        if self.requested == Some(revision) {
            return;
        }
        self.requested = Some(revision);
        self.worker.request(CompileRequest {
            project: project.clone(),
            revision,
            project_dir: dir,
        });
    }

    /// The revision of the graph now running, if any was installed.
    pub fn installed_revision(&self) -> Option<u64> {
        self.installed
    }

    /// The revision last asked for.
    pub fn requested_revision(&self) -> Option<u64> {
        self.requested
    }

    /// Installs finished compiles. Returns `true` if a new graph went in.
    pub fn poll(&mut self) -> bool {
        self.link.collect_garbage();
        self.controller.collect_garbage();
        match self.worker.try_latest() {
            Some(result) => self.install(result),
            None => false,
        }
    }

    /// Waits up to `timeout` for the compile of the requested revision and
    /// installs it. For tests and headless runs.
    pub fn wait_for_graph(&mut self, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            let Some(result) = self.worker.wait(left) else {
                return false;
            };
            let revision = result.revision;
            let installed = self.install(result);
            if Some(revision) == self.requested {
                return installed;
            }
        }
        false
    }

    fn install(&mut self, result: super::compile_worker::CompileResult) -> bool {
        let compiled = match result.outcome {
            Ok(compiled) => compiled,
            Err(error) => {
                self.compile_error = Some(error);
                return false;
            }
        };
        self.compile_error = None;
        let faders: Vec<DeviceId> = compiled.tracks.iter().map(|track| track.fader).collect();
        let layout = GraphLayout {
            tap_slots: compiled
                .tracks
                .iter()
                .enumerate()
                .map(|(slot, track)| (track.track, slot))
                .collect(),
            tracks: compiled
                .tracks
                .iter()
                .map(|track| (track.track, track.clone()))
                .collect(),
            warnings: compiled.warnings.clone(),
            revision: result.revision,
        };
        if let Err(error) = self.controller.swap_graph(compiled.graph) {
            self.compile_error = Some(error.to_string());
            return false;
        }
        // Sent after the swap so the callback applies both together.
        let _ = self
            .link
            .tx
            .push(CallbackMessage::Taps(TapPlan::new(&faders)));
        self.layout = layout;
        self.installed = Some(result.revision);
        // Launcher state starts over with a new graph.
        self.launches.clear();
        if let Some((start, end)) = self.loop_region {
            self.send_loop(start, end);
        }
        true
    }

    /// Renders `frames` on the offline backend (no-op when live).
    pub fn pump(&mut self, frames: usize) {
        // Without the `audio` feature there is no other backend.
        #[allow(irrefutable_let_patterns)]
        let Backend::Offline(callback) = &mut self.backend
        else {
            return;
        };
        let mut left = frames;
        while left > 0 {
            let now = left.min(MAX_BLOCK);
            callback.render(&mut self.scratch[..now * 2], 2);
            left -= now;
        }
    }

    /// Hands every queued master sample (interleaved stereo) to `sink`.
    pub fn drain_master(&mut self, mut sink: impl FnMut(&[f32])) {
        let available = self.link.master_rx.slots();
        let even = available - available % 2;
        if even == 0 {
            return;
        }
        if let Ok(chunk) = self.link.master_rx.read_chunk(even) {
            let (first, second) = chunk.as_slices();
            sink(first);
            sink(second);
            chunk.commit_all();
        }
    }

    fn send(&mut self, message: AudioMessage) {
        // A full queue drops the message; the UI re-sends state on the
        // next change, and a recompile carries every value.
        let _ = self.controller.send(message);
    }

    /// Starts playback.
    pub fn play(&mut self) {
        self.send(AudioMessage::Play);
    }

    /// Stops playback.
    pub fn stop(&mut self) {
        self.send(AudioMessage::Stop);
    }

    /// Moves the playhead.
    pub fn seek(&mut self, ticks: i64) {
        self.send(AudioMessage::Seek {
            position: Ticks::new(ticks.max(0)).to_beats(),
        });
    }

    /// Sets (or clears) the loop, in ticks.
    pub fn set_loop(&mut self, region: Option<(i64, i64)>) {
        self.loop_region = region.filter(|(start, end)| end > start);
        match self.loop_region {
            Some((start, end)) => self.send_loop(start, end),
            None => self.send(AudioMessage::ClearLoop),
        }
    }

    fn send_loop(&mut self, start: i64, end: i64) {
        self.send(AudioMessage::SetLoop {
            start: Beats::new(start as f64 / TICKS_PER_BEAT as f64),
            end: Beats::new(end as f64 / TICKS_PER_BEAT as f64),
        });
    }

    /// The loop region in ticks.
    pub fn loop_region(&self) -> Option<(i64, i64)> {
        self.loop_region
    }

    /// Sets the tempo without recompiling.
    pub fn set_tempo(&mut self, bpm: f64) {
        if let Ok(tempo) = Tempo::new(bpm) {
            self.send(AudioMessage::SetTempo { tempo });
        }
    }

    /// Moves a device parameter directly (a live drag, a macro).
    pub fn set_param(&mut self, device: DeviceId, param: ParamId, value: f32) {
        self.send(AudioMessage::SetParam {
            device,
            param,
            value,
        });
    }

    /// Sends audio messages produced by a command.
    pub fn send_all(&mut self, messages: Vec<AudioMessage>) {
        for message in messages {
            self.send(message);
        }
    }

    /// Moves a track's fader live (during a drag, before the command).
    pub fn set_track_gain(&mut self, track: TrackId, gain_db: f32) {
        if let Some(fader) = self.layout.tracks.get(&track).map(|t| t.fader) {
            self.set_param(fader, Fader::GAIN_DB, gain_db);
        }
    }

    /// Moves a track's pan live.
    pub fn set_track_pan(&mut self, track: TrackId, pan: f32) {
        if let Some(fader) = self.layout.tracks.get(&track).map(|t| t.fader) {
            self.set_param(fader, Fader::PAN, pan);
        }
    }

    /// Plays `track`'s slot in `scene` (or returns it to the arrangement
    /// with `None`) at the next launch-quantize boundary.
    pub fn launch(&mut self, track: TrackId, scene: Option<SceneId>, project: &Project) {
        let Some(launcher) = self.layout.tracks.get(&track).and_then(|t| t.launcher) else {
            return;
        };
        let value = match scene {
            None => 0.0,
            Some(scene) => match project.scene_index(scene) {
                Some(row) => (row + 1) as f32,
                None => return,
            },
        };
        self.set_param(launcher, ClipLauncher::LAUNCH, value);
        let at = next_boundary(self.readout().position, project.launch_quantize.get());
        self.launches.insert(track, LaunchState { scene, at });
    }

    /// Launches a whole scene on every track that has a launcher.
    pub fn launch_scene(&mut self, scene: Option<SceneId>, project: &Project) {
        let tracks: Vec<TrackId> = self
            .layout
            .tracks
            .values()
            .filter(|track| track.launcher.is_some())
            .map(|track| track.track)
            .collect();
        for track in tracks {
            self.launch(track, scene, project);
        }
    }

    /// What `track`'s launcher was last told to play.
    pub fn launch_state(&self, track: TrackId) -> Option<LaunchState> {
        self.launches.get(&track).copied()
    }

    /// Transport position and state from the engine's atomics.
    pub fn readout(&self) -> TransportReadout {
        let tempo = self.controller.tempo();
        let beats = self.controller.playhead_beats();
        TransportReadout {
            playing: self.controller.is_playing(),
            position: beats.get() * TICKS_PER_BEAT as f64,
            tempo,
        }
    }

    /// Takes the held post-fader peaks of `track`.
    pub fn take_track_peak(&self, track: TrackId) -> [f32; 2] {
        self.layout
            .tap_slots
            .get(&track)
            .map_or([0.0; 2], |slot| self.meters.take_tap(*slot))
    }

    /// Takes the held master peaks.
    pub fn take_master_peak(&self) -> [f32; 2] {
        self.meters.take_master()
    }

    /// Changes what is monitored (mix or reference, metronome).
    pub fn set_monitor(&mut self, monitor: Monitor) {
        self.monitor = monitor;
        let _ = self.link.tx.push(CallbackMessage::Monitor(monitor));
    }

    /// The current monitor settings.
    pub fn monitor(&self) -> Monitor {
        self.monitor
    }

    /// Hands reference audio to the callback (or clears it).
    pub fn set_reference(&mut self, audio: Option<Arc<ReferenceAudio>>) {
        let _ = self.link.tx.push(CallbackMessage::Reference(audio));
    }

    /// Runs the DJ decks' graph in the callback (or removes it with
    /// `None`). Returns `false` if the queue was full.
    pub fn install_dj(&mut self, graph: Option<Box<CompiledGraph>>, taps: DeckTaps) -> bool {
        self.link
            .tx
            .push(CallbackMessage::DjGraph(graph, taps))
            .is_ok()
    }

    /// Sets a parameter of a DJ graph node. `false` if the queue was full.
    pub fn dj_param(&mut self, device: DeviceId, param: ParamId, value: f32) -> bool {
        self.link
            .tx
            .push(CallbackMessage::DjParam {
                device,
                param,
                value,
            })
            .is_ok()
    }

    /// Gives the DJ graph the tempo master's tempo.
    pub fn set_dj_tempo(&mut self, tempo: Tempo) {
        let _ = self.link.tx.push(CallbackMessage::DjTempo(tempo));
    }

    /// Auditions `audio` once over the output (or stops with `None`).
    pub fn set_preview(&mut self, audio: Option<Arc<ReferenceAudio>>) {
        let _ = self.link.tx.push(CallbackMessage::Preview(audio));
    }

    /// Takes the held peaks of the DJ deck strips.
    pub fn take_deck_peaks(&self) -> [[f32; 2]; MAX_DECK_METERS] {
        self.link.deck_meters.take()
    }
}

/// The first multiple of `quantize` at or after `position` (ticks).
pub fn next_boundary(position: f64, quantize: i64) -> i64 {
    if quantize <= 0 {
        return position.ceil() as i64;
    }
    let position = position.max(0.0);
    ((position / quantize as f64).ceil() as i64) * quantize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launches_wait_for_the_next_boundary() {
        assert_eq!(next_boundary(0.0, 3840), 0);
        assert_eq!(next_boundary(1.0, 3840), 3840);
        assert_eq!(next_boundary(3840.0, 3840), 3840);
        assert_eq!(next_boundary(5000.0, 960), 5760);
        assert_eq!(next_boundary(5000.4, 0), 5001);
    }
}
