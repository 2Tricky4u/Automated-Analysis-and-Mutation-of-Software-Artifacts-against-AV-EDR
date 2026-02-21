//! UI Backend REST API Server
//!
//! Provides HTTP REST API endpoints that wrap
//! the Controller gRPC service for frontend integration.
//!
//! ## Endpoints
//!
//! ### Health
//! - GET /health - Health check
//! - GET /api/health - Detailed health with controller status
//!
//! ### Jobs
//! - POST /api/jobs - Submit new job
//! - GET /api/jobs/:id - Get job status
//! - GET /api/jobs/:id/progress - Get detailed progress with rounds
//! - POST /api/jobs/:id/stop - Stop running job
//! - GET /api/jobs/:job_id/rounds/:round_id - Get round details
//!
//! ### Runs
//! - GET /api/runs/:run_id/trace?last=N - Get trace lines for a run
//! - GET /api/runs/compare?baseline=X&instrumented=Y - Compare runs
//!
//! ### Workers
//! - GET /api/workers - List all workers
//!
//! ### Orchestrator
//! - GET /api/orchestrator/status - Get orchestrator status (active jobs, pending runs)
//!
//! ### Query
//! - POST /api/query - Execute ES query
//! - POST /api/triage - Submit triage data

mod api;
mod generated;
mod grpc_client;

use api::{ApiResponse, HealthResponse};
use axum::{
    Json, Router,
    extract::State,
    http::{Method, header},
    routing::{get, post},
};
use grpc_client::{ControllerConfig, ControllerGrpcClient};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{Level, info};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(false)
        .init();

    // Configuration from environment
    let controller_addr = std::env::var("CONTROLLER_ADDR")
        .unwrap_or_else(|_| "http://10.200.200.1:50051".to_string());
    let listen_addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
    let frontend_dir = std::env::var("FRONTEND_DIR").unwrap_or_else(|_| "../frontend".to_string());

    info!("UI Backend starting...");
    info!("  Controller: {}", controller_addr);
    info!("  Listen: {}", listen_addr);
    info!("  Frontend: {}", frontend_dir);

    // Create gRPC client
    let grpc_client = Arc::new(ControllerGrpcClient::new(ControllerConfig {
        address: controller_addr,
        timeout_secs: 30,
    }));

    // CORS configuration - allow all origins for development
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    // Build router
    let app = Router::new()
        // Health endpoints
        .route("/health", get(health_check))
        .route("/api/health", get(health_detailed))
        // Job endpoints
        .route("/api/jobs", post(api::jobs::submit_job))
        .route("/api/jobs/:id", get(api::jobs::get_job_status))
        .route("/api/jobs/:id/progress", get(api::jobs::get_job_progress))
        .route("/api/jobs/:id/stop", post(api::jobs::stop_job))
        .route(
            "/api/jobs/:job_id/rounds/:round_id",
            get(api::jobs::get_round),
        )
        // Run endpoints
        .route("/api/runs/:run_id/trace", get(api::jobs::get_trace_lines))
        .route("/api/runs/compare", get(api::jobs::compare_runs))
        // Worker endpoints (literal paths before parameterized)
        .route("/api/workers/metadata", get(api::workers::get_worker_metadata))
        .route("/api/workers/available", get(api::workers::get_available_workers))
        .route("/api/workers/disconnect-all", post(api::workers::disconnect_all_workers))
        .route("/api/workers/:id/ping", post(api::workers::ping_worker))
        .route("/api/workers/:id/disconnect", post(api::workers::disconnect_worker))
        .route("/api/workers", get(api::workers::list_workers))
        // Orchestrator endpoints
        .route(
            "/api/orchestrator/status",
            get(api::workers::get_orchestrator_status),
        )
        // Query endpoints
        .route("/api/query", post(api::query::query_results))
        .route("/api/triage", post(api::query::submit_triage))
        // Middleware
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        // State
        .with_state(grpc_client)
        // Serve static frontend files (fallback for non-API routes)
        .fallback_service(ServeDir::new(&frontend_dir));

    info!("UI Backend API ready on {}", listen_addr);

    // Start server
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Simple health check
async fn health_check() -> &'static str {
    "OK"
}

/// Detailed health check with controller status
async fn health_detailed(
    State(client): State<Arc<ControllerGrpcClient>>,
) -> Json<ApiResponse<HealthResponse>> {
    let controller_connected = client.is_healthy().await;

    Json(ApiResponse::new(HealthResponse {
        status: if controller_connected {
            "healthy".to_string()
        } else {
            "degraded".to_string()
        },
        controller_connected,
        version: env!("CARGO_PKG_VERSION").to_string(),
    }))
}
