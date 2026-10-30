//! The MCP server, driven by an rmcp client: tool listing, tool calls that
//! change the project, undo through the same bus, resources, errors, and
//! the Streamable HTTP transport.

mod common;

use common::{call, connect, data, project, shared_in, text};
use fourfloor_model::{Command, TrackKind};
use rmcp::model::ReadResourceRequestParams;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::ServiceExt;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn tools_cover_every_command_and_the_high_level_set() {
    let dir = tempfile::tempdir().unwrap();
    let client = connect(shared_in(dir.path())).await;
    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();

    // One tool per model command (set_swing is the extended high-level one).
    for command in Command::NAMES {
        assert!(names.contains(command), "missing {command}");
    }
    for high in [
        "add_house_drum_pattern",
        "add_bassline",
        "add_chords",
        "set_swing",
        "sidechain",
        "add_filter_riser",
        "render",
        "export_stems",
        "undo",
        "redo",
        "mix_report",
        "instruct",
    ] {
        assert!(names.contains(&high), "missing {high}");
    }
    let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
    assert_eq!(unique.len(), names.len(), "tool names are unique");

    // Schemas are objects with properties; destructive tools are flagged.
    let tool = |name: &str| tools.iter().find(|tool| tool.name == name).unwrap();
    let create = tool("create_track");
    assert_eq!(create.input_schema.get("type"), Some(&json!("object")));
    assert!(create.input_schema["properties"].get("kind").is_some());
    let hints = |name: &str| tool(name).annotations.clone().unwrap();
    assert_eq!(hints("delete_track").destructive_hint, Some(true));
    assert_eq!(hints("remove_device").destructive_hint, Some(true));
    assert_eq!(hints("create_track").destructive_hint, Some(false));
    assert_eq!(hints("add_bassline").destructive_hint, Some(false));
    assert_eq!(hints("project_summary").read_only_hint, Some(true));
    assert_eq!(hints("set_tempo").read_only_hint, Some(false));
    assert!(tool("delete_track")
        .description
        .as_deref()
        .unwrap()
        .contains("Destructive"));
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn command_tools_edit_the_project_and_undo_reverts_them() {
    let dir = tempfile::tempdir().unwrap();
    let agent = shared_in(dir.path());
    let client = connect(agent.clone()).await;
    let empty = project(&agent);

    let created = call(
        &client,
        "create_track",
        json!({ "name": "Pad", "kind": "midi" }),
    )
    .await;
    let track = data(&created)["track"].as_u64().unwrap();
    assert_eq!(data(&created)["project_changed"], json!(true));
    let after_create = project(&agent);
    assert_eq!(after_create.tracks.len(), 1);
    assert_eq!(after_create.tracks[0].name, "Pad");
    assert_eq!(after_create.tracks[0].kind, TrackKind::Midi);

    call(&client, "set_tempo", json!({ "bpm": 124 })).await;
    assert_eq!(project(&agent).tempo.bpm(), 124.0);
    call(
        &client,
        "set_track_gain",
        json!({ "track": track, "gain_db": -6.0 }),
    )
    .await;
    assert_eq!(project(&agent).tracks[0].gain_db, -6.0);

    // Undo walks back through exactly these steps, then redo replays one.
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(data(&undone)["undone"], json!("set_track_gain"));
    assert_eq!(project(&agent).tracks[0].gain_db, 0.0);
    call(&client, "undo", json!({})).await;
    assert_eq!(project(&agent).tempo.bpm(), 120.0);
    call(&client, "undo", json!({})).await;
    assert_eq!(project(&agent), empty);
    let nothing = call(&client, "undo", json!({})).await;
    assert_eq!(nothing.is_error, Some(true));
    call(&client, "redo", json!({})).await;
    assert_eq!(project(&agent), after_create);
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn high_level_tools_are_one_undo_step_each() {
    let dir = tempfile::tempdir().unwrap();
    let agent = shared_in(dir.path());
    let client = connect(agent.clone()).await;

    let drums = call(
        &client,
        "add_house_drum_pattern",
        json!({ "bars": 4, "seed": 7 }),
    )
    .await;
    let written = data(&drums);
    assert_eq!(written["kick_track_created"], json!(true));
    let state = project(&agent);
    assert_eq!(state.tracks.len(), 2);
    assert_eq!(state.tracks[0].devices[0].kind, "drum.kick909");
    assert_eq!(state.tracks[1].devices[0].kind, "drum.kit");
    let kick_notes = state.tracks[0].clips[0].notes();
    assert_eq!(kick_notes.len(), 16, "four kicks per bar");

    call(
        &client,
        "add_chords",
        json!({ "bars": 4, "style": "sustained", "key": "F minor" }),
    )
    .await;
    let ducked = call(
        &client,
        "sidechain",
        json!({ "track": "chords", "key_track": "kick" }),
    )
    .await;
    let device = data(&ducked)["device"].as_u64().unwrap();
    let state = project(&agent);
    let chords = &state.tracks[2];
    let compressor = chords
        .devices
        .iter()
        .find(|d| d.id.get() == device)
        .unwrap();
    assert_eq!(compressor.kind, "fx.compressor");
    assert_eq!(compressor.sidechain.unwrap().track, state.tracks[0].id);

    // One undo removes the compressor and its routing together.
    let undone = call(&client, "undo", json!({})).await;
    assert_eq!(data(&undone)["undone"], json!("sidechain"));
    assert_eq!(project(&agent).tracks[2].devices.len(), 1);
    // And one more removes the whole chord part, track included.
    call(&client, "undo", json!({})).await;
    assert_eq!(project(&agent).tracks.len(), 2);
    call(&client, "undo", json!({})).await;
    assert!(project(&agent).tracks.is_empty());
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_calls_report_errors_and_change_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let agent = shared_in(dir.path());
    let client = connect(agent.clone()).await;
    let before = project(&agent);

    let bad_type = call(&client, "set_tempo", json!({ "bpm": "fast" })).await;
    assert_eq!(bad_type.is_error, Some(true));
    let out_of_range = call(&client, "set_tempo", json!({ "bpm": 5 })).await;
    assert_eq!(out_of_range.is_error, Some(true));
    let unknown_field = call(&client, "add_bassline", json!({ "wobble": 1 })).await;
    assert_eq!(unknown_field.is_error, Some(true));
    assert!(text(&unknown_field).contains("wobble"));
    let no_track = call(
        &client,
        "sidechain",
        json!({ "track": "pad", "key_track": "kick" }),
    )
    .await;
    assert_eq!(no_track.is_error, Some(true));
    assert!(text(&no_track).contains("no track matches"));
    let escape = call(&client, "render", json!({ "path": "../out.wav" })).await;
    assert_eq!(escape.is_error, Some(true));

    let mut unknown = rmcp::model::CallToolRequestParams::new("not_a_tool");
    unknown = unknown.with_arguments(serde_json::Map::new());
    assert!(client.call_tool(unknown).await.is_err(), "protocol error");

    assert_eq!(project(&agent), before);
    assert!(!agent.lock().unwrap().bus().can_undo());
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn resources_describe_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let agent = shared_in(dir.path());
    let client = connect(agent.clone()).await;
    call(&client, "add_house_drum_pattern", json!({ "bars": 2 })).await;
    call(&client, "set_tempo", json!({ "bpm": 126 })).await;

    let resources = client.list_all_resources().await.unwrap();
    let uris: Vec<&str> = resources.iter().map(|r| r.uri.as_str()).collect();
    for uri in [
        "fourfloor://project/summary",
        "fourfloor://project/tracks",
        "fourfloor://project/mix-report",
        "fourfloor://devices",
    ] {
        assert!(uris.contains(&uri), "missing {uri}");
    }
    let read = |uri: &'static str| {
        let client = &client;
        async move {
            let result = client
                .read_resource(ReadResourceRequestParams::new(uri))
                .await
                .unwrap();
            match &result.contents[0] {
                rmcp::model::ResourceContents::TextResourceContents { text, .. } => {
                    serde_json::from_str::<serde_json::Value>(text).unwrap()
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    };
    let summary = read("fourfloor://project/summary").await;
    assert_eq!(summary["bpm"], json!(126.0));
    assert_eq!(summary["tracks"].as_array().unwrap().len(), 2);
    assert_eq!(summary["undo"], json!("set_tempo"));

    let tracks = read("fourfloor://project/tracks").await;
    let kick = &tracks["tracks"][0];
    assert_eq!(kick["devices"][0]["kind"], json!("drum.kick909"));
    assert!(!kick["devices"][0]["params"].as_array().unwrap().is_empty());

    let devices = read("fourfloor://devices").await;
    let kinds: Vec<&str> = devices["devices"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"fx.compressor") && kinds.contains(&"synth.acid303"));

    let report = read("fourfloor://project/mix-report").await;
    let lufs = report["master"]["integrated_lufs"].as_f64().unwrap();
    assert!((-40.0..0.0).contains(&lufs), "master {lufs} LUFS");
    assert_eq!(report["stems"].as_array().unwrap().len(), 2);
    assert!(report["spectral_balance_db"]["sub_and_bass_20_150"].is_number());

    let missing = client
        .read_resource(ReadResourceRequestParams::new("fourfloor://nope"))
        .await;
    assert!(missing.is_err());
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn streamable_http_serves_the_same_tools_on_localhost() {
    let dir = tempfile::tempdir().unwrap();
    let agent = shared_in(dir.path());
    let server = fourfloor_agent::serve_http(agent.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    assert!(server.addr.ip().is_loopback());
    let transport = StreamableHttpClientTransport::from_uri(server.url());
    let client = ().serve(transport).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert!(tools
        .iter()
        .any(|tool| tool.name == "add_house_drum_pattern"));
    call(&client, "set_tempo", json!({ "bpm": 127 })).await;
    assert_eq!(project(&agent).tempo.bpm(), 127.0);
    call(&client, "undo", json!({})).await;
    assert_eq!(project(&agent).tempo.bpm(), 120.0);
    client.cancel().await.unwrap();
    server.shutdown().await.unwrap();

    // A non-loopback address is refused.
    assert!(
        fourfloor_agent::serve_http(agent, "0.0.0.0:0".parse().unwrap())
            .await
            .is_err()
    );
}
