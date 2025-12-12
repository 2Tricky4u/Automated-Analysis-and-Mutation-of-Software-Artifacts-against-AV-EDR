// Scheduler library exports
// Provides job queue, worker pool, and scheduler core functionality

pub mod job;
pub mod queue;
pub mod run_queue;  // NEW: Async run queue for worker pull model
pub mod worker_pool;
pub mod scheduler_core;
pub mod round;
pub mod run_result;
pub mod round_processor;

// Protobuf definitions (shared with main.rs)
pub mod edr {
    pub mod common {
        tonic::include_proto!("edr.common");
    }
    pub mod controller {
        tonic::include_proto!("edr.controller");
    }
    pub mod worker {
        tonic::include_proto!("edr.worker");
    }
}

// Re-export commonly used types
pub use job::{Job, JobStatus, MutationSpec};
pub use queue::JobQueue;
pub use run_queue::{RunQueue, PendingRun, RunResult as QueuedRunResult};  // NEW
pub use worker_pool::{WorkerPool, WorkerState, WorkerStatus};
pub use scheduler_core::{SchedulerCore, SchedulerConfig, create_scheduler_core};
pub use round::{Round, RoundSummary, RoundStatus, RunType, BehaviorComparison, Feedback};
pub use run_result::{RunResult, RunOutcome};
pub use round_processor::RoundProcessor;
