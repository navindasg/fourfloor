//! Waveform overview tests and whole-track analysis tests, including a
//! round trip through a WAV file on disk.

mod common;

use common::{add, chord, drum_loop, dual_mono, frames, kick, note, place, Loop, RATE};
use fourfloor_analysis::{
    analyze_file, analyze_track, AnalysisError, Key, Peak, TempoOptions, TrackOptions,
    WaveformOptions, WaveformOverview,
};
use fourfloor_audio::{write_wav_file, AudioBuffer, WavSpec};
use fourfloor_testkit::{sine, white_noise};

fn direct(block: &[f32]) -> Peak {
    let square: f64 = block.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    Peak {
        min: block.iter().copied().fold(f32::INFINITY, f32::min),
        max: block.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        rms: (square / block.len() as f64).sqrt() as f32,
    }
}

fn assert_peak(found: Peak, expected: Peak, what: &str) {
    assert_eq!(
        (found.min, found.max),
        (expected.min, expected.max),
        "{what}"
    );
    assert!(
        (found.rms - expected.rms).abs() < 1e-5,
        "{what}: {} {}",
        found.rms,
        expected.rms
    );
}

#[test]
fn every_level_matches_a_direct_scan_of_the_samples() {
    // A length that is not a multiple of any block size.
    let audio = white_noise(3, 0.7, 2.0, RATE)[..90_001].to_vec();
    let options = WaveformOptions {
        base_samples_per_peak: 64,
        factor: 4,
        min_peaks: 16,
    };
    let overview = WaveformOverview::build(&audio, RATE, &options).unwrap();
    assert_eq!((overview.sample_rate, overview.frames), (48_000, 90_001));
    let sizes: Vec<usize> = overview.levels.iter().map(|l| l.samples_per_peak).collect();
    assert_eq!(sizes, [64, 256, 1024, 4096, 16_384]);
    for level in &overview.levels {
        assert_eq!(
            level.peaks.len(),
            audio.len().div_ceil(level.samples_per_peak)
        );
        for (peak, block) in level.peaks.iter().zip(audio.chunks(level.samples_per_peak)) {
            assert_peak(
                *peak,
                direct(block),
                &format!("{} per peak", level.samples_per_peak),
            );
        }
    }
    assert!(overview.levels.last().unwrap().peaks.len() <= 16);
}

#[test]
fn a_sine_has_its_amplitude_as_extent_and_rms() {
    let overview = WaveformOverview::build(
        &sine(1000.0, 0.5, 1.0, RATE),
        RATE,
        &WaveformOptions::default(),
    )
    .unwrap();
    for level in &overview.levels {
        for peak in &level.peaks[..level.peaks.len() - 1] {
            assert!((peak.max - 0.5).abs() < 0.01 && (peak.min + 0.5).abs() < 0.01);
            assert!((peak.rms - 0.5 / 2f32.sqrt()).abs() < 0.02, "{}", peak.rms);
        }
    }
}

#[test]
fn a_hit_shows_up_where_it_is() {
    let mut audio = vec![0.0f32; frames(2.0, RATE)];
    place(&mut audio, &kick(RATE), 1.0, RATE);
    let overview = WaveformOverview::build(&audio, RATE, &WaveformOptions::default()).unwrap();
    let columns = overview.columns(0, audio.len(), 200);
    assert_eq!(columns.len(), 200);
    // 10 ms per column: silent before the hit, loud from column 100.
    assert!(columns[..100]
        .iter()
        .all(|p| p.max == 0.0 && p.min == 0.0 && p.rms == 0.0));
    assert!(columns[100].max > 0.5 && columns[100].rms > 0.2);
    assert!(columns[101].min < -0.5);
    assert!(columns[135..].iter().all(|p| p.max.abs() < 0.01));
}

#[test]
fn columns_pick_a_level_and_cover_the_requested_range() {
    let audio = white_noise(5, 0.5, 4.0, RATE);
    let overview = WaveformOverview::build(&audio, RATE, &WaveformOptions::default()).unwrap();
    let sizes: Vec<usize> = overview.levels.iter().map(|l| l.samples_per_peak).collect();
    assert_eq!(sizes, [64, 256, 1024]);
    assert_eq!(overview.level_for(1.0).samples_per_peak, 64);
    assert_eq!(overview.level_for(64.0).samples_per_peak, 64);
    assert_eq!(overview.level_for(300.0).samples_per_peak, 256);
    assert_eq!(overview.level_for(1e9).samples_per_peak, 1024);

    // Columns that line up with blocks are exact at every zoom.
    for per_column in [64usize, 256, 512, 4096] {
        let start = 8192;
        let columns = overview.columns(start, start + 40 * per_column, 40);
        assert_eq!(columns.len(), 40);
        for (index, column) in columns.iter().enumerate() {
            let from = start + index * per_column;
            assert_peak(
                *column,
                direct(&audio[from..from + per_column]),
                "aligned column",
            );
        }
    }
    // Zoomed in past the finest level, a column shows the block around it.
    let columns = overview.columns(6400, 6464, 8);
    assert!(columns.iter().all(|c| *c == overview.levels[0].peaks[100]));

    // Columns that do not line up are widened to blocks of the finest
    // level, whatever the zoom, and no further.
    let columns = overview.columns(1000, 1000 + 30 * 3000, 30);
    for (index, column) in columns.iter().enumerate() {
        let from = (1000 + index * 3000) / 64 * 64;
        let to = (1000 + (index + 1) * 3000).div_ceil(64) * 64;
        assert_peak(*column, direct(&audio[from..to]), "unaligned column");
    }
    assert_peak(
        overview.range(0, audio.len()),
        direct(&audio),
        "whole range",
    );

    // Past the end of the audio there is nothing.
    let beyond = overview.columns(audio.len(), audio.len() + 1000, 4);
    assert_eq!(beyond, vec![Peak::default(); 4]);
    assert!(overview.columns(10, 10, 5).is_empty());
    assert!(overview.columns(0, 100, 0).is_empty());
}

#[test]
fn empty_audio_buffers_and_bad_options() {
    let empty = WaveformOverview::build(&[], RATE, &WaveformOptions::default()).unwrap();
    assert_eq!(empty.levels.len(), 1);
    assert!(empty.levels[0].peaks.is_empty());
    assert!(empty
        .columns(0, 100, 3)
        .iter()
        .all(|p| *p == Peak::default()));

    let buffer = AudioBuffer::new(RATE, vec![sine(100.0, 0.5, 0.5, RATE), vec![0.0; 24_000]]);
    let both = WaveformOverview::of_buffer(&buffer).unwrap();
    assert_eq!(both.len(), 2);
    assert!(both[0].levels[0].peaks[10].max > 0.4);
    assert_eq!(both[1].levels[0].peaks[10], Peak::default());

    for options in [
        WaveformOptions {
            base_samples_per_peak: 0,
            ..WaveformOptions::default()
        },
        WaveformOptions {
            factor: 1,
            ..WaveformOptions::default()
        },
    ] {
        assert!(matches!(
            WaveformOverview::build(&[0.0; 10], RATE, &options),
            Err(AnalysisError::InvalidOption { .. })
        ));
    }
}

/// Sixteen bars of drums at `bpm` with an A minor progression over them.
fn track(bpm: f64) -> Vec<f32> {
    let spec = Loop::new(bpm, 64).offset(0.1);
    let bar = 4.0 * spec.period();
    let mut harmony = vec![0.0f32; frames(spec.seconds(), RATE)];
    // i, iv, v, i in A minor, with the root in the bass.
    let chords: [[f64; 3]; 4] = [
        [57.0, 60.0, 64.0],
        [62.0, 65.0, 69.0],
        [64.0, 67.0, 71.0],
        [57.0, 60.0, 64.0],
    ];
    for index in 0..16 {
        let pitches = chords[index % 4];
        let time = 0.1 + index as f64 * bar;
        place(&mut harmony, &chord(&pitches, 0.12, bar, RATE), time, RATE);
        place(
            &mut harmony,
            &note(pitches[0] - 24.0, 0.25, bar, RATE),
            time,
            RATE,
        );
    }
    // Scaled to leave headroom: the parts add up to more than full scale.
    common::gain(&add(&common::gain(&drum_loop(&spec), 0.6), &harmony), 0.5)
}

#[test]
fn a_track_is_analyzed_in_one_call() {
    let audio = track(124.0);
    let buffer = dual_mono(&audio, RATE);
    let analysis = analyze_track(&buffer, &TrackOptions::default()).unwrap();

    assert_eq!((analysis.sample_rate, analysis.channels), (48_000, 2));
    assert!((analysis.duration_seconds - buffer.duration_seconds()).abs() < 1e-9);
    let tempo = analysis.tempo.expect("a tempo");
    assert!((tempo.bpm() - 124.0).abs() < 0.5, "{}", tempo.bpm());
    let offset = common::circular_difference(tempo.grid.first_beat_seconds, 0.1, 60.0 / 124.0);
    assert!(offset < 0.010, "{offset}");

    let key = analysis.key.expect("a key").key;
    assert_eq!(key, Key::minor(9), "found {key}");
    assert_eq!(key.camelot(), "8A");

    assert!(analysis.levels.integrated_lufs.is_finite());
    assert!(analysis.levels.true_peak_dbtp < 0.0);
    assert_eq!(analysis.levels.correlation, 1.0);
    assert_eq!(analysis.features.tempo_bpm, Some(tempo.bpm()));
    assert!(analysis.features.chroma[9] > 0.3);
}

#[test]
fn a_wav_file_on_disk_gives_the_same_analysis() {
    let buffer = dual_mono(&track(128.0), RATE);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("track.wav");
    write_wav_file(&path, WavSpec::float_stereo(RATE), &buffer.interleaved()).unwrap();

    let from_file = analyze_file(&path).unwrap();
    let direct = analyze_track(&buffer, &TrackOptions::default()).unwrap();
    assert_eq!(from_file, direct);
    assert!((from_file.tempo.unwrap().bpm() - 128.0).abs() < 0.5);
    assert_eq!(from_file.key.unwrap().key, Key::minor(9));

    assert!(matches!(
        analyze_file(&directory.path().join("missing.wav")),
        Err(AnalysisError::Decode(_))
    ));
}

#[test]
fn one_shots_tones_and_silence_have_optional_parts_missing() {
    let options = TrackOptions::default();

    let mut one_shot = vec![0.0f32; frames(0.5, RATE)];
    place(&mut one_shot, &kick(RATE), 0.01, RATE);
    let analysis = analyze_track(&AudioBuffer::new(RATE, vec![one_shot]), &options).unwrap();
    assert!(analysis.tempo.is_none());
    assert_eq!(analysis.channels, 1);

    let pad = chord(&[60.0, 64.0, 67.0], 0.2, 4.0, RATE);
    let analysis = analyze_track(&dual_mono(&pad, RATE), &options).unwrap();
    assert!(analysis.tempo.is_none());
    assert!(analysis.key.is_some());
    assert_eq!(analysis.features.tempo_bpm, None);

    let silent = analyze_track(&dual_mono(&vec![0.0; 96_000], RATE), &options).unwrap();
    assert!(silent.tempo.is_none() && silent.key.is_none());
    assert_eq!(silent.levels.integrated_lufs, f64::NEG_INFINITY);

    assert!(matches!(
        analyze_track(&AudioBuffer::new(RATE, vec![Vec::new()]), &options),
        Err(AnalysisError::Empty)
    ));
    let bad = TrackOptions {
        tempo: TempoOptions {
            min_bpm: -1.0,
            ..TempoOptions::default()
        },
        ..options
    };
    assert!(matches!(
        analyze_track(&dual_mono(&pad, RATE), &bad),
        Err(AnalysisError::InvalidOption { .. })
    ));
}

#[cfg(feature = "serde")]
#[test]
fn results_round_trip_through_json() {
    let buffer = dual_mono(&track(124.0), RATE);
    let analysis = analyze_track(&buffer, &TrackOptions::default()).unwrap();
    let json = serde_json::to_string(&analysis).unwrap();
    let back: fourfloor_analysis::TrackAnalysis = serde_json::from_str(&json).unwrap();
    // serde_json's default float parser can be one unit in the last place
    // out, so numbers are compared with a tolerance.
    let (tempo, tempo_back) = (analysis.tempo.unwrap(), back.tempo.unwrap());
    assert!((tempo_back.bpm() - tempo.bpm()).abs() < 1e-9);
    let first = |estimate: &fourfloor_analysis::TempoEstimate| estimate.grid.first_beat_seconds;
    assert!((first(&tempo_back) - first(&tempo)).abs() < 1e-9);
    assert_eq!(tempo_back.grid.downbeat_index, tempo.grid.downbeat_index);
    assert_eq!(tempo_back.onsets, tempo.onsets);
    assert_eq!(back.key.map(|k| k.key), analysis.key.map(|k| k.key));
    for (a, b) in back.features.chroma.iter().zip(&analysis.features.chroma) {
        assert!((a - b).abs() < 1e-6);
    }
    for (a, b) in back
        .features
        .embedding()
        .iter()
        .zip(&analysis.features.embedding())
    {
        assert!((a - b).abs() < 1e-5);
    }
    assert!((back.levels.integrated_lufs - analysis.levels.integrated_lufs).abs() < 1e-9);

    let overview =
        WaveformOverview::build(&buffer.channels[0], RATE, &WaveformOptions::default()).unwrap();
    let json = serde_json::to_string(&overview).unwrap();
    assert_eq!(
        serde_json::from_str::<WaveformOverview>(&json).unwrap(),
        overview
    );
}
