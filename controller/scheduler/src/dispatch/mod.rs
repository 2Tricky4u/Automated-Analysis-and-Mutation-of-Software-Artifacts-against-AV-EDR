//! Dispatch module - Worker-VM-bound dispatch and orchestration.
//!
//! Architecture:
//! - Worker: One task per VM, owns job execution
//! - Orchestrator: Routes jobs to compatible workers
//! - Types: JobSession, RoundSpec, RunEnvelope, etc.

pub mod channels;
pub mod orchestrator;
pub mod types;
pub mod worker;

// Re-exports
pub use channels::{OrchestratorEvent, RemoteRunResult, WorkerCommand, WorkerEvent};
pub use orchestrator::{Orchestrator, WorkerHandle};
pub use types::{
    ArtifactRef, JobId, JobOutcome, JobSession, MutationSpec, RoundAgg, RoundId, RoundSpec,
    RoundSummary, RunEnvelope, RunId, RunOutcome, RunType, WorkerId, WorkerInfo,
};
pub use worker::{ArtifactSender, Worker};