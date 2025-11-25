/// Worker agent library - exposes modules for testing and reuse
///
/// This allows integration tests to access internal modules like
/// telemetry collectors without duplicating code.

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

pub mod telemetry;
pub mod execution;
