//! Shellcode file listing endpoint.
//!
//! Returns sorted `.bin` filenames from the configured shellcode directory.

use super::{ApiError, ApiResponse};
use axum::{Extension, Json};
use std::path::PathBuf;
use std::sync::Arc;

/// `GET /api/shellcodes` — List available `.bin` files in the shellcode directory.
///
/// # Errors
///
/// - `INTERNAL_ERROR` if the shellcode directory cannot be read.
pub async fn list_shellcodes(
    Extension(shellcode_dir): Extension<Arc<PathBuf>>,
) -> Result<Json<ApiResponse<Vec<String>>>, ApiError> {
    let entries = std::fs::read_dir(shellcode_dir.as_ref())
        .map_err(|e| ApiError::internal(format!("Cannot read shellcode dir: {e}")))?;
    let mut files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "bin"))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    files.sort();
    Ok(Json(ApiResponse::new(files)))
}
