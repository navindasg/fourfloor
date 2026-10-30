//! Turning intents into commands and service calls.

use fourfloor_compile::{learned_command, midi_router};
use fourfloor_core::{AudioMessage, EventKind, Ticks};
use fourfloor_engine::midi::parse;
use fourfloor_model::command::AddAudioClip;
use fourfloor_model::{AudioPool, Command, Outcome};

use crate::services::midi_in::MidiInput;
use crate::services::{EngineHost, Monitor, OutputMode};
use crate::state::{Intent, TransportIntent};

use super::{FourfloorApp, PreviewOff};

/// How long a preview note sounds, in seconds.
pub const PREVIEW_SECONDS: f64 = 0.3;

impl FourfloorApp {
    /// Applies intents in order.
    pub fn apply_intents(&mut self, intents: Vec<Intent>) {
        for intent in intents {
            self.apply(intent);
        }
    }

    fn apply(&mut self, intent: Intent) {
        match intent {
            Intent::Command(command) => self.run_command(command),
            Intent::Transaction { label, commands } => {
                if let Err(error) = self.doc.transaction(&label, commands) {
                    self.ui_state.error(error.to_string());
                }
            }
            Intent::Undo => match self.doc.undo() {
                Ok(label) => self.ui_state.info(format!("Undid {label}")),
                Err(error) => self.ui_state.error(error.to_string()),
            },
            Intent::Redo => match self.doc.redo() {
                Ok(label) => self.ui_state.info(format!("Redid {label}")),
                Err(error) => self.ui_state.error(error.to_string()),
            },
            Intent::Transport(transport) => self.transport(transport),
            Intent::LiveParam {
                device,
                param,
                value,
            } => self.host.set_param(device, param, value),
            Intent::LiveTrackGain { track, gain_db } => self.host.set_track_gain(track, gain_db),
            Intent::LiveTrackPan { track, pan } => self.host.set_track_pan(track, pan),
            Intent::Launch { track, scene } => {
                let project = self.doc.project().clone();
                self.host.launch(track, scene, &project);
            }
            Intent::LaunchScene(scene) => {
                let project = self.doc.project().clone();
                self.host.launch_scene(scene, &project);
            }
            Intent::PreviewNote {
                track,
                pitch,
                velocity,
            } => self.preview(track, pitch, velocity),
            Intent::File(file) => self.file(file),
            Intent::NeedWaveform(source) => self.request_waveform(source),
            Intent::MonitorReference(on) => {
                self.ui_state.reference.monitoring = on && self.reference.is_some();
                self.update_monitor();
            }
            Intent::MatchReference(on) => {
                self.ui_state.reference.matched = on;
                self.update_monitor();
            }
            Intent::MidiLearn {
                device,
                param,
                min,
                max,
            } => {
                self.learn.arm(device, param, min, max);
                self.ui_state.info("MIDI learn: move a controller");
            }
            Intent::ImportAudio { track, path, at } => self.import_audio(track, &path, at),
            Intent::OpenMidiInput(name) => self.open_midi(name),
            Intent::RestartAudio {
                device,
                buffer_frames,
            } => self.restart_audio(device, buffer_frames),
            Intent::Dj(dj) => self.dj_intent(dj),
            Intent::Library(library) => self.library_intent(library),
            Intent::ToggleLibrary => {
                self.ui_state.panels.library = !self.ui_state.panels.library;
            }
            Intent::Settings(settings) => self.settings_intent(settings),
            Intent::Ml(crate::state::MlIntent::Download(name)) => self.download_model(name),
            Intent::Ml(crate::state::MlIntent::Refresh) => self.refresh_models(),
            Intent::Assistant(assistant) => self.assistant_intent(assistant),
            Intent::Ml(ml) => self.clip_ml(ml),
        }
    }

    pub(super) fn run_command(&mut self, command: Command) {
        let name = command.name();
        match self.doc.execute(command.clone()) {
            Ok(outcome) => {
                let messages: Vec<AudioMessage> = command.audio_messages(self.doc.project());
                self.host.send_all(messages);
                self.follow_outcome(outcome);
            }
            Err(error) => self.ui_state.error(format!("{name}: {error}")),
        }
    }

    fn follow_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::TrackCreated { track } => self.ui_state.selection.select_track(track),
            Outcome::ClipWritten {
                clip,
                created: true,
            } => {
                if let Some((track, _)) = self.doc.project().find_any_clip(clip) {
                    let track = track.id;
                    self.ui_state.selection.select_track(track);
                }
                self.ui_state.selection.clip = Some(clip);
            }
            Outcome::DeviceAdded { device } => self.ui_state.selection.device = Some(device),
            _ => {}
        }
    }

    fn transport(&mut self, intent: TransportIntent) {
        match intent {
            TransportIntent::Play => self.play(),
            TransportIntent::Stop => self.host.stop(),
            TransportIntent::Toggle => {
                if self.host.readout().playing {
                    self.host.stop();
                } else {
                    self.play();
                }
            }
            TransportIntent::Seek(ticks) => self.host.seek(ticks),
            TransportIntent::SetLoop(region) => {
                if let Some(range) = region {
                    self.ui_state.loop_range = range;
                }
                self.ui_state.loop_on = region.is_some();
                self.host.set_loop(region);
            }
            TransportIntent::ToggleLoop => {
                self.ui_state.loop_on = !self.ui_state.loop_on;
                let region = self.ui_state.loop_on.then_some(self.ui_state.loop_range);
                self.host.set_loop(region);
            }
            TransportIntent::Metronome(on) => {
                self.ui_state.metronome = on;
                self.update_monitor();
            }
        }
    }

    fn play(&mut self) {
        self.analysis.reset();
        self.host.play();
    }

    pub(super) fn update_monitor(&mut self) {
        let gain_db = match (&self.reference, self.ui_state.reference.matched) {
            (Some(reference), true) => reference.match_gain_db(),
            _ => 0.0,
        };
        let monitor = Monitor {
            reference: self.ui_state.reference.monitoring && self.reference.is_some(),
            reference_gain: 10f64.powf(gain_db / 20.0) as f32,
            metronome: self.ui_state.metronome,
        };
        self.host.set_monitor(monitor);
    }

    fn preview(&mut self, track: fourfloor_core::TrackId, pitch: u8, velocity: u8) {
        let Some(device) = self
            .host
            .layout()
            .tracks
            .get(&track)
            .and_then(|t| t.instrument)
        else {
            return;
        };
        self.host.send_all(vec![AudioMessage::SendEvent {
            device,
            event: EventKind::note_on(pitch, velocity),
        }]);
        self.preview_offs.push(PreviewOff {
            device,
            pitch,
            at: self.clock + PREVIEW_SECONDS,
        });
    }

    pub(super) fn release_previews(&mut self) {
        let now = self.clock;
        let (due, waiting): (Vec<PreviewOff>, Vec<PreviewOff>) =
            self.preview_offs.drain(..).partition(|off| off.at <= now);
        self.preview_offs = waiting;
        let messages = due
            .into_iter()
            .map(|off| AudioMessage::SendEvent {
                device: off.device,
                event: EventKind::note_off(off.pitch),
            })
            .collect();
        self.host.send_all(messages);
    }

    pub(super) fn midi_input(&mut self) {
        let Some(midi) = &self.midi else {
            return;
        };
        let messages = midi.drain();
        if messages.is_empty() {
            return;
        }
        if self.learn.is_armed() {
            for bytes in &messages {
                let Some(message) = parse(bytes) else {
                    continue;
                };
                if let Some(binding) = self.learn.feed(&message) {
                    self.run_command(Command::MapController(learned_command(&binding)));
                    self.ui_state.info("Controller mapped");
                }
            }
            return;
        }
        let instrument = self
            .ui_state
            .armed
            .and_then(|track| self.host.layout().tracks.get(&track))
            .and_then(|t| t.instrument);
        let router = midi_router(self.doc.project(), instrument);
        let routed = messages
            .iter()
            .filter_map(|bytes| router.route_bytes(bytes))
            .collect();
        self.host.send_all(routed);
    }

    fn open_midi(&mut self, name: Option<String>) {
        self.midi = None;
        self.ui_state.devices.midi_input = None;
        let Some(name) = name else {
            return;
        };
        match MidiInput::open(&name, || {}) {
            Ok(input) => {
                self.ui_state.devices.midi_input = Some(input.name.clone());
                self.ui_state.info(format!("MIDI input: {}", input.name));
                self.midi = Some(input);
            }
            Err(error) => self.ui_state.error(format!("MIDI: {error}")),
        }
    }

    fn restart_audio(&mut self, device: Option<String>, buffer_frames: Option<u32>) {
        #[cfg(feature = "audio")]
        let mode = OutputMode::Device(crate::services::output::OutputSettings {
            device,
            buffer_frames,
        });
        #[cfg(not(feature = "audio"))]
        let mode = {
            let _ = (device, buffer_frames);
            OutputMode::Offline {
                sample_rate: self.host.sample_rate(),
            }
        };
        match EngineHost::new(std::sync::Arc::clone(&self.registry), &mode) {
            Ok(host) => {
                self.host = host;
                self.analysis = crate::services::MasterAnalysis::new(self.host.sample_rate());
                self.doc.invalidate_graph();
                if let Some(reference) = &self.reference {
                    self.host
                        .set_reference(Some(std::sync::Arc::clone(&reference.audio)));
                }
                self.update_monitor();
                self.reinstall_dj();
                match &self.host.output_error {
                    Some(error) => self
                        .ui_state
                        .error(format!("audio output unavailable: {error}")),
                    None => self
                        .ui_state
                        .info(format!("Audio: {}", self.host.output_name())),
                }
            }
            Err(error) => self.ui_state.error(error.to_string()),
        }
    }

    fn import_audio(&mut self, track: fourfloor_core::TrackId, path: &std::path::Path, at: i64) {
        let Some(dir) = self.doc.dir.clone() else {
            self.ui_state
                .error("Save the project first: audio is copied into its folder");
            return;
        };
        let pool = AudioPool::new(&dir);
        let source = match pool.import_file(path) {
            Ok(source) => source,
            Err(error) => {
                self.ui_state.error(format!("import failed: {error}"));
                return;
            }
        };
        let seconds =
            fourfloor_audio::decode_file(path).map_or(4.0, |audio| audio.duration_seconds());
        let ticks = (seconds * self.doc.project().tempo.bpm() / 60.0 * 960.0)
            .round()
            .max(1.0) as i64;
        let name = path
            .file_stem()
            .map_or("Audio".to_owned(), |s| s.to_string_lossy().into_owned());
        self.run_command(Command::AddAudioClip(AddAudioClip {
            track,
            name,
            start: Ticks::new(at.max(0)),
            length: Ticks::new(ticks),
            source,
            source_offset_frames: 0,
        }));
    }
}
