//! Reusable ES query helpers for API handlers.
//!
//! Returns raw `serde_json::Value` to avoid coupling with proto types.

use elasticsearch::{Elasticsearch, SearchParts};
use serde_json::{json, Value};

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
