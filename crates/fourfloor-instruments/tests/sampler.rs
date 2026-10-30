//! Rendered-audio tests for `sampler`, under the allocation guard.

mod common;

use std::sync::Arc;

use common::{cents, crossing_frequency, play, window, RATE};
use fourfloor_core::{Device, DeviceInit, DeviceRegistry, EventKind, ParamId, SampleRate};
use fourfloor_instruments::{SampleData, Sampler, SliceOptions, SAMPLER_KIND};
use fourfloor_testkit::{
    assert_buffer_ok, dominant_frequency, is_silent, notes, null_residual_db, onset_times, peak,
    render_instrument, rms, sine, times_match, white_noise, AllocDisabler, RenderConfig,
    TimedEvent,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

/// A sampler at full level playing `sample`.
fn sampler(sample: SampleData, params: &[(ParamId, f32)]) -> Sampler {
    let mut device = Sampler::with_sample(Arc::new(sample));
    device.set_param(Sampler::LEVEL, 1.0);
    for (id, value) in params {
        device.set_param(*id, *value);
    }
    device
}

fn tone_sample(freq: f64, seconds: f64, rate: SampleRate) -> SampleData {
    SampleData::from_mono(rate, sine(freq, 0.5, seconds, rate)).unwrap()
}

fn frames(seconds: f64) -> usize {
    (seconds * RATE.as_f64()) as usize
}

#[test]
fn plays_the_sample_unchanged_at_its_root_in_stereo() {
    let left = white_noise(1, 0.4, 1.0, RATE);
    let right = sine(330.0, 0.4, 1.0, RATE);
    let data = SampleData::new(RATE, left.clone(), right.clone()).unwrap();
    let out = play(&mut sampler(data, &[]), &[(0.0, 1.2, 60, 127)], 1.5);
    assert_buffer_ok(&out);
    // Everything before the 2 ms fade at the end is the sample itself.
    let n = frames(0.99);
    assert!(null_residual_db(&out.left[..n], &left[..n]) < -100.0);
    assert!(null_residual_db(&out.right[..n], &right[..n]) < -100.0);
    // The sample is over after one second.
    assert!(is_silent(window(&out.left, 1.001, 1.5), -100.0));
    assert!(is_silent(window(&out.right, 1.001, 1.5), -100.0));
    // Velocity and level scale it.
    let data = SampleData::new(RATE, left.clone(), right).unwrap();
    let quiet = play(
        &mut sampler(data, &[(Sampler::LEVEL, 0.5)]),
        &[(0.0, 1.2, 60, 64)],
        1.5,
    );
    let ratio = rms(&quiet.left[..n]) / rms(&left[..n]);
    assert!((ratio - 0.5 * 64.0 / 127.0).abs() < 1e-3, "ratio {ratio}");
}

#[test]
fn transposes_by_the_equal_tempered_ratio() {
    for pitch in [48u8, 53, 60, 67, 72] {
        let mut device = sampler(tone_sample(440.0, 2.0, RATE), &[]);
        let out = play(&mut device, &[(0.0, 0.9, pitch, 100)], 1.0);
        assert_buffer_ok(&out);
        let expected = 440.0 * 2.0f64.powf((f64::from(pitch) - 60.0) / 12.0);
        let found = crossing_frequency(window(&out.left, 0.1, 0.8));
        assert!(
            cents(found, expected).abs() < 1.0,
            "pitch {pitch}: {found} Hz, expected {expected}"
        );
    }
    // Root and tune move the reference.
    let mut device = sampler(
        tone_sample(440.0, 2.0, RATE),
        &[(Sampler::ROOT, 69.0), (Sampler::TUNE, 7.0)],
    );
    let out = play(&mut device, &[(0.0, 0.9, 69, 100)], 1.0);
    let found = crossing_frequency(window(&out.left, 0.1, 0.8));
    assert!(cents(found, 440.0 * 2.0f64.powf(7.0 / 12.0)).abs() < 1.0);
    // An octave up plays twice as fast, so two seconds last one.
    let mut device = sampler(tone_sample(440.0, 2.0, RATE), &[(Sampler::ONE_SHOT, 1.0)]);
    let out = play(&mut device, &[(0.0, 0.1, 72, 100)], 1.5);
    assert!(rms(window(&out.left, 0.8, 0.95)) > 0.2);
    assert!(is_silent(window(&out.left, 1.001, 1.5), -100.0));
}

#[test]
fn a_sample_keeps_its_pitch_whatever_its_rate_and_the_engine_rate() {
    // A 44.1 kHz sample in a 48 kHz engine.
    let data = tone_sample(440.0, 1.0, SampleRate::HZ_44100);
    let out = play(&mut sampler(data, &[]), &[(0.0, 0.9, 60, 100)], 1.0);
    assert_buffer_ok(&out);
    let found = crossing_frequency(window(&out.left, 0.1, 0.8));
    assert!(cents(found, 440.0).abs() < 1.0, "{found} Hz");
    // A 48 kHz sample in a 96 kHz engine.
    let rate = SampleRate::HZ_96000;
    let events = notes(&[(0.0, 0.9, 60, 100)], rate);
    let mut device = sampler(tone_sample(440.0, 1.0, RATE), &[]);
    let out = render_instrument(&mut device, &events, 1.0, &RenderConfig::at_rate(rate));
    assert_buffer_ok(&out);
    let found = f64::from(dominant_frequency(&out.left[9_600..76_800], rate));
    assert!((found - 440.0).abs() < 2.0, "{found} Hz");
}

#[test]
fn one_shot_plays_to_the_end_and_gated_notes_stop_when_released() {
    let gated = play(
        &mut sampler(tone_sample(440.0, 1.0, RATE), &[]),
        &[(0.0, 0.2, 60, 100)],
        1.5,
    );
    assert!(rms(window(&gated.left, 0.05, 0.19)) > 0.2);
    // Default release is 50 ms to -60 dB.
    assert!(is_silent(window(&gated.left, 0.3, 1.5), -90.0));

    let mut device = sampler(tone_sample(440.0, 1.0, RATE), &[(Sampler::ONE_SHOT, 1.0)]);
    let shot = play(&mut device, &[(0.0, 0.2, 60, 100)], 1.5);
    assert_buffer_ok(&shot);
    assert!(rms(window(&shot.left, 0.8, 0.95)) > 0.2);
    assert!(is_silent(window(&shot.left, 1.001, 1.5), -100.0));

    // All-notes-off stops even a one-shot.
    let mut device = sampler(tone_sample(440.0, 1.0, RATE), &[(Sampler::ONE_SHOT, 1.0)]);
    let events = [
        TimedEvent::new(0, EventKind::note_on(60, 100)),
        TimedEvent::new(9_600, EventKind::AllNotesOff),
    ];
    let cut = render_instrument(&mut device, &events, 1.0, &RenderConfig::default());
    assert!(is_silent(window(&cut.left, 0.35, 1.0), -90.0));
}

#[test]
fn a_loop_sounds_for_as_long_as_the_note_and_stays_in_its_region() {
    // 0.25 s of 440 Hz is exactly 110 cycles, so the loop is seamless.
    let plain = play(
        &mut sampler(tone_sample(440.0, 0.25, RATE), &[]),
        &[(0.0, 1.5, 60, 127)],
        2.0,
    );
    assert!(is_silent(window(&plain.left, 0.26, 2.0), -100.0));

    let mut device = sampler(tone_sample(440.0, 0.25, RATE), &[(Sampler::LOOP, 1.0)]);
    let looped = play(&mut device, &[(0.0, 1.5, 60, 127)], 2.0);
    assert_buffer_ok(&looped);
    let held = window(&looped.left, 0.3, 1.45);
    let expected = 0.5 / 2.0f32.sqrt();
    assert!((rms(held) - expected).abs() < 0.005, "rms {}", rms(held));
    assert!(cents(crossing_frequency(held), 440.0).abs() < 0.5);
    // A seamless loop is the endless tone itself.
    let endless = sine(440.0, 0.5, 2.0, RATE);
    let n = frames(1.45);
    assert!(null_residual_db(&looped.left[..n], &endless[..n]) < -80.0);
    assert!(is_silent(window(&looped.left, 1.65, 2.0), -90.0));

    // First half 240 Hz, second half 480 Hz; looping the second half.
    let mut both = sine(240.0, 0.5, 0.125, RATE);
    both.extend(sine(480.0, 0.5, 0.125, RATE));
    let data = SampleData::from_mono(RATE, both).unwrap();
    let mut device = sampler(
        data,
        &[
            (Sampler::LOOP, 1.0),
            (Sampler::LOOP_START, 50.0),
            (Sampler::LOOP_END, 100.0),
        ],
    );
    let out = play(&mut device, &[(0.0, 1.5, 60, 127)], 2.0);
    assert_buffer_ok(&out);
    assert!(cents(crossing_frequency(window(&out.left, 0.01, 0.12)), 240.0).abs() < 2.0);
    let rest = window(&out.left, 0.13, 1.45);
    assert!(cents(crossing_frequency(rest), 480.0).abs() < 0.5);
    assert!((rms(rest) - expected).abs() < 0.005);
}

#[test]
fn the_envelope_shapes_the_note_and_releases_to_silence() {
    let params = [
        (Sampler::ATTACK, 0.2),
        (Sampler::DECAY, 0.1),
        (Sampler::SUSTAIN, 0.5),
        (Sampler::RELEASE, 0.3),
    ];
    let mut device = sampler(tone_sample(440.0, 3.0, RATE), &params);
    let out = play(&mut device, &[(0.0, 1.0, 60, 127)], 2.0);
    assert_buffer_ok(&out);
    let full = 0.5 / 2.0f32.sqrt();
    // Linear attack: half way up after 100 ms.
    let rising = rms(window(&out.left, 0.09, 0.11));
    assert!((rising / full - 0.5).abs() < 0.08, "attack {rising}");
    let top = peak(window(&out.left, 0.19, 0.21));
    assert!((top - 0.5).abs() < 0.03, "top {top}");
    let sustained = rms(window(&out.left, 0.6, 0.99));
    assert!((sustained / full - 0.5).abs() < 0.02, "sustain {sustained}");
    // 300 ms to fall 60 dB: 20 dB down after 100 ms.
    let falling = rms(window(&out.left, 1.09, 1.11));
    let db = 20.0 * (falling / sustained).log10();
    assert!((db + 20.0).abs() < 2.0, "release {db} dB after 100 ms");
    assert!(is_silent(window(&out.left, 1.45, 2.0), -100.0));
}

/// A two-second loop of decaying sine hits and where they start.
fn drum_loop() -> (Vec<f32>, [(f64, f64, f32); 5]) {
    let hits = [
        (0.0, 100.0, 0.9f32),
        (0.5, 2_000.0, 0.5),
        (1.0, 300.0, 0.8),
        (1.25, 5_000.0, 0.5),
        (1.5, 800.0, 0.7),
    ];
    let mut signal = vec![0.0f32; frames(2.0)];
    for (at, freq, level) in hits {
        let start = frames(at);
        for (n, slot) in signal[start..].iter_mut().enumerate().take(frames(0.24)) {
            let t = n as f64 / RATE.as_f64();
            let value = (-t * 50.0).exp() * (std::f64::consts::TAU * freq * t).sin();
            *slot += level * value as f32;
        }
    }
    (signal, hits)
}

#[test]
fn slices_land_on_the_onsets_of_a_loop_and_play_on_consecutive_notes() {
    let (signal, hits) = drum_loop();
    let data = SampleData::from_mono(RATE, signal.clone())
        .unwrap()
        .with_detected_slices(&SliceOptions::default());
    let starts = data.slices().to_vec();
    assert_eq!(starts.len(), hits.len(), "slices {starts:?}");
    for (start, (at, ..)) in starts.iter().zip(hits) {
        // Within two milliseconds.
        assert!(
            start.abs_diff(frames(at)) <= 96,
            "slice at {start} for a hit at {}",
            frames(at)
        );
    }

    let params = [(Sampler::MODE, 1.0), (Sampler::ONE_SHOT, 1.0)];
    for (index, (_, freq, level)) in hits.iter().enumerate() {
        let mut device = sampler(data.clone(), &params);
        let out = play(&mut device, &[(0.0, 0.05, 36 + index as u8, 127)], 0.6);
        assert_buffer_ok(&out);
        let found = f64::from(dominant_frequency(&out.left[..frames(0.25)], RATE));
        assert!(
            (found - freq).abs() < 0.03 * freq + 8.0,
            "slice {index}: {found} Hz, expected {freq}"
        );
        assert!(peak(&out.left) > 0.5 * level, "slice {index}");
        // The note plays the loop from the slice's start, unchanged.
        let (start, end) = data.slice_range(index).unwrap();
        let n = end - start - 200;
        assert!(null_residual_db(&out.left[..n], &signal[start..start + n]) < -100.0);
        assert!(is_silent(&out.left[end - start..], -100.0));
    }
    // Notes outside the slices make no sound.
    for pitch in [35u8, 41, 100] {
        let out = play(
            &mut sampler(data.clone(), &params),
            &[(0.0, 0.3, pitch, 127)],
            0.5,
        );
        assert!(is_silent(&out.left, -120.0), "pitch {pitch}");
    }
    // Slice base moves the mapping.
    let mut device = sampler(data.clone(), &params);
    device.set_param(Sampler::SLICE_BASE, 60.0);
    let out = play(&mut device, &[(0.0, 0.05, 61, 127)], 0.5);
    let found = dominant_frequency(&out.left[..frames(0.25)], RATE);
    assert!((found - 2_000.0).abs() < 70.0, "{found} Hz");

    // Triggering every slice at its own start rebuilds the loop.
    let events: Vec<TimedEvent> = starts
        .iter()
        .enumerate()
        .map(|(index, start)| {
            TimedEvent::new(*start as u64, EventKind::note_on(36 + index as u8, 127))
        })
        .collect();
    let mut device = sampler(data, &params);
    let rebuilt = render_instrument(&mut device, &events, 2.0, &RenderConfig::default());
    assert_buffer_ok(&rebuilt);
    let expected: Vec<f64> = hits.iter().map(|(at, ..)| *at).collect();
    let found = onset_times(&rebuilt.left, RATE);
    assert!(times_match(&found, &expected, 0.005), "onsets {found:?}");
    // Away from the slice boundaries the rebuilt loop is the loop. (At a
    // boundary a slice detected a few samples late loses those samples to
    // the 2 ms fade that ends the slice before it.)
    let (mut a, mut b) = (rebuilt.left.clone(), signal.clone());
    for (at, ..) in hits {
        let centre = frames(at);
        let range = centre.saturating_sub(144)..(centre + 144).min(a.len());
        a[range.clone()].fill(0.0);
        b[range].fill(0.0);
    }
    let residual = null_residual_db(&a, &b);
    assert!(residual < -60.0, "residual {residual} dB");
}

#[test]
fn slices_set_by_hand_can_be_retuned_and_looped() {
    let mut both = sine(240.0, 0.5, 0.125, RATE);
    both.extend(sine(480.0, 0.5, 0.125, RATE));
    let data = SampleData::from_mono(RATE, both)
        .unwrap()
        .with_slices(vec![0, 6_000]);
    let params = [(Sampler::MODE, 1.0), (Sampler::LOOP, 1.0)];
    let out = play(
        &mut sampler(data.clone(), &params),
        &[(0.0, 1.0, 37, 127)],
        1.5,
    );
    assert_buffer_ok(&out);
    let held = window(&out.left, 0.2, 0.95);
    assert!(cents(crossing_frequency(held), 480.0).abs() < 0.5);
    assert!(is_silent(window(&out.left, 1.2, 1.5), -90.0));
    // In slice mode the note does not transpose; tune does.
    let mut device = sampler(data, &params);
    device.set_param(Sampler::TUNE, -12.0);
    let out = play(&mut device, &[(0.0, 1.0, 36, 127)], 1.5);
    let held = window(&out.left, 0.2, 0.95);
    assert!(cents(crossing_frequency(held), 120.0).abs() < 1.0);
}

#[test]
fn chords_sum_and_extra_notes_steal_voices_without_breaking() {
    let make = || sampler(tone_sample(220.0, 1.0, RATE), &[(Sampler::LEVEL, 0.3)]);
    let chord = [
        (0.0, 0.5, 60u8, 100u8),
        (0.0, 0.5, 64, 100),
        (0.1, 0.5, 67, 100),
    ];
    let together = play(&mut make(), &chord, 1.0);
    assert_buffer_ok(&together);
    let mut sum = vec![0.0f32; together.frames()];
    for spec in chord {
        let one = play(&mut make(), &[spec], 1.0);
        for (slot, x) in sum.iter_mut().zip(&one.left) {
            *slot += x;
        }
    }
    assert!(null_residual_db(&together.left, &sum) < -100.0);

    // More notes than voices.
    let specs: Vec<(f64, f64, u8, u8)> = (0..24u8)
        .map(|n| (0.01 * f64::from(n), 0.5, 40 + n, 30))
        .collect();
    let mut device = sampler(tone_sample(220.0, 1.0, RATE), &[(Sampler::LEVEL, 0.1)]);
    let out = play(&mut device, &specs, 1.5);
    assert_buffer_ok(&out);
    assert!(rms(window(&out.left, 0.3, 0.45)) > 0.01);
    assert!(is_silent(window(&out.left, 1.0, 1.5), -90.0));
}

#[test]
fn renders_the_same_at_any_block_size_and_is_silent_without_a_sample() {
    let (signal, _) = drum_loop();
    let make = || {
        let data = SampleData::from_mono(RATE, signal.clone()).unwrap();
        sampler(data, &[(Sampler::TUNE, 3.0), (Sampler::RELEASE, 0.2)])
    };
    let specs = [(0.013, 0.4, 60u8, 100u8), (0.2, 0.3, 55, 80)];
    let events = notes(&specs, RATE);
    let a = render_instrument(&mut make(), &events, 1.0, &RenderConfig::default());
    for block in [1usize, 64, 333] {
        let config = RenderConfig::default().with_block(block);
        let b = render_instrument(&mut make(), &events, 1.0, &config);
        assert!(null_residual_db(&a.left, &b.left) < -120.0, "block {block}");
    }

    let mut empty = Sampler::new();
    let out = play(&mut empty, &specs, 0.5);
    assert_buffer_ok(&out);
    assert!(is_silent(&out.left, -120.0));

    // Out-of-range and non-finite parameter values are clamped or ignored.
    let mut device = make();
    for info in device.params().to_vec() {
        device.set_param(info.id, f32::NAN);
        device.set_param(info.id, 1.0e9);
        device.set_param(info.id, -1.0e9);
    }
    device.set_param(ParamId(999), 1.0);
    let out = play(&mut device, &specs, 0.5);
    assert_buffer_ok(&out);
}

#[test]
fn a_loader_swaps_samples_while_running_and_takes_the_old_ones_back() {
    let (mut device, mut loader) = Sampler::with_loader();
    device.set_param(Sampler::LEVEL, 1.0);
    let specs = [(0.05, 0.3, 60u8, 100u8)];
    let out = play(&mut device, &specs, 0.5);
    assert!(is_silent(&out.left, -120.0));

    let first = Arc::new(tone_sample(300.0, 1.0, RATE));
    let second = Arc::new(tone_sample(700.0, 1.0, RATE));
    loader.load(Arc::clone(&first)).unwrap();
    let out = play(&mut device, &specs, 0.5);
    assert_buffer_ok(&out);
    assert!(cents(crossing_frequency(window(&out.left, 0.1, 0.3)), 300.0).abs() < 1.0);
    assert_eq!(loader.collect(), 0);

    // The swap, and handing the old sample back, happen inside `process`
    // under the allocation guard.
    loader.load(Arc::clone(&second)).unwrap();
    let out = play(&mut device, &specs, 0.5);
    assert_buffer_ok(&out);
    assert!(cents(crossing_frequency(window(&out.left, 0.1, 0.3)), 700.0).abs() < 1.0);
    assert_eq!(loader.collect(), 1);
    // The device no longer holds the first sample.
    assert_eq!(Arc::strong_count(&first), 1);

    // The queue is bounded; a full one hands the sample back.
    let mut accepted = 0;
    for _ in 0..SampleLoaderProbe::TRIES {
        if loader.load(Arc::clone(&first)).is_ok() {
            accepted += 1;
        }
    }
    assert_eq!(accepted, fourfloor_instruments::SampleLoader::CAPACITY);
    let out = play(&mut device, &specs, 0.5);
    assert_buffer_ok(&out);
    assert!(cents(crossing_frequency(window(&out.left, 0.1, 0.3)), 300.0).abs() < 1.0);
    assert!(loader.collect() >= 1);
    drop(device);
    drop(loader);
    assert_eq!(Arc::strong_count(&first), 1);
    assert_eq!(Arc::strong_count(&second), 1);
}

struct SampleLoaderProbe;

impl SampleLoaderProbe {
    const TRIES: usize = 10;
}

#[test]
fn the_registry_builds_a_sampler_from_an_audio_file_in_its_state() {
    let mut registry = DeviceRegistry::new();
    fourfloor_instruments::register(&mut registry).unwrap();
    let tone = sine(330.0, 0.5, 1.0, RATE);
    let interleaved: Vec<f32> = tone.iter().flat_map(|x| [*x, -*x]).collect();
    let spec = fourfloor_audio::WavSpec::float_stereo(RATE);
    let bytes = fourfloor_audio::encode_wav(spec, &interleaved).unwrap();
    let params = [(Sampler::LEVEL, 1.0), (Sampler::ROOT, 48.0)];
    let init = DeviceInit::new().with_params(&params).with_state(&bytes);
    let mut device = registry.create(SAMPLER_KIND, &init).unwrap();
    let out = play(device.as_mut(), &[(0.0, 0.9, 48, 127)], 1.0);
    assert_buffer_ok(&out);
    let n = frames(0.9);
    assert!(null_residual_db(&out.left[..n], &tone[..n]) < -100.0);
    // The right channel of the file is the left inverted.
    let mid: Vec<f32> = out
        .left
        .iter()
        .zip(&out.right)
        .map(|(l, r)| l + r)
        .collect();
    assert!(is_silent(&mid, -100.0));

    // A file on disk loads the same way.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tone.wav");
    std::fs::write(&path, &bytes).unwrap();
    let data = SampleData::load(&path).unwrap();
    assert_eq!(data.frames(), tone.len());
    assert_eq!(data.sample_rate(), RATE);
    assert!(SampleData::load(&dir.path().join("missing.wav")).is_err());
}

#[test]
fn a_registry_sampler_in_slice_mode_plays_the_detected_slices_of_its_file() {
    // The sampler as the app, CLI and agent build it: from a file's bytes.
    let mut registry = DeviceRegistry::new();
    fourfloor_instruments::register(&mut registry).unwrap();
    let (signal, hits) = drum_loop();
    let interleaved: Vec<f32> = signal.iter().flat_map(|x| [*x, *x]).collect();
    let spec = fourfloor_audio::WavSpec::float_stereo(RATE);
    let bytes = fourfloor_audio::encode_wav(spec, &interleaved).unwrap();
    let params = [
        (Sampler::LEVEL, 1.0),
        (Sampler::MODE, 1.0),
        (Sampler::ONE_SHOT, 1.0),
    ];
    let init = DeviceInit::new().with_params(&params).with_state(&bytes);
    for (index, (_, freq, _)) in hits.iter().enumerate() {
        let mut device = registry.create(SAMPLER_KIND, &init).unwrap();
        let out = play(device.as_mut(), &[(0.0, 0.05, 36 + index as u8, 127)], 0.4);
        assert_buffer_ok(&out);
        let found = f64::from(dominant_frequency(&out.left[..frames(0.2)], RATE));
        assert!(
            (found - freq).abs() < 0.03 * freq + 8.0,
            "slice {index}: {found} Hz, expected {freq}"
        );
    }
}
