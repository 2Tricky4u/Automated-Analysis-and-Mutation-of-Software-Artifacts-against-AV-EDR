// Scheduler library - Dispatch-based architecture
//
// Core modules:
// - dispatch: Worker-VM-bound job execution (Worker, Orchestrator, JobSession)
// - target_manager: Connection management and Worker spawning
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
    JobId, JobOutcome, JobSession, Orchestrator, OrchestratorEvent, RoundSpec, RunEnvelope,
    RunId, RunOutcome, RunType, Worker, WorkerCommand, WorkerEvent, WorkerId, WorkerInfo,
};
pub use service::SchedulerService;
pub use target_manager::{Target, TargetConfig, TargetEvent, TargetManager, TargetStatus};
