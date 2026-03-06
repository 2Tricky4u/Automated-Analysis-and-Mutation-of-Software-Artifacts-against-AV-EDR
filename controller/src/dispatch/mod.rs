//! Dispatch module for concurrent job execution.
//!
//! The [`Orchestrator`] spawns a `JobWorker` per job. Each worker produces
//! [`RunEnvelope`](types::RunEnvelope)s into the shared [`RunPool`], where
//! [`VMExecutor`]s pull them for remote execution. Results route back through
//! the pool to the originating worker. All components communicate via async
//! channels and run in parallel without polling.

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

// Types (used by internal modules, not all consumed by binary target)
#[allow(unused_imports)]
pub use types::{
    JobId, JobSession, ModularBuildSpec, ModuleSelectionSpec, RunId, TargetId, VMInfo, WorkerId,
    WorkerInfo,
};
