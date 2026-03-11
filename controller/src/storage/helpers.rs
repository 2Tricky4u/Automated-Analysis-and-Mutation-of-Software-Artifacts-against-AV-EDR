//! Shared ES operation helpers — composable functions for repeated patterns.
//!
//! Centralizes index-name formatting, timestamp conversion, doc updates,
//! and response checking that were previously copy-pasted across storage modules.

use elasticsearch::{Elasticsearch, UpdateParts};
use serde_json::Value;
use std::time::SystemTime;
use tracing::{debug, info, warn};

use super::queries;

/// Build a monthly ES index name: `"jobs"` → `"jobs-2026.02"`.
pub fn es_index_name(prefix: &str) -> String {
    format!("{}-{}", prefix, chrono::Utc::now().format("%Y.%m"))
}

/// Build a daily ES index name: `"telemetry"` → `"telemetry-2026.02.18"`.
pub fn es_index_name_daily(prefix: &str) -> String {
    format!("{}-{}", prefix, chrono::Utc::now().format("%Y.%m.%d"))
}

/// Convert a `SystemTime` to an RFC 3339 string.
pub fn system_time_to_rfc3339(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Utc> = t.into();
    dt.to_rfc3339()
}

/// Return `Utc::now()` as an RFC 3339 string.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Find a document by ID field, then update it. Returns `Ok(true)` on success,
/// `Ok(false)` if the document was not found (logged as a warning).
///
/// `body` must already be wrapped as `{"doc": { ... }}`.
///
/// Uses the actual ES `_id` (which may differ from the field value) for the update.
/// Retries up to 3 times on version conflict (HTTP 409) to handle concurrent updates.
///
/// Returns `Err` if the update request fails with a non-409 status or all retries
/// are exhausted.
pub async fn update_doc_by_id(
    es: &Elasticsearch,
    index_pattern: &str,
    id_field: &str,
    doc_id: &str,
    body: Value,
    entity_label: &str,
) -> anyhow::Result<bool> {
    const MAX_RETRIES: u32 = 3;

    for attempt in 0..=MAX_RETRIES {
        let found = queries::find_index(es, index_pattern, id_field, doc_id).await;

        if let Some((ref idx, ref es_doc_id)) = found {
            let response = es
                .update(UpdateParts::IndexId(idx, es_doc_id))
                .body(body.clone())
                .refresh(elasticsearch::params::Refresh::WaitFor)
                .send()
                .await?;

            let status = response.status_code();
            if status.is_success() {
                info!("Updated {} {} successfully", entity_label, doc_id);
                return Ok(true);
            }

            let err_body = response.text().await.unwrap_or_default();

            // Retry on version conflict (409)
            if status.as_u16() == 409 && attempt < MAX_RETRIES {
                debug!(
                    "Version conflict updating {} {}, retrying ({}/{})",
                    entity_label,
                    doc_id,
                    attempt + 1,
                    MAX_RETRIES
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    50 * (attempt as u64 + 1),
                ))
                .await;
                continue;
            }

            return Err(anyhow::anyhow!(
                "Failed to update {} {}: {}",
                entity_label,
                doc_id,
                err_body
            ));
        } else {
            warn!("{} {} not found in ES for update", entity_label, doc_id);
            return Ok(false);
        }
    }

    Err(anyhow::anyhow!(
        "Failed to update {} {} after {} retries (version conflict)",
        entity_label,
        doc_id,
        MAX_RETRIES
    ))
}

/// Find a document by ES `_id`, then update it. Returns `Ok(true)` on success,
/// `Ok(false)` if the document was not found (logged as a warning).
///
/// `body` must already be wrapped as `{"doc": { ... }}`.
///
/// Unlike `update_doc_by_id` which searches by a field value, this uses the
/// document's `_id` directly via an `ids` query, avoiding collision risk when
/// the field value alone is not unique across indices.
///
/// Retries up to 3 times on version conflict (HTTP 409).
///
/// Returns `Err` if the update request fails with a non-409 status or all retries
/// are exhausted.
pub async fn update_doc_by_es_id(
    es: &Elasticsearch,
    index_pattern: &str,
    es_id: &str,
    body: Value,
    entity_label: &str,
) -> anyhow::Result<bool> {
    const MAX_RETRIES: u32 = 3;

    for attempt in 0..=MAX_RETRIES {
        let found = queries::find_index_by_es_id(es, index_pattern, es_id).await;

        if let Some((ref idx, ref doc_id)) = found {
            let response = es
                .update(UpdateParts::IndexId(idx, doc_id))
                .body(body.clone())
                .refresh(elasticsearch::params::Refresh::WaitFor)
                .send()
                .await?;

            let status = response.status_code();
            if status.is_success() {
                info!("Updated {} {} successfully", entity_label, es_id);
                return Ok(true);
            }

            let err_body = response.text().await.unwrap_or_default();

            // Retry on version conflict (409)
            if status.as_u16() == 409 && attempt < MAX_RETRIES {
                debug!(
                    "Version conflict updating {} {}, retrying ({}/{})",
                    entity_label,
                    es_id,
                    attempt + 1,
                    MAX_RETRIES
                );
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    50 * (attempt as u64 + 1),
                ))
                .await;
                continue;
            }

            return Err(anyhow::anyhow!(
                "Failed to update {} {}: {}",
                entity_label,
                es_id,
                err_body
            ));
        } else {
            warn!("{} {} not found in ES for update", entity_label, es_id);
            return Ok(false);
        }
    }

    Err(anyhow::anyhow!(
        "Failed to update {} {} after {} retries (version conflict)",
        entity_label,
        es_id,
        MAX_RETRIES
    ))
}

/// Check an ES index response status. Returns `Err` with context on failure.
///
/// Consumes the response because extracting error body text requires `.text().await`.
pub async fn check_index_response(
    response: elasticsearch::http::response::Response,
    entity_label: &str,
    entity_id: &str,
) -> anyhow::Result<()> {
    if !response.status_code().is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Failed to index {} {}: {}",
            entity_label,
            entity_id,
            body
        ));
    }
    Ok(())
}

/// Return current Unix timestamp in seconds as `i64`.
pub fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Insert an optional `&str` field into a JSON object (no-op if `None`).
pub fn insert_optional_field(doc: &mut Value, key: &str, value: Option<&str>) {
    if let Some(v) = value
        && let Some(obj) = doc.as_object_mut()
    {
        obj.insert(key.to_string(), serde_json::json!(v));
    }
}
