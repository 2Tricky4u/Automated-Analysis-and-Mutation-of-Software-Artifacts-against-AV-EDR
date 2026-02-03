//! UI Backend REST API Server
//!
//! Provides HTTP REST API and WebSocket endpoints that wrap
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
//! ### Workers
//! - GET /api/workers - List all workers
//!
//! ### Query
//! - POST /api/query - Execute ES query
//! - POST /api/triage - Submit triage data
//! - GET /api/runs/compare?baseline=X&instrumented=Y - Compare runs
//!
//! ### WebSocket
//! - GET /ws/telemetry - Real-time telemetry stream
//! - GET /ws/telemetry/filtered?job_id=X - Filtered telemetry stream

mod api;
mod generated;
mod grpc_client;
mod websocket;

use api::{ApiResponse, HealthResponse};
use axum::{
    extract::{FromRef, State},
    http::{header, Method},
    routing::{get, post},
    Json, Router,
};
use grpc_client::{ControllerConfig, ControllerGrpcClient};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::{info, Level};
use websocket::TelemetryBroadcaster;

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub grpc_client: Arc<ControllerGrpcClient>,
    pub telemetry_broadcaster: Arc<TelemetryBroadcaster>,
}

// Allow extracting Arc<ControllerGrpcClient> from AppState
impl FromRef<AppState> for Arc<ControllerGrpcClient> {
    fn from_ref(state: &AppState) -> Self {
        state.grpc_client.clone()
    }
}

// Allow extracting Arc<TelemetryBroadcaster> from AppState
impl FromRef<AppState> for Arc<TelemetryBroadcaster> {
    fn from_ref(state: &AppState) -> Self {
        state.telemetry_broadcaster.clone()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(Level::DEBUG)
        .with_target(false)
        .init();

    // Configuration from environment
    let controller_addr =
        std::env::var("CONTROLLER_ADDR").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());
    let listen_addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    info!("UI Backend starting...");
    info!("  Controller: {}", controller_addr);
    info!("  Listen: {}", listen_addr);

    // Create gRPC client
    let grpc_client = Arc::new(ControllerGrpcClient::new(ControllerConfig {
        address: controller_addr,
        timeout_secs: 30,
    }));

    // Create telemetry broadcaster
    let telemetry_broadcaster = Arc::new(TelemetryBroadcaster::new(1000));

    // Create app state
    let app_state = AppState {
        grpc_client,
        telemetry_broadcaster,
    };

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
        // Worker endpoints
        .route("/api/workers", get(api::workers::list_workers))
        // Query endpoints
        .route("/api/query", post(api::query::query_results))
        .route("/api/triage", post(api::query::submit_triage))
        .route("/api/runs/compare", get(api::jobs::compare_runs))
        // WebSocket endpoints
        .route("/ws/telemetry", get(websocket::ws_handler))
        .route("/ws/telemetry/filtered", get(websocket::ws_handler_filtered))
        // Middleware
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        // State
        .with_state(app_state);

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
