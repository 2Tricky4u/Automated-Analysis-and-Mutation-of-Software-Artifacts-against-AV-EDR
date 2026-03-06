//! Controller library for the AutoMutate++ EDR evaluation framework.
//!
//! Provides the dispatch engine (job orchestration, run pooling, VM execution),
//! gRPC API handlers, Elasticsearch storage, triage token analysis, and
//! VM connection management.

pub mod api;
pub mod dispatch;
pub mod storage;
pub mod triage;
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
pub use dispatch::{JobSession, Orchestrator, RunPool, VMExecutor, VMInfo, WorkerId, WorkerInfo};
pub use vm::{Target, TargetEvent, TargetManager, TargetStatus};
