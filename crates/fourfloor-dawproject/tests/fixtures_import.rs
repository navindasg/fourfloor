//! Import of DAWproject files fourfloor did not write: the example from the
//! official README (verbatim) and a hand-written file using the parts of
//! the format fourfloor has to flatten, approximate or report.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::{registry, render};
use fourfloor_core::{SampleRate, Ticks};
use fourfloor_dawproject::container::Container;
use fourfloor_dawproject::{import_project, ImportOptions, Outcome, Subject};
use fourfloor_model::{AudioPool, ClipContent, MixerParam, PluginStateStore, Track, TrackKind};
use fourfloor_testkit::{dominant_frequency, sine, StereoBuffer};

const README_STATE: &str = "plugins/d19b1f6e-bbb6-42fe-a6c9-54b41d97a05d.clap-preset";
const README_AUDIO: &str = "audio/Drumfunk3 170bpm.wav";

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

fn archive(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut container = Container::new();
    for (name, bytes) in entries {
        container.insert(name, bytes.clone()).unwrap();
    }
    container.to_bytes().unwrap()
}

/// The README's drum loop: 2.8235 s of stereo 48 kHz audio (a 220 Hz tone
/// stands in for the drums).
fn readme_audio() -> Vec<u8> {
    let rate = SampleRate::new(48_000).unwrap();
    let tone = sine(220.0, 0.5, 2.823_541_666_666_667, rate);
    let buffer = StereoBuffer::from_mono(rate, &tone);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loop.wav");
    buffer.write_wav(&path).unwrap();
    std::fs::read(path).unwrap()
}

fn readme_archive() -> Vec<u8> {
    archive(&[
        ("project.xml", fixture("bitwig-readme-example.xml")),
        (README_STATE, b"surge preset bytes".to_vec()),
        (README_AUDIO, readme_audio()),
    ])
}

fn db(linear: f64) -> f32 {
    (20.0 * linear.log10()) as f32
}

#[test]
fn the_official_readme_example_imports() {
    let dir = tempfile::tempdir().unwrap();
    let imported = import_project(&readme_archive(), dir.path(), &ImportOptions::new()).unwrap();
    let project = &imported.project;
    project.validate().unwrap();
    assert!((project.tempo.bpm() - 149.0).abs() < 1e-9);

    // The unity master adds no track.
    let names: Vec<&str> = project.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Bass", "Drumloop"]);
    let (bass, drums) = (&project.tracks[0], &project.tracks[1]);
    assert_eq!(bass.kind, TrackKind::Midi);
    assert_eq!(drums.kind, TrackKind::Audio);
    assert!(
        (bass.gain_db - db(0.659_140)).abs() < 0.01,
        "{}",
        bass.gain_db
    );
    assert!(
        (drums.gain_db - db(0.177_125)).abs() < 0.01,
        "{}",
        drums.gain_db
    );
    assert!(bass.pan.abs() < 1e-6 && !bass.mute && bass.output.is_none());

    // The CLAP plugin reference, with its state in the project's store.
    assert_eq!(bass.devices.len(), 1);
    let surge = &bass.devices[0];
    assert_eq!(surge.kind, "plugin.clap:org.surge-synth-team.surge-xt");
    assert_eq!(surge.name, "Surge XT");
    assert!(!surge.bypassed);
    let blob = surge.state_blob.clone().expect("state blob");
    assert_eq!(
        PluginStateStore::new(dir.path()).get(&blob).unwrap(),
        b"surge preset bytes"
    );

    // The note clip: 8 beats, 9 notes, velocity 0.787402 = 100/127.
    assert_eq!(bass.clips.len(), 1);
    let clip = &bass.clips[0];
    assert_eq!(
        (clip.start, clip.length),
        (Ticks::ZERO, Ticks::new(8 * 960))
    );
    let ClipContent::Midi { notes } = &clip.content else {
        panic!("not a MIDI clip")
    };
    assert_eq!(notes.len(), 9);
    assert!(notes.iter().all(|n| n.velocity == 100));
    let long = notes.iter().find(|n| n.start == Ticks::new(1_440)).unwrap();
    assert_eq!((long.pitch, long.length), (53, Ticks::new(2_400)));

    // The warped audio clip: 8 beats over 2.8235 s is 170 BPM, stretched
    // (so key-locked), and the file is in the pool.
    assert_eq!(drums.clips.len(), 1);
    let clip = &drums.clips[0];
    assert_eq!(clip.name, "Drumfunk3 170bpm");
    let ClipContent::Audio { warp, .. } = &clip.content else {
        panic!("not an audio clip")
    };
    let warp = warp.expect("warp");
    assert!(
        (warp.source_bpm - 170.0).abs() < 0.01,
        "{}",
        warp.source_bpm
    );
    assert!(warp.key_lock);
    assert!(AudioPool::new(dir.path()).missing(project).is_empty());

    // Plugin references are kept, so nothing is lost outright.
    assert!(
        imported
            .report
            .with_outcome(Outcome::Dropped)
            .next()
            .is_none(),
        "{}",
        imported.report
    );
    assert!(imported
        .report
        .with_outcome(Outcome::Missing)
        .next()
        .is_none());
}

#[test]
fn the_readme_example_without_its_media_reports_what_is_missing() {
    let bytes = archive(&[("project.xml", fixture("bitwig-readme-example.xml"))]);
    let dir = tempfile::tempdir().unwrap();
    let imported = import_project(&bytes, dir.path(), &ImportOptions::new()).unwrap();
    imported.project.validate().unwrap();
    assert!(imported.report.has(Outcome::Missing, Subject::PluginState));
    assert!(imported.report.has(Outcome::Missing, Subject::Audio));
    assert!(imported.project.tracks[0].devices[0].state_blob.is_none());
}

#[test]
fn the_readme_drum_loop_renders_at_the_project_tempo() {
    // The plugin is not installed, so the compiler would play the bass
    // notes on its default instrument; the bass is muted so that only the
    // 220 Hz loop sounds, warped to 8 beats at 149 BPM.
    let registry = registry();
    let dir = tempfile::tempdir().unwrap();
    let imported = import_project(&readme_archive(), dir.path(), &ImportOptions::new()).unwrap();
    let mut project = imported.project.clone();
    project.tracks[0] = Arc::new(Track {
        mute: true,
        ..(*project.tracks[0]).clone()
    });
    let rendered = render(&project, &registry, dir.path());
    let seconds = 8.0 * 60.0 / 149.0;
    let body = rendered.slice_seconds(0.2, seconds - 0.2);
    let pitch = dominant_frequency(&body.mono(), body.sample_rate);
    // Without the `stretch` feature fourfloor-compile resamples instead of
    // key-locking, which moves the pitch by 149/170.
    let resampled = 220.0 * 149.0 / 170.0;
    assert!(
        (pitch - 220.0).abs() < 3.0 || (pitch - resampled).abs() < 3.0,
        "pitch {pitch} Hz"
    );
    let after = rendered.slice_seconds(seconds + 0.1, seconds + 0.5);
    assert!(fourfloor_testkit::is_silent(&after.mono(), -60.0));
}

#[test]
fn a_hand_written_file_is_flattened_and_everything_lost_is_reported() {
    let bytes = archive(&[("project.xml", fixture("handwritten.xml"))]);
    let dir = tempfile::tempdir().unwrap();
    let imported = import_project(&bytes, dir.path(), &ImportOptions::new()).unwrap();
    let project = &imported.project;
    let report = &imported.report;
    project.validate().unwrap();
    assert!((project.tempo.bpm() - 122.5).abs() < 1e-9);
    assert_eq!(imported.project.name, "Imported");

    let by_name = |name: &str| {
        project
            .tracks
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("no track {name}: {report}"))
    };
    let (group, kick, echo) = (by_name("Drums"), by_name("Kick"), by_name("Echo Return"));
    assert_eq!(project.tracks.len(), 3, "{report}");

    // The group became a bus its child feeds; the return is a bus too.
    assert_eq!(group.kind, TrackKind::Bus);
    assert_eq!(echo.kind, TrackKind::Bus);
    assert_eq!(kick.kind, TrackKind::Midi);
    assert_eq!(kick.output, Some(group.id));
    assert!(group.output.is_none());
    assert!((group.gain_db - db(0.5)).abs() < 0.01);

    // Channel settings and the pre-fader send.
    assert!(kick.mute);
    assert!((kick.pan - -0.5).abs() < 1e-6, "{}", kick.pan);
    assert_eq!(kick.sends.len(), 1);
    let send = kick.sends[0];
    assert_eq!(send.target, echo.id);
    assert!(send.pre_fader);
    assert!((send.gain_db - db(0.25)).abs() < 0.01);

    // A foreign built-in device is kept as a bypassed placeholder with its
    // parameter, and reported.
    assert_eq!(kick.devices.len(), 1);
    let eq = &kick.devices[0];
    assert!(eq.kind.starts_with("dawproject.equalizer:"), "{}", eq.kind);
    assert!(eq.bypassed);
    assert!(eq
        .params
        .iter()
        .any(|p| p.name.contains("Freq") && (p.value - 40.0).abs() < 1e-3));
    assert!(report.has(Outcome::Preserved, Subject::Device), "{report}");

    // The looped clip: placed at beat 4, 6 beats long, 2-beat loop.
    assert_eq!(kick.clips.len(), 1);
    let clip = &kick.clips[0];
    assert_eq!(clip.start, Ticks::new(4 * 960));
    assert_eq!(clip.length, Ticks::new(6 * 960));
    assert_eq!(clip.loop_length, Some(Ticks::new(2 * 960)));
    let ClipContent::Midi { notes } = &clip.content else {
        panic!("not MIDI")
    };
    let velocities: Vec<u8> = notes.iter().map(|n| n.velocity).collect();
    assert_eq!(velocities, [127, 64]);

    // Volume automation became a fader gain lane in dB.
    let lane = kick
        .mixer_automation
        .iter()
        .find(|l| l.param == MixerParam::Gain)
        .unwrap_or_else(|| panic!("no gain lane: {report}"));
    let first = lane.points.first().unwrap();
    let last = lane.points.last().unwrap();
    assert!(first.value.abs() < 0.01 && first.time == Ticks::ZERO);
    assert!((last.value - db(0.5)).abs() < 0.01 && last.time == Ticks::new(8 * 960));

    // What fourfloor cannot hold is listed, not silently dropped.
    assert!(
        report.has(Outcome::Approximated, Subject::Transport),
        "{report}"
    );
    assert!(
        report.has(Outcome::Dropped, Subject::Automation),
        "{report}"
    );
    assert!(report.has(Outcome::Dropped, Subject::Marker), "{report}");
    for item in &report.items {
        assert!(
            !item.message.is_empty() && !item.location.is_empty(),
            "{item}"
        );
    }
}

#[test]
fn malformed_archives_are_errors_not_panics() {
    let dir = tempfile::tempdir().unwrap();
    let options = ImportOptions::new();
    assert!(import_project(b"not a zip", dir.path(), &options).is_err());
    let empty = archive(&[("metadata.xml", b"<MetaData/>".to_vec())]);
    assert!(import_project(&empty, dir.path(), &options).is_err());
    let wrong_root = archive(&[("project.xml", b"<Song/>".to_vec())]);
    assert!(import_project(&wrong_root, dir.path(), &options).is_err());
    let broken = archive(&[(
        "project.xml",
        b"<Project version=\"1.0\"><Structure>".to_vec(),
    )]);
    assert!(import_project(&broken, dir.path(), &options).is_err());
    let doctype = archive(&[(
        "project.xml",
        b"<!DOCTYPE p [<!ENTITY x \"y\">]><Project version=\"1.0\"/>".to_vec(),
    )]);
    assert!(import_project(&doctype, dir.path(), &options).is_err());
    // An empty but valid project imports as an empty project.
    let bare = archive(&[("project.xml", b"<Project version=\"1.0\"/>".to_vec())]);
    let imported = import_project(&bare, dir.path(), &options).unwrap();
    assert!(imported.project.tracks.is_empty());
}
