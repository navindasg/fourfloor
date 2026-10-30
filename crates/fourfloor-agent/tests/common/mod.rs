//! Shared test helpers: an agent on a temporary project folder and an MCP
//! client connected to it in-process.

#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use fourfloor_agent::{default_registry, share, Agent, FourfloorServer, SharedAgent};
use fourfloor_model::Project;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::Value;

/// An agent editing a new project whose folder is `dir`.
pub fn agent_in(dir: &Path) -> Agent {
    let registry = Arc::new(default_registry().expect("registry"));
    Agent::new(Project::new("Agent Test"), registry).with_project_dir(dir)
}

/// The same, shared.
pub fn shared_in(dir: &Path) -> SharedAgent {
    share(agent_in(dir))
}

/// An MCP client talking to a server for `agent` over an in-memory pipe.
pub async fn connect(agent: SharedAgent) -> RunningService<RoleClient, ()> {
    let (server_side, client_side) = tokio::io::duplex(1 << 20);
    let server = FourfloorServer::new(agent);
    tokio::spawn(async move {
        if let Ok(running) = server.serve(server_side).await {
            let _ = running.waiting().await;
        }
    });
    ().serve(client_side).await.expect("client connects")
}

/// Calls a tool and returns its result.
pub async fn call(
    client: &RunningService<RoleClient, ()>,
    name: &str,
    args: Value,
) -> CallToolResult {
    let mut params = CallToolRequestParams::new(name.to_owned());
    if let Value::Object(object) = args {
        params = params.with_arguments(object);
    }
    client.call_tool(params).await.expect("tool call is routed")
}

/// The structured result of a successful call.
pub fn data(result: &CallToolResult) -> Value {
    assert_ne!(
        result.is_error,
        Some(true),
        "tool failed: {:?}",
        result.content
    );
    result.structured_content.clone().unwrap_or(Value::Null)
}

/// The text of a result's first content block.
pub fn text(result: &CallToolResult) -> String {
    result
        .content
        .first()
        .and_then(|block| block.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default()
}

/// A snapshot of the project.
pub fn project(agent: &SharedAgent) -> Project {
    agent.lock().expect("lock").project().clone()
}
