// Concurrency management and Redis-backed pipeline infrastructure.
//
// - worker.rs: Semaphore-bounded task spawning (each msg gets its own tokio task)
// - session.rs: Redis session get-or-create with composite key
// - dedup.rs: Atomic SETNX deduplication to prevent double-processing

pub mod dedup;
pub mod session;
pub mod worker;
