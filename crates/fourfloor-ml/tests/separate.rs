//! Stem separation: the DSP fallback on rendered drums and pads, the mask
//! pipeline, the content-hash cache, and Spleeter when its model is cached.

mod common;

use common::{cached, stereo};
use fourfloor_audio::AudioBuffer;
use fourfloor_core::{Device, SampleRate};
use fourfloor_instruments::{Kick909, PolyPad};
use fourfloor_ml::separate::{Magnitudes, StftLayout};
use fourfloor_ml::{
    HpssEstimator, MaskEstimator, MaskSeparator, MlError, ModelRegistry, Separator, SpleeterOnnx,
    StemCache, Stems, SPLEETER_2STEMS,
};
use fourfloor_testkit::{
    notes, null_residual_db, render_instrument, rms, rms_window, sine, spectrum_band_energy,
    RenderConfig, StereoBuffer,
};

fn rate_hz(config: &RenderConfig) -> f64 {
    config.sample_rate.as_f64()
}

fn kicks(config: &RenderConfig, seconds: f64) -> StereoBuffer {
    let spec: Vec<(f64, f64, u8, u8)> = (0..(seconds * 2.0) as usize)
        .map(|beat| (beat as f64 * 0.5, 0.1, 36, 120))
        .collect();
    render_instrument(
        &mut Kick909::new(),
        &notes(&spec, config.sample_rate),
        seconds,
        config,
    )
}

fn pad_chord(config: &RenderConfig, seconds: f64) -> StereoBuffer {
    let spec: Vec<(f64, f64, u8, u8)> = [57u8, 60, 64, 69]
        .iter()
        .map(|&pitch| (0.0, seconds, pitch, 90))
        .collect();
    let mut pad = PolyPad::new();
    pad.set_param(PolyPad::ATTACK, 0.05);
    render_instrument(&mut pad, &notes(&spec, config.sample_rate), seconds, config)
}

fn buffer(stereo_buffer: &StereoBuffer) -> AudioBuffer {
    stereo(
        stereo_buffer.sample_rate,
        &stereo_buffer.left,
        &stereo_buffer.right,
    )
}

fn energy(audio: &AudioBuffer) -> f64 {
    audio
        .channels
        .iter()
        .flatten()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum()
}

fn share(stems: &Stems, names: &[&str]) -> f64 {
    let total: f64 = stems.stems.iter().map(|stem| energy(&stem.audio)).sum();
    let part: f64 = names
        .iter()
        .filter_map(|name| stems.get(name))
        .map(|stem| energy(&stem.audio))
        .sum();
    part / total.max(1e-30)
}

fn sum_of_stems(stems: &Stems, channel: usize) -> Vec<f32> {
    let frames = stems.stems[0].audio.frames();
    (0..frames)
        .map(|i| {
            stems
                .stems
                .iter()
                .map(|stem| stem.audio.channels[channel][i])
                .sum()
        })
        .collect()
}

#[test]
fn the_fallback_puts_kicks_in_drums_and_pads_in_the_tonal_stems() {
    let config = RenderConfig::default();
    let separator = HpssEstimator::separator();
    assert!(separator.is_fallback());
    assert_eq!(separator.stem_names(), vec!["drums", "bass", "other"]);

    let kick = separator
        .separate(&buffer(&kicks(&config, 4.0)))
        .expect("kicks");
    assert!(kick.fallback);
    let drums = share(&kick, &["drums"]);
    eprintln!("kick alone: {:.1}% in drums", drums * 100.0);
    assert!(
        drums > 0.6,
        "only {:.0}% of the kick energy is in drums",
        drums * 100.0
    );

    let pad = separator
        .separate(&buffer(&pad_chord(&config, 4.0)))
        .expect("pad");
    let tonal = share(&pad, &["bass", "other"]);
    eprintln!("pad alone: {:.1}% in bass + other", tonal * 100.0);
    assert!(
        tonal > 0.9,
        "only {:.0}% of the pad energy is tonal",
        tonal * 100.0
    );
    // The chord is A3 and up: nothing for the bass stem.
    assert!(share(&pad, &["other"]) > 0.85);
}

#[test]
fn in_a_mix_the_drum_stem_follows_the_kick_and_the_other_stem_follows_the_pad() {
    let config = RenderConfig::default();
    let kick = kicks(&config, 4.0);
    let pad = pad_chord(&config, 4.0);
    let mixed = AudioBuffer::new(
        config.sample_rate,
        vec![
            kick.left
                .iter()
                .zip(&pad.left)
                .map(|(a, b)| a + b)
                .collect(),
            kick.right
                .iter()
                .zip(&pad.right)
                .map(|(a, b)| a + b)
                .collect(),
        ],
    );
    let stems = HpssEstimator::separator()
        .separate(&mixed)
        .expect("separate");
    let drums = &stems.get("drums").expect("drums").audio.channels[0];
    let other = &stems.get("other").expect("other").audio.channels[0];
    let bass = &stems.get("bass").expect("bass").audio.channels[0];
    let rate = config.sample_rate;
    let band = |signal: &[f32], low: f32, high: f32| spectrum_band_energy(signal, rate, low, high);
    // Below 150 Hz the mix is kick: it goes to drums and bass (HPSS hears
    // the kick's sustained sine tail as tonal), not to `other`.
    let low_end: Vec<f32> = drums.iter().zip(bass).map(|(a, b)| a + b).collect();
    let kick_low = band(&kick.left, 30.0, 150.0);
    let kept_low = band(&low_end, 30.0, 150.0) / kick_low;
    let leaked_low = band(other, 30.0, 150.0) / kick_low;
    // From 300 Hz to 3 kHz the mix is pad: it goes to `other`.
    let pad_mid = band(&pad.left, 300.0, 3000.0);
    let kept_mid = band(other, 300.0, 3000.0) / pad_mid;
    let leaked_mid = band(drums, 300.0, 3000.0) / pad_mid;
    eprintln!(
        "kick lows kept {kept_low:.2}, leaked {leaked_low:.3}; pad mids kept {kept_mid:.2}, leaked {leaked_mid:.3}"
    );
    assert!(kept_low > 0.7 && leaked_low < 0.05);
    assert!(kept_mid > 0.7 && leaked_mid < 0.1);
    // The drum stem keeps the attacks: right after each hit it is far
    // louder than half a beat later, when only the pad is sounding.
    let hop = (0.5 * rate_hz(&config)) as usize;
    let attack: f32 = (0..8).map(|beat| rms_window(drums, beat * hop, 960)).sum();
    let between: f32 = (0..8)
        .map(|beat| rms_window(drums, beat * hop + hop / 2, 960))
        .sum();
    eprintln!("drum stem: attack {attack:.3}, between hits {between:.3}");
    assert!(attack > 4.0 * between);
}

#[test]
fn fallback_stems_sum_back_to_the_mix() {
    let config = RenderConfig::default();
    let kick = kicks(&config, 2.0);
    let pad = pad_chord(&config, 2.0);
    let mixed = stereo(
        config.sample_rate,
        &kick
            .left
            .iter()
            .zip(&pad.left)
            .map(|(a, b)| a + b)
            .collect::<Vec<_>>(),
        &pad.right,
    );
    let stems = HpssEstimator::separator()
        .separate(&mixed)
        .expect("separate");
    for channel in 0..2 {
        let residual = null_residual_db(&mixed.channels[channel], &sum_of_stems(&stems, channel));
        assert!(residual < -90.0, "channel {channel}: {residual:.1} dB");
    }
    // A bass line below the crossover goes to the bass stem.
    let rate = SampleRate::HZ_44100;
    let bass = stereo(
        rate,
        &sine(55.0, 0.5, 2.0, rate),
        &sine(55.0, 0.5, 2.0, rate),
    );
    let stems = HpssEstimator::separator()
        .separate(&bass)
        .expect("separate");
    assert!(share(&stems, &["bass"]) > 0.9);
}

/// A model stand-in: everything below 1 kHz is `low`, the rest `high`.
struct SplitAtOneKilohertz;

impl MaskEstimator for SplitAtOneKilohertz {
    fn name(&self) -> &str {
        "test-split-1k"
    }
    fn is_fallback(&self) -> bool {
        false
    }
    fn layout(&self) -> StftLayout {
        StftLayout {
            sample_rate: Some(44_100),
            fft_size: 4096,
            hop: 1024,
        }
    }
    fn stem_names(&self) -> Vec<String> {
        vec!["low".to_owned(), "high".to_owned()]
    }
    fn masks(&self, input: &Magnitudes) -> Result<Vec<Vec<Vec<f32>>>, MlError> {
        let low: Vec<f32> = (0..input.frames)
            .flat_map(|_| {
                (0..input.bins).map(|bin| f32::from(u8::from(bin as f64 * input.bin_hz < 1000.0)))
            })
            .collect();
        let high = low.iter().map(|m| 1.0 - m).collect();
        let channels = input.channels.len();
        Ok(vec![vec![low; channels], vec![high; channels]])
    }
}

#[test]
fn the_mask_pipeline_resamples_and_keeps_rate_length_and_channels() {
    let rate = SampleRate::HZ_48000;
    let low = sine(220.0, 0.4, 1.5, rate);
    let high = sine(5000.0, 0.4, 1.5, rate);
    let mono = AudioBuffer::new(
        rate,
        vec![low.iter().zip(&high).map(|(a, b)| a + b).collect()],
    );
    let stems = MaskSeparator::new(SplitAtOneKilohertz)
        .separate(&mono)
        .expect("separate");
    assert_eq!(stems.names(), vec!["low", "high"]);
    for stem in &stems.stems {
        assert_eq!(stem.audio.sample_rate, rate);
        assert_eq!(stem.audio.channel_count(), 1);
        assert_eq!(stem.audio.frames(), mono.frames());
    }
    // Compare away from the edges, where resampling has its transients.
    let mid = 4800..mono.frames() - 4800;
    let got_low = &stems.get("low").expect("low").audio.channels[0][mid.clone()];
    let got_high = &stems.get("high").expect("high").audio.channels[0][mid.clone()];
    assert!(null_residual_db(&low[mid.clone()], got_low) < -40.0);
    assert!(null_residual_db(&high[mid], got_high) < -40.0);
    assert!(MaskSeparator::new(SplitAtOneKilohertz)
        .separate(&AudioBuffer::new(rate, vec![]))
        .is_err());
}

#[test]
fn the_stem_cache_hits_on_identical_audio_and_misses_on_any_change() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = StemCache::new(dir.path());
    let separator = HpssEstimator::separator();
    let rate = SampleRate::HZ_44100;
    let audio = stereo(
        rate,
        &sine(330.0, 0.3, 1.0, rate),
        &sine(440.0, 0.3, 1.0, rate),
    );

    let (first, hit) = cache.get_or_separate(&separator, &audio).expect("first");
    assert!(!hit);
    let (second, hit) = cache.get_or_separate(&separator, &audio).expect("second");
    assert!(hit);
    assert_eq!(first, second, "cached stems are bit-exact (float WAV)");

    let mut changed = audio.clone();
    changed.channels[1][1234] += 1e-6;
    assert_ne!(
        StemCache::key(&separator, &audio),
        StemCache::key(&separator, &changed)
    );
    let (_, hit) = cache
        .get_or_separate(&separator, &changed)
        .expect("changed");
    assert!(!hit);
    let other = MaskSeparator::new(SplitAtOneKilohertz);
    assert!(cache.get(&other, &audio).expect("get").is_none());
    assert_eq!(StemCache::key(&separator, &audio).len(), 64);
}

#[test]
fn spleeter_keeps_an_instrumental_in_accompaniment_and_sums_to_the_mix() {
    let Some((cache, _)) = cached(SPLEETER_2STEMS) else {
        return;
    };
    let model = SpleeterOnnx::load(&cache, &ModelRegistry::builtin()).expect("load Spleeter");
    let separator = model.separator();
    assert!(!separator.is_fallback());
    let config = RenderConfig::at_rate(SampleRate::HZ_44100);
    let kick = kicks(&config, 6.0);
    let pad = pad_chord(&config, 6.0);
    let mixed = stereo(
        config.sample_rate,
        &kick
            .left
            .iter()
            .zip(&pad.left)
            .map(|(a, b)| a + b)
            .collect::<Vec<_>>(),
        &kick
            .right
            .iter()
            .zip(&pad.right)
            .map(|(a, b)| a + b)
            .collect::<Vec<_>>(),
    );
    let stems = separator.separate(&mixed).expect("separate");
    assert_eq!(stems.names(), vec!["vocals", "accompaniment"]);
    let accompaniment = share(&stems, &["accompaniment"]);
    eprintln!(
        "spleeter: accompaniment holds {:.1}% of an instrumental",
        accompaniment * 100.0
    );
    assert!(accompaniment > 0.9);
    for channel in 0..2 {
        let residual = null_residual_db(&mixed.channels[channel], &sum_of_stems(&stems, channel));
        assert!(residual < -90.0, "channel {channel}: {residual:.1} dB");
    }
    assert!(rms(&stems.stems[0].audio.channels[0]) < rms(&mixed.channels[0]));
}

#[test]
#[ignore = "downloads the Spleeter 2-stem model (79 MB) from Hugging Face"]
fn download_spleeter_and_verify_its_checksums() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cache = fourfloor_ml::ModelCache::new(dir.path());
    let model = SpleeterOnnx::load(&cache, &ModelRegistry::builtin()).expect("download and load");
    let rate = SampleRate::HZ_44100;
    let audio = stereo(
        rate,
        &sine(440.0, 0.3, 2.0, rate),
        &sine(440.0, 0.3, 2.0, rate),
    );
    let stems = model.separator().separate(&audio).expect("separate");
    assert_eq!(stems.stems.len(), 2);
}

/// A crude sung vowel: a band-limited sawtooth around A3 with 5.5 Hz
/// vibrato, through three formant resonators ("ah"), phrased in notes.
fn synthetic_voice(rate: SampleRate, seconds: f64) -> Vec<f32> {
    let sr = rate.as_f64();
    let frames = (seconds * sr) as usize;
    let melody = [57.0, 60.0, 62.0, 64.0, 62.0, 60.0];
    let mut phase = 0.0f64;
    let mut source = vec![0.0f32; frames];
    for (i, out) in source.iter_mut().enumerate() {
        let t = i as f64 / sr;
        let note = (t / 0.8) as usize;
        let within = t - note as f64 * 0.8;
        if within > 0.7 {
            continue;
        }
        let pitch = melody[note % melody.len()] + 0.3 * (core::f64::consts::TAU * 5.5 * t).sin();
        let hz = 440.0 * 2f64.powf((pitch - 69.0) / 12.0);
        phase = (phase + hz / sr).fract();
        let harmonics = (sr / 2.0 / hz) as usize;
        let mut value = 0.0;
        for k in 1..=harmonics.min(40) {
            value += (core::f64::consts::TAU * k as f64 * phase).sin() / k as f64;
        }
        let envelope = (within / 0.05).min(1.0) * ((0.7 - within) / 0.05).min(1.0);
        *out = (value * envelope * 0.2) as f32;
    }
    let mut voice = vec![0.0f32; frames];
    for (formant, gain) in [(700.0, 1.0), (1220.0, 0.5), (2600.0, 0.25)] {
        let r = (-core::f64::consts::PI * 90.0 / sr).exp();
        let theta = core::f64::consts::TAU * formant / sr;
        let (a1, a2) = (2.0 * r * theta.cos(), -r * r);
        let (mut y1, mut y2) = (0.0f64, 0.0f64);
        for (out, x) in voice.iter_mut().zip(&source) {
            let y = (1.0 - r) * f64::from(*x) + a1 * y1 + a2 * y2;
            y2 = y1;
            y1 = y;
            *out += (y * gain * 4.0) as f32;
        }
    }
    voice
}

#[test]
fn spleeter_moves_a_synthetic_vocal_into_the_vocal_stem() {
    let Some((cache, _)) = cached(SPLEETER_2STEMS) else {
        return;
    };
    let model = SpleeterOnnx::load(&cache, &ModelRegistry::builtin()).expect("load Spleeter");
    let config = RenderConfig::at_rate(SampleRate::HZ_44100);
    let rate = config.sample_rate;
    let kick = kicks(&config, 6.0);
    let pad = pad_chord(&config, 6.0);
    let voice = synthetic_voice(rate, 6.0);
    let left: Vec<f32> = (0..voice.len())
        .map(|i| kick.left[i] + pad.left[i] * 0.5 + voice[i])
        .collect();
    let right: Vec<f32> = (0..voice.len())
        .map(|i| kick.right[i] + pad.right[i] * 0.5 + voice[i])
        .collect();
    let stems = model
        .clone()
        .separator()
        .separate(&stereo(rate, &left, &right))
        .expect("separate");
    let vocals = &stems.get("vocals").expect("vocals").audio.channels[0];
    let accompaniment = &stems
        .get("accompaniment")
        .expect("accompaniment")
        .audio
        .channels[0];
    let instrumental: Vec<f32> = (0..voice.len())
        .map(|i| kick.left[i] + pad.left[i] * 0.5)
        .collect();
    let alone = model
        .separator()
        .separate(&stereo(rate, &voice, &voice))
        .expect("voice alone");
    let corr = fourfloor_testkit::stereo_correlation;
    eprintln!(
        "spleeter voice: alone {:.1}% in vocals; in the mix corr(vocals, voice) {:.2}, \
         corr(vocals, backing) {:.2}, corr(accompaniment, voice) {:.2}, corr(accompaniment, backing) {:.2}",
        share(&alone, &["vocals"]) * 100.0,
        corr(vocals, &voice),
        corr(vocals, &instrumental),
        corr(accompaniment, &voice),
        corr(accompaniment, &instrumental),
    );
    assert!(share(&alone, &["vocals"]) > 0.9);
    assert!(corr(vocals, &voice) > 0.9 && corr(vocals, &instrumental) < 0.2);
    assert!(corr(accompaniment, &instrumental) > 0.8 && corr(accompaniment, &voice) < 0.2);
}
