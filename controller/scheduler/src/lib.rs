// Scheduler library - JobWorker architecture
//
// Core modules:
// - dispatch: JobWorker-based job execution (JobWorker, RunPool, VMExecutor, Orchestrator)
// - vm: Connection management and VMExecutor spawning
// - api: gRPC handler implementations

pub mod api;
pub mod dispatch;
pub mod vm;

// Protobuf definitions
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

// Re-exports
pub use api::SchedulerService;
pub use dispatch::{
    JobSession, Orchestrator, RunPool, VMExecutor, VMInfo, WorkerId, WorkerInfo,
};
pub use vm::{Target, TargetConfig, TargetEvent, TargetManager, TargetStatus};
