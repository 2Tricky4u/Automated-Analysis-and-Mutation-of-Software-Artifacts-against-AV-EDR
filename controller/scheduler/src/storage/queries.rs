//! Reusable ES query helpers for API handlers.
//!
//! All read-side ES operations live here. Returns raw `serde_json::Value`
//! to keep this layer proto-agnostic — the API handlers do the mapping.

use elasticsearch::{Elasticsearch, SearchParts};
use serde_json::{Value, json};

/// Look up a single job document by job_id.
pub async fn query_job(es: &Elasticsearch, job_id: &str) -> Option<Value> {
    let response = es
        .search(SearchParts::Index(&["jobs-*"]))
        .body(json!({
            "query": { "term": { "job_id": job_id } },
            "size": 1
        }))
        .send()
        .await
        .ok()?;

    let body = response.json::<Value>().await.ok()?;
    body["hits"]["hits"]
        .as_array()
        .and_then(|h| h.first())
        .map(|hit| hit["_source"].clone())
}

/// Query all rounds for a job, sorted by round_number ascending.
pub async fn query_rounds(es: &Elasticsearch, job_id: &str) -> Vec<Value> {
    let response = es
        .search(SearchParts::Index(&["rounds-*"]))
        .body(json!({
            "query": { "term": { "job_id": job_id } },
            "sort": [{ "round_number": "asc" }],
            "size": 100
        }))
        .send()
        .await;

    extract_sources(response).await
}

/// Query a single round by job_id + round_id.
pub async fn query_round(es: &Elasticsearch, job_id: &str, round_id: &str) -> Option<Value> {
    let response = es
        .search(SearchParts::Index(&["rounds-*"]))
        .body(json!({
            "query": {
                "bool": {
                    "must": [
                        { "term": { "job_id": job_id } },
                        { "term": { "round_id": round_id } }
                    ]
                }
            },
            "size": 1
        }))
        .send()
        .await
        .ok()?;

    let body = response.json::<Value>().await.ok()?;
    body["hits"]["hits"]
        .as_array()
        .and_then(|h| h.first())
        .map(|hit| hit["_source"].clone())
}

/// Query runs by a list of run IDs.
pub async fn query_runs_by_ids(es: &Elasticsearch, run_ids: &[&str]) -> Vec<Value> {
    if run_ids.is_empty() {
        return Vec::new();
    }

    let response = es
        .search(SearchParts::Index(&["runs-*"]))
        .body(json!({
            "query": {
                "terms": { "run_id": run_ids }
            },
            "size": run_ids.len()
        }))
        .send()
        .await;

    extract_sources(response).await
}

/// Best-effort update of a single field on a job document.
pub async fn update_job_field(
    es: &Elasticsearch,
    job_id: &str,
    field: &str,
    value: &str,
) -> anyhow::Result<()> {
    let body = json!({ "doc": { field: value } });
    super::helpers::update_doc_by_id(es, "jobs-*", "job_id", job_id, body, "job").await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Extract `_source` from all hits in a search response.
async fn extract_sources(
    response: Result<elasticsearch::http::response::Response, elasticsearch::Error>,
) -> Vec<Value> {
    match response {
        Ok(resp) => {
            if let Ok(body) = resp.json::<Value>().await {
                body["hits"]["hits"]
                    .as_array()
                    .map(|hits| hits.iter().map(|h| h["_source"].clone()).collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        }
        Err(_) => Vec::new(),
    }
}

/// Find the concrete index name for a document (ES update requires it).
pub(crate) async fn find_index(
    es: &Elasticsearch,
    pattern: &str,
    id_field: &str,
    id_value: &str,
) -> Option<String> {
    let resp = es
        .search(SearchParts::Index(&[pattern]))
        .body(json!({
            "query": { "term": { id_field: id_value } },
            "size": 1,
            "_source": false
        }))
        .send()
        .await
        .ok()?;

    let body = resp.json::<Value>().await.ok()?;
    body["hits"]["hits"]
        .as_array()
        .and_then(|h| h.first())
        .and_then(|hit| hit["_index"].as_str())
        .map(String::from)
}
