//! Run types: envelope, artifact reference, VM/worker info.
//!
//! A *run* is a single artifact execution on a VM. Three runs form a round:
//! baseline (trace=off), instrumented (trace=lines), and dryrun (no payload).
//! [`RunEnvelope`] carries everything needed for dispatch and result routing.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::ids::{JobId, RoundId, RunId, WorkerId};

// ============================================================================
// Run Type
// ============================================================================

/// Classifies an execution run within the two-run differential protocol.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RunType {
    Baseline,
    Instrumented,
    DryRun,
}

impl RunType {
    pub fn as_str(&self) -> &'static str {
        match self {
            RunType::Baseline => "baseline",
            RunType::Instrumented => "instrumented",
            RunType::DryRun => "dryrun",
        }
    }

    /// Return the trace mode string passed to the worker agent.
    ///
    /// `Baseline` and `DryRun` use `"off"`; `Instrumented` uses `"lines"`.
    pub fn trace_mode(&self) -> &'static str {
        match self {
            RunType::Baseline => "off",
            RunType::Instrumented => "lines",
            RunType::DryRun => "off",
        }
    }

    /// Returns `true` if this is a dryrun (no real payload execution).
    pub fn is_dryrun(&self) -> bool {
        matches!(self, RunType::DryRun)
    }
}

impl std::fmt::Display for RunType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================================
// Artifact Reference
// ============================================================================

/// Reference to a built artifact on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub path: PathBuf,
    pub sha256: Option<String>,
}

// ============================================================================
// Run Envelope
// ============================================================================

/// Self-contained descriptor for a pending run in the [`RunPool`](crate::dispatch::RunPool).
///
/// Carries the artifact reference, routing IDs, and constraint fields needed
/// for capability-matched dispatch by [`VMExecutor`](crate::dispatch::vm_executor::VMExecutor)
/// and result routing back to the originating [`JobWorker`](crate::dispatch::job_worker::JobWorker).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEnvelope {
    /// Unique run identifier (e.g. `<round_id>-baseline`).
    pub run_id: RunId,
    /// Owning job.
    pub job_id: JobId,
    /// Owning round.
    pub round_id: RoundId,
    /// 1-based round number within the job.
    pub round_number: u32,
    /// Baseline, Instrumented, or DryRun.
    pub run_type: RunType,
    /// On-disk artifact with optional SHA-256.
    pub artifact: ArtifactRef,
    /// Mutation IDs applied to this artifact.
    pub mutations: Vec<String>,
    /// Per-run execution timeout.
    pub timeout_seconds: u32,
    /// Required OS (e.g. `"win10"`).
    pub required_os: String,
    /// Required VM capabilities (e.g. `["rededr", "mde"]`).
    pub required_capabilities: Vec<String>,
}

// ============================================================================
// Worker / VM Info
// ============================================================================

/// Basic identity and capabilities of a connected worker.
#[derive(Debug, Clone)]
pub struct WorkerInfo {
    pub id: WorkerId,
    pub os: String,
    pub capabilities: Vec<String>,
}

/// Information about a connected VM for VMExecutor.
/// Simplified version of WorkerInfo focused on execution capabilities.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct VMInfo {
    pub id: String,
    pub os: String,
    pub capabilities: Vec<String>,
}

impl From<&WorkerInfo> for VMInfo {
    fn from(info: &WorkerInfo) -> Self {
        Self {
            id: info.id.0.clone(),
            os: info.os.clone(),
            capabilities: info.capabilities.clone(),
        }
    }
}

// ============================================================================
// Capability matching
// ============================================================================

/// Check if all required capabilities are satisfied by available capabilities.
/// Comparison is case-insensitive.
pub fn capabilities_match(required: &[String], available: &[String]) -> bool {
    required
        .iter()
        .all(|req| available.iter().any(|cap| cap.eq_ignore_ascii_case(req)))
}

// ============================================================================
// Artifact Chunking
// ============================================================================

/// Split binary data into 4MB `ArtifactChunk` messages for gRPC streaming.
pub fn chunk_artifact(
    artifact_id: &str,
    data: &[u8],
) -> Vec<crate::automutate::common::ArtifactChunk> {
    use crate::automutate::common::ArtifactChunk;

    const CHUNK_SIZE: usize = 4 * 1024 * 1024;
    let total_chunks = data.len().div_ceil(CHUNK_SIZE);
    data.chunks(CHUNK_SIZE)
        .enumerate()
        .map(|(i, chunk)| ArtifactChunk {
            artifact_id: artifact_id.to_string(),
            data: chunk.to_vec(),
            chunk_index: i as u32,
            total_chunks: total_chunks as u32,
            sha256: artifact_id.to_string(),
        })
        .collect()
}
