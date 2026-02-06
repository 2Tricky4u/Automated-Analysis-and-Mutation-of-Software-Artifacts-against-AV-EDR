// Scheduler library - JobWorker architecture
//
// Core modules:
// - dispatch: JobWorker-based job execution (JobWorker, RunPool, VMExecutor, Orchestrator)
// - target_manager: Connection management and VMExecutor spawning
// - service: gRPC handler implementations

pub mod dispatch;
pub mod service;
pub mod target_manager;

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
pub use dispatch::{
    JobSession, Orchestrator, RunPool, VMExecutor, VMInfo, WorkerId, WorkerInfo,
};
pub use service::SchedulerService;
pub use target_manager::{Target, TargetConfig, TargetEvent, TargetManager, TargetStatus};
