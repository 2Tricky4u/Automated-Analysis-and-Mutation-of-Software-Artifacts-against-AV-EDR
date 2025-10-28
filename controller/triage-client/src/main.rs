use tonic::Request;
use tracing::info;

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

use edr::controller::{controller_client::ControllerClient, TriageRequest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let mut client = ControllerClient::connect("http://localhost:50051").await?;

    info!("Triage client connected to controller");

    // Example triage submission
    let request = Request::new(TriageRequest {
        job_id: "job-000001".to_string(),
        detected: false,
        av_product: "Windows Defender".to_string(),
        detection_type: "none".to_string(),
        iocs: std::collections::HashMap::new(),
    });

    let response = client.submit_triage(request).await?;
    info!("Triage response: {:?}", response.into_inner());

    Ok(())
}
