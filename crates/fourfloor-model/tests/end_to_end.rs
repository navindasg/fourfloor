//! Drives the crate through its public API only, the way the UI or an agent
//! tool layer would: JSON commands in, a saved project folder out.

use fourfloor_model::sequencer::{write_house_pattern, HousePattern, GM_OPEN_HAT};
use fourfloor_model::{
    command_schemas, load_project, save_project, Command, CommandBus, ModelError, Outcome, Project,
};
use serde_json::json;

fn call(bus: &mut CommandBus, name: &str, args: serde_json::Value) -> Result<Outcome, ModelError> {
    bus.execute(Command::from_name_and_args(name, args)?)
}

#[test]
fn an_agent_style_session_builds_saves_and_reloads_a_house_loop() {
    let mut bus = CommandBus::new(Project::new("Untitled"));

    // Every tool an agent could call is listed with a schema.
    let tools: Vec<&str> = command_schemas().iter().map(|tool| tool.name).collect();
    assert!(tools.contains(&"set_swing") && tools.contains(&"write_midi_clip"));

    call(
        &mut bus,
        "rename_project",
        json!({ "name": "Friday night" }),
    )
    .unwrap();
    call(&mut bus, "set_tempo", json!({ "bpm": 124 })).unwrap();
    call(
        &mut bus,
        "set_swing",
        json!({ "percent": 60, "grid": "eighth" }),
    )
    .unwrap();

    let written = write_house_pattern(
        &mut bus,
        &HousePattern {
            bars: 2,
            ..HousePattern::default()
        },
    )
    .unwrap();

    let outcome = call(
        &mut bus,
        "add_device",
        json!({
            "track": written.hat_track,
            "kind": "builtin.filter",
            "params": [{ "id": 1, "name": "Cutoff", "value": 8000, "min": 20, "max": 20000 }]
        }),
    )
    .unwrap();
    let filter = outcome.device().unwrap();
    call(
        &mut bus,
        "set_param",
        json!({ "device": filter, "param": 1, "value": 2500 }),
    )
    .unwrap();

    // A bad call is refused with a typed error and changes nothing.
    let before = bus.project().clone();
    let error = call(
        &mut bus,
        "set_param",
        json!({ "device": filter, "param": 1, "value": 99999 }),
    )
    .unwrap_err();
    assert!(matches!(error, ModelError::ParamOutOfRange { .. }));
    assert_eq!(bus.project(), &before);

    // Save, reload, and compare.
    let dir = tempfile::tempdir().unwrap();
    save_project(dir.path(), bus.project()).unwrap();
    let loaded = load_project(dir.path()).unwrap();
    assert_eq!(&loaded, bus.project());
    assert_eq!(loaded.name, "Friday night");
    assert_eq!(loaded.tempo.bpm(), 124.0);

    // The hats were written with the project's 60% eighth-note swing.
    let (_, hats) = loaded.find_clip(written.hat_clip).unwrap();
    let first_open_hat = hats
        .notes()
        .iter()
        .find(|note| note.pitch == GM_OPEN_HAT)
        .unwrap();
    assert_eq!(first_open_hat.start.get(), 576);

    // Every edit, including the agent's, can be undone back to the start.
    while bus.can_undo() {
        bus.undo().unwrap();
    }
    assert_eq!(bus.project(), &Project::new("Untitled"));
}
