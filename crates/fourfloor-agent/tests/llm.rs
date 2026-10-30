//! The chat layer against a scripted OpenAI-compatible server on
//! localhost: the model's tool calls run through the agent, results go
//! back to the model, and the parser still handles what it knows without
//! asking the model.

mod common;

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use fourfloor_agent::nl::llm::OpenAiCompatible;
use fourfloor_agent::nl::{instruct, ChatOptions};
use serde_json::{json, Value};

type Log = Arc<Mutex<Vec<Value>>>;

async fn completions(State(log): State<Log>, Json(body): Json<Value>) -> Json<Value> {
    let round = {
        let mut log = log.lock().unwrap();
        log.push(body);
        log.len()
    };
    let message = if round == 1 {
        json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [
                { "id": "t1", "type": "function",
                  "function": { "name": "set_tempo", "arguments": "{\"bpm\": 126}" } },
                { "id": "t2", "type": "function",
                  "function": { "name": "add_house_drum_pattern",
                                "arguments": "{\"bars\": 2, \"style\": \"deep\"}" } },
                { "id": "t3", "type": "function",
                  "function": { "name": "no_such_tool", "arguments": "{}" } }
            ]
        })
    } else {
        json!({ "role": "assistant", "content": "Set 126 BPM and wrote a deep groove." })
    };
    Json(
        json!({ "id": "x", "object": "chat.completion", "choices": [{ "index": 0, "message": message }] }),
    )
}

async fn mock_server() -> (String, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let app = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(log.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/v1"), log)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_local_model_drives_the_tools_and_sees_their_results() {
    let (url, log) = mock_server().await;
    let backend = OpenAiCompatible::new(&url, "scripted").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let agent = common::shared_in(dir.path());

    let done = instruct(
        &agent,
        "give me something groovy and a bit faster",
        Some(&backend),
        &ChatOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(done.handled_by, "scripted");
    assert_eq!(
        done.reply.as_deref(),
        Some("Set 126 BPM and wrote a deep groove.")
    );
    assert_eq!(done.steps.len(), 3);
    assert!(done.steps[0].result.is_ok() && done.steps[1].result.is_ok());
    assert!(done.steps[2].result.is_err(), "unknown tools are refused");

    let state = common::project(&agent);
    assert_eq!(state.tempo.bpm(), 126.0);
    assert_eq!(state.tracks.len(), 2);

    let requests = log.lock().unwrap().clone();
    assert_eq!(requests.len(), 2);
    let first = &requests[0];
    assert_eq!(first["model"], json!("scripted"));
    assert_eq!(first["messages"][0]["role"], json!("system"));
    assert_eq!(
        first["messages"][1]["content"],
        json!("give me something groovy and a bit faster")
    );
    let offered: Vec<&str> = first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .collect();
    assert!(offered.contains(&"add_house_drum_pattern") && offered.contains(&"set_tempo"));
    assert!(!offered.contains(&"instruct"));
    // The second request carries the assistant's calls and each result.
    let second = requests[1]["messages"].as_array().unwrap();
    let tool_messages: Vec<&Value> = second.iter().filter(|m| m["role"] == "tool").collect();
    assert_eq!(tool_messages.len(), 3);
    assert_eq!(tool_messages[0]["tool_call_id"], json!("t1"));
    assert!(tool_messages[2]["content"]
        .as_str()
        .unwrap()
        .contains("not available"));

    // Every model edit is undoable like any other.
    for _ in 0..2 {
        instruct(&agent, "undo", None, &ChatOptions::default())
            .await
            .unwrap();
    }
    let reverted = common::project(&agent);
    assert!(reverted.tracks.is_empty());
    assert_eq!(reverted.tempo.bpm(), 120.0);
}

#[tokio::test(flavor = "multi_thread")]
async fn known_phrases_never_reach_the_model() {
    let (url, log) = mock_server().await;
    let backend = OpenAiCompatible::new(&url, "scripted").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let agent = common::shared_in(dir.path());
    let done = instruct(
        &agent,
        "set tempo to 123",
        Some(&backend),
        &ChatOptions::default(),
    )
    .await
    .unwrap();
    assert_eq!(done.handled_by, "parser");
    assert_eq!(common::project(&agent).tempo.bpm(), 123.0);
    assert!(log.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_model_is_an_error_not_a_hang() {
    // Bind and drop a listener to get a port nothing is listening on.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let backend = OpenAiCompatible::new(&format!("http://127.0.0.1:{port}/v1"), "none").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let agent = common::shared_in(dir.path());
    let result = instruct(
        &agent,
        "do something clever",
        Some(&backend),
        &ChatOptions::default(),
    )
    .await;
    assert!(
        matches!(result, Err(fourfloor_agent::AgentError::Llm(_))),
        "{result:?}"
    );
    assert!(common::project(&agent).tracks.is_empty());
}
