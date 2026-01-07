/// Worker agent library - exposes modules for testing and reuse
///
/// This allows integration tests to access internal modules like
/// telemetry collectors without duplicating code.

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

pub mod capabilities;
pub mod execution;
pub mod stream_handler;
pub mod telemetry;
