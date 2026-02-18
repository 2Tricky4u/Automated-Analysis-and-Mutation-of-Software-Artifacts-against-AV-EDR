//! VM module - connection management and artifact transport
//!
//! Handles:
//! - Target/VM registration (from TOML or dynamic)
//! - gRPC connection/stream management
//! - Target state (Available/Busy/Offline)
//! - Spawning VMExecutor tasks on connection
//! - Artifact deployment to VMs

pub mod manager;

// Re-exports (used by internal modules, not all consumed by binary target)
#[allow(unused_imports)]
pub use manager::{RegistrationType, Target, TargetEvent, TargetManager, TargetStatus};
