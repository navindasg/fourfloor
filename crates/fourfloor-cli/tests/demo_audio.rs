//! End-to-end checks on the rendered `fourfloor demo` track: the project is
//! built through edit commands, compiled through the device registry and
//! rendered offline by the engine, and every claim is measured on the
//! audio.

mod demo_common;

use demo_common::*;
use fourfloor_analysis::{estimate_tempo, TempoOptions};
use fourfloor_cli::demo::parts::{BUILD, DEMO_BARS, DROP, INTRO};
use fourfloor_cli::demo::LIMITER_CEILING_DB;
use fourfloor_model::command::{SetDeviceBypass, SetTrackSolo};
use fourfloor_model::Command;
use fourfloor_testkit::{
    assert_buffer_ok, loudness, lufs, stereo_correlation, true_peak_dbtp, StereoBuffer,
};

#[test]
fn the_mix_is_clean_full_length_and_in_a_sane_loudness_range() {
    let mix = demo_mix();
    assert_buffer_ok(mix);
    let expected = bar_seconds(DEMO_BARS) + TAIL;
    assert!(
        (mix.seconds() - expected).abs() < 0.01,
        "{} s, expected {expected} s",
        mix.seconds()
    );
    let measured = loudness(mix);
    eprintln!(
        "demo: {:.2} LUFS, {:.2} dBTP, LRA {:.1} LU",
        measured.integrated_lufs, measured.true_peak_dbtp, measured.loudness_range_lu
    );
    assert!(
        (-20.0..=-8.0).contains(&measured.integrated_lufs),
        "{} LUFS",
        measured.integrated_lufs
    );
}

#[test]
fn the_master_true_peak_stays_under_the_limiter_ceiling() {
    let peak = true_peak_dbtp(demo_mix());
    eprintln!("true peak {peak:.3} dBTP, ceiling {LIMITER_CEILING_DB} dB");
    // The limiter detects at 4x like the meter, with 0.1 dB of margin; the
    // two interpolators differ, so allow 0.05 dB.
    assert!(peak <= f64::from(LIMITER_CEILING_DB) + 0.05, "{peak} dBTP");
    // And it is actually working: the master is driven into it.
    assert!(peak > f64::from(LIMITER_CEILING_DB) - 1.0, "{peak} dBTP");
}

#[test]
fn the_detected_tempo_is_the_project_tempo() {
    let mix = demo_mix();
    let estimate =
        estimate_tempo(&mix.mono(), RATE, &TempoOptions::default()).expect("a tempo is found");
    eprintln!(
        "tempo {:.3} BPM, confidence {:.2}, regularity {:.2}",
        estimate.bpm(),
        estimate.confidence,
        estimate.regularity
    );
    assert!((estimate.bpm() - BPM).abs() < 0.1, "{} BPM", estimate.bpm());
    assert!(estimate.is_reliable());
}

#[test]
fn the_stereo_image_is_wide_but_mono_compatible() {
    let mix = demo_mix();
    let correlation = stereo_correlation(&mix.left, &mix.right);
    eprintln!("correlation {correlation:.3}");
    assert!(correlation > 0.3 && correlation < 0.995, "{correlation}");
}

#[test]
fn the_intro_is_quieter_than_the_drop() {
    let mix = demo_mix();
    let section = |bars: std::ops::Range<u32>| {
        mix.slice_seconds(bar_seconds(bars.start), bar_seconds(bars.end))
    };
    let intro = lufs(&section(section_bars(INTRO)));
    let build = lufs(&section(section_bars(BUILD)));
    let drop = lufs(&section(section_bars(DROP)));
    eprintln!("intro {intro:.2}, build {build:.2}, drop {drop:.2} LUFS");
    assert!(drop - intro > 2.0, "intro {intro} LUFS, drop {drop} LUFS");
}

/// Low-band level in the 40 ms after a beat and in the 40 ms before it.
fn kick_window(low: &[f32], beat: u32) -> (f64, f64) {
    let at = beat_sample(beat);
    let window = (0.04 * RATE.as_f64()) as usize;
    (rms(&low[at..at + window]), rms(&low[at - window..at]))
}

#[test]
fn every_beat_of_the_drop_carries_the_kick() {
    let low = low_pass(&demo_mix().mono(), 90.0);
    let first = DROP.start_bar * 4;
    let mut weakest = f64::INFINITY;
    for beat in first..DROP.end_bar() * 4 {
        let (on, before) = kick_window(&low, beat);
        weakest = weakest.min(on / before);
        assert!(
            on > 2.0 * before,
            "beat {beat}: {on:.4} on the beat, {before:.4} before"
        );
    }
    eprintln!("weakest on/before ratio in the drop: {weakest:.2}");
    // The build has no kick: no beat there stands out the same way.
    let build_beats = BUILD.start_bar * 4 + 1..BUILD.end_bar() * 4;
    let kicked = build_beats
        .filter(|&beat| {
            let (on, before) = kick_window(&low, beat);
            on > 2.0 * before
        })
        .count();
    assert!(kicked <= 2, "{kicked} build beats look like kicks");
}

/// The pad's level just after each drop beat over its level in the middle
/// of the beat, rendered with the pad soloed (the kick still keys the
/// ducker, which listens before the kick's fader).
fn pad_dip(bypass_ducker: bool) -> f64 {
    let project = demo();
    let pad = track(&project, "Pad");
    let ducker = device(&project, "Pad", "fx.compressor");
    let soloed = edit(
        &project,
        vec![
            Command::SetTrackSolo(SetTrackSolo {
                track: pad,
                solo: true,
            }),
            Command::SetDeviceBypass(SetDeviceBypass {
                device: ducker,
                bypassed: bypass_ducker,
            }),
        ],
    );
    let audio: StereoBuffer = render(&soloed);
    let mono = audio.mono();
    let beat = (beat_seconds() * RATE.as_f64()) as usize;
    let window = (0.03 * RATE.as_f64()) as usize;
    let (mut ducked, mut open) = (0.0, 0.0);
    for n in DROP.start_bar * 4..DROP.end_bar() * 4 - 1 {
        let at = beat_sample(n);
        ducked += rms(&mono[at + window..at + 2 * window]);
        open += rms(&mono[at + beat / 2..at + beat / 2 + window]);
    }
    ducked / open
}

#[test]
fn the_pad_ducks_under_every_kick() {
    let ducked = pad_dip(false);
    let bypassed = pad_dip(true);
    eprintln!("pad level after the kick / mid-beat: {ducked:.3} ducked, {bypassed:.3} bypassed");
    assert!(ducked < 0.6, "the pad only dips to {ducked:.3}");
    assert!(
        bypassed > 0.8,
        "without the ducker the pad dips to {bypassed:.3}"
    );
}
