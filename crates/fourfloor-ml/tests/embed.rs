//! Sample embeddings on one-shots rendered by fourfloor's drum machine and
//! synths: the DSP fallback always, LAION CLAP with `--features clap` when
//! its model is cached.

mod common;

use common::stereo;
use fourfloor_audio::AudioBuffer;
use fourfloor_core::Device;
use fourfloor_instruments::{Clap909, Hat909, Kick808, Kick909, PolyPad, Snare909};
use fourfloor_ml::{rank, AudioEmbedder, DspEmbedder, Embedding};
use fourfloor_testkit::{notes, render_instrument, RenderConfig};

fn one_shot(device: &mut dyn Device, pitch: u8, seconds: f64) -> AudioBuffer {
    let config = RenderConfig::default();
    let events = notes(&[(0.0, 0.2, pitch, 110)], config.sample_rate);
    let out = render_instrument(device, &events, seconds, &config);
    stereo(config.sample_rate, &out.left, &out.right)
}

fn kick(tune: Option<f32>) -> AudioBuffer {
    let mut device = Kick909::new();
    if let Some(tune) = tune {
        device.set_param(fourfloor_instruments::drums::Kick909Voice::TUNE, tune);
    }
    one_shot(&mut device, 36, 0.8)
}

// Some samples are only used by the CLAP tests.
#[cfg_attr(not(feature = "clap"), allow(dead_code))]
struct Samples {
    kick_a: AudioBuffer,
    kick_b: AudioBuffer,
    kick_808: AudioBuffer,
    hat: AudioBuffer,
    open_hat: AudioBuffer,
    snare: AudioBuffer,
    clap: AudioBuffer,
    pad: AudioBuffer,
}

fn samples() -> Samples {
    let default_tune = Kick909::new().params()[0].default;
    let mut pad = PolyPad::new();
    let config = RenderConfig::default();
    let chord = notes(
        &[(0.0, 1.8, 57, 90), (0.0, 1.8, 60, 90), (0.0, 1.8, 64, 90)],
        config.sample_rate,
    );
    let pad_out = render_instrument(&mut pad, &chord, 2.0, &config);
    Samples {
        kick_a: kick(None),
        kick_b: kick(Some(default_tune * 1.1)),
        kick_808: one_shot(&mut Kick808::new(), 36, 0.8),
        hat: one_shot(&mut Hat909::new(), 42, 0.5),
        open_hat: one_shot(&mut Hat909::new(), 46, 0.8),
        snare: one_shot(&mut Snare909::new(), 38, 0.5),
        clap: one_shot(&mut Clap909::new(), 39, 0.5),
        pad: stereo(config.sample_rate, &pad_out.left, &pad_out.right),
    }
}

fn similarity(a: &Embedding, b: &Embedding) -> f32 {
    a.similarity(b).expect("same model")
}

#[test]
fn the_dsp_fallback_keeps_kicks_together_and_hats_apart() {
    let s = samples();
    let embed = |audio: &AudioBuffer| DspEmbedder.embed_audio(audio).expect("embed");
    let kick_a = embed(&s.kick_a);
    let kick_b = embed(&s.kick_b);
    let hat = embed(&s.hat);
    let open_hat = embed(&s.open_hat);
    let pad = embed(&s.pad);
    assert_eq!(kick_a.len(), DspEmbedder.dimensions());
    assert_eq!(kick_a.model, fourfloor_ml::DSP_EMBEDDING);
    let kicks = similarity(&kick_a, &kick_b);
    let kick_hat = similarity(&kick_a, &hat);
    let hats = similarity(&hat, &open_hat);
    eprintln!("dsp: kick~kick {kicks:.3}, kick~hat {kick_hat:.3}, hat~open hat {hats:.3}");
    assert!(kicks > kick_hat);
    assert!(hats > kick_hat);
    assert!(similarity(&kick_a, &pad) < kicks);

    let library = [
        ("hat", &hat),
        ("pad", &pad),
        ("kick b", &kick_b),
        ("open hat", &open_hat),
    ];
    let ranked = rank(&kick_a, library);
    assert_eq!(ranked[0].0, "kick b", "{ranked:?}");
    // Same input, same embedding: the library can rely on it.
    assert_eq!(embed(&s.kick_a), kick_a);
}

#[test]
fn embeddings_survive_a_database_round_trip() {
    let embedding = DspEmbedder.embed_audio(&kick(None)).expect("embed");
    let bytes = embedding.to_le_bytes();
    assert_eq!(bytes.len(), embedding.len() * 4);
    let back = Embedding::from_le_bytes(&embedding.model, &bytes).expect("bytes");
    assert_eq!(back, embedding);
    assert!(DspEmbedder
        .embed_audio(&AudioBuffer::new(
            fourfloor_core::SampleRate::HZ_48000,
            vec![]
        ))
        .is_err());
}

#[cfg(feature = "clap")]
mod clap {
    use super::*;
    use fourfloor_ml::{ClapEmbedder, ModelRegistry, CLAP_GENERAL};

    fn model() -> Option<ClapEmbedder> {
        let (cache, _) = common::cached(CLAP_GENERAL)?;
        Some(ClapEmbedder::load(&cache, &ModelRegistry::builtin()).expect("load CLAP"))
    }

    #[test]
    fn text_queries_find_the_right_drum_and_audio_finds_its_kin() {
        let Some(clap) = model() else { return };
        let s = samples();
        let library: Vec<(&str, Embedding)> = [
            ("kick 909", &s.kick_a),
            ("kick 909 tuned", &s.kick_b),
            ("kick 808", &s.kick_808),
            ("closed hat", &s.hat),
            ("open hat", &s.open_hat),
            ("snare", &s.snare),
            ("clap", &s.clap),
            ("pad", &s.pad),
        ]
        .into_iter()
        .map(|(name, audio)| (name, clap.embed_audio(audio).expect("embed audio")))
        .collect();
        assert!(library.iter().all(|(_, e)| e.len() == 512));

        let search = |query: &str| -> Vec<(&str, f32)> {
            let text = clap.embed_text(query).expect("embed text");
            rank(&text, library.iter().map(|(name, e)| (*name, e)))
        };
        let mut failures = Vec::new();
        for (query, wanted) in [
            (
                "a kick drum",
                &["kick 909", "kick 909 tuned", "kick 808"][..],
            ),
            ("hi-hat cymbal", &["closed hat", "open hat"][..]),
            ("hand clap", &["clap"][..]),
            ("a soft synthesizer pad chord", &["pad"][..]),
        ] {
            let ranked = search(query);
            eprintln!("clap \"{query}\": {:?}", &ranked[..4]);
            if !wanted.contains(&ranked[0].0) {
                failures.push(format!("\"{query}\" ranked {:?} first", ranked[0]));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");

        let by_audio = rank(&library[0].1, library[1..].iter().map(|(n, e)| (*n, e)));
        eprintln!("clap audio query kick 909: {:?}", &by_audio[..3]);
        assert!(by_audio[0].0.starts_with("kick"), "{by_audio:?}");
        // Deterministic: the same sample embeds identically.
        assert_eq!(clap.embed_audio(&s.kick_a).expect("again"), library[0].1);
    }

    #[test]
    fn tokens_are_bounded_and_wrapped_in_special_tokens() {
        let Some(clap) = model() else { return };
        let ids = clap.tokens("dusty 909 open hat").expect("tokens");
        assert_eq!(ids.first(), Some(&0));
        assert_eq!(ids.last(), Some(&2));
        let long = "very ".repeat(200);
        assert_eq!(clap.tokens(&long).expect("tokens").len(), 77);
        let text = clap.embed_text("dusty 909 open hat").expect("text");
        assert!((text.vector.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-4);
    }

    #[test]
    #[ignore = "downloads LAION CLAP (207 MB) from Hugging Face"]
    fn download_clap_and_verify_its_checksums() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cache = fourfloor_ml::ModelCache::new(dir.path());
        let clap = ClapEmbedder::load(&cache, &ModelRegistry::builtin()).expect("download");
        assert_eq!(clap.embed_text("kick").expect("text").len(), 512);
    }
}
