//! REST API handlers
//!
//! Wraps gRPC calls to Controller service.

pub mod jobs;
pub mod workers;
pub mod query;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Standard API error response
#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
    pub code: String,
}

impl ApiError {
    pub fn new(error: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            code: code.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(msg, "INTERNAL_ERROR")
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(msg, "NOT_FOUND")
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::new(msg, "BAD_REQUEST")
    }

    pub fn unavailable(msg: impl Into<String>) -> Self {
        Self::new(msg, "SERVICE_UNAVAILABLE")
    }
}

impl IntoResponse for ApiError {
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

/// Standard API success response wrapper
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub data: T,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn new(data: T) -> Self {
        Self { data }
    }
}

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub controller_connected: bool,
    pub version: String,
}
