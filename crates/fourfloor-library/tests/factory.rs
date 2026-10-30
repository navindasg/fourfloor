//! The factory set: rendered, written, scanned, and what the index makes
//! of it.

use fourfloor_analysis::Key;
use fourfloor_audio::decode_file;
use fourfloor_library::factory::{
    factory_loop_frames, factory_samples, write_factory, FACTORY_BPM, FACTORY_LICENSE_FILE,
    FACTORY_PEAK,
};
use fourfloor_library::{FileKind, Library, Query, ScanOptions};
use fourfloor_testkit::{
    assert_no_nan_or_clipping, dominant_frequency, onset_times, peak, rms_window,
};

#[test]
fn the_factory_set_is_written_once_and_decodes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let folder = dir.path().join("factory");
    let first = write_factory(&folder).expect("write");
    let samples = factory_samples();
    assert_eq!(first.written.len(), samples.len());
    let license = std::fs::read_to_string(folder.join(FACTORY_LICENSE_FILE)).expect("licence");
    assert!(license.contains("CC0"));
    // A second call keeps every file.
    let second = write_factory(&folder).expect("write");
    assert!(second.written.is_empty());
    assert_eq!(second.kept.len(), samples.len());

    for sample in &samples {
        let audio = decode_file(&sample.path_in(&folder)).expect("decode");
        assert_eq!(audio.channel_count(), 2);
        let left = &audio.channels[0];
        assert_no_nan_or_clipping(left);
        // Normalised to -1 dBFS, give or take 24-bit rounding.
        let top = audio
            .channels
            .iter()
            .map(|c| peak(c))
            .fold(0.0f32, f32::max);
        assert!((top - FACTORY_PEAK).abs() < 1e-3, "{}: {top}", sample.path);
        if sample.kind == FileKind::Loop {
            assert_eq!(audio.frames(), factory_loop_frames(), "{}", sample.path);
        }
        // Files end on silence (hits fade, loops wrap their tail).
        if sample.kind == FileKind::OneShot {
            let end = rms_window(left, left.len() - 48, 48);
            assert!(end < 0.01, "{}: tail {end}", sample.path);
        }
    }
}

#[test]
fn factory_sounds_are_what_they_say() {
    let rate = fourfloor_library::factory::FACTORY_SAMPLE_RATE;
    let by_name = |name: &str| {
        factory_samples()
            .into_iter()
            .find(|s| s.filename() == name)
            .unwrap_or_else(|| panic!("{name}"))
            .render()
            .to_mono()
    };
    // The bass note is A1 (55 Hz).
    let bass = by_name("FF Bass Note A1.wav");
    let heard = dominant_frequency(&bass, rate);
    assert!((heard - 55.0).abs() < 2.0, "{heard}");
    // The kick's body sits low.
    let kick = dominant_frequency(&by_name("FF Kick 909.wav"), rate);
    assert!((30.0..120.0).contains(&kick), "{kick}");
    // The drum loop has a kick on each of its 16 beats (plus other hits).
    let drums = by_name("FF Drum Loop 124bpm.wav");
    let onsets = onset_times(&drums, rate);
    let beat = 60.0 / FACTORY_BPM;
    let on_beats = (0..16)
        .filter(|&i| {
            onsets
                .iter()
                .any(|&t| (t - f64::from(i) * beat).abs() < 0.01)
        })
        .count();
    assert!(
        on_beats >= 15,
        "{on_beats} of 16 beats start a sound: {onsets:?}"
    );
}

#[test]
fn the_scanned_factory_has_tempo_kind_key_and_tags() {
    let dir = tempfile::tempdir().expect("tempdir");
    let folder = dir.path().join("factory");
    let mut library = Library::open_in_memory().expect("open");
    let (written, scan) = library
        .install_factory(&folder, &ScanOptions::new())
        .expect("install");
    assert_eq!(scan.added.len(), written.written.len());
    assert!(scan.failed.is_empty(), "{:?}", scan.failed);

    for sample in factory_samples() {
        let path = sample.path_in(&folder.canonicalize().expect("canonical"));
        let record = library
            .file_by_path(&path)
            .expect("lookup")
            .expect("indexed");
        assert_eq!(record.kind, sample.kind, "{}", sample.path);
        match sample.bpm {
            Some(bpm) => assert!(
                record.bpm.is_some_and(|found| (found - bpm).abs() < 0.05),
                "{}: {:?}",
                sample.path,
                record.bpm
            ),
            None => assert_eq!(record.bpm, None, "{}", sample.path),
        }
        if let Some(key) = sample.key {
            assert_eq!(record.key, Some(key), "{}", sample.path);
        }
        for tag in sample.tags {
            assert!(
                record.has_tag(tag),
                "{} lacks {tag}: {:?}",
                sample.path,
                record.tag_names()
            );
        }
    }
    let house_loops = library
        .search(&Query::new().kind(FileKind::Loop).bpm_range(123.5, 124.5))
        .expect("search");
    assert_eq!(house_loops.len(), 5);
    let a_minor = library
        .search(&Query::new().key(Key::minor(9)))
        .expect("search");
    assert!(a_minor.len() >= 4, "{}", a_minor.len());
    // Installing again writes nothing and reads nothing.
    let (again, rescan) = library
        .install_factory(&folder, &ScanOptions::new())
        .expect("install");
    assert!(again.written.is_empty());
    assert_eq!(rescan.unchanged, scan.added.len());
}
