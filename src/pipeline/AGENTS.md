# AGENTS.md — pipeline/

## Purpose

Concurrency management for message processing. Handles the transition from
webhook handler (must return fast) to background processing (may take seconds).

## Pattern: Semaphore + tokio::spawn

NOT a worker pool. Each message gets its own lightweight tokio task, bounded
by a semaphore:

```rust
let permit = semaphore.try_acquire()?;  // 503 if full
tokio::spawn(async move {
    let _permit = permit;  // held until task completes
    process_message(app, msg).await;
});
// Return 200 OK — processing happens in background
```

This scales to 100k msg/sec because:
- tokio tasks are cheap (~few KB stack each)
- I/O-bound work (HTTP calls) doesn't need CPU pinning
- Semaphore provides natural backpressure (503 when overloaded)

## Files

| File | Purpose |
|------|---------|
| `worker.rs` | `Pipeline::try_process()` — semaphore acquire, spawn, orchestrate |
| `session.rs` | Redis session get-or-create, composite key, retry-on-401 |

## Session Keys (Redis)

Format: `session:{tenant_id}:{channel_type}:{channel_user_id}`
TTL: configurable, default 30 minutes idle

## Conventions

- Acquire semaphore BEFORE returning 200 to webhook provider
- If semaphore exhausted → return 503 (platform will retry)
- `tokio::join!` for parallel independent downstream calls
- Dedup check happens in the webhook handler, BEFORE pipeline
