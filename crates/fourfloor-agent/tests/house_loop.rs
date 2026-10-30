//! A 16-bar house loop built only through MCP tool calls, rendered and
//! checked by ear-substitute: kicks land on the beats, the pad ducks when
//! the kick hits (and stops ducking when the sidechain is undone), the
//! render has the right length and sane loudness.

mod common;

use common::{call, connect, data, project, shared_in};
use fourfloor_testkit::{
    find_nan_or_clipping, lufs, onset_times, rms_window_seconds, times_match, Fault, StereoBuffer,
};
use serde_json::json;

const BPM: f64 = 124.0;
const BARS: usize = 16;

fn beat_seconds() -> f64 {
    60.0 / BPM
}

/// Mean RMS of `buffer` in a window `offset` seconds after beats 2 to 4 of
/// every bar (beat 1 is skipped: the pad's own chord change lands there).
fn after_beats(buffer: &StereoBuffer, offset: f64, length: f64) -> f32 {
    let mono = buffer.mono();
    let rate = buffer.sample_rate;
    let windows: Vec<f32> = (0..BARS * 4)
        .filter(|beat| beat % 4 != 0)
        .map(|beat| rms_window_seconds(&mono, rate, beat as f64 * beat_seconds() + offset, length))
        .collect();
    windows.iter().sum::<f32>() / windows.len() as f32
}

fn stem(dir: &std::path::Path, files: &serde_json::Value, name: &str) -> StereoBuffer {
    let path = files
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|file| file.as_str())
        .find(|file| file.ends_with(&format!("{name}.wav")))
        .unwrap_or_else(|| panic!("no {name} stem in {files}"));
    assert!(path.starts_with(dir.to_str().unwrap()));
    StereoBuffer::read_wav(std::path::Path::new(path)).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tool_built_house_loop_renders_with_kicks_on_the_beat_and_ducking() {
    let dir = tempfile::tempdir().unwrap();
    let agent = shared_in(dir.path());
    let client = connect(agent.clone()).await;

    call(&client, "set_tempo", json!({ "bpm": BPM })).await;
    let drums = call(
        &client,
        "add_house_drum_pattern",
        json!({ "bars": BARS, "style": "classic", "seed": 3 }),
    )
    .await;
    data(&drums);
    let bass = call(
        &client,
        "add_bassline",
        json!({ "bars": BARS, "key": "A minor", "style": "offbeat", "seed": 3 }),
    )
    .await;
    data(&bass);
    let pad = call(
        &client,
        "add_chords",
        json!({ "bars": BARS, "key": "A minor", "style": "sustained", "seed": 3 }),
    )
    .await;
    data(&pad);
    let duck = call(
        &client,
        "sidechain",
        json!({ "track": "chords", "key_track": "kick", "depth": 0.9 }),
    )
    .await;
    data(&duck);
    let state = project(&agent);
    assert_eq!(state.tracks.len(), 4);

    // The mix: written into the project folder, the right length, sound.
    let rendered = call(&client, "render", json!({ "path": "loop.wav" })).await;
    let rendered = data(&rendered);
    let path = dir.path().join("loop.wav");
    assert_eq!(rendered["path"], json!(path.display().to_string()));
    let mix = StereoBuffer::read_wav(&path).unwrap();
    let expected = BARS as f64 * 4.0 * beat_seconds() + 1.0;
    assert!(
        (mix.seconds() - expected).abs() < 0.05,
        "{} s, expected {expected}",
        mix.seconds()
    );
    for channel in mix.channels() {
        assert!(
            !matches!(find_nan_or_clipping(channel), Some(Fault::NotFinite { .. })),
            "the mix has NaN"
        );
    }
    let loudness = lufs(&mix);
    assert!((-30.0..0.0).contains(&loudness), "mix at {loudness} LUFS");

    // Stems: one per track, aligned with the master.
    let stems = call(&client, "export_stems", json!({ "dir": "stems" })).await;
    let files = data(&stems)["files"].clone();
    assert_eq!(files.as_array().unwrap().len(), 5, "master + 4 stems");
    let kick = stem(dir.path(), &files, "Kick");
    let chords = stem(dir.path(), &files, "Chords");

    // Kicks on every beat, and nowhere else.
    let beats: Vec<f64> = (0..BARS * 4)
        .map(|beat| beat as f64 * beat_seconds())
        .collect();
    let found = onset_times(&kick.mono(), kick.sample_rate);
    assert!(
        times_match(&found, &beats, 0.010),
        "kick onsets {:?} (first 8) vs beats {:?}",
        &found[..found.len().min(8)],
        &beats[..8]
    );

    // The pad dips right after each kick and recovers before the next.
    let on_kick = after_beats(&chords, 0.010, 0.040);
    let between = after_beats(&chords, 0.360, 0.080);
    assert!(between > 0.0, "the pad sounds");
    assert!(
        on_kick < 0.5 * between,
        "ducked on the kick: {on_kick} vs {between} between kicks"
    );

    // Undo the sidechain through the same bus: the dip goes away.
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(data(&undone)["undone"], json!("sidechain"));
    let stems = call(&client, "export_stems", json!({ "dir": "stems-dry" })).await;
    let dry = stem(dir.path(), &data(&stems)["files"], "Chords");
    let dry_on_kick = after_beats(&dry, 0.010, 0.040);
    let dry_between = after_beats(&dry, 0.360, 0.080);
    assert!(
        dry_on_kick > 0.8 * dry_between,
        "no ducking without the sidechain: {dry_on_kick} vs {dry_between}"
    );
    // The un-ducked pad is louder right after the kick than the ducked one.
    assert!(dry_on_kick > 2.0 * on_kick);
    client.cancel().await.unwrap();
}
