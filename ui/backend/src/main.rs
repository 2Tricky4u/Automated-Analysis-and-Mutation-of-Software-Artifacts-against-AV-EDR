//! UI Backend REST API Server
//!
//! Thin REST-to-gRPC adapter that exposes the Controller service to the
//! browser-based frontend. Every REST handler deserializes JSON, forwards the
//! call to the Controller over gRPC, and re-serializes the response as a JSON
//! envelope (`{"data":...}` on success, `{"error":...,"code":...}` on failure).
//!
//! ## Shared State
//!
//! - [`Arc<ControllerGrpcClient>`](grpc_client::ControllerGrpcClient) — lazily-connected,
//!   thread-safe gRPC channel shared by all handlers via Axum `State`.
//! - `Option<Arc<Elasticsearch>>` — optional ES client injected via Axum `Extension`.
//!   Only the token endpoint uses it; all other endpoints degrade gracefully when absent.
//!
//! ## Environment Variables
//!
//! | Variable            | Default                       | Description                          |
//! |---------------------|-------------------------------|--------------------------------------|
//! | `CONTROLLER_ADDR`   | `http://10.200.200.1:50051`   | Controller gRPC endpoint             |
//! | `LISTEN_ADDR`       | `0.0.0.0:3000`                | Address this server listens on       |
//! | `FRONTEND_DIR`      | `../frontend`                 | Path to static HTML/JS/CSS assets    |
//! | `ELASTICSEARCH_URL` | `http://localhost:9200`        | ElasticSearch node URL (optional)    |
//!
//! ## Endpoints
//!
//! ### Health
//! - `GET  /health`       — plain-text liveness probe
//! - `GET  /api/health`   — JSON health with controller connectivity
//!
//! ### Jobs
//! - `POST /api/jobs`                              — submit new job
//! - `GET  /api/jobs/:id`                          — job status
//! - `GET  /api/jobs/:id/progress`                 — detailed progress with rounds
//! - `POST /api/jobs/:id/stop`                     — stop running job
//! - `GET  /api/jobs/:job_id/rounds/:round_id`     — round details
//!
//! ### Runs
//! - `GET  /api/runs/:run_id/trace?last=N`                  — trace lines for a run
//! - `GET  /api/runs/compare?baseline=X&instrumented=Y`     — compare two runs
//!
//! ### Workers & Admin
//! - `GET  /api/workers`                — list all workers
//! - `GET  /api/workers/metadata`       — enhanced metadata (health, tools, last_seen)
//! - `GET  /api/workers/available`      — filter by OS / capabilities
//! - `POST /api/workers/:id/ping`       — ping a specific worker
//! - `POST /api/workers/:id/disconnect` — disconnect a specific worker
//! - `POST /api/workers/disconnect-all` — disconnect all workers
//!
//! ### Orchestrator
//! - `GET  /api/orchestrator/status` — active jobs, queue depth, pool metrics
//!
//! ### Tokens
//! - `GET  /api/jobs/:job_id/rounds/:round_id/tokens` — per-round tokens (ES direct)
//! - `GET  /api/tokens/compare?run_a=X&run_b=Y`       — token set diff via gRPC
//!
//! ### Query & Triage
//! - `POST /api/query`   — flexible ES query (job IDs + date range)
//! - `POST /api/triage`  — submit triage verdict
//!
//! ## Static File Fallback
//!
//! Non-API routes fall through to [`ServeDir`],
//! which serves the frontend SPA from `FRONTEND_DIR`. This allows the UI to be
//! served from the same origin without a separate web server.

mod api;
mod generated;
mod grpc_client;

use api::{ApiResponse, HealthResponse};
use axum::{
    Extension, Json, Router,
    extract::State,
    http::{Method, header},
    routing::{get, post},
};
use elasticsearch::Elasticsearch;
use elasticsearch::http::transport::Transport;
use grpc_client::{ControllerConfig, ControllerGrpcClient};
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;
use tracing::{Level, info, warn};

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
    let shellcode_dir = std::env::var("SHELLCODE_DIR").unwrap_or_else(|_| {
        // Discover repo root via git, fall back to relative path
        std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|root| format!("{}/data/shellcodes", root.trim()))
            .unwrap_or_else(|| "../../data/shellcodes".to_string())
    });

    info!("UI Backend starting...");
    info!("  Controller: {}", controller_addr);
    info!("  Listen: {}", listen_addr);
    info!("  Frontend: {}", frontend_dir);
    info!("  Shellcode dir: {}", shellcode_dir);

    // Create gRPC client
    let grpc_client = Arc::new(ControllerGrpcClient::new(ControllerConfig {
        address: controller_addr,
        timeout_secs: 30,
    }));

    // Create Elasticsearch client (optional — token endpoint degrades gracefully)
    let es_url =
        std::env::var("ELASTICSEARCH_URL").unwrap_or_else(|_| "http://localhost:9200".to_string());
    let es_client: Option<Arc<Elasticsearch>> = match Transport::single_node(&es_url) {
        Ok(transport) => {
            info!("  Elasticsearch: {}", es_url);
            Some(Arc::new(Elasticsearch::new(transport)))
        }
        Err(e) => {
            warn!("Elasticsearch not available ({}): {}", es_url, e);
            None
        }
    };

    let shellcode_dir_arc: Arc<PathBuf> = Arc::new(PathBuf::from(&shellcode_dir));

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
        .route(
            "/api/jobs/:job_id/rounds/:round_id/source",
            get(api::jobs::get_round_source),
        )
        .route(
            "/api/jobs/:job_id/rounds/:round_id/coverage",
            get(api::jobs::get_round_coverage),
        )
        // Run endpoints
        .route("/api/runs/:run_id/trace", get(api::jobs::get_trace_lines))
        .route("/api/runs/compare", get(api::jobs::compare_runs))
        // Worker endpoints (literal paths before parameterized)
        .route(
            "/api/workers/metadata",
            get(api::workers::get_worker_metadata),
        )
        .route(
            "/api/workers/available",
            get(api::workers::get_available_workers),
        )
        .route(
            "/api/workers/disconnect-all",
            post(api::workers::disconnect_all_workers),
        )
        .route("/api/workers/:id/ping", post(api::workers::ping_worker))
        .route(
            "/api/workers/:id/disconnect",
            post(api::workers::disconnect_worker),
        )
        .route("/api/workers", get(api::workers::list_workers))
        // Orchestrator endpoints
        .route(
            "/api/orchestrator/status",
            get(api::workers::get_orchestrator_status),
        )
        // Query endpoints
        .route("/api/query", post(api::query::query_results))
        .route("/api/triage", post(api::query::submit_triage))
        // Token endpoints
        .route(
            "/api/jobs/:job_id/rounds/:round_id/tokens",
            get(api::tokens::get_round_tokens),
        )
        .route("/api/tokens/compare", get(api::tokens::compare_tokens))
        // Shellcode listing
        .route("/api/shellcodes", get(api::shellcodes::list_shellcodes))
        // Middleware
        .layer(Extension(shellcode_dir_arc.clone()))
        .layer(Extension(es_client))
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

/// Returns `"OK"` as a plain-text liveness probe.
///
/// Used by load balancers and orchestrators to verify the process is running.
/// Does not check downstream dependencies.
async fn health_check() -> &'static str {
    "OK"
}

/// Detailed health check that probes the Controller gRPC connection.
///
/// Returns a JSON envelope containing:
/// - `status` — `"healthy"` when the Controller responds to a ping,
///   `"degraded"` otherwise.
/// - `controller_connected` — boolean connectivity flag.
/// - `version` — crate version from `Cargo.toml`.
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
