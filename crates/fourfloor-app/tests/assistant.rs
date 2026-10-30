//! The AI panel's requests, applied through the app's intents and checked
//! on the project and on rendered audio: phrase commands and their undo,
//! generated drums and bass, the mix report against a reference, stems and
//! audio-to-MIDI on a clip, the MCP server, and a chat model that is down.

mod library_common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use fourfloor_app::state::{AssistantIntent, FileIntent, Intent, MlIntent};
use fourfloor_app::viewmodel::assistant::{bass_args, drum_args, llm_config};
use fourfloor_app::FourfloorApp;
use fourfloor_model::command::{CreateTrack, SetTrackMute};
use fourfloor_model::{ClipContent, Command, CommandBus, Project, TrackKind};
use fourfloor_testkit::{onset_times, render_project, rms, spectrum_band_energy, StereoBuffer};
use library_common::{library_app, run, temp_dir, wait_jobs, RATE};

fn render(app: &FourfloorApp, project: &Project, tail: f64) -> StereoBuffer {
    render_project(project, app.registry(), RATE, tail).expect("renders")
}

fn last_entry(app: &FourfloorApp) -> &fourfloor_app::services::assistant::ChatEntry {
    app.features.chat.last().expect("a chat entry")
}

fn muted(project: &Project, names: &[&str]) -> Project {
    let mut bus = CommandBus::new(project.clone());
    for track in &project.tracks {
        if names.contains(&track.name.as_str()) {
            bus.execute(SetTrackMute {
                track: track.id,
                mute: true,
            })
            .expect("mute");
        }
    }
    bus.project().clone()
}

#[test]
fn a_phrase_command_edits_the_project_and_its_undo_button_reverts_it() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    let before = app.project().tempo.bpm();
    run(
        &mut app,
        AssistantIntent::Instruct("set tempo to 126".to_owned()),
    );
    assert!((app.project().tempo.bpm() - 126.0).abs() < 1e-9);
    let entry = last_entry(&app).clone();
    assert_eq!(entry.handled_by, "parser");
    assert_eq!(entry.error, None);
    let step = entry.applied.last().expect("an edit").clone();
    assert!(step.commands.iter().any(|c| c == "set_tempo"), "{step:?}");
    assert!(!app.features.assistant_busy);

    run(
        &mut app,
        AssistantIntent::UndoStep {
            depth: step.depth,
            label: step.label.clone(),
        },
    );
    assert!((app.project().tempo.bpm() - before).abs() < 1e-9);
    // Undoing it twice is refused (it is no longer the newest step).
    run(
        &mut app,
        AssistantIntent::UndoStep {
            depth: step.depth,
            label: step.label,
        },
    );
    assert!(app.ui_state.status.as_ref().is_some_and(|s| s.error));
}

#[test]
fn generated_drums_put_a_kick_on_every_beat() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    run(
        &mut app,
        AssistantIntent::Tool(
            "add_house_drum_pattern".to_owned(),
            drum_args(2, "classic", 1),
        ),
    );
    assert_eq!(last_entry(&app).error, None, "{:?}", last_entry(&app));
    let names: Vec<&str> = app
        .project()
        .tracks
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        names.contains(&"Kick") && names.contains(&"Drums"),
        "{names:?}"
    );
    // The kick alone: an onset on each of the 8 beats at 120 BPM.
    let project = muted(app.project(), &["Drums"]);
    let bpm = project.tempo.bpm();
    let audio = render(&app, &project, 0.5);
    let found = onset_times(&audio.mono(), RATE);
    let beat = 60.0 / bpm;
    for n in 0..8 {
        let t = n as f64 * beat;
        assert!(
            found.iter().any(|f| (f - t).abs() < 0.010),
            "no kick near {t:.3} s in {found:?}"
        );
    }
    let low = spectrum_band_energy(&audio.mono(), RATE, 30.0, 150.0);
    let high = spectrum_band_energy(&audio.mono(), RATE, 2_000.0, 10_000.0);
    assert!(low > high * 4.0, "a kick is mostly low: {low} vs {high}");
    // The whole kit plays more than the kick alone.
    let full = render(&app, app.project(), 0.5);
    assert!(rms(&full.mono()) > rms(&audio.mono()));
}

#[test]
fn a_generated_bassline_is_in_key_and_low() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    run(
        &mut app,
        AssistantIntent::Tool(
            "add_bassline".to_owned(),
            bass_args(2, "A minor", "offbeat", 3),
        ),
    );
    let bass = app
        .project()
        .tracks
        .iter()
        .find(|t| t.name == "Bass")
        .expect("a Bass track");
    let a_minor = [9, 11, 0, 2, 4, 5, 7];
    let mut pitches = Vec::new();
    for clip in &bass.clips {
        if let ClipContent::Midi { notes } = &clip.content {
            pitches.extend(notes.iter().map(|n| n.pitch));
        }
    }
    assert!(!pitches.is_empty());
    assert!(
        pitches.iter().all(|p| a_minor.contains(&(p % 12))),
        "{pitches:?}"
    );
    assert!(
        pitches.iter().all(|p| *p < 60),
        "bass register: {pitches:?}"
    );
    let audio = render(&app, app.project(), 0.5);
    assert!(rms(&audio.mono()) > 1e-3, "the bass sounds");
    let low = spectrum_band_energy(&audio.mono(), RATE, 30.0, 400.0);
    let total = spectrum_band_energy(&audio.mono(), RATE, 20.0, 20_000.0);
    assert!(low > total * 0.3, "bass energy is low: {low} of {total}");
}

fn write_reference(app: &FourfloorApp, path: &Path, gain: f32) {
    let mix = render(app, app.project(), 0.5);
    let quiet = StereoBuffer::new(
        RATE,
        mix.left.iter().map(|s| s * gain).collect(),
        mix.right.iter().map(|s| s * gain).collect(),
    );
    quiet.write_wav(path).expect("reference written");
}

#[test]
fn the_mix_report_measures_the_mix_and_compares_it_with_the_reference() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    run(
        &mut app,
        AssistantIntent::Tool("add_house_drum_pattern".to_owned(), drum_args(2, "deep", 2)),
    );
    run(&mut app, AssistantIntent::MixReport);
    let alone = app.features.last_report.clone().expect("a report");
    assert!(alone.mix.integrated_lufs.is_finite() && alone.mix.integrated_lufs > -40.0);
    assert!(alone.reference.is_none());
    assert!(last_entry(&app)
        .reply
        .as_deref()
        .is_some_and(|r| r.contains("LUFS")));

    // A reference that is the same mix 6 dB quieter.
    let reference = dir.join("reference.wav");
    write_reference(&app, &reference, 0.5);
    run(&mut app, Intent::File(FileIntent::LoadReference(reference)));
    run(&mut app, AssistantIntent::MixReport);
    let report = app.features.last_report.clone().expect("a report");
    let compared = report.reference.expect("compared with the reference");
    assert!(
        (compared.loudness_delta_lu - 6.02).abs() < 0.5,
        "mix is 6 dB louder: {}",
        compared.loudness_delta_lu
    );
    for band in [&compared.low, &compared.mid, &compared.high] {
        assert!(
            band.delta_db.abs() < 1.0,
            "same balance once matched: {band:?}"
        );
    }
    assert_eq!(report.reference_name.as_deref(), Some("reference.wav"));
    assert!(last_entry(&app).prompt.contains("reference.wav"));
}

/// A saved project with one audio track holding `signal` (mono, 48 kHz).
fn project_with_audio(
    app: &mut FourfloorApp,
    dir: &Path,
    signal: &[f32],
) -> fourfloor_core::ClipId {
    run(app, Intent::File(FileIntent::SaveAs(dir.join("project"))));
    run(
        app,
        Command::CreateTrack(CreateTrack {
            name: "Loop".to_owned(),
            kind: TrackKind::Audio,
            index: None,
        }),
    );
    let track = app.project().tracks.last().expect("track").id;
    let wav = dir.join("loop.wav");
    StereoBuffer::from_mono(RATE, signal)
        .write_wav(&wav)
        .expect("wav");
    run(
        app,
        Intent::ImportAudio {
            track,
            path: wav,
            at: 0,
        },
    );
    let clip = app.project().tracks.last().expect("track").clips[0].id;
    app.ui_state.selection.clip = Some(clip);
    clip
}

fn kick_and_pad(seconds: f64) -> Vec<f32> {
    let pad = fourfloor_testkit::sine(220.0, 0.2, seconds, RATE);
    let times: Vec<f64> = (0..(seconds * 2.0) as usize)
        .map(|n| n as f64 * 0.5)
        .collect();
    let clicks = fourfloor_testkit::click_train(&times, 0.6, seconds, RATE);
    fourfloor_testkit::mix(&[pad.as_slice(), clicks.as_slice()])
}

#[test]
fn stem_separation_adds_stem_tracks_that_sum_to_the_clip_and_undo_removes_them() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    let clip = project_with_audio(&mut app, &dir, &kick_and_pad(2.0));
    let before = app.project().tracks.len();
    run(
        &mut app,
        MlIntent::Separate {
            clip,
            prefer_model: false,
        },
    );
    let entry = last_entry(&app).clone();
    assert_eq!(entry.error, None, "{entry:?}");
    let stems: Vec<String> = app.project().tracks[before..]
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(stems.len(), 3, "drums, bass, other: {stems:?}");
    for track in &app.project().tracks[before..] {
        assert!(matches!(track.clips[0].content, ClipContent::Audio { .. }));
    }
    // Stems alone against the original alone: they null.
    let original = render(
        &app,
        &muted(
            app.project(),
            &stems.iter().map(String::as_str).collect::<Vec<_>>(),
        ),
        0.2,
    );
    let split = render(&app, &muted(app.project(), &["Loop"]), 0.2);
    let residual = fourfloor_testkit::null_residual_db(&original.mono(), &split.mono());
    assert!(residual < -50.0, "stems sum to the clip: {residual:.1} dB");
    let step = entry.applied[0].clone();
    run(
        &mut app,
        AssistantIntent::UndoStep {
            depth: step.depth,
            label: step.label,
        },
    );
    assert_eq!(app.project().tracks.len(), before);
}

#[test]
fn audio_to_midi_writes_the_melody_when_basic_pitch_is_downloaded() {
    let cache = fourfloor_ml::ModelCache::new(fourfloor_ml::ModelCache::default_dir());
    let registry = fourfloor_ml::ModelRegistry::builtin();
    let spec = registry.get(fourfloor_ml::BASIC_PITCH).expect("spec");
    if !cache.is_cached(spec) {
        eprintln!("Basic Pitch is not in the model cache; skipping the transcription check");
        return;
    }
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    app.features.paths.models = fourfloor_ml::ModelCache::default_dir();
    let melody: Vec<f32> = [220.0, 261.63, 329.63, 0.0]
        .iter()
        .flat_map(|hz| fourfloor_testkit::sine(*hz, if *hz > 0.0 { 0.4 } else { 0.0 }, 0.5, RATE))
        .collect();
    let clip = project_with_audio(&mut app, &dir, &melody);
    run(&mut app, MlIntent::Transcribe(clip));
    assert_eq!(last_entry(&app).error, None, "{:?}", last_entry(&app));
    let midi = app
        .project()
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Midi)
        .expect("MIDI track");
    let ClipContent::Midi { notes } = &midi.clips[0].content else {
        panic!("a MIDI clip");
    };
    let pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
    assert_eq!(pitches, vec![57, 60, 64], "A3 C4 E4");
}

fn http_initialize(addr: SocketAddr) -> String {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#;
    let mut stream = TcpStream::connect(addr).expect("connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    write!(
        stream,
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("request sent");
    let mut reply = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = stream.read(&mut chunk) {
        if n == 0 {
            break;
        }
        reply.extend_from_slice(&chunk[..n]);
        if String::from_utf8_lossy(&reply).contains("serverInfo") {
            break;
        }
    }
    String::from_utf8_lossy(&reply).into_owned()
}

#[test]
fn the_mcp_server_answers_on_localhost_and_client_edits_reach_the_document() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
    run(&mut app, AssistantIntent::StartMcp(addr));
    let server = app.features.mcp.as_ref().expect("serving");
    assert!(server.url.starts_with("http://127.0.0.1:") && server.url.ends_with("/mcp"));
    let bound: SocketAddr = server.url["http://".len()..server.url.len() - "/mcp".len()]
        .parse()
        .expect("bound address");
    let reply = http_initialize(bound);
    assert!(
        reply.contains("serverInfo"),
        "MCP initialize answered: {reply}"
    );

    let agent = std::sync::Arc::clone(server.agent());
    agent
        .lock()
        .expect("agent")
        .call_tool("set_tempo", serde_json::json!({ "bpm": 128.0 }))
        .expect("client edit");
    wait_jobs(&mut app);
    assert!((app.project().tempo.bpm() - 128.0).abs() < 1e-9);
    assert_eq!(last_entry(&app).handled_by, "mcp");
    // An edit made in the app reaches the client's view.
    run(
        &mut app,
        AssistantIntent::Instruct("set tempo to 122".to_owned()),
    );
    wait_jobs(&mut app);
    let seen = agent.lock().expect("agent").project().tempo.bpm();
    assert!((seen - 122.0).abs() < 1e-9, "client sees {seen}");

    run(&mut app, AssistantIntent::StopMcp);
    assert!(app.features.mcp.is_none());
    assert!(TcpStream::connect_timeout(&bound, Duration::from_millis(500)).is_err());
}

#[test]
fn an_unreachable_chat_model_is_an_error_in_the_log_not_a_hang() {
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    let config = llm_config("http://127.0.0.1:9/v1", "none").expect("local");
    run(&mut app, AssistantIntent::SetLlm(Some(config)));
    let before = app.project().clone();
    run(
        &mut app,
        AssistantIntent::Instruct("zxqv blorp the frobnicator".to_owned()),
    );
    let entry = last_entry(&app);
    assert!(entry.error.is_some(), "{entry:?}");
    assert!(!app.features.assistant_busy);
    assert_eq!(app.project(), &before);
    assert!(app.ui_state.toasts.iter().any(|t| t.error));
    // Phrase commands still work with the model down.
    run(
        &mut app,
        AssistantIntent::Instruct("set tempo to 125".to_owned()),
    );
    assert!((app.project().tempo.bpm() - 125.0).abs() < 1e-9);
}

#[test]
fn spleeter_stems_sum_to_the_clip_when_the_model_is_downloaded() {
    let models = fourfloor_ml::ModelCache::default_dir();
    let cache = fourfloor_ml::ModelCache::new(models.clone());
    let registry = fourfloor_ml::ModelRegistry::builtin();
    let spec = registry.get(fourfloor_ml::SPLEETER_2STEMS).expect("spec");
    if !cache.is_cached(spec) {
        eprintln!("Spleeter is not in the model cache; skipping");
        return;
    }
    let (_guard, dir) = temp_dir();
    let mut app = library_app(&dir);
    app.features.paths.models = models;
    let clip = project_with_audio(&mut app, &dir, &kick_and_pad(3.0));
    let before = app.project().tracks.len();
    run(
        &mut app,
        MlIntent::Separate {
            clip,
            prefer_model: true,
        },
    );
    let entry = last_entry(&app).clone();
    assert_eq!(entry.error, None, "{entry:?}");
    assert!(entry.tools[0].text.contains("vocals") && !entry.tools[0].text.contains("fallback"));
    let stems: Vec<String> = app.project().tracks[before..]
        .iter()
        .map(|t| t.name.clone())
        .collect();
    assert_eq!(stems.len(), 2, "{stems:?}");
    let names: Vec<&str> = stems.iter().map(String::as_str).collect();
    let original = render(&app, &muted(app.project(), &names), 0.2);
    let split = render(&app, &muted(app.project(), &["Loop"]), 0.2);
    let residual = fourfloor_testkit::null_residual_db(&original.mono(), &split.mono());
    assert!(
        residual < -40.0,
        "Spleeter stems sum to the clip: {residual:.1} dB"
    );
}
