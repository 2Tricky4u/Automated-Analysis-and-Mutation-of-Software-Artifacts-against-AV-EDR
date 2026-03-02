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

/// Query trace-line telemetry for a run.
///
/// Trace data is stored as a single `event_type: "trace_log"` document per run,
/// with `payload_content` containing JSONL lines like:
///   `{"seq":1,"thread_id":0,"file":"loader.c","line":42,"func":"main","ts_us":100}`
///
/// This function fetches that blob, parses the JSONL, and returns the last `last_n`
/// lines as individual `Value` objects with `payload_*` field names (matching the
/// shape the API handler expects).
pub async fn query_trace_lines(es: &Elasticsearch, run_id: &str, last_n: u32) -> (Vec<Value>, u64) {
    let size = if last_n == 0 { 50 } else { last_n } as usize;

    // Fetch the trace_log blob for this run.
    // Use match_phrase instead of term: telemetry indices created before the
    // template was applied have text (not keyword) mappings for run_id and
    // event_type. term queries fail on text fields with hyphens/underscores.
    let response = es
        .search(SearchParts::Index(&["telemetry-*"]))
        .body(json!({
            "query": {
                "bool": {
                    "filter": [
                        { "match_phrase": { "run_id": run_id } },
                        { "match_phrase": { "event_type": "trace_log" } }
                    ]
                }
            },
            "size": 1,
            "_source": ["payload_content", "metadata"]
        }))
        .send()
        .await;

    let doc = match response {
        Ok(resp) => {
            if let Ok(body) = resp.json::<Value>().await {
                body["hits"]["hits"]
                    .as_array()
                    .and_then(|h| h.first())
                    .map(|hit| hit["_source"].clone())
            } else {
                None
            }
        }
        Err(_) => None,
    };

    let doc = match doc {
        Some(d) => d,
        None => return (Vec::new(), 0),
    };

    // Parse the JSONL content
    let content = doc["payload_content"].as_str().unwrap_or("");
    if content.is_empty() {
        return (Vec::new(), 0);
    }

    let mut all_lines: Vec<Value> = content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            serde_json::from_str::<Value>(line).ok()
        })
        .collect();

    let total = all_lines.len() as u64;

    // Sort by seq descending, take last N
    all_lines.sort_by(|a, b| {
        let sa = a["seq"].as_u64().unwrap_or(0);
        let sb = b["seq"].as_u64().unwrap_or(0);
        sb.cmp(&sa)
    });
    all_lines.truncate(size);

    // Remap field names to payload_* prefix (what the API handler expects)
    let docs: Vec<Value> = all_lines
        .into_iter()
        .map(|v| {
            json!({
                "payload_seq": v["seq"],
                "payload_file": v["file"],
                "payload_line": v["line"],
                "payload_func": v["func"],
                "payload_ts_us": v["ts_us"],
            })
        })
        .collect();

    (docs, total)
}

/// Fetch raw trace JSONL content for a run (lightweight — returns just the string).
///
/// Same ES query as `query_trace_lines` but skips JSONL parsing and field
/// remapping. The caller extracts what it needs from the raw content.
pub async fn query_trace_content(es: &Elasticsearch, run_id: &str) -> Option<String> {
    let response = es
        .search(SearchParts::Index(&["telemetry-*"]))
        .body(json!({
            "query": {
                "bool": {
                    "filter": [
                        { "match_phrase": { "run_id": run_id } },
                        { "match_phrase": { "event_type": "trace_log" } }
                    ]
                }
            },
            "size": 1,
            "_source": ["payload_content"]
        }))
        .send()
        .await
        .ok()?;

    let body = response.json::<Value>().await.ok()?;
    let content = body["hits"]["hits"]
        .as_array()
        .and_then(|h| h.first())
        .and_then(|hit| hit["_source"]["payload_content"].as_str())?;

    if content.is_empty() {
        None
    } else {
        Some(content.to_string())
    }
}

// ---------------------------------------------------------------------------
// Triage token queries
// ---------------------------------------------------------------------------

/// Query all non-trace telemetry events for a run, sorted by `payload_id` ascending.
///
/// Returns raw `Value` docs covering all three event categories:
/// - `dll`: syscall hooks (`NtAllocateVirtualMemory`, etc.) with `payload_protect`, `payload_size`
/// - `kernel`: lifecycle events (`thread_create`, `image_load`) with `payload_image`
/// - `etw`: OS-level events with `payload_event`, `payload_etw_provider_name`, `payload_event_id`
///
/// Used by `triage::extractor` to build `api:*`, `api_arg:*`, `etw:*`, `image:*`, and `seq2:*` tokens.
pub async fn query_api_telemetry(es: &Elasticsearch, run_id: &str) -> Vec<Value> {
    let response = es
        .search(SearchParts::Index(&["telemetry-*"]))
        .body(json!({
            "query": {
                "bool": {
                    "filter": [
                        { "match_phrase": { "run_id": run_id } }
                    ],
                    "must_not": [
                        { "match_phrase": { "event_type": "trace_log" } }
                    ]
                }
            },
            "sort": [{ "payload_id": "asc" }],
            "size": 5000,
            "_source": [
                "payload_func",
                "payload_id",
                "payload_protect",
                "payload_size",
                "payload_alloc_type",
                "payload_free_type",
                "payload_event",
                "payload_etw_provider_name",
                "payload_event_id",
                "payload_image",
                "event_type",
                "payload_type"
            ]
        }))
        .send()
        .await;

    extract_sources(response).await
}

#[allow(dead_code)]
/// Query all token set documents for a job from `tokens-*`.
///
/// Each document represents one round's combined token set (module + mutation + API + seq2).
/// Used by `triage::extractor::extract_and_score()` to compute lift across all prior rounds.
pub async fn query_token_sets(es: &Elasticsearch, job_id: &str) -> Vec<Value> {
    let response = es
        .search(SearchParts::Index(&["tokens-*"]))
        .body(json!({
            "query": { "term": { "job_id": job_id } },
            "sort": [{ "timestamp": "asc" }],
            "size": 500
        }))
        .send()
        .await;

    extract_sources(response).await
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

/// Find the concrete index name and document `_id` for a document (ES update requires both).
///
/// Returns `(index_name, doc_id)`. The `doc_id` is the actual ES `_id`,
/// which may differ from the field value (e.g. rounds use composite `_id`).
pub(crate) async fn find_index(
    es: &Elasticsearch,
    pattern: &str,
    id_field: &str,
    id_value: &str,
) -> Option<(String, String)> {
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
    let hit = body["hits"]["hits"].as_array()?.first()?;
    let index = hit["_index"].as_str()?.to_string();
    let doc_id = hit["_id"].as_str()?.to_string();
    Some((index, doc_id))
}
