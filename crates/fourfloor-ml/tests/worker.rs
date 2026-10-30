//! The ML worker thread: jobs run off the caller's thread, models load
//! lazily, and missing models fall back to the DSP versions.

mod common;

use std::time::Duration;

use common::{audio, cached, stereo};
use fourfloor_core::SampleRate;
use fourfloor_ml::{
    Job, JobOutput, MlError, MlWorker, ModelCache, ModelRegistry, StemCache, TranscribeOptions,
    WorkerConfig, BASIC_PITCH, DSP_EMBEDDING, SPLEETER_2STEMS,
};
use fourfloor_testkit::{sine, white_noise};

const WAIT: Duration = Duration::from_secs(120);

fn offline_config(models: &std::path::Path, stems: Option<&std::path::Path>) -> WorkerConfig {
    WorkerConfig {
        models: ModelCache::new(models).offline(),
        registry: ModelRegistry::builtin(),
        stems: stems.map(StemCache::new),
    }
}

fn run(worker: &mut MlWorker, job: Job) -> Result<JobOutput, MlError> {
    let id = worker.submit(job).expect("submit");
    let result = worker.recv_timeout(WAIT).expect("job finished in time");
    assert_eq!(result.id, id);
    result.output
}

#[test]
fn with_no_models_separation_and_embedding_fall_back_and_transcription_reports_it() {
    let models = tempfile::tempdir().expect("tempdir");
    let mut worker = MlWorker::spawn(offline_config(models.path(), None)).expect("spawn");
    let rate = SampleRate::HZ_48000;
    let tone = sine(220.0, 0.3, 1.0, rate);

    match run(
        &mut worker,
        Job::Separate {
            audio: stereo(rate, &tone, &tone),
            prefer_model: true,
        },
    ) {
        Ok(JobOutput::Stems { stems, cached }) => {
            assert!(stems.fallback, "no model is cached, so the fallback ran");
            assert_eq!(stems.names(), vec!["drums", "bass", "other"]);
            assert!(!cached);
        }
        other => panic!("{other:?}"),
    }
    match run(
        &mut worker,
        Job::EmbedAudio {
            audio: audio(rate, &tone),
        },
    ) {
        Ok(JobOutput::Embedding(embedding)) => assert_eq!(embedding.model, DSP_EMBEDDING),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        run(
            &mut worker,
            Job::EmbedText {
                text: "dusty 909 open hat".to_owned()
            }
        ),
        Err(MlError::Unsupported(_))
    ));
    let error = run(
        &mut worker,
        Job::Transcribe {
            audio: audio(rate, &tone),
            options: TranscribeOptions::default(),
        },
    )
    .expect_err("no model");
    assert!(error.to_string().contains("not cached"), "{error}");
    // Nothing was downloaded into the empty cache.
    assert_eq!(std::fs::read_dir(models.path()).expect("dir").count(), 0);
}

#[test]
fn jobs_are_answered_in_order_and_dropping_the_worker_finishes_them() {
    let models = tempfile::tempdir().expect("tempdir");
    let mut worker = MlWorker::spawn(offline_config(models.path(), None)).expect("spawn");
    let rate = SampleRate::HZ_48000;
    let ids: Vec<u64> = (0..3)
        .map(|seed| {
            let noise = white_noise(seed, 0.2, 0.5, rate);
            worker
                .submit(Job::EmbedAudio {
                    audio: audio(rate, &noise),
                })
                .expect("submit")
        })
        .collect();
    let answered: Vec<u64> = (0..3)
        .map(|_| worker.recv_timeout(WAIT).expect("answer").id)
        .collect();
    assert_eq!(answered, ids);
    assert!(worker.try_recv().is_none());
    worker
        .submit(Job::EmbedAudio {
            audio: audio(rate, &[0.1; 4800]),
        })
        .expect("submit");
    drop(worker);
}

#[test]
fn with_cached_models_the_worker_transcribes_and_separates_with_spleeter() {
    if cached(BASIC_PITCH).is_none() || cached(SPLEETER_2STEMS).is_none() {
        return;
    }
    let stems_dir = tempfile::tempdir().expect("tempdir");
    let config = WorkerConfig {
        models: ModelCache::in_default_dir().offline(),
        registry: ModelRegistry::builtin(),
        stems: Some(StemCache::new(stems_dir.path())),
    };
    let mut worker = MlWorker::spawn(config).expect("spawn");
    let rate = SampleRate::HZ_44100;
    let mut signal = vec![0.0f32; 4410];
    signal.extend(sine(440.0, 0.4, 0.8, rate));
    signal.extend(vec![0.0f32; 4410]);

    match run(
        &mut worker,
        Job::Transcribe {
            audio: audio(rate, &signal),
            options: TranscribeOptions::default(),
        },
    ) {
        Ok(JobOutput::Transcription(found)) => {
            let pitches: Vec<u8> = found.notes.iter().map(|note| note.pitch).collect();
            assert_eq!(pitches, vec![69]);
        }
        other => panic!("{other:?}"),
    }
    let mix = stereo(rate, &signal, &signal);
    for expect_cached in [false, true] {
        match run(
            &mut worker,
            Job::Separate {
                audio: mix.clone(),
                prefer_model: true,
            },
        ) {
            Ok(JobOutput::Stems { stems, cached }) => {
                assert!(!stems.fallback);
                assert_eq!(stems.names(), vec!["vocals", "accompaniment"]);
                assert_eq!(cached, expect_cached);
            }
            other => panic!("{other:?}"),
        }
    }
}
