//! The registry bridge: a project device of kind `plugin.clap:<id>` compiles
//! into a hosted plugin and is heard.

mod common;

use std::sync::Arc;

use common::{fixture, PARAM, RATE};
use fourfloor_compile::{compile_project, render_project, CompileOptions, RenderOptions};
use fourfloor_core::{
    DeviceCategory, DeviceId, DeviceInit, DeviceRegistry, RegistryError, Ticks, TrackId,
};
use fourfloor_host::thread::HostThread;
use fourfloor_host::{parse_plugin_kind, PluginDatabase, PluginFormat, PluginHost, ScanOptions};
use fourfloor_model::command::{AddDevice, CreateTrack, SetDeviceState, WriteMidiClip};
use fourfloor_model::{CommandBus, MidiNote, Param, PluginStateStore, Project, TrackKind};
use fourfloor_testkit::{dominant_frequency, peak, AllocDisabler};

// Project renders run under the allocation guard: a host-side allocation
// in `process` aborts the test binary.
#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

const SINE_KIND: &str = "plugin.clap:org.fourfloor.test.sine";
const GAIN_KIND: &str = "plugin.clap:org.fourfloor.test.gain";
const DELAY_KIND: &str = "plugin.clap:org.fourfloor.test.delay";

/// A host, the scanned fixture database and a registry holding it.
fn hosted_registry() -> (Arc<PluginHost>, PluginDatabase, DeviceRegistry) {
    let host = PluginHost::with_thread(HostThread::spawn().unwrap());
    let options = ScanOptions::none().with_clap_dir(&fixture().dir);
    let database = PluginDatabase::scan(host.thread(), &options);
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry).unwrap();
    assert_eq!(host.register_database(&mut registry, &database).unwrap(), 3);
    (host, database, registry)
}

/// One MIDI track with one A4 note lasting a bar (two seconds at 120 BPM).
fn one_note() -> (CommandBus, TrackId) {
    let mut bus = CommandBus::new(Project::new("Test"));
    let track = bus
        .execute(CreateTrack {
            name: "Lead".to_owned(),
            kind: TrackKind::Midi,
            index: None,
        })
        .unwrap()
        .track()
        .unwrap();
    bus.execute(WriteMidiClip {
        track,
        clip: None,
        name: "Note".to_owned(),
        start: Ticks::ZERO,
        length: Ticks::from_whole_beats(4),
        notes: vec![MidiNote {
            start: Ticks::ZERO,
            length: Ticks::from_whole_beats(4),
            pitch: 69,
            velocity: 127,
        }],
    })
    .unwrap();
    (bus, track)
}

fn add_device(bus: &mut CommandBus, track: TrackId, kind: &str, params: Vec<Param>) -> DeviceId {
    bus.execute(AddDevice {
        track,
        kind: kind.to_owned(),
        name: None,
        index: None,
        params,
    })
    .unwrap()
    .device()
    .unwrap()
}

fn gain_param(value: f32) -> Param {
    Param {
        id: PARAM,
        name: "Gain".to_owned(),
        value,
        min: 0.0,
        max: 4.0,
    }
}

/// Peak of the held part of the rendered note.
fn held_peak(project: &Project, registry: &DeviceRegistry, options: &RenderOptions) -> f32 {
    let rendered = render_project(project, registry, options).unwrap();
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    let left = rendered.channel(0);
    let found = dominant_frequency(&left, RATE);
    assert!((found - 440.0).abs() < 1.0, "{found} Hz");
    peak(&left[4800..48_000])
}

#[test]
fn scanned_plugins_are_registered_with_their_descriptors() {
    let (host, database, mut registry) = hosted_registry();
    let sine = registry.descriptor(SINE_KIND).unwrap();
    assert_eq!(
        (sine.name.as_str(), sine.category),
        ("Fourfloor Test Sine", DeviceCategory::Instrument)
    );
    assert_eq!(sine.param_by_name("Level").map(|p| p.id), Some(PARAM));
    let gain = registry.descriptor(GAIN_KIND).unwrap();
    assert_eq!(gain.category, DeviceCategory::Effect);
    assert_eq!(
        gain.param(PARAM).map(|p| (p.min, p.max, p.default)),
        Some((0.0, 4.0, 1.0))
    );
    assert!(registry.contains(DELAY_KIND));
    for descriptor in registry
        .descriptors()
        .filter(|d| d.kind.starts_with("plugin."))
    {
        let (format, id) = parse_plugin_kind(&descriptor.kind).unwrap();
        assert_eq!(format, PluginFormat::Clap);
        assert!(database
            .find(&descriptor.kind)
            .is_some_and(|info| info.id == id));
    }

    // Registering the same database again adds nothing and is not an error.
    assert_eq!(host.register_database(&mut registry, &database).unwrap(), 0);

    // A plugin this build cannot load is listed but never registered.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Some Reverb.vst3/Contents")).unwrap();
    let with_vst3 = PluginDatabase::scan(
        host.thread(),
        &ScanOptions::none().with_vst3_dir(dir.path()),
    );
    let listed = with_vst3.find("plugin.vst3:file:Some%20Reverb").unwrap();
    assert!(!listed.loadable);
    assert_eq!(
        host.register_database(&mut registry, &with_vst3).unwrap(),
        0
    );
    assert!(!registry.contains(&listed.kind()));
    assert!(host.load(listed, None).is_err());
}

#[test]
fn a_project_with_plugin_devices_compiles_and_is_heard() {
    let (host, _database, registry) = hosted_registry();
    let (mut bus, track) = one_note();
    add_device(&mut bus, track, SINE_KIND, Vec::new());
    let options = RenderOptions::new(RATE).with_tail(0.0);

    // The sine plugin alone: A4 at its default level of 0.5.
    let alone = held_peak(bus.project(), &registry, &options);
    assert!((alone - 0.5).abs() < 0.01, "peak {alone}");

    // Followed by the gain plugin with a parameter from the project.
    let gain = add_device(&mut bus, track, GAIN_KIND, vec![gain_param(0.5)]);
    let halved = held_peak(bus.project(), &registry, &options);
    assert!((halved - 0.25).abs() < 0.005, "peak {halved}");

    // The compiled graph holds the plugin under the project's device id,
    // and the host tracks one controller per live plugin, in chain order.
    let compiled =
        compile_project(bus.project(), &registry, &CompileOptions::new(RATE, 512)).unwrap();
    assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
    assert!(compiled.graph.contains(gain));
    let kinds: Vec<String> = host
        .controllers()
        .iter()
        .map(|c| c.kind().to_owned())
        .collect();
    assert_eq!(kinds, [SINE_KIND, GAIN_KIND]);
    drop(compiled);
    assert!(
        host.controllers().is_empty(),
        "controllers outlived their devices"
    );
}

#[test]
fn plugin_state_saved_in_the_project_folder_is_restored_on_compile() {
    let (host, _database, registry) = hosted_registry();
    let (mut bus, track) = one_note();
    add_device(&mut bus, track, SINE_KIND, Vec::new());
    let gain = add_device(&mut bus, track, GAIN_KIND, Vec::new());

    // What the app does on save: ask the running plugin for its state and
    // put the blob in the project's plugin-state store.
    let compile = CompileOptions::new(RATE, 512);
    let compiled = compile_project(bus.project(), &registry, &compile).unwrap();
    let controllers = host.controllers();
    let controller = controllers.iter().find(|c| c.kind() == GAIN_KIND).unwrap();
    let mut live = compiled.graph;
    assert!(live.contains(gain));
    // Turn the running plugin down to 0.125 through the graph, then save.
    live.set_param(gain, PARAM, 0.125);
    let _ = fourfloor_testkit::render_graph(live, bus.project().tempo, 0.1).unwrap();
    assert_eq!(controller.param_value(PARAM), Some(0.125));
    let blob = controller.save_state().unwrap();
    drop(controllers);

    let dir = tempfile::tempdir().unwrap();
    let hash = PluginStateStore::new(dir.path()).put(&blob).unwrap();
    bus.execute(SetDeviceState {
        device: gain,
        state_blob: Some(hash),
    })
    .unwrap();

    let options = RenderOptions {
        compile: compile.with_project_dir(dir.path()),
        ..RenderOptions::new(RATE).with_tail(0.0)
    };
    let restored = held_peak(bus.project(), &registry, &options);
    assert!((restored - 0.5 * 0.125).abs() < 0.002, "peak {restored}");
}

#[test]
fn a_plugin_whose_file_has_gone_is_skipped_with_a_warning() {
    let (host, mut database, _registry) = hosted_registry();
    for plugin in &mut database.plugins {
        plugin.path = fixture().dir.join("moved-away.clap");
    }
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry).unwrap();
    host.register_database(&mut registry, &database).unwrap();

    let error = registry
        .create(GAIN_KIND, &DeviceInit::new())
        .err()
        .unwrap();
    assert!(
        matches!(&error, RegistryError::Build { kind, .. } if kind == GAIN_KIND),
        "{error:?}"
    );
    assert!(error.to_string().contains("moved-away.clap"), "{error}");

    let (mut bus, track) = one_note();
    add_device(&mut bus, track, GAIN_KIND, Vec::new());
    let compiled =
        compile_project(bus.project(), &registry, &CompileOptions::new(RATE, 512)).unwrap();
    assert_eq!(compiled.warnings.len(), 1, "{:?}", compiled.warnings);
}
