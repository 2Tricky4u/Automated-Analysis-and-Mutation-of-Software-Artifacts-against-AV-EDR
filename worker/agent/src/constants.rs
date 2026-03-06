//! Compile-time constants for the worker agent.

/// Timeout for cleanup operations in Drop (seconds)
pub const CLEANUP_TIMEOUT_SECS: u64 = 10;

/// Monitor polling interval (seconds)
pub const MONITOR_POLL_INTERVAL_SECS: u64 = 3;

/// CPU threshold (%) below which a process is considered idle
pub const CPU_IDLE_THRESHOLD: i32 = 5;

/// Consecutive idle polls before marking telemetry_idle
pub const IDLE_COUNT_THRESHOLD: i32 = 3;

/// Seconds before timeout to start warning
pub const TIMEOUT_APPROACH_SECS: i32 = 5;

/// Max serialized payload bytes (gRPC default max is 4MB)
pub const MAX_SERIALIZED_PAYLOAD: usize = 3_500_000;
