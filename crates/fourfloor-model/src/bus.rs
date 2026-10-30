//! The command bus: the single path by which the UI and the agent edit a
//! project, with undo and redo.
//!
//! Undo works by snapshot. Every history entry keeps the project value from
//! before and after the edit; because projects share their tracks through
//! `Arc`, a snapshot costs one pointer per track, not a deep copy.

use std::collections::VecDeque;

use crate::command::{Command, Outcome};
use crate::error::ModelError;
use crate::project::Project;

/// Default number of undo steps kept.
pub const DEFAULT_HISTORY_LIMIT: usize = 256;

/// One undoable step: a single command, or a transaction of several.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    /// Short description: the command name, or the transaction's label.
    pub label: String,
    /// The commands that made up the step, in order.
    pub commands: Vec<Command>,
    before: Project,
    after: Project,
}

/// Owns the current project and its undo and redo stacks.
#[derive(Clone, Debug)]
pub struct CommandBus {
    project: Project,
    undo: VecDeque<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    history_limit: usize,
    revision: u64,
}

impl CommandBus {
    /// A bus holding `project` with empty history.
    pub fn new(project: Project) -> Self {
        Self::with_history_limit(project, DEFAULT_HISTORY_LIMIT)
    }

    /// Like [`CommandBus::new`], keeping at most `history_limit` undo steps
    /// (at least one).
    pub fn with_history_limit(project: Project, history_limit: usize) -> Self {
        Self {
            project,
            undo: VecDeque::new(),
            redo: Vec::new(),
            history_limit: history_limit.max(1),
            revision: 0,
        }
    }

    /// The current project.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// A counter that goes up every time the project changes, including on
    /// undo and redo. Readers can compare it to know when to refresh.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Validates and applies one command as one undo step.
    ///
    /// On error the project and the history are unchanged.
    pub fn execute(&mut self, command: impl Into<Command>) -> Result<Outcome, ModelError> {
        let command = command.into();
        let (after, outcome) = command.apply(&self.project)?;
        self.commit(command.name().to_owned(), vec![command], after);
        Ok(outcome)
    }

    /// Runs several commands as one undo step.
    ///
    /// The closure executes commands on a [`Transaction`], which sees its own
    /// working copy of the project. If the closure returns an error, none of
    /// its commands take effect. A transaction that ran no commands leaves
    /// no history entry.
    pub fn transaction<T>(
        &mut self,
        label: &str,
        run: impl FnOnce(&mut Transaction) -> Result<T, ModelError>,
    ) -> Result<T, ModelError> {
        let mut transaction = Transaction {
            project: self.project.clone(),
            commands: Vec::new(),
        };
        let value = run(&mut transaction)?;
        if !transaction.commands.is_empty() {
            self.commit(label.to_owned(), transaction.commands, transaction.project);
        }
        Ok(value)
    }

    fn commit(&mut self, label: String, commands: Vec<Command>, after: Project) {
        let before = std::mem::replace(&mut self.project, after.clone());
        self.undo.push_back(HistoryEntry {
            label,
            commands,
            before,
            after,
        });
        if self.undo.len() > self.history_limit {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.revision += 1;
    }

    /// Reverts the most recent step and returns it.
    pub fn undo(&mut self) -> Result<&HistoryEntry, ModelError> {
        let entry = self.undo.pop_back().ok_or(ModelError::NothingToUndo)?;
        self.project = entry.before.clone();
        self.revision += 1;
        self.redo.push(entry);
        Ok(&self.redo[self.redo.len() - 1])
    }

    /// Re-applies the most recently undone step and returns it.
    pub fn redo(&mut self) -> Result<&HistoryEntry, ModelError> {
        let entry = self.redo.pop().ok_or(ModelError::NothingToRedo)?;
        self.project = entry.after.clone();
        self.revision += 1;
        self.undo.push_back(entry);
        Ok(&self.undo[self.undo.len() - 1])
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Label of the step [`CommandBus::undo`] would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.back().map(|entry| entry.label.as_str())
    }

    /// Label of the step [`CommandBus::redo`] would re-apply.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|entry| entry.label.as_str())
    }

    /// Number of steps that can be undone.
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Replaces the project (for example after loading) and clears history.
    pub fn replace_project(&mut self, project: Project) {
        self.project = project;
        self.undo.clear();
        self.redo.clear();
        self.revision += 1;
    }
}

/// A group of commands that commit or fail together. See
/// [`CommandBus::transaction`].
#[derive(Debug)]
pub struct Transaction {
    project: Project,
    commands: Vec<Command>,
}

impl Transaction {
    /// The project as the transaction's commands have left it so far.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Validates and applies one command inside the transaction.
    pub fn execute(&mut self, command: impl Into<Command>) -> Result<Outcome, ModelError> {
        let command = command.into();
        let (after, outcome) = command.apply(&self.project)?;
        self.project = after;
        self.commands.push(command);
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use fourfloor_core::TrackId;

    use super::*;
    use crate::command::{CreateTrack, RenameTrack, SetTempo};
    use crate::track::TrackKind;

    fn create(name: &str) -> CreateTrack {
        CreateTrack {
            name: name.to_owned(),
            kind: TrackKind::Midi,
            index: None,
        }
    }

    #[test]
    fn undo_and_redo_restore_exact_project_values() {
        let mut bus = CommandBus::new(Project::new("Test"));
        let empty = bus.project().clone();
        bus.execute(create("Kick")).unwrap();
        let one = bus.project().clone();
        bus.execute(SetTempo { bpm: 124.0 }).unwrap();
        let two = bus.project().clone();

        assert_eq!(bus.undo().unwrap().label, "set_tempo");
        assert_eq!(bus.project(), &one);
        assert_eq!(bus.undo().unwrap().label, "create_track");
        assert_eq!(bus.project(), &empty);
        assert_eq!(bus.undo().unwrap_err(), ModelError::NothingToUndo);

        bus.redo().unwrap();
        assert_eq!(bus.project(), &one);
        bus.redo().unwrap();
        assert_eq!(bus.project(), &two);
        assert_eq!(bus.redo().unwrap_err(), ModelError::NothingToRedo);
    }

    #[test]
    fn a_new_command_clears_redo() {
        let mut bus = CommandBus::new(Project::new("Test"));
        bus.execute(create("Kick")).unwrap();
        bus.undo().unwrap();
        assert!(bus.can_redo());
        assert_eq!(bus.redo_label(), Some("create_track"));
        bus.execute(create("Hats")).unwrap();
        assert!(!bus.can_redo());
        assert_eq!(bus.undo_label(), Some("create_track"));
    }

    #[test]
    fn redo_reuses_the_same_ids() {
        let mut bus = CommandBus::new(Project::new("Test"));
        let first = bus.execute(create("Kick")).unwrap().track().unwrap();
        bus.undo().unwrap();
        bus.redo().unwrap();
        assert_eq!(bus.project().tracks[0].id, first);
        let second = bus.execute(create("Hats")).unwrap().track().unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn a_failed_command_changes_nothing() {
        let mut bus = CommandBus::new(Project::new("Test"));
        bus.execute(create("Kick")).unwrap();
        let before = bus.project().clone();
        let revision = bus.revision();
        let error = bus.execute(SetTempo { bpm: 5.0 }).unwrap_err();
        assert_eq!(error, ModelError::InvalidTempo { bpm: 5.0 });
        assert_eq!(bus.project(), &before);
        assert_eq!(bus.revision(), revision);
        assert_eq!(bus.undo_depth(), 1);
    }

    #[test]
    fn a_transaction_is_one_undo_step() {
        let mut bus = CommandBus::new(Project::new("Test"));
        let empty = bus.project().clone();
        let track = bus
            .transaction("make kick", |tx| {
                let track = tx.execute(create("Kick"))?.track().unwrap();
                tx.execute(RenameTrack {
                    track,
                    name: "Kick 909".to_owned(),
                })?;
                assert_eq!(tx.project().tracks[0].name, "Kick 909");
                Ok(track)
            })
            .unwrap();
        assert_eq!(bus.project().track(track).unwrap().name, "Kick 909");
        assert_eq!(bus.undo_depth(), 1);
        let entry = bus.undo().unwrap();
        assert_eq!(entry.label, "make kick");
        assert_eq!(entry.commands.len(), 2);
        assert_eq!(bus.project(), &empty);
    }

    #[test]
    fn a_failed_transaction_rolls_back_everything() {
        let mut bus = CommandBus::new(Project::new("Test"));
        let empty = bus.project().clone();
        let result = bus.transaction("broken", |tx| {
            tx.execute(create("Kick"))?;
            tx.execute(RenameTrack {
                track: TrackId::new(999),
                name: "Nope".to_owned(),
            })
        });
        assert_eq!(
            result.unwrap_err(),
            ModelError::TrackNotFound(TrackId::new(999))
        );
        assert_eq!(bus.project(), &empty);
        assert!(!bus.can_undo());
    }

    #[test]
    fn an_empty_transaction_leaves_no_history() {
        let mut bus = CommandBus::new(Project::new("Test"));
        bus.transaction("nothing", |_| Ok(())).unwrap();
        assert!(!bus.can_undo());
        assert_eq!(bus.revision(), 0);
    }

    #[test]
    fn history_is_capped_and_drops_the_oldest_step() {
        let mut bus = CommandBus::with_history_limit(Project::new("Test"), 2);
        for bpm in [121.0, 122.0, 123.0] {
            bus.execute(SetTempo { bpm }).unwrap();
        }
        assert_eq!(bus.undo_depth(), 2);
        bus.undo().unwrap();
        bus.undo().unwrap();
        assert_eq!(bus.project().tempo.bpm(), 121.0);
        assert!(!bus.can_undo());
    }

    #[test]
    fn untouched_tracks_are_shared_between_snapshots() {
        let mut bus = CommandBus::new(Project::new("Test"));
        bus.execute(create("Kick")).unwrap();
        let hats = bus.execute(create("Hats")).unwrap().track().unwrap();
        let before = bus.project().clone();
        bus.execute(RenameTrack {
            track: hats,
            name: "Open hats".to_owned(),
        })
        .unwrap();
        let after = bus.project();
        assert!(std::sync::Arc::ptr_eq(&before.tracks[0], &after.tracks[0]));
        assert!(!std::sync::Arc::ptr_eq(&before.tracks[1], &after.tracks[1]));
    }

    #[test]
    fn replace_project_clears_history_and_bumps_revision() {
        let mut bus = CommandBus::new(Project::new("Test"));
        bus.execute(create("Kick")).unwrap();
        bus.replace_project(Project::new("Loaded"));
        assert!(!bus.can_undo() && !bus.can_redo());
        assert_eq!(bus.project().name, "Loaded");
        assert_eq!(bus.revision(), 2);
    }
}
