//! Reusable ES query helpers for API handlers.
//!
//! All read-side ES operations live here. Returns raw `serde_json::Value`
//! to keep this layer proto-agnostic — the API handlers do the mapping.

use elasticsearch::{Elasticsearch, SearchParts};
use serde_json::{Value, json};
use tracing::warn;

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
        .map_err(|e| {
            warn!("ES query_job failed: {}", e);
            e
        })
        .ok()?;

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| {
            warn!("ES query_job: failed to parse response: {}", e);
            e
        })
        .ok()?;
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
        .map_err(|e| {
            warn!("ES query_round failed: {}", e);
            e
        })
        .ok()?;

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| {
            warn!("ES query_round: failed to parse response: {}", e);
            e
        })
        .ok()?;
    body["hits"]["hits"]
        .as_array()
        .and_then(|h| h.first())
        .map(|hit| hit["_source"].clone())
}

/// Query a single round by job_id + round_id, excluding heavy fields.
///
/// Excludes `assembled_source` and `function_coverage` for fast round detail loads.
pub async fn query_round_light(es: &Elasticsearch, job_id: &str, round_id: &str) -> Option<Value> {
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
            "_source": { "excludes": ["assembled_source", "function_coverage"] },
            "size": 1
        }))
        .send()
        .await
        .map_err(|e| {
            warn!("ES query_round_light failed: {}", e);
            e
        })
        .ok()?;

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| {
            warn!("ES query_round_light: failed to parse response: {}", e);
            e
        })
        .ok()?;
    body["hits"]["hits"]
        .as_array()
        .and_then(|h| h.first())
        .map(|hit| hit["_source"].clone())
}

/// Query a single round by job_id + round_id, returning only assembled_source + instrumented_run_id.
pub async fn query_round_source_only(
    es: &Elasticsearch,
    job_id: &str,
    round_id: &str,
) -> Option<Value> {
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
            "_source": ["assembled_source", "instrumented_run_id"],
            "size": 1
        }))
        .send()
        .await
        .map_err(|e| {
            warn!("ES query_round_source_only failed: {}", e);
            e
        })
        .ok()?;

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| {
            warn!(
                "ES query_round_source_only: failed to parse response: {}",
                e
            );
            e
        })
        .ok()?;
    body["hits"]["hits"]
        .as_array()
        .and_then(|h| h.first())
        .map(|hit| hit["_source"].clone())
}

/// Query a single round by job_id + round_id, returning only function coverage fields.
pub async fn query_round_coverage_only(
    es: &Elasticsearch,
    job_id: &str,
    round_id: &str,
) -> Option<Value> {
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
            "_source": ["function_coverage", "coverage_total_lines", "coverage_executable_lines", "coverage_executed_lines"],
            "size": 1
        }))
        .send()
        .await
        .map_err(|e| { warn!("ES query_round_coverage_only failed: {}", e); e })
        .ok()?;

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| {
            warn!(
                "ES query_round_coverage_only: failed to parse response: {}",
                e
            );
            e
        })
        .ok()?;
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
///
/// # Errors
///
/// Returns an error if the job document cannot be found or the update request fails.
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
// Analysis result queries (UI)
// ---------------------------------------------------------------------------

/// Query run documents for the UI's QueryResults RPC.
///
/// Searches `runs-*` filtered by job_ids (if non-empty) and date range
/// (if `date_from`/`date_to` are non-empty). Returns up to 100 docs.
pub async fn query_analysis_results(
    es: &Elasticsearch,
    job_ids: &[String],
    date_from: &str,
    date_to: &str,
) -> Vec<Value> {
    let mut filters: Vec<Value> = Vec::new();

    if !job_ids.is_empty() {
        filters.push(json!({ "terms": { "job_id": job_ids } }));
    }

    if !date_from.is_empty() || !date_to.is_empty() {
        let mut range = serde_json::Map::new();
        if !date_from.is_empty() {
            range.insert("gte".to_string(), json!(date_from));
        }
        if !date_to.is_empty() {
            range.insert("lte".to_string(), json!(date_to));
        }
        filters.push(json!({ "range": { "timestamp": range } }));
    }

    let query = if filters.is_empty() {
        json!({ "match_all": {} })
    } else {
        json!({ "bool": { "filter": filters } })
    };

    let response = es
        .search(SearchParts::Index(&["runs-*"]))
        .body(json!({
            "query": query,
            "sort": [{ "timestamp": "desc" }],
            "size": 100
        }))
        .send()
        .await;

    extract_sources(response).await
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

/// Query a single token set document by job_id + round_id from `tokens-*`.
///
/// Returns the `_source` document containing `tokens[]`, `detected`, `evasion_score`, etc.
pub async fn query_token_set_by_round_id(
    es: &Elasticsearch,
    job_id: &str,
    round_id: &str,
) -> Option<Value> {
    let response = es
        .search(SearchParts::Index(&["tokens-*"]))
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
        .map_err(|e| {
            warn!("ES query_token_set_by_round_id failed: {}", e);
            e
        })
        .ok()?;

    let body = response
        .json::<Value>()
        .await
        .map_err(|e| {
            warn!(
                "ES query_token_set_by_round_id: failed to parse response: {}",
                e
            );
            e
        })
        .ok()?;
    body["hits"]["hits"]
        .as_array()
        .and_then(|h| h.first())
        .map(|hit| hit["_source"].clone())
}

// ---------------------------------------------------------------------------
// Checkpoint event queries (triage)
// ---------------------------------------------------------------------------

/// Query all checkpoint events for a run from `telemetry-*`, sorted by timestamp.
///
/// Checkpoint events are emitted by instrumented artifacts via `ARTIFACT_CHECKPOINT()`.
/// Each is stored as a separate document with `event_type: "checkpoint"` and
/// `payload_checkpoint_name` identifying the checkpoint reached.
///
/// Used by `triage::extractor` to build `checkpoint:*` tokens.
pub async fn query_checkpoint_events(es: &Elasticsearch, run_id: &str) -> Vec<Value> {
    let response = es
        .search(SearchParts::Index(&["telemetry-*"]))
        .body(json!({
            "query": {
                "bool": {
                    "filter": [
                        { "match_phrase": { "run_id": run_id } },
                        { "match_phrase": { "event_type": "checkpoint" } }
                    ]
                }
            },
            "sort": [{ "payload_ts_us": "asc" }],
            "size": 500,
            "_source": ["payload_checkpoint_name", "payload_ts_us", "event_type"]
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
        Ok(resp) => match resp.json::<Value>().await {
            Ok(body) => body["hits"]["hits"]
                .as_array()
                .map(|hits| hits.iter().map(|h| h["_source"].clone()).collect())
                .unwrap_or_default(),
            Err(e) => {
                warn!("ES extract_sources: failed to parse response: {}", e);
                Vec::new()
            }
        },
        Err(e) => {
            warn!("ES search request failed: {}", e);
            Vec::new()
        }
    }
}

/// Find a document by its ES `_id` (not a field value) using an `ids` query.
///
/// Returns `(index_name, es_id)`. Use when the `_id` is known (e.g. composite
/// round IDs like `{job_id}/{round_id}`), avoiding collision risk from field-only lookups.
pub(crate) async fn find_index_by_es_id(
    es: &Elasticsearch,
    pattern: &str,
    es_id: &str,
) -> Option<(String, String)> {
    let resp = es
        .search(SearchParts::Index(&[pattern]))
        .body(json!({
            "query": { "ids": { "values": [es_id] } },
            "size": 1,
            "_source": false
        }))
        .send()
        .await
        .map_err(|e| {
            warn!("ES search by _id failed: {}", e);
            e
        })
        .ok()?;

    let body = resp
        .json::<Value>()
        .await
        .map_err(|e| {
            warn!("ES search by _id: failed to parse response: {}", e);
            e
        })
        .ok()?;
    let hit = body["hits"]["hits"].as_array()?.first()?;
    let index = hit["_index"].as_str()?.to_string();
    let doc_id = hit["_id"].as_str()?.to_string();
    Some((index, doc_id))
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
        .map_err(|e| {
            warn!("ES find_index failed: {}", e);
            e
        })
        .ok()?;

    let body = resp
        .json::<Value>()
        .await
        .map_err(|e| {
            warn!("ES find_index: failed to parse response: {}", e);
            e
        })
        .ok()?;
    let hit = body["hits"]["hits"].as_array()?.first()?;
    let index = hit["_index"].as_str()?.to_string();
    let doc_id = hit["_id"].as_str()?.to_string();
    Some((index, doc_id))
}
