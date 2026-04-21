use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::AppError;
use crate::hospital::ToolDef;

/// OpenAI-style chat message. Used both as input (prompt) and output (history).
/// Serialization matches the OpenAI chat.completions wire format so we can
/// round-trip through the API without a translation layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_type")]
    pub kind: String,
    pub function: ToolCallFunction,
}

fn default_type() -> String {
    "function".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    /// Arguments are a JSON-encoded STRING per OpenAI spec, not an object.
    pub arguments: String,
}

pub enum ChatResponse {
    /// Plain assistant reply. Final answer for the turn.
    Content(String),
    /// Assistant requested tool invocations. Caller must execute and loop.
    ToolCalls(Vec<ToolCall>),
}

#[derive(Clone)]
pub struct LlmClient {
    http: Client,
    base_url: String,
    api_key: String,
    model: String,
}

impl LlmClient {
    pub fn new(http: Client, base_url: String, api_key: String, model: String) -> Self {
        Self {
            http,
            base_url,
            api_key,
            model,
        }
    }

    pub async fn complete(
        &self,
        messages: &[ChatMessage],
        tools: &[ToolDef],
    ) -> Result<ChatResponse, AppError> {
        let tools_json: Vec<Value> = tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();

        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tools_json,
            "tool_choice": "auto",
        });

        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let raw: Value = resp.json().await?;
        if !status.is_success() {
            return Err(AppError::Downstream(format!(
                "LLM {status}: {}",
                raw.to_string().chars().take(500).collect::<String>()
            )));
        }

        let choice = raw
            .pointer("/choices/0/message")
            .ok_or_else(|| AppError::Downstream("LLM response missing choices[0].message".into()))?
            .clone();

        let tool_calls = choice
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .filter(|a| !a.is_empty());

        if let Some(calls) = tool_calls {
            let parsed: Vec<ToolCall> = calls
                .iter()
                .filter_map(|c| serde_json::from_value(c.clone()).ok())
                .collect();
            if !parsed.is_empty() {
                return Ok(ChatResponse::ToolCalls(parsed));
            }
        }

        let content = choice
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(ChatResponse::Content(content))
    }
}
