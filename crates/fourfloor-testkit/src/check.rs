//! Pass/fail checks: null tests, NaN and clipping detection, and loudness.

use fourfloor_audio::{measure, Loudness};

use crate::buffer::StereoBuffer;
use crate::level::{peak, to_db};

/// How much is left when `b` is subtracted from `a`: the peak of the
/// difference in dB relative to the louder of the two signals. Negative
/// infinity when they are identical (including both silent); positive
/// infinity when their lengths differ or either holds a NaN, so that such
/// a pair can never pass a null test.
pub fn null_residual_db(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return f32::INFINITY;
    }
    let reference = peak(a).max(peak(b));
    let difference = a
        .iter()
        .zip(b)
        .fold(0.0f32, |worst, (x, y)| worst.max((x - y).abs()));
    if reference.is_nan() || difference.is_nan() || peak(a).is_nan() || peak(b).is_nan() {
        return f32::INFINITY;
    }
    if difference == 0.0 {
        return f32::NEG_INFINITY;
    }
    to_db(difference / reference)
}

/// A null test: whether `a` minus `b` cancels to `tolerance_db` or better
/// (for example `-90.0`), relative to the louder signal. Use it to prove
/// two renders are the same, that a bypassed effect changes nothing, or
/// that delay compensation lines two paths up.
pub fn null_test(a: &[f32], b: &[f32], tolerance_db: f32) -> bool {
    null_residual_db(a, b) <= tolerance_db
}

/// Panics, naming the residual, unless [`null_test`] passes.
#[track_caller]
pub fn assert_null(a: &[f32], b: &[f32], tolerance_db: f32) {
    let residual = null_residual_db(a, b);
    assert!(
        residual <= tolerance_db,
        "null test failed: residual {residual:.1} dB, allowed {tolerance_db:.1} dB \
         (lengths {} and {})",
        a.len(),
        b.len()
    );
}

/// A sample that should not be in a render.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fault {
    /// A NaN or infinite sample.
    NotFinite {
        /// Index of the sample.
        index: usize,
    },
    /// A sample beyond full scale.
    Clipped {
        /// Index of the sample.
        index: usize,
        /// Its value.
        value: f32,
    },
}

/// The first NaN, infinite or beyond-full-scale (`|x| > 1.0`) sample, if
/// any.
pub fn find_nan_or_clipping(signal: &[f32]) -> Option<Fault> {
    signal
        .iter()
        .enumerate()
        .find_map(|(index, sample)| match *sample {
            value if !value.is_finite() => Some(Fault::NotFinite { index }),
            value if value.abs() > 1.0 => Some(Fault::Clipped { index, value }),
            _ => None,
        })
}

/// Panics if `signal` holds a NaN, an infinity or a sample beyond full
/// scale (`|x| > 1.0`). Run it on every render.
#[track_caller]
pub fn assert_no_nan_or_clipping(signal: &[f32]) {
    match find_nan_or_clipping(signal) {
        None => {}
        Some(Fault::NotFinite { index }) => {
            panic!(
                "sample {index} is not finite ({})",
                signal.get(index).copied().unwrap_or(f32::NAN)
            )
        }
        Some(Fault::Clipped { index, value }) => {
            panic!("sample {index} clips: {value}")
        }
    }
}

/// [`assert_no_nan_or_clipping`] on both channels of a render.
#[track_caller]
pub fn assert_buffer_ok(buffer: &StereoBuffer) {
    assert_no_nan_or_clipping(&buffer.left);
    assert_no_nan_or_clipping(&buffer.right);
}

/// EBU R 128 loudness and peaks of a render. Panics if the render is empty
/// or its channels differ in length, which a [`StereoBuffer`] built by this
/// crate never is.
#[track_caller]
pub fn loudness(buffer: &StereoBuffer) -> Loudness {
    match measure(&buffer.channels(), buffer.sample_rate) {
        Ok(loudness) => loudness,
        Err(error) => panic!("cannot measure loudness: {error}"),
    }
}

/// Integrated loudness of a render in LUFS. Negative infinity for silence
/// and for renders shorter than 400 ms.
#[track_caller]
pub fn lufs(buffer: &StereoBuffer) -> f64 {
    loudness(buffer).integrated_lufs
}

/// True (inter-sample) peak of a render in dBTP.
#[track_caller]
pub fn true_peak_dbtp(buffer: &StereoBuffer) -> f64 {
    loudness(buffer).true_peak_dbtp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::{dc, sine, white_noise};
    use fourfloor_core::SampleRate;

    const RATE: SampleRate = SampleRate::HZ_48000;

    #[test]
    fn identical_signals_null_completely() {
        let noise = white_noise(5, 0.5, 0.1, RATE);
        assert_eq!(null_residual_db(&noise, &noise), f32::NEG_INFINITY);
        assert!(null_test(&noise, &noise, -200.0));
        assert!(null_test(&[], &[], -200.0));
        assert!(null_test(&dc(0.0, 8), &dc(0.0, 8), -200.0));
        assert_null(&noise, &noise, -120.0);
    }

    #[test]
    fn the_residual_is_relative_to_the_louder_signal() {
        let tone = sine(1_000.0, 0.5, 0.1, RATE);
        // 0.1 % louder: the difference is 60 dB down.
        let louder: Vec<f32> = tone.iter().map(|s| s * 1.001).collect();
        let residual = null_residual_db(&tone, &louder);
        assert!((residual + 60.0).abs() < 0.1, "{residual}");
        assert!(null_test(&tone, &louder, -59.0));
        assert!(!null_test(&tone, &louder, -61.0));
        // Against silence nothing cancels: 0 dB.
        assert_eq!(null_residual_db(&tone, &dc(0.0, tone.len())), 0.0);
    }

    #[test]
    fn a_one_sample_shift_does_not_null() {
        let tone = sine(1_000.0, 0.5, 0.1, RATE);
        let mut shifted = vec![0.0];
        shifted.extend_from_slice(&tone[..tone.len() - 1]);
        assert!(!null_test(&tone, &shifted, -40.0));
    }

    #[test]
    fn length_mismatch_and_nan_never_pass() {
        assert!(!null_test(&[0.0; 4], &[0.0; 5], 0.0));
        assert!(!null_test(&[0.0, f32::NAN], &[0.0, f32::NAN], 100.0));
        assert!(!null_test(&[0.0, 0.5], &[0.0, f32::NAN], 100.0));
    }

    #[test]
    #[should_panic(expected = "null test failed: residual 0.0 dB")]
    fn assert_null_reports_the_residual() {
        assert_null(&[0.5, 0.5], &[0.0, 0.0], -60.0);
    }

    #[test]
    fn faults_are_found_at_their_index() {
        assert_eq!(find_nan_or_clipping(&[0.0, 1.0, -1.0]), None);
        assert_eq!(
            find_nan_or_clipping(&[0.0, f32::NAN]),
            Some(Fault::NotFinite { index: 1 })
        );
        assert_eq!(
            find_nan_or_clipping(&[f32::INFINITY]),
            Some(Fault::NotFinite { index: 0 })
        );
        assert_eq!(
            find_nan_or_clipping(&[0.5, 0.5, -1.25]),
            Some(Fault::Clipped {
                index: 2,
                value: -1.25
            })
        );
        assert_no_nan_or_clipping(&sine(440.0, 1.0, 0.1, RATE));
        assert_buffer_ok(&StereoBuffer::silent(RATE, 16));
    }

    #[test]
    #[should_panic(expected = "sample 3 clips: 1.5")]
    fn clipping_panics_with_the_sample() {
        assert_no_nan_or_clipping(&[0.0, 0.0, 0.0, 1.5]);
    }

    #[test]
    #[should_panic(expected = "sample 1 is not finite")]
    fn nan_panics_with_the_sample() {
        assert_no_nan_or_clipping(&[0.0, f32::NAN]);
    }

    #[test]
    fn loudness_of_a_reference_tone_is_right() {
        // EBU Tech 3341 case 1: 1 kHz stereo sine at -23 dBFS is -23 LUFS.
        let tone = sine(1_000.0, 10.0f32.powf(-23.0 / 20.0), 5.0, RATE);
        let buffer = StereoBuffer::from_mono(RATE, &tone);
        assert!((lufs(&buffer) + 23.0).abs() < 0.1, "{}", lufs(&buffer));
        assert!((true_peak_dbtp(&buffer) + 23.0).abs() < 0.2);
        assert!((loudness(&buffer).sample_peak_dbfs + 23.0).abs() < 0.1);
        assert_eq!(lufs(&StereoBuffer::silent(RATE, 48_000)), f64::NEG_INFINITY);
    }
}
