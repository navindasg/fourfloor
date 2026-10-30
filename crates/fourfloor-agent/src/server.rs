//! The MCP server, on the official Rust SDK (`rmcp`).
//!
//! Tools come from [`crate::tools::catalog`]: one per model command plus
//! the high-level tools, with `readOnlyHint` and `destructiveHint` set.
//! Resources come from [`crate::resources::RESOURCES`]. Tool calls run on a
//! blocking thread because renders take real time.
//!
//! Two transports: stdio ([`serve_stdio`]) for clients that launch the
//! server, and Streamable HTTP on a loopback address ([`serve_http`]).

use std::net::SocketAddr;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::AgentError;
use crate::nl::{with_agent, SharedAgent};
use crate::resources::{read_resource, RESOURCES};
use crate::tools::{catalog, ToolSpec};

const INSTRUCTIONS: &str = "fourfloor is a DAW for house music. Every editing tool goes \
through the project's command bus and can be reverted with `undo`. Read \
fourfloor://project/summary or call list_tracks for track and device ids. High-level tools \
write whole parts (add_house_drum_pattern, add_bassline, add_chords), route ducking \
(sidechain), add risers and render. `instruct` accepts plain English such as \"swing the \
hats to 56%\".";

/// The MCP handler around a shared agent. Cloning shares the agent, so
/// every HTTP session edits the same project.
#[derive(Clone, Debug)]
pub struct FourfloorServer {
    agent: SharedAgent,
}

impl FourfloorServer {
    /// A server for `agent`.
    pub fn new(agent: SharedAgent) -> Self {
        Self { agent }
    }

    /// The shared agent.
    pub fn agent(&self) -> &SharedAgent {
        &self.agent
    }
}

/// An MCP tool for a catalog entry.
pub fn mcp_tool(spec: &ToolSpec) -> Tool {
    let schema: JsonObject = match &spec.input_schema {
        Value::Object(object) => object.clone(),
        _ => JsonObject::new(),
    };
    let annotations = ToolAnnotations::with_title(spec.title.clone())
        .read_only(spec.read_only)
        .destructive(spec.destructive)
        .idempotent(false)
        .open_world(false);
    Tool::new(
        spec.name.clone(),
        spec.description.clone(),
        Arc::new(schema),
    )
    .with_title(spec.title.clone())
    .with_annotations(annotations)
}

fn tool_result(result: Result<crate::tools::ToolOutput, AgentError>) -> CallToolResult {
    match result {
        Ok(output) => {
            let mut done = CallToolResult::success(vec![ContentBlock::text(output.text)]);
            let mut data = output.data;
            if !data.is_object() {
                data = serde_json::json!({ "value": data });
            }
            data["project_changed"] = Value::Bool(output.changed);
            done.structured_content = Some(data);
            done
        }
        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    }
}

impl ServerHandler for FourfloorServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(
            "fourfloor-agent",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(
            catalog().iter().map(mcp_tool).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        if !catalog().iter().any(|tool| tool.name == name) {
            return Err(McpError::invalid_params(
                format!("unknown tool \"{name}\""),
                None,
            ));
        }
        let args = request.arguments.map_or(Value::Null, Value::Object);
        let agent = Arc::clone(&self.agent);
        let result = tokio::task::spawn_blocking(move || {
            with_agent(&agent, |agent| agent.call_tool(&name, args))
        })
        .await
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(tool_result(result).into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(
            RESOURCES
                .iter()
                .map(|info| {
                    Resource::new(info.uri, info.name)
                        .with_description(info.description)
                        .with_mime_type("application/json")
                })
                .collect(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri.clone();
        if !RESOURCES.iter().any(|info| info.uri == uri) {
            return Err(McpError::resource_not_found(
                format!("unknown resource {uri}"),
                None,
            ));
        }
        let agent = Arc::clone(&self.agent);
        let read_uri = uri.clone();
        let text = tokio::task::spawn_blocking(move || {
            with_agent(&agent, |agent| read_resource(agent, &read_uri))
        })
        .await
        .map_err(|error| McpError::internal_error(error.to_string(), None))?
        .map_err(|error| McpError::internal_error(error.to_string(), None))?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type("application/json")
        ])
        .into())
    }
}

/// Serves `agent` over stdin and stdout until the client disconnects.
pub async fn serve_stdio(agent: SharedAgent) -> Result<(), AgentError> {
    let running = FourfloorServer::new(agent)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| AgentError::Unavailable(format!("MCP start failed: {error}")))?;
    running
        .waiting()
        .await
        .map_err(|error| AgentError::Unavailable(format!("MCP server stopped: {error}")))?;
    Ok(())
}

/// A running Streamable HTTP server.
#[derive(Debug)]
pub struct HttpServer {
    /// Where it listens. The MCP endpoint is `http://<addr>/mcp`.
    pub addr: SocketAddr,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

impl HttpServer {
    /// The MCP endpoint URL.
    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// Stops the server and waits for it to finish.
    pub async fn shutdown(self) -> Result<(), AgentError> {
        self.cancel.cancel();
        self.task
            .await
            .map_err(|error| AgentError::Unavailable(error.to_string()))?
            .map_err(|error| AgentError::Unavailable(error.to_string()))
    }

    /// Waits until the server stops (for example on [`HttpServer::cancel_token`]).
    pub async fn wait(self) -> Result<(), AgentError> {
        self.task
            .await
            .map_err(|error| AgentError::Unavailable(error.to_string()))?
            .map_err(|error| AgentError::Unavailable(error.to_string()))
    }

    /// A token that stops the server when cancelled.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Serves `agent` over Streamable HTTP at `addr`, which must be a loopback
/// address (port 0 picks a free port). The endpoint is `/mcp`.
pub async fn serve_http(agent: SharedAgent, addr: SocketAddr) -> Result<HttpServer, AgentError> {
    if !addr.ip().is_loopback() {
        return Err(AgentError::Unavailable(format!(
            "{addr} is not a loopback address; the agent server only listens on this machine"
        )));
    }
    let cancel = CancellationToken::new();
    let config =
        StreamableHttpServerConfig::default().with_cancellation_token(cancel.child_token());
    let server = FourfloorServer::new(agent);
    let service: StreamableHttpService<FourfloorServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(LocalSessionManager::default()),
            config,
        );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|error| AgentError::Unavailable(format!("cannot listen on {addr}: {error}")))?;
    let bound = listener
        .local_addr()
        .map_err(|error| AgentError::Unavailable(error.to_string()))?;
    let stop = cancel.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { stop.cancelled_owned().await })
            .await
    });
    Ok(HttpServer {
        addr: bound,
        cancel,
        task,
    })
}
