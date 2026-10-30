//! Language-model chat backends.
//!
//! [`ChatBackend`] is the seam: anything that can answer a chat with tool
//! calls. [`OpenAiCompatible`] talks to any server that implements the
//! OpenAI chat-completions API with tools (llama.cpp's `llama-server`,
//! Ollama, LM Studio, vLLM, mistral.rs's server, ...). It is meant for a
//! model running on this machine: it refuses hosts other than localhost
//! unless [`OpenAiCompatible::allow_remote`] is called, so nothing reaches
//! a paid cloud API by accident. No weights are downloaded here.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::AgentError;
use crate::tools::ToolSpec;

/// Who said a chat message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Instructions for the model.
    System,
    /// The person.
    User,
    /// The model.
    Assistant,
    /// A tool result.
    Tool,
}

/// A tool call the model asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LlmToolCall {
    /// The backend's id for the call, echoed in the result message.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Arguments as JSON text, as the model wrote them.
    pub arguments: String,
}

/// One message of a chat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    /// Who said it.
    pub role: Role,
    /// Text, if any.
    pub content: Option<String>,
    /// Tool calls (assistant messages only).
    pub tool_calls: Vec<LlmToolCall>,
    /// Which call this answers (tool messages only).
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    /// A plain text message.
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// The result of tool call `id`.
    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.into()),
        }
    }

    /// The message in the OpenAI wire format.
    pub fn to_openai(&self) -> Value {
        let mut message = json!({ "role": self.role });
        message["content"] = self.content.clone().map_or(Value::Null, Value::String);
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = self
                .tool_calls
                .iter()
                .map(|call| {
                    json!({
                        "id": call.id,
                        "type": "function",
                        "function": { "name": call.name, "arguments": call.arguments },
                    })
                })
                .collect();
        }
        if let Some(id) = &self.tool_call_id {
            message["tool_call_id"] = json!(id);
        }
        message
    }
}

/// What the model answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatReply {
    /// Text, if any.
    pub content: Option<String>,
    /// Tool calls, if any.
    pub tool_calls: Vec<LlmToolCall>,
}

impl ChatReply {
    /// The reply as an assistant message, to append to the chat.
    pub fn to_message(&self) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: self.content.clone(),
            tool_calls: self.tool_calls.clone(),
            tool_call_id: None,
        }
    }
}

/// A boxed future, so the trait stays object safe.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Anything that can continue a chat, possibly with tool calls.
pub trait ChatBackend: Send + Sync {
    /// A short name for reports.
    fn name(&self) -> &str;

    /// The model's next reply to `messages`, given the tools it may call.
    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<ChatReply, AgentError>>;
}

/// A tool in the OpenAI function-calling format.
pub fn openai_tool(spec: &ToolSpec) -> Value {
    let mut parameters = spec.input_schema.clone();
    if let Some(object) = parameters.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
    }
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": spec.description,
            "parameters": parameters,
        }
    })
}

/// Reads an OpenAI chat-completions response body.
pub fn parse_openai_reply(body: &Value) -> Result<ChatReply, AgentError> {
    let message = body
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| AgentError::Llm(format!("response has no choices[0].message: {body}")))?;
    let content = message
        .get("content")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned);
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .enumerate()
                .filter_map(|(index, call)| {
                    let function = call.get("function")?;
                    let name = function.get("name")?.as_str()?.to_owned();
                    // Some servers send the arguments as an object, not text.
                    let arguments = match function.get("arguments") {
                        Some(Value::String(text)) => text.clone(),
                        Some(other) => other.to_string(),
                        None => "{}".to_owned(),
                    };
                    let id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .map_or_else(|| format!("call_{index}"), str::to_owned);
                    Some(LlmToolCall {
                        id,
                        name,
                        arguments,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(ChatReply {
        content,
        tool_calls,
    })
}

/// A chat backend for an OpenAI-compatible server on this machine.
#[derive(Clone, Debug)]
pub struct OpenAiCompatible {
    base_url: reqwest::Url,
    model: String,
    api_key: Option<String>,
    temperature: f32,
    client: reqwest::Client,
}

fn is_loopback(url: &reqwest::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

impl OpenAiCompatible {
    /// A backend for `base_url` (the part before `/chat/completions`, such
    /// as `http://127.0.0.1:8080/v1`) running `model`.
    ///
    /// Refuses anything but `http` or `https` on localhost; see
    /// [`OpenAiCompatible::allow_remote`].
    pub fn new(base_url: &str, model: &str) -> Result<Self, AgentError> {
        let parsed = Self::parse_url(base_url)?;
        if !is_loopback(&parsed) {
            return Err(AgentError::Llm(format!(
                "{base_url} is not on this machine; fourfloor only talks to a local model \
                 unless allow_remote is used"
            )));
        }
        Self::build(parsed, model)
    }

    /// A backend for a server on another machine, such as a model server
    /// on the local network. The caller takes responsibility for what it
    /// connects to.
    pub fn allow_remote(base_url: &str, model: &str) -> Result<Self, AgentError> {
        Self::build(Self::parse_url(base_url)?, model)
    }

    fn parse_url(base_url: &str) -> Result<reqwest::Url, AgentError> {
        let trimmed = base_url.trim_end_matches('/');
        let parsed = reqwest::Url::parse(&format!("{trimmed}/"))
            .map_err(|error| AgentError::Llm(format!("bad endpoint {base_url}: {error}")))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(AgentError::Llm(format!("{base_url} is not an http URL")));
        }
        Ok(parsed)
    }

    fn build(base_url: reqwest::Url, model: &str) -> Result<Self, AgentError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|error| AgentError::Llm(error.to_string()))?;
        Ok(Self {
            base_url,
            model: model.to_owned(),
            api_key: None,
            temperature: 0.2,
            client,
        })
    }

    /// Sends `Authorization: Bearer <key>`; some local servers want one.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Sampling temperature. Defaults to 0.2.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }

    /// The model name sent with each request.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The request body for a chat.
    pub fn request_body(&self, messages: &[ChatMessage], tools: &[ToolSpec]) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": messages.iter().map(ChatMessage::to_openai).collect::<Vec<_>>(),
            "temperature": self.temperature,
            "stream": false,
        });
        if !tools.is_empty() {
            body["tools"] = tools.iter().map(openai_tool).collect();
            body["tool_choice"] = json!("auto");
        }
        body
    }

    async fn send(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolSpec],
    ) -> Result<ChatReply, AgentError> {
        let url = self
            .base_url
            .join("chat/completions")
            .map_err(|error| AgentError::Llm(error.to_string()))?;
        let mut request = self
            .client
            .post(url)
            .json(&self.request_body(messages, tools));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| AgentError::Llm(format!("request failed: {error}")))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|error| AgentError::Llm(format!("unreadable response ({status}): {error}")))?;
        if !status.is_success() {
            return Err(AgentError::Llm(format!("server answered {status}: {body}")));
        }
        parse_openai_reply(&body)
    }
}

impl ChatBackend for OpenAiCompatible {
    fn name(&self) -> &str {
        &self.model
    }

    fn complete<'a>(
        &'a self,
        messages: &'a [ChatMessage],
        tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<ChatReply, AgentError>> {
        Box::pin(self.send(messages, tools))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_endpoints_are_accepted_by_default() {
        for local in [
            "http://localhost:8080/v1",
            "http://127.0.0.1:11434/v1/",
            "http://[::1]:1234/v1",
        ] {
            assert!(OpenAiCompatible::new(local, "m").is_ok(), "{local}");
        }
        for remote in [
            "https://api.openai.com/v1",
            "http://10.0.0.5:8080/v1",
            "ftp://localhost/",
        ] {
            assert!(OpenAiCompatible::new(remote, "m").is_err(), "{remote}");
        }
        assert!(OpenAiCompatible::allow_remote("http://10.0.0.5:8080/v1", "m").is_ok());
    }

    #[test]
    fn replies_with_tool_calls_are_read() {
        let body = json!({
            "choices": [{ "message": {
                "role": "assistant", "content": null,
                "tool_calls": [
                    { "id": "a", "type": "function",
                      "function": { "name": "set_tempo", "arguments": "{\"bpm\":124}" } },
                    { "type": "function",
                      "function": { "name": "undo", "arguments": {} } }
                ]
            }}]
        });
        let reply = parse_openai_reply(&body).unwrap();
        assert_eq!(reply.content, None);
        assert_eq!(reply.tool_calls.len(), 2);
        assert_eq!(reply.tool_calls[0].arguments, "{\"bpm\":124}");
        assert_eq!(reply.tool_calls[1].id, "call_1");
        assert_eq!(reply.tool_calls[1].arguments, "{}");
        assert!(parse_openai_reply(&json!({})).is_err());
    }

    #[test]
    fn requests_carry_messages_and_tools() {
        let backend = OpenAiCompatible::new("http://localhost:1/v1", "qwen").unwrap();
        let spec = crate::tools::catalog()
            .into_iter()
            .find(|tool| tool.name == "set_tempo")
            .unwrap();
        let body = backend.request_body(&[ChatMessage::text(Role::User, "hi")], &[spec]);
        assert_eq!(body["model"], "qwen");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["tools"][0]["function"]["name"], "set_tempo");
        assert!(body["tools"][0]["function"]["parameters"]["properties"]["bpm"].is_object());
    }
}
