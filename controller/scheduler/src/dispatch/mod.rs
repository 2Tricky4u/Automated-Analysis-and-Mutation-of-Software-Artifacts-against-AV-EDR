//! Dispatch module - JobWorker architecture for job execution.
//!
//! Architecture:
//! - JobWorker: Spawned per job, owns job lifecycle, builds artifacts, aggregates results
//! - RunPool: Shared queue where all jobs put runs, VMExecutors take from it
//! - VMExecutor: Thin dispatcher per VM, takes runs, routes results back
//! - Orchestrator: Spawns JobWorkers, manages RunPool, tracks VMs
//!
//! Key Features:
//! - Jobs run in parallel (no active_job: Option limit)
//! - VMs are dumb executors shared across all jobs
//! - Results route back to originating JobWorker via RunPool
//! - Signal-driven dispatch (efficient, no polling)

pub mod channels;
pub mod job_worker;
pub mod orchestrator;
pub mod run_pool;
pub mod types;
pub mod vm_executor;

// Re-exports for public API
pub use channels::{JobControlCommand, RemoteRunResult};
pub use orchestrator::Orchestrator;
pub use run_pool::RunPool;
pub use vm_executor::{ArtifactSender, VMExecutor};

// Types
pub use types::{
    JobId, JobSession, ModularBuildSpec, ModuleSelectionSpec, RunId, TargetId, VMInfo, WorkerId,
    WorkerInfo,
};
