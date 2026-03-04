use crate::error::AppError;
use crate::gateway::acr_client::AgentConfig;
use crate::gateway::tenant_client::DataSource;

use super::client::{
    ChatMessage, FunctionDefinition, LlmClient, LlmChoice, MessageRole, ToolDefinition,
};
use super::tool_executor::ToolExecutor;

/// Maximum number of tool-call round trips per conversation turn.
/// Prevents infinite loops if the LLM keeps requesting tools.
const MAX_TOOL_ITERATIONS: usize = 10;

/// Execute a full conversation turn: LLM call → optional tool calls → final answer.
///
/// This is the core loop described in the AGENTS.md:
/// 1. Build messages (system prompt + user message)
/// 2. Call LLM with tool definitions
/// 3. If LLM returns tool_calls → execute them → append results → call LLM again
/// 4. Repeat until LLM returns a text response or max iterations reached
///
/// Returns the final text response from the LLM.
pub async fn execute_turn(
    llm_client: &dyn LlmClient,
    tool_executor: &ToolExecutor,
    agent_config: &AgentConfig,
    data_sources: &[DataSource],
    user_message: &str,
    conversation_history: &[ChatMessage],
) -> Result<String, AppError> {
    // Build tool definitions from agent config
    let tools = build_tool_definitions(agent_config);
    let tools_ref: Option<&[ToolDefinition]> = if tools.is_empty() {
        None
    } else {
        Some(&tools)
    };

    // Build initial messages: system prompt + history + new user message
    let mut messages = Vec::with_capacity(conversation_history.len() + 2);

    // System prompt from agent config
    messages.push(ChatMessage {
        role: MessageRole::System,
        content: Some(agent_config.llm_params.system_prompt.clone()),
        tool_calls: None,
        tool_call_id: None,
    });

    // Conversation history (if any)
    messages.extend_from_slice(conversation_history);

    // Current user message
    messages.push(ChatMessage {
        role: MessageRole::User,
        content: Some(user_message.to_string()),
        tool_calls: None,
        tool_call_id: None,
    });

    let model = &agent_config.llm_params.model;
    let temperature = agent_config.llm_params.temperature;
    let max_tokens = agent_config.llm_params.max_tokens;

    // Turn loop: call LLM, handle tool calls, repeat
    for iteration in 0..MAX_TOOL_ITERATIONS {
        let choice: LlmChoice = llm_client
            .chat_completion(model, temperature, max_tokens, &messages, tools_ref)
            .await?;

        if !choice.has_tool_calls() {
            // Final text response — return it
            let text = choice.content.unwrap_or_default();
            if text.is_empty() {
                return Err(AppError::LlmError(
                    "LLM returned empty response with no tool calls".into(),
                ));
            }
            tracing::info!(iterations = iteration + 1, "turn completed");
            return Ok(text);
        }

        // LLM wants tool calls — process them
        tracing::debug!(
            iteration = iteration + 1,
            tool_count = choice.tool_calls.len(),
            tools = ?choice.tool_calls.iter().map(|tc| &tc.function.name).collect::<Vec<_>>(),
            "executing tool calls"
        );

        // Append assistant message with tool_calls to history
        messages.push(ChatMessage {
            role: MessageRole::Assistant,
            content: choice.content.clone(),
            tool_calls: Some(choice.tool_calls.clone()),
            tool_call_id: None,
        });

        // Execute all tool calls (in parallel within the batch)
        let results = tool_executor
            .execute_batch(&choice.tool_calls, agent_config, data_sources)
            .await;

        // Append each tool result as a "tool" message
        for result in results {
            messages.push(ChatMessage {
                role: MessageRole::Tool,
                content: Some(result.output),
                tool_calls: None,
                tool_call_id: Some(result.tool_call_id),
            });
        }
    }

    // If we exhausted iterations, return the last content or an error
    Err(AppError::LlmError(format!(
        "exceeded max tool call iterations ({MAX_TOOL_ITERATIONS})"
    )))
}

/// Build tool definitions for the LLM from the agent config's tool_permissions.
///
/// Each tool permission entry specifies the tool name. The function schema
/// (parameters) comes from the constraints field if present, or defaults
/// to an empty object schema.
fn build_tool_definitions(agent_config: &AgentConfig) -> Vec<ToolDefinition> {
    agent_config
        .tool_permissions
        .iter()
        .map(|tp| {
            let parameters = if tp.constraints.is_null() || tp.constraints == serde_json::json!({})
            {
                serde_json::json!({
                    "type": "object",
                    "properties": {}
                })
            } else {
                tp.constraints.clone()
            };

            ToolDefinition {
                tool_type: "function".to_string(),
                function: FunctionDefinition {
                    name: tp.tool_name.clone(),
                    description: format!("Execute the {} operation", tp.tool_name),
                    parameters,
                },
            }
        })
        .collect()
}
