//! VM target management, gRPC connection lifecycle, and artifact transport.
//!
//! Registers targets from TOML configuration or dynamic discovery, maintains
//! per-target state (available, busy, offline), establishes bidirectional
//! gRPC streams, spawns [`VMExecutor`](crate::dispatch::VMExecutor) tasks,
//! and handles artifact deployment.

pub mod manager;

// Re-exports (used by internal modules, not all consumed by binary target)
#[allow(unused_imports)]
pub use manager::{RegistrationType, Target, TargetEvent, TargetManager, TargetStatus};
