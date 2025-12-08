// Scheduler library exports
// Provides job queue, worker pool, and scheduler core functionality

pub mod job;
pub mod queue;
pub mod worker_pool;
pub mod scheduler_core;

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
pub use worker_pool::{WorkerPool, WorkerState, WorkerStatus};
pub use scheduler_core::{SchedulerCore, SchedulerConfig, create_scheduler_core};
