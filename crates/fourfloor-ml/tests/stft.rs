//! STFT / inverse STFT round trips, measured as null tests.

use fourfloor_core::SampleRate;
use fourfloor_ml::{Stft, Window};
use fourfloor_testkit::{mix, null_residual_db, sine, white_noise};

fn material() -> Vec<f32> {
    let rate = SampleRate::HZ_44100;
    let noise = white_noise(7, 0.3, 1.3, rate);
    let tone = sine(523.25, 0.4, 1.3, rate);
    let mut signal = mix(&[&noise, &tone]);
    signal[0] = 0.9; // the very first and last samples must survive too
    let last = signal.len() - 1;
    signal[last] = -0.9;
    signal
}

#[test]
fn forward_then_inverse_nulls_for_every_window_and_hop() {
    let signal = material();
    for window in [Window::Hann, Window::SqrtHann] {
        for (fft, hop) in [
            (4096, 1024),
            (2048, 512),
            (1024, 256),
            (512, 256),
            (1000, 250),
        ] {
            let stft = Stft::new(fft, hop, window).expect("stft");
            let spectrum = stft.forward(&signal);
            assert_eq!(spectrum.frames, stft.frames_for(signal.len()));
            let back = stft.inverse(&spectrum, signal.len()).expect("inverse");
            let residual = null_residual_db(&signal, &back);
            assert!(
                residual < -100.0,
                "{window:?} {fft}/{hop}: residual {residual:.1} dB"
            );
            assert!((back[0] - 0.9).abs() < 1e-4 && (back[back.len() - 1] + 0.9).abs() < 1e-4);
        }
    }
}

#[test]
fn odd_lengths_and_short_signals_round_trip() {
    let signal = material();
    let stft = Stft::new(2048, 512, Window::Hann).expect("stft");
    for len in [1, 7, 511, 2047, 2049, 30_001] {
        let part = &signal[..len];
        let back = stft.inverse(&stft.forward(part), len).expect("inverse");
        assert_eq!(back.len(), len);
        assert!(null_residual_db(part, &back) < -100.0, "length {len}");
    }
}

#[test]
fn a_unity_mask_nulls_and_complementary_masks_sum_to_the_input() {
    let signal = material();
    let stft = Stft::new(2048, 512, Window::Hann).expect("stft");
    let spectrum = stft.forward(&signal);
    let ones = vec![1.0f32; spectrum.data.len()];
    let back = stft
        .inverse(&spectrum.masked(&ones).expect("mask"), signal.len())
        .expect("inverse");
    assert!(null_residual_db(&signal, &back) < -100.0);

    // A low-pass mask and its complement: the two halves add back up.
    let low: Vec<f32> = (0..spectrum.frames)
        .flat_map(|_| (0..spectrum.bins).map(|bin| if bin < 100 { 1.0 } else { 0.25 }))
        .collect();
    let high: Vec<f32> = low.iter().map(|m| 1.0 - m).collect();
    let a = stft
        .inverse(&spectrum.masked(&low).expect("mask"), signal.len())
        .expect("a");
    let b = stft
        .inverse(&spectrum.masked(&high).expect("mask"), signal.len())
        .expect("b");
    let sum: Vec<f32> = a.iter().zip(&b).map(|(x, y)| x + y).collect();
    assert!(null_residual_db(&signal, &sum) < -100.0);
    assert!(
        null_residual_db(&signal, &a) > -20.0,
        "the halves differ from the input"
    );
}

#[test]
fn bins_line_up_with_frequencies() {
    let rate = SampleRate::HZ_44100;
    let tone = sine(1000.0, 0.5, 1.0, rate);
    let stft = Stft::new(4096, 1024, Window::Hann).expect("stft");
    let spectrum = stft.forward(&tone);
    let frame = spectrum.frame(spectrum.frames / 2);
    let peak = (0..spectrum.bins)
        .max_by(|&a, &b| frame[a].norm().total_cmp(&frame[b].norm()))
        .expect("bins");
    let hz = spectrum.bin_hz(peak, rate.as_f64());
    assert!(
        (hz - 1000.0).abs() < rate.as_f64() / 4096.0,
        "peak at {hz} Hz"
    );
    assert!(stft.frame_center(0) < 0.0);
}
