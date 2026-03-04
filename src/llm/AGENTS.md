# AGENTS.md — llm/

## Purpose

LLM integration for the conversation turn loop. Handles communication with
language model providers (OpenAI for MVP) and execution of tool calls against
external data sources.

## Files

| File | Purpose |
|------|---------|
| `client.rs` | `LlmClient` trait + `OpenAiClient` implementation |
| `tool_executor.rs` | Execute tool calls against tenant data sources (route_configs) |
| `turn_loop.rs` | Complete → tool_call → execute → complete loop |

## Turn Loop

The core conversation loop:

1. Build messages array (system prompt + history + user message)
2. Call LLM with tool definitions from agent config
3. If LLM returns `tool_calls` → execute each via tool_executor
4. Append tool results to messages
5. Call LLM again with tool results
6. Repeat until LLM returns a final text response (or max iterations)

## Tool Execution

Tools are defined in the Agent Config Registry (`tool_permissions` JSONB).
Each tool maps to a data source operation via `route_configs`:

```
Tool: list_doctors → DataSource: Hospital Mock API
  → route_configs["list_doctors"] → GET /doctors
```

The tool executor:
1. Looks up the tool name in agent config
2. Finds the matching data source + route config
3. Constructs the HTTP request (method + path + params from LLM)
4. Calls the external API
5. Returns the response to the LLM

## Conventions

- `LlmClient` is a trait for testability (mock in tests, OpenAI in prod)
- Max 10 tool call iterations per turn (prevent infinite loops)
- Tool execution respects tenant role: tenant_operators can only call GET operations
- System prompt comes from agent config `llm_params.system_prompt`
