use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{transport::Server, Request, Response, Status};
use tracing::info;

pub mod edr {
    tonic::include_proto!("edr");
}

use edr::{
    controller_server::{Controller, ControllerServer},
    JobRequest, JobResponse, JobStatusRequest, JobStatusResponse, QueryRequest, QueryResponse,
    TriageRequest, TriageResponse,
};

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Job {
    id: String,
    name: String,
    status: String,
    progress: i32,
    phase: String,
    logs: Vec<String>,
}

#[derive(Debug, Default)]
struct SchedulerState {
    jobs: HashMap<String, Job>,
    job_counter: u64,
}

#[derive(Debug)]
pub struct SchedulerService {
    state: Arc<Mutex<SchedulerState>>,
}

impl SchedulerService {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SchedulerState::default())),
        }
    }
}

#[tonic::async_trait]
impl Controller for SchedulerService {
    async fn schedule_job(
        &self,
        request: Request<JobRequest>,
    ) -> Result<Response<JobResponse>, Status> {
        let req = request.into_inner();
        let mut state = self.state.lock().await;

        state.job_counter += 1;
        let job_id = format!("job-{:06}", state.job_counter);

        let job = Job {
            id: job_id.clone(),
            name: req.name.clone(),
            status: "queued".to_string(),
            progress: 0,
            phase: "initializing".to_string(),
            logs: vec![format!("Job {} created", job_id)],
        };

        state.jobs.insert(job_id.clone(), job);

        info!("Scheduled job: {} ({})", job_id, req.name);

        Ok(Response::new(JobResponse {
            job_id: job_id.clone(),
            accepted: true,
            message: format!("Job {} scheduled successfully", job_id),
            estimated_duration_seconds: 300,
        }))
    }

    async fn get_job_status(
        &self,
        request: Request<JobStatusRequest>,
    ) -> Result<Response<JobStatusResponse>, Status> {
        let req = request.into_inner();
        let state = self.state.lock().await;

        match state.jobs.get(&req.job_id) {
            Some(job) => Ok(Response::new(JobStatusResponse {
                job_id: job.id.clone(),
                status: job.status.clone(),
                progress_percent: job.progress,
                current_phase: job.phase.clone(),
                logs: job.logs.clone(),
            })),
            None => Err(Status::not_found(format!("Job {} not found", req.job_id))),
        }
    }

    async fn submit_triage(
        &self,
        request: Request<TriageRequest>,
    ) -> Result<Response<TriageResponse>, Status> {
        let req = request.into_inner();
        let triage_id = format!(
            "triage-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );

        info!(
            "Triage submitted for job {}: detected={}",
            req.job_id, req.detected
        );

        Ok(Response::new(TriageResponse {
            job_id: req.job_id,
            stored: true,
            triage_id,
        }))
    }

    async fn query_results(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        let _req = request.into_inner();

        Ok(Response::new(QueryResponse {
            results: vec![],
            total_count: 0,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr = "0.0.0.0:50051".parse()?;
    let scheduler = SchedulerService::new();

    info!("Controller/Scheduler starting on {}", addr);

    Server::builder()
        .add_service(ControllerServer::new(scheduler))
        .serve(addr)
        .await?;

    Ok(())
}
