use std::time::SystemTime;
use tonic::{transport::Server, Request, Response, Status};
use tracing::info;

pub mod edr {
    tonic::include_proto!("edr");
}

use edr::{
    worker_agent_server::{WorkerAgent, WorkerAgentServer},
    BuildRequest, BuildResponse, SampleRequest, SampleResponse,
    HealthRequest, HealthResponse, TelemetryData, TelemetryAck,
};

#[derive(Debug, Default)]
pub struct WorkerAgentService {
    worker_id: String,
}

impl WorkerAgentService {
    pub fn new(worker_id: String) -> Self {
        Self { worker_id }
    }
}

#[tonic::async_trait]
impl WorkerAgent for WorkerAgentService {
    async fn execute_build(
        &self,
        request: Request<BuildRequest>,
    ) -> Result<Response<BuildResponse>, Status> {
        let req = request.into_inner();
        info!("Building job: {} (language: {})", req.job_id, req.language);
        
        let start_time = SystemTime::now();
        
        // Simulate build process
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        
        let build_time = SystemTime::now()
            .duration_since(start_time)
            .unwrap()
            .as_millis() as i64;
        
        let artifact_path = format!("/artifacts/{}.exe", req.job_id);
        
        Ok(Response::new(BuildResponse {
            job_id: req.job_id,
            success: true,
            artifact_path,
            error_message: String::new(),
            build_time_ms: build_time,
        }))
    }

    async fn run_sample(
        &self,
        request: Request<SampleRequest>,
    ) -> Result<Response<SampleResponse>, Status> {
        let req = request.into_inner();
        info!("Running sample: {} (ETW: {})", req.job_id, req.enable_etw);
        
        // Simulate sample execution
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        
        Ok(Response::new(SampleResponse {
            job_id: req.job_id,
            success: true,
            exit_code: 0,
            output: "Sample executed successfully".to_string(),
            telemetry_ids: vec!["telemetry-001".to_string()],
        }))
    }

    async fn health_check(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        let _req = request.into_inner();
        
        Ok(Response::new(HealthResponse {
            worker_id: self.worker_id.clone(),
            healthy: true,
            cpu_percent: 25,
            memory_percent: 40,
            active_jobs: 0,
        }))
    }

    async fn stream_telemetry(
        &self,
        request: Request<tonic::Streaming<TelemetryData>>,
    ) -> Result<Response<TelemetryAck>, Status> {
        let mut stream = request.into_inner();
        let mut count = 0;
        
        while let Some(telemetry) = stream.message().await? {
            info!("Received telemetry for job: {}", telemetry.job_id);
            count += 1;
        }
        
        Ok(Response::new(TelemetryAck {
            received: true,
            events_count: count,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    
    let worker_id = std::env::var("WORKER_ID").unwrap_or_else(|_| "worker-01".to_string());
    let addr = "0.0.0.0:50052".parse()?;
    
    let agent = WorkerAgentService::new(worker_id.clone());
    
    info!("Worker Agent {} starting on {}", worker_id, addr);
    
    Server::builder()
        .add_service(WorkerAgentServer::new(agent))
        .serve(addr)
        .await?;
    
    Ok(())
}
