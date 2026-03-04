// Library crate entry point — exposes modules for integration tests.
//
// The binary entry point is main.rs. This file re-exports modules so
// integration tests in tests/ can access internal types.

pub mod config;
pub mod conversation;
pub mod error;
pub mod router;
pub mod state;

pub mod auth;
pub mod gateway;
pub mod ingest;
pub mod llm;
pub mod pipeline;
pub mod types;
