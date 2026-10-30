//! The sample browser's services, measured on audio: the factory sounds
//! scan into the library and filter as expected, auditions play through
//! the app's output at the project tempo and in the chosen key, and files
//! added to a track or a sampler render as they sound.

mod library_common;

use fourfloor_analysis::Key;
use fourfloor_app::state::{FileIntent, Intent, LibraryIntent};
use fourfloor_core::Ticks;
use fourfloor_library::preview::key_shift;
use fourfloor_library::{FileKind, PreviewMode, Query};
use fourfloor_model::command::{SetTempo, WriteMidiClip};
use fourfloor_model::{ClipContent, Command, MidiNote, TrackKind};
use fourfloor_testkit::{dominant_frequency, onset_times, rms};
use library_common::{
    capture, file_id, find, install_factory, library_app, run, sounding_seconds, temp_dir, RATE,
};

#[test]
fn factory_sounds_scan_and_filter() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    install_factory(&mut app);
    let last = app.features.scan.last.clone().expect("a scan summary");
    assert!(last.starts_with("18 added"), "{last}");
    assert_eq!(app.features.library.rows.len(), 18, "all files listed");

    let count = |app: &mut fourfloor_app::FourfloorApp, query: Query| {
        run(app, LibraryIntent::Search(query));
        app.features.library.rows.clone()
    };
    assert_eq!(
        count(&mut app, Query::new().kind(FileKind::OneShot)).len(),
        13
    );
    let loops = count(&mut app, Query::new().kind(FileKind::Loop));
    assert_eq!(loops.len(), 5);
    let tempo = count(&mut app, Query::new().bpm_range(122.0, 126.0));
    assert!(
        tempo
            .iter()
            .all(|r| r.record.bpm.is_some_and(|b| (122.0..=126.0).contains(&b))),
        "tempo filter"
    );
    assert!(tempo.len() >= 5, "the 124 BPM loops: {}", tempo.len());
    let kicks = count(&mut app, Query::new().text("kick"));
    assert!(kicks.len() >= 2, "kick 909 and 808");
    assert!(kicks
        .iter()
        .all(|r| r.record.title.to_lowercase().contains("kick")));
    let a_minor = count(&mut app, Query::new().key(Key::minor(9)));
    assert!(
        a_minor.iter().all(|r| r.record.key == Some(Key::minor(9))),
        "key filter"
    );
    assert!(!a_minor.is_empty(), "the Am bass, stab and pad");
    let by_length = count(
        &mut app,
        Query::new()
            .sort_by(fourfloor_library::SortKey::Duration)
            .descending(),
    );
    let lengths: Vec<f64> = by_length
        .iter()
        .map(|r| r.record.duration_seconds)
        .collect();
    assert!(lengths.windows(2).all(|w| w[0] >= w[1]), "{lengths:?}");
}

#[test]
fn find_similar_puts_kicks_near_a_kick() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    install_factory(&mut app);
    let kick = file_id(&app, "kick 909", FileKind::OneShot);
    run(&mut app, LibraryIntent::FindSimilar(kick, Query::new()));
    let rows = &app.features.library.rows;
    assert!(!rows.iter().any(|r| r.record.id == kick), "itself left out");
    let titles: Vec<String> = rows.iter().map(|r| r.record.title.to_lowercase()).collect();
    let rank = |word: &str| titles.iter().position(|t| t.contains(word));
    let kick = rank("kick").expect("the other kick is listed");
    for other in ["hat", "clap", "snare"] {
        if let Some(found) = rank(other) {
            assert!(kick < found, "kick 808 ranks above the {other}: {titles:?}");
        }
    }
    assert!(kick < 3, "the other kick is near the top: {titles:?}");
    let scores: Vec<f32> = rows.iter().filter_map(|r| r.score).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{scores:?}");
}

#[test]
fn a_loop_auditions_at_the_project_tempo_through_the_output() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    install_factory(&mut app);
    let drums = find(&app, "drum", FileKind::Loop);
    let source_bpm = drums.bpm.expect("loop tempo");
    assert!((source_bpm - 124.0).abs() < 0.5, "{source_bpm}");

    run(
        &mut app,
        Intent::Command(Command::SetTempo(SetTempo { bpm: 128.0 })),
    );
    run(
        &mut app,
        LibraryIntent::Audition(drums.id, PreviewMode::Original, None),
    );
    assert_eq!(app.features.auditioning, Some(drums.id));
    let original = capture(&mut app, 9.0);
    run(
        &mut app,
        LibraryIntent::Audition(drums.id, PreviewMode::KeyLocked, None),
    );
    let locked = capture(&mut app, 9.0);

    let original_len = sounding_seconds(&original);
    let locked_len = sounding_seconds(&locked);
    assert!(
        (original_len - drums.duration_seconds).abs() < 0.1,
        "original plays its {:.2} s: {original_len:.2}",
        drums.duration_seconds
    );
    let ratio = locked_len / original_len;
    assert!(
        (ratio - source_bpm / 128.0).abs() < 0.01,
        "key-locked at 128 BPM: {locked_len:.3} s against {original_len:.3} s"
    );
    // Every hit of the loop lands where the faster tempo puts it.
    let hits = onset_times(&locked.mono(), RATE);
    let wanted: Vec<f64> = onset_times(&original.mono(), RATE)
        .into_iter()
        .map(|t| t * source_bpm / 128.0)
        .collect();
    let matched = wanted
        .iter()
        .filter(|t| hits.iter().any(|h| (h - *t).abs() < 0.012))
        .count();
    assert!(
        wanted.len() >= 12 && matched * 10 >= wanted.len() * 9,
        "{matched} of {} hits moved to 128 BPM (found {})",
        wanted.len(),
        hits.len()
    );

    run(&mut app, LibraryIntent::StopAudition);
    assert!(app.features.auditioning.is_none());
    let silent = capture(&mut app, 0.5);
    assert!(rms(&silent.mono()) < 1e-4, "stop silences the audition");
}

#[test]
fn a_one_shot_auditions_in_the_chosen_key() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    install_factory(&mut app);
    let bass = find(&app, "bass", FileKind::OneShot);
    let own_key = bass.key.expect("the bass note has a key");
    let plain_hz = 55.0;
    run(
        &mut app,
        LibraryIntent::Audition(bass.id, PreviewMode::Original, None),
    );
    let plain = capture(&mut app, 1.5);
    let measured = f64::from(dominant_frequency(&plain.mono(), RATE));
    assert!((measured - plain_hz).abs() < 2.0, "A1: {measured:.1} Hz");

    let target = Key::minor(0);
    let shift = key_shift(own_key, target);
    assert_ne!(shift, 0, "{own_key} to {target}");
    run(
        &mut app,
        LibraryIntent::Audition(bass.id, PreviewMode::KeyLocked, Some(target)),
    );
    let moved = capture(&mut app, 1.5);
    let expected = measured * 2f64.powf(f64::from(shift) / 12.0);
    let found = f64::from(dominant_frequency(&moved.mono(), RATE));
    assert!(
        (found / expected - 1.0).abs() < 0.03,
        "{shift:+} st: expected {expected:.1} Hz, found {found:.1} Hz"
    );
    let length = sounding_seconds(&moved);
    let original = sounding_seconds(&plain);
    assert!(
        (length - original).abs() < 0.15,
        "a one-shot keeps its length: {length:.2} against {original:.2}"
    );
}

#[test]
fn files_go_onto_tracks_and_into_samplers_and_render() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    install_factory(&mut app);
    // Without a project folder there is nowhere to copy the audio.
    let drums = find(&app, "drum", FileKind::Loop);
    run(&mut app, LibraryIntent::AddToTrack(drums.id));
    assert!(app.project().tracks.is_empty());
    let project_dir = dir.join("song");
    run(
        &mut app,
        Intent::File(FileIntent::SaveAs(project_dir.clone())),
    );

    run(&mut app, LibraryIntent::AddToTrack(drums.id));
    let track = app
        .project()
        .tracks
        .first()
        .expect("a new audio track")
        .clone();
    assert_eq!(track.kind, TrackKind::Audio);
    let clip = track.clips.first().expect("a clip");
    assert!(matches!(clip.content, ClipContent::Audio { .. }));
    let expected = drums.duration_seconds * app.project().tempo.bpm() / 60.0 * 960.0;
    assert!(
        (clip.length.get() as f64 - expected).abs() < 2.0,
        "clip is the file's length"
    );
    let rendered = render(&app, &project_dir);
    let source = fourfloor_testkit::StereoBuffer::read_wav(&drums.path).expect("source");
    let hits = onset_times(&rendered.mono(), RATE);
    let wanted = onset_times(&source.mono(), RATE);
    assert!(
        fourfloor_testkit::times_match(&hits, &wanted, 0.005),
        "the clip plays the loop's hits: {hits:?} vs {wanted:?}"
    );
    run(&mut app, Intent::Undo);
    assert!(app.project().tracks.is_empty(), "one undo step");

    let bass = find(&app, "bass", FileKind::OneShot);
    run(&mut app, LibraryIntent::LoadSampler(bass.id));
    let track = app
        .project()
        .tracks
        .first()
        .expect("a sampler track")
        .clone();
    assert_eq!(track.kind, TrackKind::Midi);
    assert_eq!(track.devices.len(), 1);
    assert_eq!(track.devices[0].kind, "sampler");
    assert!(track.devices[0].state_blob.is_some());
    // Loading again into the selected track replaces the sample.
    let hat = find(&app, "hat", FileKind::OneShot);
    run(&mut app, LibraryIntent::LoadSampler(hat.id));
    let again = app.project().track(track.id).expect("same track").clone();
    assert_eq!(again.devices.len(), 1, "no second sampler");
    assert_ne!(again.devices[0].state_blob, track.devices[0].state_blob);
    run(&mut app, Intent::Undo);
    run(
        &mut app,
        Intent::Command(Command::WriteMidiClip(WriteMidiClip {
            track: track.id,
            clip: None,
            name: "note".to_owned(),
            start: Ticks::new(0),
            length: Ticks::new(1920),
            notes: vec![MidiNote {
                start: Ticks::new(0),
                length: Ticks::new(960),
                pitch: 60,
                velocity: 100,
            }],
        })),
    );
    let rendered = render(&app, &project_dir);
    let hz = dominant_frequency(&rendered.slice_seconds(0.0, 0.45).mono(), RATE);
    assert!(
        (hz - 55.0).abs() < 2.0,
        "the sampler plays A1 at its root: {hz}"
    );
}

fn render(
    app: &fourfloor_app::FourfloorApp,
    dir: &std::path::Path,
) -> fourfloor_testkit::StereoBuffer {
    let mut options = fourfloor_testkit::RenderOptions::new(RATE);
    options.compile = options.compile.with_project_dir(dir);
    options.tail_seconds = 0.5;
    fourfloor_testkit::render_project_with(app.project(), app.registry(), &options)
        .expect("renders")
}
