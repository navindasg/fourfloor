//! The application: owns the document, UI state and services, draws the
//! shell and the current view each frame, then applies the frame's
//! intents.

mod apply;
pub mod assistant;
mod dj;
mod files;
pub mod library;
pub mod placement;
mod settings;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fourfloor_core::{DeviceId, DeviceRegistry, RegistryError};
use fourfloor_engine::midi::MidiLearn;
use fourfloor_model::Project;

use crate::project::{build_demo_project, DemoError, JobOutput, ReferenceLoaded};
use crate::services::engine_host::HostError;
use crate::services::midi_in::MidiInput;
use crate::services::{build_registry, EngineHost, Jobs, MasterAnalysis, OutputMode};
use crate::state::{
    Assets, Document, FeaturePaths, Features, Intents, LiveReadout, MeterSmoother, UiState, ViewId,
};
use crate::theme;
use crate::views::assistant::AssistantPanel;
use crate::views::browser::DeviceBrowser;
use crate::views::devices::DevicePanel;
use crate::views::library::LibraryPanel;
use crate::views::shell::{self, dialogs, TransportBar};
use crate::views::{self, View, ViewCtx};

/// How to start the app.
#[derive(Clone, Debug, PartialEq)]
pub struct AppConfig {
    /// Where audio goes.
    pub output: OutputMode,
    /// A project folder to open; otherwise the demo loads.
    pub project_dir: Option<PathBuf>,
    /// Register hosted plugins from the scan cache.
    pub with_plugins: bool,
    /// Native file dialogs.
    pub native_dialogs: bool,
    /// Where the library, models, plugin list and caches live.
    pub paths: FeaturePaths,
}

impl AppConfig {
    /// No audio device, no plugins, typed paths: for tests and `--smoke`.
    pub fn headless() -> Self {
        Self {
            output: OutputMode::Offline {
                sample_rate: fourfloor_core::SampleRate::HZ_48000,
            },
            project_dir: None,
            with_plugins: false,
            native_dialogs: false,
            paths: FeaturePaths::none(),
        }
    }
}

/// Starting the app failed.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The device registry would not build.
    #[error("device registry: {0}")]
    Registry(#[from] RegistryError),
    /// The engine would not start.
    #[error(transparent)]
    Host(#[from] HostError),
    /// The demo project would not build.
    #[error("demo project: {0}")]
    Demo(#[from] DemoError),
}

/// A note sounding from a preview click, released later.
#[derive(Clone, Copy, Debug)]
struct PreviewOff {
    device: DeviceId,
    pitch: u8,
    at: f64,
}

/// The fourfloor application.
pub struct FourfloorApp {
    /// The open project.
    pub doc: Document,
    /// UI state.
    pub ui_state: UiState,
    /// The engine.
    pub host: EngineHost,
    /// Master loudness, correlation and spectrum.
    pub analysis: MasterAnalysis,
    /// Background jobs.
    pub jobs: Jobs<JobOutput>,
    /// Decoded waveforms.
    pub assets: Assets,
    /// Frames drawn so far.
    pub frames: u64,
    /// The last readout shown to the views.
    pub live: LiveReadout,
    /// DJ decks, library, assistant, models and plugins.
    pub features: Features,
    registry: Arc<DeviceRegistry>,
    views: Vec<Box<dyn View>>,
    device_panel: DevicePanel,
    /// The sample browser panel's state.
    pub library_panel: LibraryPanel,
    /// The AI panel's state.
    pub assistant_panel: AssistantPanel,
    browser: DeviceBrowser,
    transport_bar: TransportBar,
    intents: Intents,
    meters: MeterSmoother,
    reference: Option<ReferenceLoaded>,
    midi: Option<MidiInput>,
    learn: MidiLearn,
    preview_offs: Vec<PreviewOff>,
    clock: f64,
    themed: bool,
}

impl std::fmt::Debug for FourfloorApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FourfloorApp")
            .field("project", &self.doc.project().name)
            .field("view", &self.ui_state.view)
            .finish_non_exhaustive()
    }
}

impl FourfloorApp {
    /// Builds the app: registry, engine, and the project (the folder in
    /// `config`, or the built-in demo).
    pub fn new(config: &AppConfig) -> Result<Self, AppError> {
        let registry = Arc::new(build_registry(config.with_plugins)?);
        let host = EngineHost::new(Arc::clone(&registry), &config.output)?;
        let mut ui_state = UiState {
            native_dialogs: config.native_dialogs,
            ..UiState::default()
        };
        let doc = match &config.project_dir {
            Some(dir) => match files::open_folder(dir) {
                Ok(project) => Document::opened(project, dir.clone()),
                Err(error) => {
                    ui_state.error(format!(
                        "could not open {}: {error}; opened the demo",
                        dir.display()
                    ));
                    Document::new(build_demo_project(&registry)?)
                }
            },
            None => Document::new(build_demo_project(&registry)?),
        };
        if let Some(error) = &host.output_error {
            ui_state.error(format!(
                "audio output unavailable ({error}); running without sound"
            ));
        }
        ui_state.devices.outputs = Self::output_names();
        ui_state.devices.midi_inputs = crate::services::midi_in::input_port_names();
        let analysis = MasterAnalysis::new(host.sample_rate());
        let mut app = Self {
            doc,
            ui_state,
            host,
            analysis,
            jobs: Jobs::new(),
            assets: Assets::default(),
            frames: 0,
            live: LiveReadout::default(),
            features: Features {
                paths: config.paths.clone(),
                ..Features::default()
            },
            registry,
            views: views::registry(),
            device_panel: DevicePanel::default(),
            library_panel: LibraryPanel::default(),
            assistant_panel: AssistantPanel::default(),
            browser: DeviceBrowser::default(),
            transport_bar: TransportBar::default(),
            intents: Intents::default(),
            meters: MeterSmoother::default(),
            reference: None,
            midi: None,
            learn: MidiLearn::new(false),
            preview_offs: Vec::new(),
            clock: 0.0,
            themed: false,
        };
        app.select_first_track();
        app.sync_engine();
        Ok(app)
    }

    #[cfg(feature = "audio")]
    fn output_names() -> Vec<String> {
        crate::services::output::output_device_names()
    }

    #[cfg(not(feature = "audio"))]
    fn output_names() -> Vec<String> {
        Vec::new()
    }

    fn select_first_track(&mut self) {
        let first = self.doc.project().tracks.first().map(|t| t.id);
        self.ui_state.selection = Default::default();
        if let Some(track) = first {
            self.ui_state.selection.select_track(track);
        }
    }

    /// The current project.
    pub fn project(&self) -> &Project {
        self.doc.project()
    }

    /// The device registry.
    pub fn registry(&self) -> &Arc<DeviceRegistry> {
        &self.registry
    }

    /// The views, in tab order.
    pub fn views(&self) -> &[Box<dyn View>] {
        &self.views
    }

    /// A view by id, for tests that inspect view state.
    pub fn view_mut(&mut self, id: ViewId) -> Option<&mut Box<dyn View>> {
        self.views.iter_mut().find(|view| view.id() == id)
    }

    /// Queues intents as if a view had pushed them (tests, scripting).
    pub fn push_intent(&mut self, intent: crate::state::Intent) {
        self.intents.push(intent);
    }

    /// Applies queued intents now.
    pub fn flush(&mut self) {
        let intents = self.intents.take();
        self.apply_intents(intents);
        self.sync_engine();
    }

    /// Waits until the engine runs a graph of the current project
    /// (headless runs and tests).
    pub fn wait_for_engine(&mut self, timeout: Duration) -> bool {
        self.sync_engine();
        match self.host.requested_revision() {
            None => true,
            Some(revision) if self.host.installed_revision() == Some(revision) => true,
            Some(_) => self.host.wait_for_graph(timeout),
        }
    }

    /// Advances services by `dt` seconds: graph swaps, offline audio,
    /// analysis, jobs, MIDI, meters.
    pub fn tick(&mut self, dt: f64) {
        let dt = dt.clamp(0.0, 0.25);
        self.clock += dt;
        self.host.poll();
        if !self.host.is_live() {
            let frames = (dt * self.host.sample_rate().as_f64()).round() as usize;
            self.host.pump(frames);
        }
        let analysis = &mut self.analysis;
        self.host.drain_master(|samples| analysis.push(samples));
        for done in self.jobs.poll() {
            self.job_finished(done);
        }
        self.tick_dj();
        self.tick_mcp();
        self.ui_state.age_toasts(dt as f32);
        self.midi_input();
        self.release_previews();
        let tracks: Vec<_> = self.doc.project().tracks.iter().map(|t| t.id).collect();
        for track in tracks {
            let peak = self.host.take_track_peak(track);
            self.meters.update_track(track, peak, dt as f32);
        }
        let master = self.host.take_master_peak();
        self.meters.update_master(master, dt as f32);
        self.live = self.readout();
    }

    fn readout(&self) -> LiveReadout {
        let meters = self.host.meters();
        let layout = self.host.layout();
        LiveReadout {
            transport: self.host.readout(),
            loop_region: self.host.loop_region(),
            meters: self.meters.clone(),
            master: self.analysis.readout().clone(),
            cpu_load: meters.load(),
            xruns: meters.xruns(),
            stream_errors: meters.stream_errors(),
            launches: self
                .doc
                .project()
                .tracks
                .iter()
                .filter_map(|t| self.host.launch_state(t.id).map(|s| (t.id, s)))
                .collect(),
            jobs: self
                .jobs
                .running()
                .iter()
                .map(|j| (j.label.clone(), j.progress.get()))
                .collect(),
            output_name: self.host.output_name(),
            sample_rate: self.host.sample_rate().hz(),
            warnings: layout.warnings.clone(),
            compile_error: self.host.compile_error.clone(),
            undo_label: self.doc.bus().undo_label().map(str::to_owned),
            redo_label: self.doc.bus().redo_label().map(str::to_owned),
            dirty: self.doc.is_dirty(),
            undo_depth: self.doc.bus().undo_depth(),
            project_dir: self.doc.dir.clone(),
        }
    }

    /// Draws one frame into `ui` (the whole window) and applies what the
    /// views asked for. Used by eframe and by the headless test harness.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        if !self.themed {
            theme::apply(ui.ctx());
            self.themed = true;
        }
        let dt = f64::from(ui.input(|i| i.stable_dt));
        self.tick(dt);
        self.shortcuts(ui);
        self.draw(ui);
        let intents = self.intents.take();
        self.apply_intents(intents);
        self.sync_engine();
        self.frames += 1;
        if self.live.transport.playing
            || !self.jobs.running().is_empty()
            || self.host.is_live()
            || self.features.dj.is_some()
        {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let project = self.doc.project().clone();
        let egui_ctx = ui.ctx().clone();
        let Self {
            ui_state,
            registry,
            live,
            assets,
            features,
            intents,
            views,
            device_panel,
            library_panel,
            assistant_panel,
            browser,
            transport_bar,
            ..
        } = self;
        let mut ctx = ViewCtx {
            project: &project,
            ui: ui_state,
            registry,
            live,
            assets,
            features,
            intents,
        };
        egui::Panel::top("menu").show(ui, |ui| shell::menu_bar(ui, &mut ctx, views));
        egui::Panel::top("transport").show(ui, |ui| transport_bar.ui(ui, &mut ctx));
        egui::Panel::top("views").show(ui, |ui| shell::view_tabs(ui, &mut ctx, views));
        egui::Panel::bottom("status").show(ui, |ui| shell::status_bar(ui, &mut ctx));
        if ctx.ui.panels.devices {
            egui::Panel::bottom("devices")
                .resizable(true)
                .default_size(232.0)
                .show(ui, |ui| device_panel.ui(ui, &mut ctx));
        }
        if ctx.ui.panels.library {
            egui::Panel::left("library")
                .resizable(true)
                .default_size(crate::views::library::PANEL_WIDTH)
                .show(ui, |ui| library_panel.ui(ui, &mut ctx));
        }
        if ctx.ui.panels.assistant {
            egui::Panel::right("assistant")
                .resizable(true)
                .default_size(crate::views::assistant::PANEL_WIDTH)
                .min_size(280.0)
                .show(ui, |ui| assistant_panel.ui(ui, &mut ctx));
        }
        if ctx.ui.panels.master {
            egui::Panel::right("master")
                .resizable(true)
                .default_size(240.0)
                .show(ui, |ui| crate::views::master::master_panel(ui, &mut ctx));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            let current = ctx.ui.view;
            if let Some(view) = views.iter_mut().find(|view| view.id() == current) {
                view.ui(ui, &mut ctx);
            }
        });
        browser.window(&egui_ctx, &mut ctx);
        shell::settings_window(&egui_ctx, &mut ctx);
        dialogs::prompt_window(&egui_ctx, &mut ctx);
        shell::toasts(&egui_ctx, &mut ctx);
    }

    fn shortcuts(&mut self, ui: &egui::Ui) {
        use crate::state::{FileIntent, Intent, TransportIntent};
        use egui::{Key, Modifiers};
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let views: Vec<ViewId> = self.views.iter().map(|v| v.id()).collect();
        let pressed = ui.input_mut(|i| {
            let mut out = Vec::new();
            if i.consume_key(Modifiers::NONE, Key::Space) {
                out.push(Intent::Transport(TransportIntent::Toggle));
            }
            if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                || i.consume_key(Modifiers::COMMAND, Key::Y)
            {
                out.push(Intent::Redo);
            }
            if i.consume_key(Modifiers::COMMAND, Key::Z) {
                out.push(Intent::Undo);
            }
            if i.consume_key(Modifiers::COMMAND, Key::S) {
                out.push(Intent::File(FileIntent::Save));
            }
            if i.consume_key(Modifiers::NONE, Key::B) {
                out.push(Intent::ToggleLibrary);
            }
            if i.consume_key(Modifiers::NONE, Key::L) {
                out.push(Intent::Transport(TransportIntent::ToggleLoop));
            }
            let digits = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ];
            let back = i.consume_key(Modifiers::NONE, Key::Escape);
            let view = digits
                .iter()
                .zip(&views)
                .find(|(key, _)| i.consume_key(Modifiers::COMMAND, **key))
                .map(|(_, view)| *view);
            (out, view, back)
        });
        let (intents, view, back) = pressed;
        for intent in intents {
            self.intents.push(intent);
        }
        if let Some(view) = view {
            self.ui_state.switch_view(view);
        } else if back {
            self.ui_state.go_back();
        }
    }

    /// Recompiles when the project changed in a way the graph cannot take
    /// as parameter messages, and keeps the engine tempo in step.
    pub fn sync_engine(&mut self) {
        if self.doc.needs_compile() {
            let revision = self.doc.revision();
            let dir = self.doc.dir.clone();
            self.host.request_compile(self.doc.project(), revision, dir);
            self.doc.mark_compiled();
        }
        let bpm = self.doc.project().tempo.bpm();
        if (self.host.readout().tempo.bpm() - bpm).abs() > 1e-9 {
            self.host.set_tempo(bpm);
        }
    }
}

impl eframe::App for FourfloorApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}
