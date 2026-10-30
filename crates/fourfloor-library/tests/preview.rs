//! Preview plans: ratio maths, and varispeed renders measured by FFT.

mod common;

use common::{write, RATE};
use fourfloor_analysis::Key;
use fourfloor_audio::decode_file;
use fourfloor_core::{SampleRate, Tempo};
use fourfloor_library::{FileId, Library, LibraryError, PreviewMode, PreviewTarget};
use fourfloor_testkit::{dominant_frequency, null_test, sine, StereoBuffer};

fn tempo(bpm: f64) -> Option<Tempo> {
    Tempo::new(bpm).ok()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// A 440 Hz sine, 4 bars at 120 BPM, named as an A minor loop.
fn sine_loop(dir: &std::path::Path) -> (Library, FileId, std::path::PathBuf) {
    let signal = sine(440.0, 0.5, 8.0, RATE);
    let path = write(
        dir,
        "Sine Loop 120bpm Am.wav",
        &StereoBuffer::from_mono(RATE, &signal),
    );
    let mut library = Library::open_in_memory().expect("open");
    let id = library.index_file(&path).expect("index");
    let record = library.file(id).expect("file");
    assert_eq!(record.kind, fourfloor_library::FileKind::Loop);
    assert_eq!((record.bpm, record.key), (Some(120.0), Some(Key::minor(9))));
    (library, id, path)
}

#[test]
fn key_locked_plans_stretch_to_tempo_and_shift_to_key() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (library, id, _) = sine_loop(dir.path());
    // B minor at 126: two semitones up, 5 % faster.
    let target = PreviewTarget::new(tempo(126.0), Some(Key::minor(11)), RATE);
    let plan = library.preview(id, &target).expect("plan");
    assert_eq!(plan.mode, PreviewMode::KeyLocked);
    assert!(plan.looped);
    assert!(close(plan.speed, 1.05));
    assert!(close(plan.time_stretch, 1.0 / 1.05));
    assert!(close(plan.pitch_semitones, 2.0));
    assert!(close(plan.pitch_ratio, 2f64.powf(2.0 / 12.0)));
    assert!(
        close(plan.source_step, 1.0),
        "no rate change, the stretcher does the rest"
    );
    assert_eq!(plan.source_frames, 384_000);
    assert_eq!(plan.output_frames, (384_000.0f64 / 1.05).round() as u64);
    assert_eq!(
        (plan.source_bpm, plan.target_bpm),
        (Some(120.0), Some(126.0))
    );
    // C major is A minor's relative: no shift. E minor: down five.
    let relative = PreviewTarget::new(tempo(120.0), Some(Key::major(0)), RATE);
    assert!(close(
        library
            .preview(id, &relative)
            .expect("plan")
            .pitch_semitones,
        0.0
    ));
    let e_minor = PreviewTarget::new(None, Some(Key::minor(4)), RATE);
    let plan = library.preview(id, &e_minor).expect("plan");
    assert!(close(plan.pitch_semitones, -5.0) && close(plan.speed, 1.0));
    // A key-locked plan cannot be rendered here.
    let audio = decode_file(&plan.path).expect("decode");
    assert!(plan.render_varispeed(&audio).is_err());
}

#[test]
fn varispeed_moves_pitch_with_tempo_and_sounds_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (library, id, path) = sine_loop(dir.path());
    let audio = decode_file(&path).expect("decode");
    for out_rate in [SampleRate::HZ_48000, SampleRate::HZ_44100] {
        let target = PreviewTarget::new(tempo(126.0), Some(Key::minor(11)), out_rate)
            .with_mode(PreviewMode::Varispeed);
        let plan = library.preview(id, &target).expect("plan");
        assert!(close(plan.speed, 1.05));
        assert!(close(plan.pitch_semitones, 12.0 * 1.05f64.log2()));
        let expected_step = 48_000.0 / f64::from(out_rate.hz()) * 1.05;
        assert!(close(plan.source_step, expected_step));

        let rendered = plan.render_varispeed(&audio).expect("render");
        assert_eq!(rendered.sample_rate, out_rate);
        let frames = rendered.frames();
        let want = 8.0 / 1.05 * f64::from(out_rate.hz());
        assert!(
            (frames as f64 - want).abs() <= 1.0,
            "{frames} frames, want {want}"
        );
        let heard = dominant_frequency(&rendered.channels[0], out_rate);
        assert!((heard - 462.0).abs() < 1.0, "{heard} Hz at {out_rate:?}");
        // At 126 BPM a beat is 60/126 s: the loop is still 16 beats.
        let beats = frames as f64 / f64::from(out_rate.hz()) * 126.0 / 60.0;
        assert!((beats - 16.0).abs() < 0.01, "{beats}");
    }
}

#[test]
fn one_shots_keep_their_length_unless_repitched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = sine(220.0, 0.5, 1.0, RATE);
    let path = write(
        dir.path(),
        "Bass Note Am.wav",
        &StereoBuffer::from_mono(RATE, &signal),
    );
    let mut library = Library::open_in_memory().expect("open");
    let id = library.index_file(&path).expect("index");
    assert_eq!(
        library.file(id).expect("file").kind,
        fourfloor_library::FileKind::OneShot
    );

    // Tempo is ignored for a one-shot; the key is matched.
    let target = PreviewTarget::new(tempo(130.0), Some(Key::minor(0)), RATE);
    let plan = library.preview(id, &target).expect("plan");
    assert!(close(plan.speed, 1.0) && close(plan.pitch_semitones, 3.0));
    assert!(!plan.looped);

    // Varispeed repitches sampler-style: three semitones up, shorter.
    let plan = library
        .preview(id, &target.with_mode(PreviewMode::Varispeed))
        .expect("plan");
    let ratio = 2f64.powf(3.0 / 12.0);
    assert!(close(plan.speed, ratio) && close(plan.pitch_ratio, ratio));
    let rendered = plan
        .render_varispeed(&decode_file(&path).expect("decode"))
        .expect("render");
    let heard = f64::from(dominant_frequency(&rendered.channels[0], RATE));
    assert!((heard - 220.0 * ratio).abs() < 1.5, "{heard}");
    assert!((rendered.frames() as f64 - 48_000.0 / ratio).abs() <= 1.0);
}

#[test]
fn original_mode_plays_the_file_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (library, id, path) = sine_loop(dir.path());
    let target =
        PreviewTarget::new(tempo(90.0), Some(Key::major(3)), RATE).with_mode(PreviewMode::Original);
    let plan = library.preview(id, &target).expect("plan");
    assert!(close(plan.speed, 1.0) && close(plan.pitch_semitones, 0.0));
    let audio = decode_file(&path).expect("decode");
    let rendered = plan.render_varispeed(&audio).expect("render");
    assert!(null_test(&rendered.channels[0], &audio.channels[0], -120.0));
}

#[test]
fn half_and_double_time_loops_play_near_unity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let signal = sine(330.0, 0.5, 60.0 / 62.0 * 8.0, RATE);
    let path = write(
        dir.path(),
        "Slow Loop 62bpm.wav",
        &StereoBuffer::from_mono(RATE, &signal),
    );
    let mut library = Library::open_in_memory().expect("open");
    let id = library.index_file(&path).expect("index");
    let mut target = PreviewTarget::new(tempo(124.0), None, RATE);
    let plan = library.preview(id, &target).expect("plan");
    assert!(close(plan.speed, 1.0), "{}", plan.speed);
    assert_eq!(plan.source_bpm, Some(124.0));
    target.half_double = false;
    assert!(close(
        library.preview(id, &target).expect("plan").speed,
        2.0
    ));
    assert!(matches!(
        library.preview(FileId(77), &target),
        Err(LibraryError::NotFound { .. })
    ));
}
