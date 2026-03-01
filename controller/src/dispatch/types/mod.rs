//! Data models for dispatch system.
//!
//! JobSession: ephemeral runtime state (no persistence needed)
//! RoundSpec: immutable mutation recipe
//! RoundAgg: ephemeral join state until both runs complete
//! RunEnvelope: dispatch envelope for the per-worker pool

pub mod config;
pub mod ids;
pub mod round;
pub mod run;
pub mod session;

// Re-export all public types for backward compatibility
pub use config::{ModularBuildSpec, ModuleSelectionSpec};
pub use ids::{JobId, RoundId, RunId, TargetId, WorkerId};
pub use round::{
    DifferentialCategory, MutationSpec, RoundAgg, RoundSpec, RoundSummary, RunOutcome,
};
pub use run::{
    ArtifactRef, RunEnvelope, RunType, VMInfo, WorkerInfo, capabilities_match, chunk_artifact,
};
pub use session::{JobInfo, JobOutcome, JobSession, JobStatus};
