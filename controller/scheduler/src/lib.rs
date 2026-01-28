// Scheduler library exports
// Provides job queue, worker pool, and scheduler core functionality

pub mod job;
pub mod queue;
pub mod round;
pub mod round_processor;
pub mod run_queue;
pub mod run_result;
pub mod scheduler_core;
pub mod target_manager;
pub mod worker_manager;
pub mod worker_pool;

// Protobuf definitions (shared with main.rs)
pub mod automutate {
    pub mod common {
        tonic::include_proto!("automutate.common");
    }
    pub mod controller {
        tonic::include_proto!("automutate.controller");
    }
    pub mod worker {
        tonic::include_proto!("automutate.worker");
    }
}

// Re-export commonly used types
pub use job::{Job, JobStatus, MutationSpec};
pub use queue::JobQueue;
pub use round::{BehaviorComparison, Feedback, Round, RoundStatus, RoundSummary, RunType};
pub use round_processor::RoundProcessor;
pub use run_queue::{PendingRun, RunQueue, RunResult as QueuedRunResult};
pub use run_result::{RunOutcome, RunResult};
pub use scheduler_core::{SchedulerConfig, SchedulerCore, create_scheduler_core};
pub use target_manager::{Target, TargetConfig, TargetEvent, TargetManager, TargetStatus};
pub use worker_manager::{WorkerConfig, WorkerManager};
pub use worker_pool::{WorkerPool, WorkerState, WorkerStatus};
