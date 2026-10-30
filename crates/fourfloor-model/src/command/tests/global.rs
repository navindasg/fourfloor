//! Tempo, swing and project name command tests.

use super::*;

#[test]
fn set_tempo_validates_the_range() {
    let mut f = fixture();
    round_trip(&mut f.bus, SetTempo { bpm: 124.0 });
    assert_eq!(f.bus.project().tempo, Tempo::new(124.0).unwrap());
    for bpm in [19.9, 1_000.0, f64::NAN, f64::INFINITY, -120.0] {
        let error = rejected(&mut f.bus, SetTempo { bpm });
        assert!(matches!(error, ModelError::InvalidTempo { .. }));
    }
}

#[test]
fn set_swing_updates_the_groove_and_keeps_the_grid_when_omitted() {
    let mut f = fixture();
    round_trip(
        &mut f.bus,
        SetSwing {
            percent: 58.0,
            grid: Some(SwingGrid::Eighth),
        },
    );
    round_trip(
        &mut f.bus,
        SetSwing {
            percent: 62.0,
            grid: None,
        },
    );
    assert_eq!(
        f.bus.project().groove,
        Groove {
            swing_percent: 62.0,
            grid: SwingGrid::Eighth
        }
    );
    for percent in [49.9, 75.1, f64::NAN] {
        let error = rejected(
            &mut f.bus,
            SetSwing {
                percent,
                grid: None,
            },
        );
        assert!(matches!(error, ModelError::InvalidSwing { .. }));
    }
}

#[test]
fn rename_project_changes_the_name() {
    let mut f = fixture();
    round_trip(
        &mut f.bus,
        RenameProject {
            name: "Warehouse".to_owned(),
        },
    );
    assert_eq!(f.bus.project().name, "Warehouse");
    let error = rejected(
        &mut f.bus,
        RenameProject {
            name: "\n".to_owned(),
        },
    );
    assert!(matches!(
        error,
        ModelError::InvalidName {
            field: "project name",
            ..
        }
    ));
}

#[test]
fn applying_a_command_leaves_the_input_project_untouched() {
    let f = fixture();
    let project = f.bus.project().clone();
    let copy = project.clone();
    let (updated, _) = Command::from(SetTempo { bpm: 128.0 })
        .apply(&project)
        .unwrap();
    assert_eq!(project, copy);
    assert_ne!(updated, project);
}
