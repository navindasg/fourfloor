//! Basic Pitch transcription of audio rendered by fourfloor.
//!
//! These tests need the 230 kB `nmp.onnx` in the model cache. The fast path
//! reads it from the default cache directory without touching the network;
//! when it is missing the tests print a notice and pass, unless
//! `FOURFLOOR_REQUIRE_MODELS=1` is set, in which case they fail. The
//! `#[ignore]`d `download_*` test fetches it (run it once with
//! `cargo test -p fourfloor-ml -- --ignored`).

mod common;

use common::{audio, basic_pitch, f1_score};
use fourfloor_core::{Device, SampleRate, Tempo, Ticks, TrackId, TICKS_PER_BEAT};
use fourfloor_instruments::{FmSynth, PolyPad};
use fourfloor_ml::{BasicPitch, ModelCache, ModelRegistry, NoteEvent, TranscribeOptions};
use fourfloor_testkit::{notes, render_instrument, sine, RenderConfig};

/// A sine note with 10 ms raised-cosine fades, so the edges do not click.
fn tone(pitch: u8, seconds: f64, rate: SampleRate) -> Vec<f32> {
    let hz = 440.0 * 2f64.powf((f64::from(pitch) - 69.0) / 12.0);
    let mut signal = sine(hz, 0.4, seconds, rate);
    let fade = (0.01 * rate.as_f64()) as usize;
    let len = signal.len();
    for i in 0..fade.min(len / 2) {
        let gain = (0.5 - 0.5 * (core::f64::consts::PI * i as f64 / fade as f64).cos()) as f32;
        signal[i] *= gain;
        signal[len - 1 - i] *= gain;
    }
    signal
}

fn melody(pitches: &[u8], note_seconds: f64, gap_seconds: f64, rate: SampleRate) -> Vec<f32> {
    let gap = vec![0.0; (gap_seconds * rate.as_f64()) as usize];
    let mut out = gap.clone();
    for &pitch in pitches {
        out.extend(tone(pitch, note_seconds, rate));
        out.extend_from_slice(&gap);
    }
    out
}

fn summary(found: &[NoteEvent]) -> Vec<(u8, f64)> {
    found
        .iter()
        .map(|note| (note.pitch, (note.start_seconds * 1000.0).round() / 1000.0))
        .collect()
}

#[test]
fn a_sine_melody_is_transcribed_note_for_note() {
    let Some(model) = basic_pitch() else { return };
    let rate = SampleRate::HZ_48000;
    let pitches = [60, 64, 67, 72, 69, 65, 62, 57];
    let signal = melody(&pitches, 0.4, 0.1, rate);
    let found = model
        .transcribe(&audio(rate, &signal), &TranscribeOptions::default())
        .expect("transcribe");
    let got: Vec<u8> = found.notes.iter().map(|note| note.pitch).collect();
    assert_eq!(got, pitches, "{:?}", summary(&found.notes));
    for (index, note) in found.notes.iter().enumerate() {
        let start = 0.1 + index as f64 * 0.5;
        assert!(
            (note.start_seconds - start).abs() < 0.035,
            "note {index} starts at {} instead of {start}",
            note.start_seconds
        );
        assert!(
            (note.end_seconds - (start + 0.4)).abs() < 0.08,
            "note {index} ends at {} instead of {}",
            note.end_seconds,
            start + 0.4
        );
    }
}

#[test]
fn onsets_stay_aligned_across_window_boundaries() {
    // 14 notes over 7 s cross three of the model's 1.64 s window hops.
    let Some(model) = basic_pitch() else { return };
    let rate = SampleRate::HZ_44100;
    let pitches: Vec<u8> = (0..14)
        .map(|i| [55, 59, 62, 67, 71, 74, 79][i % 7])
        .collect();
    let signal = melody(&pitches, 0.35, 0.15, rate);
    let found = model
        .transcribe(&audio(rate, &signal), &TranscribeOptions::default())
        .expect("transcribe");
    let got: Vec<u8> = found.notes.iter().map(|note| note.pitch).collect();
    assert_eq!(got, pitches, "{:?}", summary(&found.notes));
    let worst = found
        .notes
        .iter()
        .enumerate()
        .map(|(index, note)| (note.start_seconds - (0.15 + index as f64 * 0.5)).abs())
        .fold(0.0, f64::max);
    assert!(worst < 0.035, "worst onset error {worst:.4} s");
}

#[test]
fn a_sustained_chord_gives_all_three_notes() {
    let Some(model) = basic_pitch() else { return };
    let rate = SampleRate::HZ_48000;
    let chord: Vec<Vec<f32>> = [57u8, 60, 64].iter().map(|&p| tone(p, 1.5, rate)).collect();
    let mut signal = vec![0.0f32; chord[0].len() + 4800];
    for voice in &chord {
        for (out, sample) in signal[2400..].iter_mut().zip(voice) {
            *out += sample / 2.0;
        }
    }
    let found = model
        .transcribe(&audio(rate, &signal), &TranscribeOptions::default())
        .expect("transcribe");
    let mut got: Vec<u8> = found.notes.iter().map(|note| note.pitch).collect();
    got.sort_unstable();
    got.dedup();
    assert_eq!(got, vec![57, 60, 64], "{:?}", summary(&found.notes));
    for note in &found.notes {
        assert!((note.start_seconds - 0.05).abs() < 0.035, "{note:?}");
        assert!(note.duration() > 1.2, "{note:?}");
    }
}

#[test]
fn an_fm_organ_line_rendered_by_fourfloor_is_recovered() {
    let Some(model) = basic_pitch() else { return };
    let config = RenderConfig::default();
    let rate = config.sample_rate;
    let spec: Vec<(f64, f64, u8, u8)> = [48u8, 55, 60, 63, 60, 55, 51, 53]
        .iter()
        .enumerate()
        .map(|(i, &pitch)| (0.1 + i as f64 * 0.375, 0.3, pitch, 100))
        .collect();
    let mut organ = FmSynth::organ();
    let rendered = render_instrument(&mut organ, &notes(&spec, rate), 3.4, &config);
    let buffer = audio(rate, &rendered.mono());
    let found = model
        .transcribe(&buffer, &TranscribeOptions::default())
        .expect("transcribe");
    let expected: Vec<(f64, u8)> = spec
        .iter()
        .map(|&(start, _, pitch, _)| (start, pitch))
        .collect();
    let score = f1_score(&found.notes, &expected, 0.05);
    assert!(score >= 0.9, "F1 {score:.2}: {:?}", summary(&found.notes));
}

#[test]
fn pad_chords_rendered_by_fourfloor_are_recovered() {
    let Some(model) = basic_pitch() else { return };
    let config = RenderConfig::default();
    let rate = config.sample_rate;
    // Am, F, C, G in close voicing, one second each.
    let chords: [[u8; 3]; 4] = [[57, 60, 64], [53, 57, 60], [55, 60, 64], [55, 59, 62]];
    let mut spec = Vec::new();
    for (index, chord) in chords.iter().enumerate() {
        for &pitch in chord {
            spec.push((0.1 + index as f64, 0.85, pitch, 100u8));
        }
    }
    let mut pad = PolyPad::new();
    // The default patch adds a sub-octave oscillator, which the model
    // rightly hears as a second, lower note; turn it off.
    pad.set_param(PolyPad::SUB, 0.0);
    let rendered = render_instrument(&mut pad, &notes(&spec, rate), 4.6, &config);
    let found = model
        .transcribe(
            &audio(rate, &rendered.mono()),
            &TranscribeOptions::default(),
        )
        .expect("transcribe");
    let expected: Vec<(f64, u8)> = spec
        .iter()
        .map(|&(start, _, pitch, _)| (start, pitch))
        .collect();
    // The pad has a slow attack, so onsets get a wider window.
    let score = f1_score(&found.notes, &expected, 0.12);
    assert!(score >= 0.8, "F1 {score:.2}: {:?}", summary(&found.notes));
}

#[test]
fn silence_and_noise_floor_give_no_notes() {
    let Some(model) = basic_pitch() else { return };
    let rate = SampleRate::HZ_48000;
    let found = model
        .transcribe(
            &audio(rate, &vec![0.0; 96_000]),
            &TranscribeOptions::default(),
        )
        .expect("transcribe");
    assert!(found.notes.is_empty(), "{:?}", found.notes);
    assert!(model
        .transcribe(&audio(rate, &[]), &TranscribeOptions::default())
        .is_err());
}

#[test]
fn louder_notes_get_higher_velocity_and_clips_land_on_the_beat() {
    let Some(model) = basic_pitch() else { return };
    let rate = SampleRate::HZ_48000;
    // At 120 BPM a beat is 0.5 s: notes on beats 1..4 after a 0.5 s lead.
    let signal = melody(&[60, 62, 64, 65], 0.45, 0.05, rate);
    let lead = vec![0.0f32; 21_600];
    let signal: Vec<f32> = lead.iter().chain(&signal).copied().collect();
    let found = model
        .transcribe(&audio(rate, &signal), &TranscribeOptions::default())
        .expect("transcribe");
    let tempo = Tempo::new(120.0).expect("tempo");
    let command = found.to_clip_command(TrackId::from(3), "bass", Ticks::BEAT, tempo);
    assert_eq!(command.length.get() % (TICKS_PER_BEAT * 4), 0);
    let starts: Vec<i64> = command.notes.iter().map(|n| n.start.get()).collect();
    assert_eq!(starts.len(), 4, "{:?}", summary(&found.notes));
    for (index, start) in starts.iter().enumerate() {
        let beat = (index as i64 + 1) * TICKS_PER_BEAT;
        assert!((start - beat).abs() <= 70, "note {index} at {start} ticks");
    }
    let pitches: Vec<u8> = command.notes.iter().map(|n| n.pitch).collect();
    assert_eq!(pitches, vec![60, 62, 64, 65]);

    let quiet: Vec<f32> = signal.iter().map(|s| s * 0.1).collect();
    let soft = model
        .transcribe(&audio(rate, &quiet), &TranscribeOptions::default())
        .expect("transcribe");
    let mean = |notes: &[NoteEvent]| {
        notes.iter().map(|n| f32::from(n.velocity)).sum::<f32>() / notes.len().max(1) as f32
    };
    assert!(!soft.notes.is_empty());
    assert!(mean(&soft.notes) <= mean(&found.notes) + 1.0);
}

#[test]
#[ignore = "downloads the Basic Pitch model (230 kB) from GitHub"]
fn download_basic_pitch_and_verify_its_checksum() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = ModelCache::new(dir.path());
    let model = BasicPitch::load(&cache, &ModelRegistry::builtin()).expect("download and load");
    let found = model
        .transcribe(
            &audio(
                SampleRate::HZ_48000,
                &melody(&[69], 0.5, 0.1, SampleRate::HZ_48000),
            ),
            &TranscribeOptions::default(),
        )
        .expect("transcribe");
    assert_eq!(
        found.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
        vec![69]
    );
}
