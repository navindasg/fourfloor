//! The UI-side copy of the project: a command bus plus where it is saved.

use std::path::PathBuf;

use fourfloor_model::{Command, CommandBus, ModelError, Outcome, Project};

/// Commands that the engine can take as parameter messages, so the graph
/// does not have to be rebuilt.
pub fn is_live_only(command: &Command) -> bool {
    matches!(command, Command::SetParam(_) | Command::SetMacro(_))
}

/// The open project.
#[derive(Debug)]
pub struct Document {
    bus: CommandBus,
    /// The project folder, once saved or opened.
    pub dir: Option<PathBuf>,
    saved_revision: u64,
    compiled_revision: Option<u64>,
}

impl Document {
    /// A document for `project`, not yet saved anywhere.
    pub fn new(project: Project) -> Self {
        let bus = CommandBus::new(project);
        Self {
            saved_revision: bus.revision(),
            bus,
            dir: None,
            compiled_revision: None,
        }
    }

    /// A document opened from `dir`.
    pub fn opened(project: Project, dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            ..Self::new(project)
        }
    }

    /// The current project.
    pub fn project(&self) -> &Project {
        self.bus.project()
    }

    /// The command bus (undo labels, depth).
    pub fn bus(&self) -> &CommandBus {
        &self.bus
    }

    /// The bus revision: it changes with every applied, undone or redone
    /// step.
    pub fn revision(&self) -> u64 {
        self.bus.revision()
    }

    /// `true` with unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.bus.revision() != self.saved_revision
    }

    /// Marks the current revision as saved.
    pub fn mark_saved(&mut self) {
        self.saved_revision = self.bus.revision();
    }

    /// Runs one command. A live-only command keeps the running graph.
    pub fn execute(&mut self, command: Command) -> Result<Outcome, ModelError> {
        let keeps_graph = is_live_only(&command) && self.graph_is_current();
        let outcome = self.bus.execute(command)?;
        if keeps_graph {
            self.compiled_revision = Some(self.bus.revision());
        }
        Ok(outcome)
    }

    /// Runs several commands as one undo step.
    pub fn transaction(&mut self, label: &str, commands: Vec<Command>) -> Result<(), ModelError> {
        let keeps_graph = commands.iter().all(is_live_only) && self.graph_is_current();
        self.bus.transaction(label, |tx| {
            for command in commands {
                tx.execute(command)?;
            }
            Ok(())
        })?;
        if keeps_graph {
            self.compiled_revision = Some(self.bus.revision());
        }
        Ok(())
    }

    /// Undoes the last step.
    pub fn undo(&mut self) -> Result<String, ModelError> {
        self.bus.undo().map(|entry| entry.label.clone())
    }

    /// Redoes the last undone step.
    pub fn redo(&mut self) -> Result<String, ModelError> {
        self.bus.redo().map(|entry| entry.label.clone())
    }

    fn graph_is_current(&self) -> bool {
        self.compiled_revision == Some(self.bus.revision())
    }

    /// `true` if the engine's graph is older than the project.
    pub fn needs_compile(&self) -> bool {
        !self.graph_is_current()
    }

    /// Forgets that a graph was built (after the engine restarted).
    pub fn invalidate_graph(&mut self) {
        self.compiled_revision = None;
    }

    /// Records that the engine has (or is building) the current revision.
    pub fn mark_compiled(&mut self) {
        self.compiled_revision = Some(self.bus.revision());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_core::{DeviceId, ParamId};
    use fourfloor_model::command::{AddDevice, CreateTrack, SetParam, SetTempo};
    use fourfloor_model::{Param, TrackKind};

    #[test]
    fn param_moves_keep_the_graph_and_structure_changes_do_not() {
        let mut doc = Document::new(Project::new("Test"));
        assert!(doc.needs_compile());
        let track = doc
            .execute(Command::CreateTrack(CreateTrack {
                name: "Lead".to_owned(),
                kind: TrackKind::Midi,
                index: None,
            }))
            .expect("create")
            .track()
            .expect("track id");
        let device = doc
            .execute(Command::AddDevice(AddDevice {
                track,
                kind: "builtin.gain".to_owned(),
                name: None,
                index: None,
                params: vec![Param {
                    id: ParamId(0),
                    name: "Gain".to_owned(),
                    value: 1.0,
                    min: 0.0,
                    max: 2.0,
                }],
            }))
            .expect("add")
            .device()
            .unwrap_or(DeviceId(0));
        doc.mark_compiled();
        doc.execute(Command::SetParam(SetParam {
            device,
            param: ParamId(0),
            value: 0.5,
        }))
        .expect("set");
        assert!(!doc.needs_compile(), "a param move is sent live");
        doc.execute(Command::SetTempo(SetTempo { bpm: 130.0 }))
            .expect("tempo");
        assert!(doc.needs_compile(), "tempo recompiles (audio warps)");
        assert!(doc.is_dirty());
        doc.mark_saved();
        assert!(!doc.is_dirty());
        assert_eq!(doc.undo().expect("undo"), "set_tempo");
        assert!(doc.is_dirty());
    }
}
