//! Runs the real `fourfloor` binary: `new`, then `render`, then `meter`, in
//! a temporary directory. No audio device is opened.
//!
//! The binary installs the engine's allocation guard, so in a debug build
//! this also proves the whole render path is allocation-free on the audio
//! callback: an allocation there would abort the process and fail the test.

use std::path::Path;
use std::process::{Command, Output};

use fourfloor_analysis::{detect_onsets, OnsetOptions as AnalysisOnsets};
use fourfloor_audio::decode_file;
use fourfloor_core::{SampleRate, Tempo, Ticks};
use fourfloor_testkit::{
    assert_buffer_ok, loudness, spectrum_band_energy, stereo_correlation, times_match, StereoBuffer,
};

const BPM: f64 = 124.0;
const BARS: u32 = 4;
const RATE: SampleRate = SampleRate::HZ_48000;

fn fourfloor(args: &[&str], dir: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_fourfloor"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("the fourfloor binary runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout(output),
        stderr(output)
    );
}

fn rms(signal: &[f32]) -> f64 {
    let sum: f64 = signal.iter().map(|s| f64::from(*s).powi(2)).sum();
    (sum / signal.len().max(1) as f64).sqrt()
}

/// Sample position of beat `beat` at the test tempo.
fn beat_sample(beat: i64) -> usize {
    let tempo = Tempo::new(BPM).unwrap();
    Ticks::from_whole_beats(beat).to_sample_pos(tempo, RATE) as usize
}

#[test]
fn new_then_render_writes_a_wav_with_energy_on_every_beat() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path();

    let created = fourfloor(&["new", "track"], work);
    assert_ok(&created);
    assert!(stdout(&created).contains("4 bars at 124 BPM"));
    assert!(work.join("track/project.json").is_file());

    let rendered = fourfloor(&["render", "track", "-o", "out.wav"], work);
    assert_ok(&rendered);
    assert!(
        stdout(&rendered).contains("48000 Hz"),
        "{}",
        stdout(&rendered)
    );

    let audio = decode_file(&work.join("out.wav")).expect("the render decodes");
    assert_eq!(audio.sample_rate, RATE);
    assert_eq!(audio.channel_count(), 2, "the render is stereo");
    let beats = i64::from(BARS) * 4;
    // The pattern plus the default half-second tail.
    assert_eq!(audio.frames(), beat_sample(beats) + 24_000);

    let left = &audio.channels[0];
    // Nothing in a new project is panned, so the two channels of the
    // stereo mix carry the same signal.
    assert_eq!(left, &audio.channels[1]);
    assert!(left.iter().all(|s| s.is_finite() && s.abs() < 1.0));
    let buffer = StereoBuffer::read_wav(&work.join("out.wav")).unwrap();
    assert_buffer_ok(&buffer);
    assert!((stereo_correlation(&buffer.left, &buffer.right) - 1.0).abs() < 1e-6);

    // A hat on every sixteenth between the kicks (closed, open on the
    // offbeat), on the sample grid of 124 BPM. In the mix
    // the kick's decay hides the closed hat after it, so measure the hats'
    // own stem (from `fourfloor stems`), with the analysis crate's
    // spectral-flux onsets.
    assert_ok(&fourfloor(&["stems", "track", "-o", "stems"], work));
    let hats = decode_file(&work.join("stems/02-Hats.wav")).expect("hat stem");
    let sixteenth = 60.0 / BPM / 4.0;
    let expected: Vec<f64> = (0..beats * 4)
        .filter(|step| step % 4 != 0)
        .map(|step| step as f64 * sixteenth)
        .collect();
    let found: Vec<f64> = detect_onsets(&hats.channels[0], RATE, &AnalysisOnsets::default())
        .iter()
        .map(|onset| onset.time_seconds)
        .collect();
    assert!(times_match(&found, &expected, 0.003), "{found:?}");
    // The kick's low end is on the beats, not between them.
    let low = |at: usize| spectrum_band_energy(&left[at..at + 4_800], RATE, 30.0, 150.0);
    assert!(low(beat_sample(1)) > 50.0 * low(beat_sample(1) + beat_sample(1) / 2));

    // Four-on-the-floor: the 10 ms after every beat is loud (the kick), and
    // the 10 ms before it is much quieter (only decayed tails).
    let window = 480;
    for beat in 0..beats {
        let at = beat_sample(beat);
        let on_beat = rms(&left[at..at + window]);
        assert!(on_beat > 0.1, "beat {beat}: RMS {on_beat}");
        if beat > 0 {
            let before = rms(&left[at - window..at]);
            assert!(
                on_beat > 5.0 * before,
                "beat {beat}: {on_beat} on the beat, {before} just before"
            );
        }
    }

    // Hats sit between the kicks: the offbeat eighth carries an open hat.
    let half_beat = beat_sample(1) / 2;
    for beat in 0..beats {
        let at = beat_sample(beat) + half_beat;
        let offbeat = rms(&left[at..at + window]);
        assert!(offbeat > 0.02, "offbeat after beat {beat}: RMS {offbeat}");
    }

    // The tail ends in silence rather than being cut off mid-sound.
    assert!(rms(&left[left.len() - window..]) < 1e-3);

    let metered = fourfloor(&["meter", "out.wav"], work);
    assert_ok(&metered);
    let report = stdout(&metered);
    assert!(
        report.contains("LUFS") && report.contains("dBTP"),
        "{report}"
    );
    assert!(
        !report.contains("inf"),
        "a drum loop has a finite loudness: {report}"
    );
    // The printed figures are the ones the library measures.
    let measured = loudness(&buffer);
    assert!(measured.integrated_lufs > -30.0 && measured.integrated_lufs < -5.0);
    assert!(report.contains(&format!("integrated {:.2} LUFS", measured.integrated_lufs)));
    assert!(report.contains(&format!("true peak {:.2} dBTP", measured.true_peak_dbtp)));
}

#[test]
fn render_honours_rate_and_tail_options() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path();
    assert_ok(&fourfloor(
        &["new", "p", "--bpm", "120", "--bars", "1"],
        work,
    ));
    assert_ok(&fourfloor(
        &[
            "render", "p", "-o", "p.wav", "--rate", "44100", "--tail", "0",
        ],
        work,
    ));
    let audio = decode_file(&work.join("p.wav")).unwrap();
    assert_eq!(audio.sample_rate, SampleRate::HZ_44100);
    // One bar at 120 BPM is exactly two seconds.
    assert_eq!(audio.frames(), 88_200);
}

#[test]
fn failures_exit_non_zero_with_a_message_and_no_panic() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path();

    let usage = fourfloor(&["render", "missing"], work);
    assert_eq!(usage.status.code(), Some(2));
    assert!(stderr(&usage).starts_with("error: "));

    let missing = fourfloor(&["render", "missing", "-o", "out.wav"], work);
    assert_eq!(missing.status.code(), Some(1));
    assert!(
        stderr(&missing).contains("project.json"),
        "{}",
        stderr(&missing)
    );
    assert!(!work.join("out.wav").exists());

    assert_ok(&fourfloor(&["new", "p"], work));
    let again = fourfloor(&["new", "p"], work);
    assert_eq!(again.status.code(), Some(1));
    assert!(stderr(&again).contains("already contains a project"));

    // A hand-edited project that breaks a model invariant is refused.
    std::fs::write(work.join("p/project.json"), "{ not json").unwrap();
    let broken = fourfloor(&["render", "p", "-o", "out.wav"], work);
    assert_eq!(broken.status.code(), Some(1));
    assert!(!stderr(&broken).contains("panicked"));

    let bad_tempo = fourfloor(&["new", "q", "--bpm", "5"], work);
    assert_eq!(bad_tempo.status.code(), Some(1));
    assert!(!work.join("q").exists());
}

#[test]
fn demo_then_stems_writes_files_that_sum_to_a_normalised_master() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path();
    let created = fourfloor(&["demo", "set", "--bpm", "122"], work);
    assert_ok(&created);
    assert!(
        stdout(&created).contains("28 bars at 122 BPM, 10 tracks"),
        "{}",
        stdout(&created)
    );

    let exported = fourfloor(
        &[
            "stems",
            "set",
            "-o",
            "out",
            "--normalize",
            "-18",
            "--tail",
            "0.25",
        ],
        work,
    );
    assert_ok(&exported);
    let report = stdout(&exported);
    assert!(
        report.contains("normalised from") && report.contains("2 files"),
        "{report}"
    );
    assert!(!report.contains("note:"), "no compiler warnings: {report}");

    let master = StereoBuffer::read_wav(&work.join("out/master.wav")).unwrap();
    assert_buffer_ok(&master);
    let measured = loudness(&master).integrated_lufs;
    assert!((measured + 18.0).abs() < 0.2, "{measured} LUFS");
    // Every demo track feeds the "Master" bus (which carries the limiter),
    // so that bus is the one stem. The per-track null test, with the bus
    // taken out, is in `tests/demo_audio.rs`.
    let stems = ["01-Master"];
    let mut sum = vec![0.0f32; master.frames()];
    for stem in stems {
        let buffer = StereoBuffer::read_wav(&work.join(format!("out/{stem}.wav"))).unwrap();
        assert_eq!(buffer.frames(), master.frames(), "{stem}");
        for (total, sample) in sum.iter_mut().zip(&buffer.left) {
            *total += sample;
        }
    }
    let residual: f64 = rms(&sum
        .iter()
        .zip(&master.left)
        .map(|(a, b)| a - b)
        .collect::<Vec<f32>>());
    assert!(residual < 1e-6, "stems minus master: RMS {residual}");

    // The same master through `render --normalize`.
    assert_ok(&fourfloor(
        &[
            "render",
            "set",
            "-o",
            "mix.wav",
            "--normalize",
            "-18",
            "--tail",
            "0.25",
        ],
        work,
    ));
    let mix = StereoBuffer::read_wav(&work.join("mix.wav")).unwrap();
    assert_eq!(mix.left, master.left);

    let again = fourfloor(&["demo", "set"], work);
    assert_eq!(again.status.code(), Some(1));
    assert!(stderr(&again).contains("already contains a project"));
}
