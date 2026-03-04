// LLM integration: client trait, OpenAI implementation, tool executor, turn loop.
//
// The `LlmClient` trait abstracts the LLM provider for testability.
// `OpenAiClient` is the production implementation (works with any OpenAI-compatible API).
// `ToolExecutor` handles calling external data source APIs when the LLM requests tool calls.
// `turn_loop::execute_turn` orchestrates the complete LLM→tools→LLM cycle.

pub mod client;
pub mod tool_executor;
pub mod turn_loop;
