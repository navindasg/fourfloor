//! Level, stereo and spectrum meter tests on signals of known level.

mod common;

use common::{dual_mono, gain, stereo, RATE};
use fourfloor_analysis::{
    measure_levels, spectrum_frames, stereo_correlation, stereo_width, AnalysisError,
    CorrelationMeter, SpectrumAnalyzer, SpectrumOptions,
};
use fourfloor_audio::AudioBuffer;
use fourfloor_core::SampleRate;
use fourfloor_testkit::{sine, white_noise};

fn close(found: f64, expected: f64, tolerance: f64, what: &str) {
    assert!(
        (found - expected).abs() <= tolerance,
        "{what}: found {found}, expected {expected} within {tolerance}"
    );
}

#[test]
fn a_sine_of_known_level_reads_that_loudness() {
    // EBU R 128: a 1 kHz sine on both channels reads its peak level in
    // dBFS as LUFS (-23 dBFS gives -23 LUFS).
    for rate in [SampleRate::HZ_44100, RATE] {
        for (amplitude, db) in [(0.1f32, -20.0), (0.070_795, -23.0), (0.5, -6.02)] {
            let tone = sine(1000.0, amplitude, 5.0, rate);
            let levels = measure_levels(&dual_mono(&tone, rate)).unwrap();
            close(levels.integrated_lufs, db, 0.15, "loudness");
            close(levels.sample_peak_dbfs, db, 0.05, "sample peak");
            close(levels.true_peak_dbtp, db, 0.2, "true peak");
            close(levels.rms_dbfs, db - 3.01, 0.05, "rms");
            close(levels.crest_factor_db, 3.01, 0.05, "crest factor");
            close(levels.loudness_range_lu, 0.0, 0.2, "loudness range");
            assert!(levels.dc_offset.abs() < 1e-3);
            assert!((levels.correlation - 1.0).abs() < 1e-5);
            assert!(levels.width < 1e-6);
        }
    }
}

#[test]
fn gain_moves_loudness_by_the_same_number_of_db() {
    let noise = white_noise(1, 0.3, 5.0, RATE);
    let loud = measure_levels(&dual_mono(&noise, RATE)).unwrap();
    let quiet = measure_levels(&dual_mono(&gain(&noise, 0.5), RATE)).unwrap();
    close(
        loud.integrated_lufs - quiet.integrated_lufs,
        6.02,
        0.05,
        "loudness",
    );
    close(loud.rms_dbfs - quiet.rms_dbfs, 6.02, 0.01, "rms");
    close(loud.crest_factor_db, quiet.crest_factor_db, 0.01, "crest");
    // Uniform noise: peak over RMS is the square root of three.
    close(loud.crest_factor_db, 4.77, 0.1, "crest of uniform noise");
}

#[test]
fn mono_is_measured_as_dual_mono() {
    let tone = sine(1000.0, 0.1, 3.0, RATE);
    let mono = measure_levels(&AudioBuffer::new(RATE, vec![tone.clone()])).unwrap();
    let both = measure_levels(&dual_mono(&tone, RATE)).unwrap();
    close(mono.integrated_lufs, both.integrated_lufs, 1e-9, "loudness");
    assert_eq!(mono.correlation, 1.0);
}

#[test]
fn a_true_peak_between_samples_is_above_the_sample_peak() {
    // A sine at a quarter of the sample rate with a 45 degree phase has
    // every sample at 0.707 of the real peak.
    let amplitude = 0.5f64;
    let tone: Vec<f32> = (0..48_000)
        .map(|n| {
            let phase = std::f64::consts::FRAC_PI_2 * n as f64 + std::f64::consts::FRAC_PI_4;
            (amplitude * phase.sin()) as f32
        })
        .collect();
    let levels = measure_levels(&dual_mono(&tone, RATE)).unwrap();
    close(levels.sample_peak_dbfs, -9.03, 0.05, "sample peak");
    close(levels.true_peak_dbtp, -6.02, 0.3, "true peak");
}

#[test]
fn silence_and_empty_buffers_are_handled() {
    let silent = measure_levels(&dual_mono(&vec![0.0; 48_000], RATE)).unwrap();
    assert_eq!(silent.integrated_lufs, f64::NEG_INFINITY);
    assert_eq!(silent.rms_dbfs, f64::NEG_INFINITY);
    assert_eq!(silent.crest_factor_db, 0.0);
    assert_eq!((silent.correlation, silent.width), (0.0, 0.0));
    assert!(matches!(
        measure_levels(&AudioBuffer::new(RATE, vec![Vec::new(), Vec::new()])),
        Err(AnalysisError::Empty)
    ));
}

#[test]
fn dc_offset_is_reported() {
    let shifted: Vec<f32> = sine(100.0, 0.2, 2.0, RATE)
        .iter()
        .map(|&s| s + 0.1)
        .collect();
    let levels = measure_levels(&dual_mono(&shifted, RATE)).unwrap();
    assert!(
        (levels.dc_offset - 0.1).abs() < 1e-3,
        "{}",
        levels.dc_offset
    );
}

#[test]
fn correlation_and_width_describe_the_stereo_image() {
    let a = white_noise(1, 0.3, 2.0, RATE);
    let b = white_noise(2, 0.3, 2.0, RATE);
    let inverted = gain(&a, -1.0);

    assert!((stereo_correlation(&a, &a) - 1.0).abs() < 1e-6);
    assert!((stereo_correlation(&a, &inverted) + 1.0).abs() < 1e-6);
    assert!(stereo_correlation(&a, &b).abs() < 0.03);
    assert!((stereo_correlation(&a, &gain(&a, 0.25)) - 1.0).abs() < 1e-6);
    assert_eq!(stereo_correlation(&a, &vec![0.0; a.len()]), 0.0);

    assert!(stereo_width(&a, &a) < 1e-9);
    assert!((stereo_width(&a, &inverted) - 1.0).abs() < 1e-6);
    assert!((stereo_width(&a, &b) - 0.5).abs() < 0.03);
    // Hard left: mid and side carry the same energy.
    assert!((stereo_width(&a, &vec![0.0; a.len()]) - 0.5).abs() < 1e-6);

    // Half the side signal of unrelated channels: narrower, more correlated.
    let narrow_left: Vec<f32> = a
        .iter()
        .zip(&b)
        .map(|(&x, &y)| 0.75 * x + 0.25 * y)
        .collect();
    let narrow_right: Vec<f32> = a
        .iter()
        .zip(&b)
        .map(|(&x, &y)| 0.25 * x + 0.75 * y)
        .collect();
    let wide = measure_levels(&stereo(a.clone(), b.clone(), RATE)).unwrap();
    let narrow = measure_levels(&stereo(narrow_left, narrow_right, RATE)).unwrap();
    assert!(
        narrow.width < wide.width - 0.2,
        "{} {}",
        narrow.width,
        wide.width
    );
    assert!(narrow.correlation > wide.correlation + 0.4);
}

#[test]
fn the_correlation_meter_follows_the_signal() {
    let a = white_noise(1, 0.3, 1.0, RATE);
    let b = white_noise(2, 0.3, 1.0, RATE);
    let inverted = gain(&a, -1.0);
    let mut meter = CorrelationMeter::new(RATE, 0.1);
    assert_eq!(meter.value(), 0.0);

    for block in a.chunks(480) {
        meter.process(block, block);
    }
    assert!(meter.value() > 0.999);

    // Out of phase: one block of 10 ms is not enough to flip a 100 ms
    // window, a second of it is.
    meter.process(&a[..480], &inverted[..480]);
    assert!(meter.value() > 0.0);
    for (left, right) in a.chunks(480).zip(inverted.chunks(480)) {
        meter.process(left, right);
    }
    assert!(meter.value() < -0.999);

    for (left, right) in a.chunks(480).zip(b.chunks(480)) {
        meter.process(left, right);
    }
    assert!(meter.value().abs() < 0.25, "{}", meter.value());

    meter.reset();
    assert_eq!(meter.value(), 0.0);
}

#[test]
fn a_full_scale_sine_reads_zero_db_in_its_band() {
    let options = SpectrumOptions::default();
    let analyzer = SpectrumAnalyzer::new(RATE, options).unwrap();
    let centres = analyzer.band_centers_hz();
    let edges = analyzer.band_edges_hz();
    assert_eq!(centres.len(), 64);
    assert_eq!(edges.len(), 65);
    assert!((edges[0] - 20.0).abs() < 1e-9 && (edges[64] - 20_000.0).abs() < 1e-6);
    assert!(edges.windows(2).all(|pair| pair[1] > pair[0]));

    // The Hann window spreads a tone over four bins. At 100 Hz the default
    // bands are narrower than that, so the exact reading needs a longer FFT.
    let long = SpectrumOptions {
        fft_size: 32_768,
        ..options
    };
    for (target, options) in [
        (100.0, long),
        (500.0, options),
        (1000.0, options),
        (8000.0, options),
    ] {
        let band = centres.iter().position(|&centre| centre >= target).unwrap();
        for (amplitude, expected) in [(1.0f32, 0.0f32), (0.1, -20.0)] {
            let tone = sine(centres[band], amplitude, 1.0, RATE);
            let frames = spectrum_frames(&tone, RATE, options).unwrap();
            assert!(!frames.is_empty());
            for frame in &frames {
                let level = frame.bands_db[band];
                assert!(
                    (level - expected).abs() < 0.5,
                    "{} Hz: {level} dB, expected {expected}",
                    centres[band]
                );
                // It is the loudest band, and bands far away are far down.
                let loudest = frame
                    .bands_db
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap()
                    .0;
                assert_eq!(loudest, band);
                let far = (band + 16) % 64;
                assert!(
                    frame.bands_db[far] < expected - 60.0,
                    "{}",
                    frame.bands_db[far]
                );
            }
        }
    }
}

#[test]
fn a_tone_in_a_band_narrower_than_the_window_is_shared_with_its_neighbours() {
    // The documented limit: at 4096 points a 100 Hz band is one bin wide,
    // so it reads low, but the bands together still hold the whole tone.
    let analyzer = SpectrumAnalyzer::new(RATE, SpectrumOptions::default()).unwrap();
    let centres = analyzer.band_centers_hz();
    let band = centres.iter().position(|&centre| centre >= 100.0).unwrap();
    let tone = sine(centres[band], 1.0, 1.0, RATE);
    for frame in spectrum_frames(&tone, RATE, SpectrumOptions::default()).unwrap() {
        assert!(
            (-6.0..-0.5).contains(&frame.bands_db[band]),
            "{}",
            frame.bands_db[band]
        );
        let total: f64 = frame
            .bands_db
            .iter()
            .map(|&db| 10f64.powf(f64::from(db) / 10.0))
            .sum();
        assert!((10.0 * total.log10()).abs() < 0.2, "{total}");
    }
}

#[test]
fn white_noise_fills_every_band_and_rises_with_band_width() {
    let noise = white_noise(4, 0.5, 4.0, RATE);
    let frames = spectrum_frames(&noise, RATE, SpectrumOptions::default()).unwrap();
    let mut mean = vec![0.0f64; 64];
    for frame in &frames {
        for (sum, &db) in mean.iter_mut().zip(&frame.bands_db) {
            *sum += f64::from(db) / frames.len() as f64;
        }
    }
    // Bands are a constant fraction of an octave, so white noise gains
    // 10 dB per decade: 20 dB from the band at 100 Hz to the one at 10 kHz.
    let analyzer = SpectrumAnalyzer::new(RATE, SpectrumOptions::default()).unwrap();
    let centres = analyzer.band_centers_hz();
    let at = |hz: f64| centres.iter().position(|&c| c >= hz).unwrap();
    let rise = mean[at(10_000.0)] - mean[at(100.0)];
    let decades = (centres[at(10_000.0)] / centres[at(100.0)]).log10();
    assert!(
        (rise - 10.0 * decades).abs() < 2.5,
        "{rise} dB over {decades} decades"
    );
    // The band powers add up to the power of the noise in 20 Hz to 20 kHz:
    // uniform noise of amplitude 0.5 has a mean square of 1/12, of which
    // the bands cover 19980 of 24000 Hz. Band levels are twice the power.
    let total: f64 = (0..64)
        .map(|band| {
            frames
                .iter()
                .map(|frame| 10f64.powf(f64::from(frame.bands_db[band]) / 10.0))
                .sum::<f64>()
                / frames.len() as f64
        })
        .sum();
    let expected = 2.0 * (0.25 / 3.0) * (19_980.0 / 24_000.0);
    assert!(
        (total / expected - 1.0).abs() < 0.05,
        "{total} against {expected}"
    );
}

#[test]
fn frames_arrive_every_hop_whatever_the_block_size() {
    let options = SpectrumOptions {
        fft_size: 1024,
        hop: 256,
        bands: 24,
        ..SpectrumOptions::default()
    };
    let audio = white_noise(8, 0.4, 0.5, RATE);
    let whole = spectrum_frames(&audio, RATE, options).unwrap();
    assert_eq!(whole.len(), (audio.len() - 1024) / 256 + 1);
    for (index, frame) in whole.iter().enumerate() {
        let end = 1024 + index * 256;
        assert!((frame.time_seconds - end as f64 / 48_000.0).abs() < 1e-9);
        assert_eq!(frame.bands_db.len(), 24);
    }

    let mut analyzer = SpectrumAnalyzer::new(RATE, options).unwrap();
    let mut streamed = Vec::new();
    for block in audio.chunks(333) {
        analyzer.push(block, |frame| streamed.push(frame));
    }
    assert_eq!(streamed, whole);

    analyzer.reset();
    let mut again = Vec::new();
    analyzer.push(&audio, |frame| again.push(frame));
    assert_eq!(again, whole);
    assert_eq!(analyzer.options().hop, 256);
}

#[test]
fn silence_reads_the_floor_and_short_input_gives_no_frames() {
    let options = SpectrumOptions {
        floor_db: -90.0,
        ..SpectrumOptions::default()
    };
    let frames = spectrum_frames(&vec![0.0; 8192], RATE, options).unwrap();
    assert_eq!(frames.len(), 5);
    assert!(frames
        .iter()
        .all(|f| f.bands_db.iter().all(|&db| db == -90.0)));
    assert!(spectrum_frames(&[0.5; 100], RATE, options)
        .unwrap()
        .is_empty());
}

#[test]
fn invalid_spectrum_options_are_rejected() {
    let base = SpectrumOptions::default();
    for options in [
        SpectrumOptions {
            fft_size: 1000,
            ..base
        },
        SpectrumOptions {
            fft_size: 32,
            ..base
        },
        SpectrumOptions { hop: 0, ..base },
        SpectrumOptions { hop: 8192, ..base },
        SpectrumOptions { bands: 0, ..base },
        SpectrumOptions {
            min_hz: 0.0,
            ..base
        },
        SpectrumOptions {
            max_hz: 10.0,
            ..base
        },
        SpectrumOptions {
            min_hz: f64::NAN,
            ..base
        },
    ] {
        assert!(matches!(
            SpectrumAnalyzer::new(RATE, options),
            Err(AnalysisError::InvalidOption { .. })
        ));
    }
    // The top edge is limited to half the sample rate.
    let low_rate = SpectrumAnalyzer::new(SampleRate::HZ_44100, base).unwrap();
    assert!((low_rate.band_edges_hz()[64] - 20_000.0).abs() < 1e-6);
}
