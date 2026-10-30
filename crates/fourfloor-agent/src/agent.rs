//! The agent session: one project behind a command bus, the device
//! registry, and where renders and saves go.
//!
//! Everything the MCP server, the phrase parser and the chat backend do
//! ends up in [`Agent::call_tool`], so every edit goes through the same
//! [`CommandBus`] and is one undo step.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use fourfloor_core::{DeviceRegistry, RegistryError, SampleRate};
use fourfloor_model::{
    load_project, save_project, Command, CommandBus, ModelError, Outcome, Project, Transaction,
};

use crate::error::AgentError;
use crate::tools::{self, ToolOutput, ToolSpec};

/// A registry with every built-in device (the engine's, the instruments
/// and the effects) plus the hosted plugins in the cached plugin scan.
pub fn default_registry() -> Result<DeviceRegistry, RegistryError> {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry)?;
    fourfloor_instruments::register(&mut registry)?;
    fourfloor_effects::register(&mut registry)?;
    // Plugins from the cached scan only; registering never loads or scans.
    fourfloor_host::register(&mut registry)?;
    Ok(registry)
}

/// How an [`Agent`] renders and saves.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentSettings {
    /// Sample rate of renders and reports.
    pub sample_rate: SampleRate,
    /// Save the project folder after every successful edit, undo and redo.
    /// Needs a project folder.
    pub autosave: bool,
    /// Where renders and stems go when there is no project folder.
    pub output_dir: Option<PathBuf>,
    /// Longest render the agent will make, in seconds. Guards the server
    /// against a request that would hold minutes of audio in memory.
    pub max_render_seconds: f64,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            sample_rate: SampleRate::HZ_48000,
            autosave: false,
            output_dir: None,
            max_render_seconds: 600.0,
        }
    }
}

/// One project being edited by an agent.
pub struct Agent {
    bus: CommandBus,
    registry: Arc<DeviceRegistry>,
    project_dir: Option<PathBuf>,
    settings: AgentSettings,
}

impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("project", &self.bus.project().name)
            .field("project_dir", &self.project_dir)
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// An agent editing `project` in memory, with no project folder.
    pub fn new(project: Project, registry: Arc<DeviceRegistry>) -> Self {
        Self {
            bus: CommandBus::new(project),
            registry,
            project_dir: None,
            settings: AgentSettings::default(),
        }
    }

    /// Opens the project folder `dir`, or starts a new project named after
    /// the folder if it has no `project.json` yet (it is written on the
    /// first save).
    pub fn open(dir: &Path, registry: Arc<DeviceRegistry>) -> Result<Self, AgentError> {
        let project = if dir.join("project.json").is_file() {
            load_project(dir)?
        } else {
            let name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("Untitled");
            Project::new(name.chars().take(128).collect::<String>())
        };
        Ok(Self::new(project, registry).with_project_dir(dir))
    }

    /// Sets the project folder used by saves, renders and plugin state.
    pub fn with_project_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.project_dir = Some(dir.into());
        self
    }

    /// Replaces the settings.
    pub fn with_settings(mut self, settings: AgentSettings) -> Self {
        self.settings = settings;
        self
    }

    /// The current project.
    pub fn project(&self) -> &Project {
        self.bus.project()
    }

    /// The command bus, for reading history.
    pub fn bus(&self) -> &CommandBus {
        &self.bus
    }

    /// The device registry.
    pub fn registry(&self) -> &DeviceRegistry {
        &self.registry
    }

    /// A shared handle to the device registry.
    pub fn registry_arc(&self) -> Arc<DeviceRegistry> {
        Arc::clone(&self.registry)
    }

    /// The project folder, if there is one.
    pub fn project_dir(&self) -> Option<&Path> {
        self.project_dir.as_deref()
    }

    /// The settings.
    pub fn settings(&self) -> &AgentSettings {
        &self.settings
    }

    /// Every tool the agent offers, model commands first.
    pub fn tools(&self) -> Vec<ToolSpec> {
        tools::catalog()
    }

    /// Runs one tool. Edits are one undo step each; on error nothing
    /// changes.
    pub fn call_tool(
        &mut self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<ToolOutput, AgentError> {
        let output = tools::call(self, name, args)?;
        if output.changed {
            self.autosave()?;
        }
        Ok(output)
    }

    /// Applies one command as one undo step.
    pub fn execute(&mut self, command: impl Into<Command>) -> Result<Outcome, AgentError> {
        Ok(self.bus.execute(command)?)
    }

    /// Runs several commands as one undo step labelled `label`.
    pub fn transaction<T>(
        &mut self,
        label: &str,
        run: impl FnOnce(&mut Transaction) -> Result<T, AgentError>,
    ) -> Result<T, AgentError> {
        // The bus wants a ModelError from the closure. Carry the agent's own
        // error out through a side slot and report it unchanged.
        let mut failure: Option<AgentError> = None;
        let result = self.bus.transaction(label, |tx| {
            run(tx).map_err(|error| {
                let model = match &error {
                    AgentError::Model(model) => model.clone(),
                    _ => ModelError::NothingToUndo,
                };
                failure = Some(error);
                model
            })
        });
        match (result, failure) {
            (Ok(value), _) => Ok(value),
            (Err(_), Some(error)) => Err(error),
            (Err(error), None) => Err(error.into()),
        }
    }

    /// Reverts the last step; returns its label.
    pub fn undo(&mut self) -> Result<String, AgentError> {
        let label = self.bus.undo()?.label.clone();
        self.autosave()?;
        Ok(label)
    }

    /// Re-applies the last undone step; returns its label.
    pub fn redo(&mut self) -> Result<String, AgentError> {
        let label = self.bus.redo()?.label.clone();
        self.autosave()?;
        Ok(label)
    }

    /// Writes the project folder.
    pub fn save(&self) -> Result<PathBuf, AgentError> {
        let dir = self.project_dir.as_ref().ok_or_else(|| {
            AgentError::Unavailable("this session has no project folder to save to".to_owned())
        })?;
        save_project(dir, self.bus.project())?;
        Ok(dir.clone())
    }

    fn autosave(&self) -> Result<(), AgentError> {
        if self.settings.autosave && self.project_dir.is_some() {
            self.save()?;
        }
        Ok(())
    }

    /// Where an output file named by a tool goes.
    ///
    /// `relative` must be a relative path without `..`; it is placed under
    /// the project folder (or the settings' output folder). Absolute paths
    /// and paths that climb out are refused, so a client cannot make the
    /// server write anywhere else on disk.
    pub fn output_path(&self, relative: &str) -> Result<PathBuf, AgentError> {
        let root = self
            .project_dir
            .as_ref()
            .or(self.settings.output_dir.as_ref())
            .ok_or_else(|| {
                AgentError::Path(
                    "no project folder or output folder is set, so there is nowhere to write"
                        .to_owned(),
                )
            })?;
        let path = Path::new(relative);
        let safe = !relative.is_empty()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
        if !safe {
            return Err(AgentError::Path(format!(
                "\"{relative}\" must be a relative path inside the project folder"
            )));
        }
        Ok(root.join(path))
    }
}
