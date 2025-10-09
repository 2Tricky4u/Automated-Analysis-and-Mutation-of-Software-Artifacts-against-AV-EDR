/// UI Backend REST API
///
/// Implements CLAUDE.md Section 2: "Controller: UI"
/// Provides HTTP REST API as alternative to gRPC CLI
/// for job submission, status monitoring, and report viewing.
use axum::{
    extract::Path,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Serialize, Deserialize)]
struct Job {
    id: String,
    status: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct SubmitJobRequest {
    artifact_path: String,
    mutation_budget: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let app = Router::new()
        .route("/api/jobs", get(list_jobs))
        .route("/api/jobs", post(submit_job))
        .route("/api/jobs/:id", get(get_job_status))
        .route("/api/jobs/:id/results", get(get_job_results))
        .route("/health", get(health_check));

    let addr = "0.0.0.0:3000";
    info!("UI Backend API starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check() -> &'static str {
    "OK"
}

async fn list_jobs() -> Json<Vec<Job>> {
    info!("Listing all jobs");
    // TODO: Query controller for active jobs
    Json(vec![Job {
        id: "job-000001".to_string(),
        status: "queued".to_string(),
        created_at: "2025-01-10T00:00:00Z".to_string(),
    }])
}

async fn submit_job(Json(payload): Json<SubmitJobRequest>) -> Json<Job> {
    info!(
        "Submitting new job: artifact={}, budget={}",
        payload.artifact_path, payload.mutation_budget
    );
    // TODO: Call controller gRPC ScheduleJob
    Json(Job {
        id: "job-000002".to_string(),
        status: "queued".to_string(),
        created_at: "2025-01-10T00:00:00Z".to_string(),
    })
}

async fn get_job_status(Path(job_id): Path<String>) -> Json<Job> {
    info!("Getting status for job: {}", job_id);
    // TODO: Call controller gRPC GetJobStatus
    Json(Job {
        id: job_id,
        status: "running".to_string(),
        created_at: "2025-01-10T00:00:00Z".to_string(),
    })
}

async fn get_job_results(Path(job_id): Path<String>) -> Json<Vec<String>> {
    info!("Getting results for job: {}", job_id);
    // TODO: Query Elasticsearch for RunResults
    Json(vec![format!("run-{}-001", job_id)])
}
