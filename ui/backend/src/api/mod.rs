//! REST API handlers and shared response types.
//!
//! Every handler follows the same JSON envelope convention:
//!
//! - **Success:** `{"data": <T>}` with HTTP 200.
//! - **Error:** `{"error": "<message>", "code": "<CODE>"}` with a matching HTTP status.
//!
//! ## Error Code → HTTP Status Mapping
//!
//! | `code`                | HTTP Status                    |
//! |-----------------------|--------------------------------|
//! | `BAD_REQUEST`         | 400 Bad Request                |
//! | `NOT_FOUND`           | 404 Not Found                  |
//! | `SERVICE_UNAVAILABLE` | 503 Service Unavailable        |
//! | anything else         | 500 Internal Server Error      |
//!
//! ## Sub-modules
//!
//! - [`jobs`]    — job lifecycle (submit, status, progress, round, trace, compare)
//! - [`workers`] — worker pool management and orchestrator status
//! - [`tokens`]  — per-round triage tokens and token-set comparison
//! - [`query`]   — flexible ES query and triage verdict submission

pub mod jobs;
pub mod query;
pub mod tokens;
pub mod workers;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Standard API error envelope.
///
/// Serialized as `{"error": "...", "code": "..."}`. The `code` field is a
/// machine-readable tag that also determines the HTTP status code via
/// [`into_response`](ApiError::into_response).
///
/// ## Error Codes
///
/// - `BAD_REQUEST` — client sent invalid input (missing field, zero rounds, …)
/// - `NOT_FOUND` — the requested resource does not exist
/// - `SERVICE_UNAVAILABLE` — the Controller gRPC backend is unreachable
/// - `INTERNAL_ERROR` — catch-all for unexpected server-side failures
#[derive(Debug, Serialize)]
pub struct ApiError {
    /// Human-readable error description.
    pub error: String,
    /// Machine-readable error code (see struct-level docs).
    pub code: String,
}

impl ApiError {
    /// Create an error with an arbitrary message and code.
    pub fn new(error: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            code: code.into(),
        }
    }

    /// Shorthand for `INTERNAL_ERROR`.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(msg, "INTERNAL_ERROR")
    }

    /// Shorthand for `NOT_FOUND`.
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(msg, "NOT_FOUND")
    }

    /// Shorthand for `BAD_REQUEST`.
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(msg, "BAD_REQUEST")
    }

    /// Shorthand for `SERVICE_UNAVAILABLE`.
    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::new(msg, "SERVICE_UNAVAILABLE")
    }
}

impl IntoResponse for ApiError {
    /// Map [`code`](ApiError::code) to an HTTP status and return the error as
    /// JSON.
    fn into_response(self) -> Response {
        let status = match self.code.as_str() {
            "NOT_FOUND" => StatusCode::NOT_FOUND,
            "BAD_REQUEST" => StatusCode::BAD_REQUEST,
            "SERVICE_UNAVAILABLE" => StatusCode::SERVICE_UNAVAILABLE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };

        (status, Json(self)).into_response()
    }
}

/// Generic success envelope: `{"data": <T>}`.
///
/// Wraps any `Serialize`-able payload so that every successful response has a
/// uniform shape the frontend can rely on.
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    /// The response payload.
    pub data: T,
}

impl<T: Serialize> ApiResponse<T> {
    /// Wrap `data` in the standard envelope.
    pub fn new(data: T) -> Self {
        Self { data }
    }
}

/// Payload for the `GET /api/health` response.
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    /// `"healthy"` when the Controller responds to a ping, `"degraded"`
    /// otherwise.
    pub status: String,
    /// `true` if the most recent Controller ping succeeded.
    pub controller_connected: bool,
    /// Crate version from `Cargo.toml`.
    pub version: String,
}
