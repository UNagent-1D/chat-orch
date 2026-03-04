// Authentication middleware for the Chat Orchestrator.
//
// - jwt.rs: JWT validation for REST API endpoints (Bearer token)
// - api_key.rs: API key validation for internal/ops endpoints (X-Api-Key header)
// - rest.rs: REST endpoint handlers (protected by JWT)
//
// Webhook endpoints use channel-specific signature verification instead
// (handled in ingest/).

pub mod api_key;
pub mod jwt;
pub mod rest;
