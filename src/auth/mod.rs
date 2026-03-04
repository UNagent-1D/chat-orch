// JWT validation middleware for REST endpoints.
// Webhook endpoints use signature verification instead (handled in ingest/).

pub mod jwt;
pub mod rest;
