use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

// ─── Message types for the LLM conversation ───────────────────────────

/// A single message in the LLM conversation history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: Option<String>,
    /// Present when the assistant requests tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// Present on "tool" role messages — references the tool_call_id it responds to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A tool call requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

/// The function name + arguments within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON-encoded arguments string from the LLM.
    pub arguments: String,
}

// ─── Tool definitions sent to the LLM ─────────────────────────────────

/// A tool definition passed in the `tools` array of the chat completion request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: FunctionDefinition,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

// ─── LLM response ─────────────────────────────────────────────────────

/// The choice returned by the LLM.
#[derive(Debug, Clone)]
pub struct LlmChoice {
    /// Text content (present when LLM gives a final answer).
    pub content: Option<String>,
    /// Tool calls (present when LLM wants to invoke tools).
    pub tool_calls: Vec<ToolCall>,
    /// Finish reason: "stop", "tool_calls", "length", etc.
    pub finish_reason: String,
}

impl LlmChoice {
    /// Returns true if the LLM wants to call tools (not a final answer).
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

// ─── LLM Client trait ─────────────────────────────────────────────────

/// Trait for LLM providers. Allows mocking in tests.
///
/// The orchestrator only calls `chat_completion` — all provider-specific
/// details (auth, retries, streaming) are handled by the implementation.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Send a chat completion request and return the first choice.
    async fn chat_completion(
        &self,
        model: &str,
        temperature: f32,
        max_tokens: u32,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmChoice, AppError>;
}

// ─── OpenAI implementation ────────────────────────────────────────────

/// OpenAI-compatible chat completion request body.
#[derive(Debug, Serialize)]
struct OpenAiRequest<'a> {
    model: &'a str,
    temperature: f32,
    max_tokens: u32,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ToolDefinition]>,
}

/// OpenAI chat completion response (subset of fields we need).
#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCall>,
}

/// OpenAI API client using `reqwest`.
///
/// Works with any OpenAI-compatible API (OpenAI, Azure OpenAI, Ollama, vLLM)
/// by changing the `base_url`.
pub struct OpenAiClient {
    client: Client,
    base_url: String,
    api_key: String,
}

impl OpenAiClient {
    pub fn new(client: Client, base_url: String, api_key: String) -> Self {
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn chat_completion(
        &self,
        model: &str,
        temperature: f32,
        max_tokens: u32,
        messages: &[ChatMessage],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<LlmChoice, AppError> {
        let url = format!("{}/chat/completions", self.base_url);

        let body = OpenAiRequest {
            model,
            temperature,
            max_tokens,
            messages,
            tools,
        };

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| AppError::LlmError(format!("LLM request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::LlmError(format!(
                "LLM returned {status}: {body}"
            )));
        }

        let data: OpenAiResponse = resp
            .json()
            .await
            .map_err(|e| AppError::LlmError(format!("invalid LLM response: {e}")))?;

        let choice = data
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AppError::LlmError("LLM returned no choices".into()))?;

        Ok(LlmChoice {
            content: choice.message.content,
            tool_calls: choice.message.tool_calls,
            finish_reason: choice.finish_reason,
        })
    }
}
