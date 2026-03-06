//! Execution engine, monitoring, and state management.
//!
//! Orchestrates the full artifact execution lifecycle: validation, RedEDR setup,
//! process spawning, monitoring, telemetry collection, outcome classification,
//! and cleanup. RAII guards ensure resource safety on all exit paths.
pub mod classifier;
pub mod engine;
pub mod guards;
pub mod monitor;
pub mod sink;
pub mod state;
pub mod types;
