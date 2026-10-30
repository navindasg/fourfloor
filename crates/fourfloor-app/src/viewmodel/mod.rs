//! Editing logic behind the views, kept apart from painting so it can be
//! tested without a display: each module turns gestures into model
//! commands.

pub mod arrange;
pub mod assistant;
pub mod browser;
pub mod dj;
pub mod mappings;
pub mod mixer;
pub mod notes;
pub mod params;
pub mod steps;

use fourfloor_model::{Command, CommandBus, ModelError, Project, Transaction};

/// Runs `plan` on a scratch copy of `project` and returns the commands it
/// executed, in order: a multi-command edit whose later commands need ids
/// the earlier ones create (a new track, then a clip on it), ready to send
/// to the real bus as one transaction.
pub fn planned_commands(
    project: &Project,
    plan: impl FnOnce(&mut Transaction) -> Result<(), ModelError>,
) -> Result<Vec<Command>, ModelError> {
    let mut bus = CommandBus::new(project.clone());
    bus.transaction("plan", plan)?;
    Ok(bus
        .undo()
        .map(|entry| entry.commands.clone())
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::command::{CreateTrack, RenameTrack};
    use fourfloor_model::TrackKind;

    #[test]
    fn a_plan_replays_to_the_same_project() {
        let project = Project::new("Test");
        let commands = planned_commands(&project, |tx| {
            let track = tx
                .execute(CreateTrack {
                    name: "Stem".to_owned(),
                    kind: TrackKind::Audio,
                    index: None,
                })?
                .track()
                .ok_or(ModelError::NothingToUndo)?;
            tx.execute(RenameTrack {
                track,
                name: "Vocals".to_owned(),
            })?;
            Ok(())
        })
        .expect("plans");
        assert_eq!(commands.len(), 2);
        let mut bus = CommandBus::new(project);
        bus.transaction("apply", |tx| {
            for command in commands {
                tx.execute(command)?;
            }
            Ok(())
        })
        .expect("applies");
        assert_eq!(bus.project().tracks[0].name, "Vocals");
        assert!(planned_commands(bus.project(), |_| Ok(()))
            .expect("empty")
            .is_empty());
    }
}
