//! Dispatch module - shared pool dispatch and orchestration.
//!
//! Architecture:
//! - PoolGroup: Shared run pool for workers with matching capabilities
//! - Worker: Thin dispatcher that owns a VM connection
//! - Orchestrator: Routes jobs to compatible pool groups
//!
//! Key Features:
//! - Signal-driven dispatch (efficient, no polling for run pool)
//! - Multiple VMs can share a pool for parallel execution
//! - Each VM runs only one execution at a time

pub mod channels;
pub mod group_id;
pub mod orchestrator;
pub mod pool_group;
pub mod types;
pub mod worker;

// Re-exports for public API
pub use channels::{OrchestratorEvent, RemoteRunResult, WorkerCommand, WorkerEvent};
pub use orchestrator::Orchestrator;
pub use pool_group::{PoolEvent, PoolGroupRegistry};
pub use types::{
    JobId, JobOutcome, JobSession, ModularBuildSpec, ModuleSelectionSpec, RoundSpec, RunEnvelope,
    RunId, RunOutcome, RunType, WorkerId, WorkerInfo,
};
pub use worker::{ArtifactSender, Worker};
