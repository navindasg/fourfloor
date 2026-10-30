//! The `fourfloor-agent serve` binary over stdio, launched the way an MCP
//! client launches it: tools are listed, an edit lands and is autosaved.

use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::ServiceExt;
use serde_json::json;
use tokio::process::Command;

#[tokio::test(flavor = "multi_thread")]
async fn the_binary_serves_mcp_on_stdio_and_autosaves() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("stdio-song");
    let transport = TokioChildProcess::new(
        Command::new(env!("CARGO_BIN_EXE_fourfloor-agent")).configure(|cmd| {
            cmd.arg("serve").arg(&folder);
        }),
    )
    .unwrap();
    let client = ().serve(transport).await.unwrap();
    let info = client.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "fourfloor-agent");
    let tools = client.list_all_tools().await.unwrap();
    assert!(tools.len() > 60, "{} tools", tools.len());

    let args = json!({ "bpm": 122 }).as_object().unwrap().clone();
    let result = client
        .call_tool(CallToolRequestParams::new("set_tempo").with_arguments(args))
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    client.cancel().await.unwrap();

    let saved = fourfloor_model::load_project(&folder).unwrap();
    assert_eq!(saved.tempo.bpm(), 122.0);
    assert_eq!(saved.name, "stdio-song");
}
