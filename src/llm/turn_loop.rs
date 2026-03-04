use crate::error::AppError;
use crate::gateway::acr_client::{AgentConfig, ToolPermission, ToolRegistryEntry};
use crate::gateway::tenant_client::DataSource;

use super::client::{
    ChatMessage, FunctionDefinition, LlmClient, LlmChoice, MessageRole, ToolDefinition,
};
use super::tool_executor::ToolExecutor;

/// Maximum number of tool-call round trips per conversation turn.
/// Prevents infinite loops if the LLM keeps requesting tools.
const MAX_TOOL_ITERATIONS: usize = 10;

/// Execute a full conversation turn: LLM call -> optional tool calls -> final answer.
///
/// This is the core loop described in the AGENTS.md:
/// 1. Build messages (system prompt + user message)
/// 2. Call LLM with tool definitions
/// 3. If LLM returns tool_calls -> execute them -> append results -> call LLM again
/// 4. Repeat until LLM returns a text response or max iterations reached
///
/// The `tool_registry` parameter provides rich OpenAI function definitions from
/// the ACR's global tool registry. If empty, falls back to constraints-based
/// definitions (auto-generated descriptions + empty parameter schemas).
pub async fn execute_turn(
    llm_client: &dyn LlmClient,
    tool_executor: &ToolExecutor,
    agent_config: &AgentConfig,
    data_sources: &[DataSource],
    user_message: &str,
    conversation_history: &[ChatMessage],
    tool_registry: &[ToolRegistryEntry],
) -> Result<String, AppError> {
    // Build tool definitions from agent config + registry
    let tools = build_tool_definitions(agent_config, tool_registry);
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

/// Build tool definitions for the LLM from the agent config's tool_permissions,
/// enriched with the global tool registry when available.
///
/// For each tool in `agent_config.tool_permissions`:
/// 1. Look up the tool_name in the `tool_registry`
/// 2. If found AND active -> use the registry's `openai_function_def`
///    (validates that it has `name` and `parameters` fields)
/// 3. If found but inactive -> skip (log warning, tool is disabled globally)
/// 4. If found but `openai_function_def` is malformed -> fall back to constraints
/// 5. If not found in registry -> fall back to constraints-based definition
///
/// The fallback generates auto-descriptions ("Execute the {name} operation")
/// with empty parameter schemas. This is functional but less accurate for
/// LLM tool calling.
fn build_tool_definitions(
    agent_config: &AgentConfig,
    tool_registry: &[ToolRegistryEntry],
) -> Vec<ToolDefinition> {
    agent_config
        .tool_permissions
        .iter()
        .filter_map(|tp| {
            match tool_registry.iter().find(|r| r.tool_name == tp.tool_name) {
                Some(entry) if entry.is_active => {
                    // Validate that openai_function_def has required fields
                    let func_def = &entry.openai_function_def;
                    if func_def.get("name").is_none() || func_def.get("parameters").is_none() {
                        tracing::warn!(
                            tool = %tp.tool_name,
                            "registry entry has malformed openai_function_def (missing name or parameters) \
                             — falling back to constraints"
                        );
                        return Some(fallback_definition(tp));
                    }

                    // Try to deserialize the registry's function definition
                    match serde_json::from_value::<FunctionDefinition>(func_def.clone()) {
                        Ok(function) => Some(ToolDefinition {
                            tool_type: "function".to_string(),
                            function,
                        }),
                        Err(e) => {
                            tracing::warn!(
                                tool = %tp.tool_name,
                                error = %e,
                                "failed to parse registry openai_function_def — falling back to constraints"
                            );
                            Some(fallback_definition(tp))
                        }
                    }
                }
                Some(_entry) => {
                    // Tool exists in registry but is_active == false
                    tracing::warn!(
                        tool = %tp.tool_name,
                        "tool exists in registry but is inactive — skipping"
                    );
                    None
                }
                None => {
                    // Not in registry — use constraints-based fallback
                    if !tool_registry.is_empty() {
                        tracing::debug!(
                            tool = %tp.tool_name,
                            "tool not found in registry — using constraints-based definition"
                        );
                    }
                    Some(fallback_definition(tp))
                }
            }
        })
        .collect()
}

/// Build a fallback tool definition from the tool permission's constraints.
///
/// This is the original behavior before tool registry integration:
/// - Description: "Execute the {tool_name} operation"
/// - Parameters: from `constraints` if present, else empty object schema
fn fallback_definition(tp: &ToolPermission) -> ToolDefinition {
    let parameters = if tp.constraints.is_null() || tp.constraints == serde_json::json!({}) {
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
}
