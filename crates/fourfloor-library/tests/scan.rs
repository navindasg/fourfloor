//! Scanning rendered audio and checking what the index says about it.

mod common;

use std::fs;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use common::{scanned, stored, write, write_material};
use fourfloor_library::{FileKind, Library, ScanOptions, ScanPhase, TagSource};

#[test]
fn the_index_holds_what_the_audio_is() {
    let dir = tempfile::tempdir().expect("tempdir");
    let expected = write_material(dir.path());
    let (library, report) = scanned(dir.path());
    assert_eq!(report.added.len(), expected.len(), "{report:?}");
    assert!(report.failed.is_empty(), "{:?}", report.failed);

    for want in &expected {
        let record = library
            .file_by_path(&stored(dir.path(), want.name))
            .expect("lookup")
            .unwrap_or_else(|| panic!("{} not indexed", want.name));
        assert_eq!(record.kind, want.kind, "{}: {record:?}", want.name);
        match want.bpm {
            Some(bpm) => {
                let found = record
                    .bpm
                    .unwrap_or_else(|| panic!("{} has no tempo", want.name));
                assert!(
                    (found - bpm).abs() < 0.1,
                    "{}: {found} BPM, want {bpm}",
                    want.name
                );
            }
            None => assert_eq!(record.bpm, None, "{}", want.name),
        }
        if let Some(key) = want.key {
            assert_eq!(record.key, Some(key), "{}: {record:?}", want.name);
        }
        assert!(
            record.has_tag(want.instrument),
            "{} lacks tag {}: {:?}",
            want.name,
            want.instrument,
            record.tag_names()
        );
        assert_eq!(record.sample_rate, 48_000);
        assert_eq!(record.channels, 2);
        assert_eq!(record.content_hash.len(), 64);
        assert!(record.size_bytes > 0 && record.mtime_ns > 0);
        assert!(record.tags.iter().all(|tag| tag.source == TagSource::Auto));
        // Every stored waveform and feature vector reads back.
        let waveform = library
            .waveform(record.id)
            .expect("waveform")
            .expect("some");
        assert!((waveform.duration_seconds() - record.duration_seconds).abs() < 1e-6);
        assert!(library.features(record.id).expect("features").is_some());
    }

    // Exact lengths and levels of two files.
    let groove = library
        .file_by_path(&stored(dir.path(), "loops/groove_a.wav"))
        .expect("lookup")
        .expect("groove");
    assert!((groove.duration_seconds - 7.5).abs() < 1e-6);
    let lufs = groove.loudness_lufs.expect("loud enough to measure");
    assert!((-40.0..0.0).contains(&lufs), "{lufs}");
    assert!(groove.true_peak_dbtp.is_some());
    assert!(groove.bpm_confidence.is_some_and(|c| c > 0.5));
    // A 0.3 s hat is shorter than one loudness gate block.
    let short_hat = library
        .file_by_path(&stored(dir.path(), "drums/sample_003.wav"))
        .expect("lookup")
        .expect("hat");
    assert_eq!(short_hat.loudness_lufs, None);
    // Drums get no key even though the chroma always has a maximum.
    let kick = library
        .file_by_path(&stored(dir.path(), "drums/sample_001.wav"))
        .expect("lookup")
        .expect("kick");
    assert_eq!(kick.key, None);
    assert!(kick.has_tag("drums"));
}

#[test]
fn the_waveform_follows_the_audio() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A kick at the start of two seconds of otherwise silence.
    let mut buffer = common::kick_909(120);
    buffer.left.resize(96_000, 0.0);
    buffer.right.resize(96_000, 0.0);
    let path = write(dir.path(), "Kick Tail.wav", &buffer);
    let mut library = Library::open_in_memory().expect("open");
    let id = library.index_file(&path).expect("index");
    let waveform = library.waveform(id).expect("waveform").expect("some");
    assert_eq!(waveform.frames, 96_000);
    let start = waveform.peak_at(0.01).expect("start");
    let end = waveform.peak_at(1.9).expect("end");
    assert!(start.max > 0.3, "{start:?}");
    assert_eq!((end.max, end.min, end.rms), (0.0, 0.0, 0.0));
}

#[test]
fn unreadable_files_are_reported_and_others_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(dir.path(), "Clap.wav", &common::clap());
    fs::write(dir.path().join("broken.wav"), b"RIFF not really a wave").expect("write");
    fs::write(dir.path().join("notes.txt"), b"not audio").expect("write");
    fs::write(dir.path().join(".hidden.wav"), b"junk").expect("write");
    let (library, report) = scanned(dir.path());
    assert_eq!(report.added.len(), 1);
    assert_eq!(report.failed.len(), 1);
    assert!(report.failed[0].path.ends_with("broken.wav"));
    assert_eq!(library.len().expect("len"), 1);
}

#[test]
fn progress_is_reported_in_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..5 {
        write(
            dir.path(),
            &format!("Hat {index}.wav"),
            &common::hat(42, 0.3),
        );
    }
    let mut library = Library::open_in_memory().expect("open");
    let mut seen = Vec::new();
    library
        .scan(
            &[dir.path().to_path_buf()],
            &ScanOptions::new().threads(3),
            |p| seen.push(p.clone()),
        )
        .expect("scan");
    assert_eq!(seen.first().map(|p| p.phase), Some(ScanPhase::Walking));
    let last = seen.last().expect("progress");
    assert_eq!(
        (last.phase, last.found, last.total, last.done),
        (ScanPhase::Done, 5, 5, 5)
    );
    let done: Vec<usize> = seen
        .iter()
        .filter(|p| p.phase == ScanPhase::Analysing && p.current.is_some())
        .map(|p| p.done)
        .collect();
    assert_eq!(done, [1, 2, 3, 4, 5]);
}

#[test]
fn one_thread_and_many_give_the_same_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    let mut one = Library::open_in_memory().expect("open");
    one.scan(
        &[dir.path().to_path_buf()],
        &ScanOptions::new().threads(1),
        |_| {},
    )
    .expect("scan");
    let mut many = Library::open_in_memory().expect("open");
    many.scan(
        &[dir.path().to_path_buf()],
        &ScanOptions::new().threads(8),
        |_| {},
    )
    .expect("scan");
    let query = fourfloor_library::Query::new().sort_by(fourfloor_library::SortKey::Path);
    let strip = |library: &Library| {
        library
            .search(&query)
            .expect("search")
            .into_iter()
            .map(|r| (r.path, r.content_hash, r.kind, r.bpm, r.key, r.tags))
            .collect::<Vec<_>>()
    };
    assert_eq!(strip(&one), strip(&many));
}

#[test]
fn a_rescan_only_reads_what_changed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    let (mut library, first) = scanned(dir.path());
    let total = first.added.len();

    // Nothing changed: nothing is read.
    let again = library.rescan(&ScanOptions::new(), |_| {}).expect("rescan");
    assert_eq!(again.unchanged, total);
    assert!(again.added.is_empty() && again.updated.is_empty() && again.touched == 0);

    // A new time on the same bytes: hashed, not analysed.
    let clap = stored(dir.path(), "drums/Clap.wav");
    let file = fs::File::options().write(true).open(&clap).expect("open");
    file.set_modified(SystemTime::now() + Duration::from_secs(60))
        .expect("touch");
    drop(file);
    let touched = library.rescan(&ScanOptions::new(), |_| {}).expect("rescan");
    assert_eq!(touched.touched, 1);
    assert_eq!(touched.unchanged, total - 1);
    assert!(touched.updated.is_empty());

    // New contents under the same name: analysed again, same row, and
    // user tags survive while automatic ones are replaced.
    let id = library
        .file_by_path(&clap)
        .expect("lookup")
        .expect("clap")
        .id;
    library.add_tag(id, "Favourite").expect("tag");
    write(dir.path(), "drums/Clap.wav", &common::kick_808());
    let changed = library.rescan(&ScanOptions::new(), |_| {}).expect("rescan");
    assert_eq!(changed.updated, [id]);
    let record = library.file(id).expect("file");
    assert!(record.has_tag("favourite"));
    assert!(record.has_tag("clap"), "the name still says clap");
    assert!(
        (record.duration_seconds - 0.9).abs() < 1e-6,
        "the new audio is the kick"
    );

    // Forcing re-analyses everything.
    let forced = library
        .rescan(&ScanOptions::new().force(), |_| {})
        .expect("rescan");
    assert_eq!(forced.updated.len(), total);
}

#[test]
fn moved_files_keep_their_row_and_vanished_ones_are_flagged() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    let (mut library, first) = scanned(dir.path());
    let old = stored(dir.path(), "drums/Hat Open.wav");
    let id = library.file_by_path(&old).expect("lookup").expect("hat").id;
    library.set_rating(id, 4).expect("rate");
    library.add_tag(id, "keeper").expect("tag");
    library.record_play_at(id, "preview", 1_000).expect("play");

    fs::create_dir_all(dir.path().join("moved")).expect("mkdir");
    fs::rename(&old, dir.path().join("moved/Hat Open.wav")).expect("move");
    fs::remove_file(stored(dir.path(), "drums/Clap.wav")).expect("delete");

    let report = library.rescan(&ScanOptions::new(), |_| {}).expect("rescan");
    assert_eq!(report.moved, [id]);
    assert_eq!(report.missing.len(), 1);
    assert!(report.added.is_empty(), "{report:?}");
    let record = library.file(id).expect("file");
    assert!(record.path.ends_with("moved/Hat Open.wav"));
    assert_eq!((record.rating, record.play_count), (4, 1));
    assert!(record.has_tag("keeper"));

    let gone = library.file(report.missing[0]).expect("file");
    assert!(gone.missing && gone.path.ends_with("Clap.wav"));
    let visible = library
        .search(&fourfloor_library::Query::new())
        .expect("search");
    assert_eq!(visible.len(), first.added.len() - 1);

    // The file comes back: it is no longer missing.
    write(dir.path(), "drums/Clap.wav", &common::clap());
    let back = library.rescan(&ScanOptions::new(), |_| {}).expect("rescan");
    assert!(back.missing.is_empty());
    assert!(!library.file(gone.id).expect("file").missing);
    assert_eq!(library.purge_missing().expect("purge"), 0);
}

#[test]
fn identical_copies_move_and_go_missing_one_by_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let clap = common::clap();
    for name in ["a/Clap.wav", "b/Clap.wav", "c/Clap.wav", "d/Clap.wav"] {
        write(dir.path(), name, &clap);
    }
    let (mut library, first) = scanned(dir.path());
    assert_eq!(first.added.len(), 4);
    let id_of = |library: &Library, name: &str| {
        library
            .file_by_path(&stored(dir.path(), name))
            .expect("lookup")
            .expect("indexed")
            .id
    };
    let (a, b) = (id_of(&library, "a/Clap.wav"), id_of(&library, "b/Clap.wav"));
    let (c, d) = (id_of(&library, "c/Clap.wav"), id_of(&library, "d/Clap.wav"));

    // Two byte-identical copies are deleted, two others move.
    fs::remove_file(stored(dir.path(), "a/Clap.wav")).expect("delete");
    fs::remove_file(stored(dir.path(), "b/Clap.wav")).expect("delete");
    fs::create_dir_all(dir.path().join("moved")).expect("mkdir");
    fs::rename(
        stored(dir.path(), "c/Clap.wav"),
        dir.path().join("moved/Clap C.wav"),
    )
    .expect("move");
    fs::rename(
        stored(dir.path(), "d/Clap.wav"),
        dir.path().join("moved/Clap D.wav"),
    )
    .expect("move");

    let report = library.rescan(&ScanOptions::new(), |_| {}).expect("rescan");
    assert!(report.added.is_empty(), "{report:?}");
    let mut moved = report.moved.clone();
    moved.sort();
    assert_eq!(moved.len(), 2, "{report:?}");
    assert_eq!(report.missing.len(), 2, "{report:?}");
    let mut all: Vec<_> = moved.iter().chain(&report.missing).copied().collect();
    all.sort();
    let mut want = vec![a, b, c, d];
    want.sort();
    assert_eq!(all, want, "every old row is either moved or missing");
    assert_eq!(
        library
            .search(&fourfloor_library::Query::new())
            .expect("search")
            .len(),
        2
    );
}

#[test]
fn a_cancelled_scan_keeps_what_it_wrote_and_flags_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    let (mut library, first) = scanned(dir.path());
    // Something new to analyse, so the cancelled scan has work to skip.
    write(dir.path(), "late/Kick Late.wav", &common::kick_808());
    let flag = Arc::new(AtomicBool::new(true));
    let report = library
        .scan(
            &[dir.path().to_path_buf()],
            &ScanOptions::new().cancel_with(flag),
            |_| {},
        )
        .expect("scan");
    assert!(report.cancelled);
    assert!(
        report.missing.is_empty(),
        "a partial walk proves nothing missing"
    );
    assert!(report.added.is_empty());
    let visible = library
        .search(&fourfloor_library::Query::new())
        .expect("search");
    assert_eq!(visible.len(), first.added.len());
}

#[test]
fn long_files_are_tracks_with_a_tempo() {
    let dir = tempfile::tempdir().expect("tempdir");
    // 33 bars at 124 BPM: about 64 seconds.
    let path = write(dir.path(), "Demo Mix.wav", &common::drum_loop(124.0, 33));
    let mut library = Library::open_in_memory().expect("open");
    let id = library.index_file(&path).expect("index");
    let record = library.file(id).expect("file");
    assert_eq!(record.kind, FileKind::Track);
    let bpm = record.bpm.expect("tempo");
    assert!((bpm - 124.0).abs() < 0.1, "{bpm}");
    let first = record.first_beat_seconds.expect("beat grid");
    // The library stores the analysis crate's grid as it is. On this
    // pattern that grid locks to the loud off-beat hats (0.242 s, half a
    // beat), a known fourfloor-analysis phase limit, so only the
    // eighth-note grid is checked here.
    let halves = first * 2.0 * 124.0 / 60.0;
    assert!(
        (halves - halves.round()).abs() * 30.0 / 124.0 < 0.02,
        "{first}"
    );
    assert!(!record.has_tag("loop"));
}

#[test]
fn a_hand_set_tempo_survives_rescans() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    let (mut library, _) = scanned(dir.path());
    let path = stored(dir.path(), "loops/groove_a.wav");
    let id = library
        .file_by_path(&path)
        .expect("lookup")
        .expect("groove")
        .id;
    let manual = fourfloor_library::ManualAnalysis {
        bpm: Some(64.0),
        key: None,
        kind: Some(FileKind::Track),
    };
    library.set_manual_analysis(id, &manual).expect("manual");
    library
        .rescan(&ScanOptions::new().force(), |_| {})
        .expect("rescan");
    let record = library.file(id).expect("file");
    assert_eq!(
        (record.bpm, record.kind, record.locked),
        (Some(64.0), FileKind::Track, true)
    );
    library.unlock(id).expect("unlock");
    library
        .rescan(&ScanOptions::new().force(), |_| {})
        .expect("rescan");
    let record = library.file(id).expect("file");
    assert!(record.bpm.is_some_and(|bpm| (bpm - 128.0).abs() < 0.1));
    assert_eq!(record.kind, FileKind::Loop);
    let bad = fourfloor_library::ManualAnalysis {
        bpm: Some(f64::NAN),
        ..manual
    };
    assert!(library.set_manual_analysis(id, &bad).is_err());
}
