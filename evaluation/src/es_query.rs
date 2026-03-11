//! B1: Execution Phase Timing Distribution (ElasticSearch Query)
//!
//! Provides query templates and parsers for extracting `RunPhaseTimings`
//! from stored telemetry events in ElasticSearch.
//!
//! Data already exists in ES — this module provides:
//! 1. ES query JSON to extract phase timings
//! 2. Parser for the response
//! 3. Statistical analysis of phase distributions
//!
//! **RQ:** What fraction is infrastructure overhead vs artifact execution?
//!         Does `process_wait_ms` correlate with detection?

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

/// Phase timings from a single execution run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseTimings {
    pub run_id: String,
    pub job_id: String,
    pub run_type: String, // "baseline", "instrumented", "dryrun"
    pub rededr_setup_ms: u64,
    pub process_spawn_ms: u64,
    pub process_wait_ms: u64,
    pub telemetry_collect_ms: u64,
    pub rededr_reset_ms: u64,
    pub verdict: Option<String>,
}

impl PhaseTimings {
    pub fn total_ms(&self) -> u64 {
        self.rededr_setup_ms
            + self.process_spawn_ms
            + self.process_wait_ms
            + self.telemetry_collect_ms
            + self.rededr_reset_ms
    }

    pub fn overhead_ms(&self) -> u64 {
        self.total_ms() - self.process_wait_ms
    }

    pub fn overhead_ratio(&self) -> f64 {
        let total = self.total_ms();
        if total == 0 {
            return 0.0;
        }
        self.overhead_ms() as f64 / total as f64
    }
}

/// Generate the ES query to extract phase timings for a job.
pub fn phase_timing_query(job_id: &str, max_results: usize) -> serde_json::Value {
    json!({
        "size": max_results,
        "query": {
            "bool": {
                "must": [
                    { "term": { "job_id": job_id } },
                    { "exists": { "field": "phase_timings.process_wait_ms" } }
                ]
            }
        },
        "_source": [
            "run_id", "job_id", "run_type",
            "phase_timings.rededr_setup_ms",
            "phase_timings.process_spawn_ms",
            "phase_timings.process_wait_ms",
            "phase_timings.telemetry_collect_ms",
            "phase_timings.rededr_reset_ms",
            "detection_verdict"
        ],
        "sort": [{ "completed_at": "asc" }]
    })
}

/// Parse ES response JSON into PhaseTimings records.
pub fn parse_phase_timings(es_response: &serde_json::Value) -> Vec<PhaseTimings> {
    let hits = es_response
        .get("hits")
        .and_then(|h| h.get("hits"))
        .and_then(|h| h.as_array());

    let Some(hits) = hits else {
        return Vec::new();
    };

    hits.iter()
        .filter_map(|hit| {
            let source = hit.get("_source")?;
            let pt = source.get("phase_timings")?;

            Some(PhaseTimings {
                run_id: source
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                job_id: source
                    .get("job_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                run_type: source
                    .get("run_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                rededr_setup_ms: pt
                    .get("rededr_setup_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                process_spawn_ms: pt
                    .get("process_spawn_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                process_wait_ms: pt
                    .get("process_wait_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                telemetry_collect_ms: pt
                    .get("telemetry_collect_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                rededr_reset_ms: pt
                    .get("rededr_reset_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                verdict: source
                    .get("detection_verdict")
                    .and_then(|v| v.as_str())
                    .map(String::from),
            })
        })
        .collect()
}

/// Analyze phase timing distribution for a set of runs.
pub fn analyze_phase_timings(timings: &[PhaseTimings]) -> serde_json::Value {
    if timings.is_empty() {
        return json!({ "error": "no timing data" });
    }

    let phases = [
        "rededr_setup_ms",
        "process_spawn_ms",
        "process_wait_ms",
        "telemetry_collect_ms",
        "rededr_reset_ms",
    ];

    let extract = |t: &PhaseTimings, phase: &str| -> u64 {
        match phase {
            "rededr_setup_ms" => t.rededr_setup_ms,
            "process_spawn_ms" => t.process_spawn_ms,
            "process_wait_ms" => t.process_wait_ms,
            "telemetry_collect_ms" => t.telemetry_collect_ms,
            "rededr_reset_ms" => t.rededr_reset_ms,
            _ => 0,
        }
    };

    let mut phase_stats = Vec::new();
    for &phase in &phases {
        let mut values: Vec<u64> = timings.iter().map(|t| extract(t, phase)).collect();
        values.sort();
        let n = values.len();
        let mean = values.iter().sum::<u64>() as f64 / n as f64;
        let median = values[n / 2];
        let p95 = values[(n as f64 * 0.95) as usize];
        let min = values[0];
        let max = values[n - 1];

        phase_stats.push(json!({
            "phase": phase,
            "mean_ms": mean,
            "median_ms": median,
            "p95_ms": p95,
            "min_ms": min,
            "max_ms": max,
            "values": values,
        }));
    }

    // Overall overhead ratio
    let overhead_ratios: Vec<f64> = timings.iter().map(|t| t.overhead_ratio()).collect();
    let mean_overhead = overhead_ratios.iter().sum::<f64>() / overhead_ratios.len() as f64;

    // Per-verdict breakdown
    let mut by_verdict: HashMap<String, Vec<u64>> = HashMap::new();
    for t in timings {
        let verdict = t.verdict.clone().unwrap_or("unknown".to_string());
        by_verdict
            .entry(verdict)
            .or_default()
            .push(t.process_wait_ms);
    }

    let verdict_summary: Vec<serde_json::Value> = by_verdict
        .iter()
        .map(|(verdict, waits)| {
            let mean = waits.iter().sum::<u64>() as f64 / waits.len() as f64;
            json!({
                "verdict": verdict,
                "n": waits.len(),
                "mean_process_wait_ms": mean,
            })
        })
        .collect();

    json!({
        "n_runs": timings.len(),
        "phase_statistics": phase_stats,
        "mean_overhead_ratio": mean_overhead,
        "per_verdict": verdict_summary,
    })
}

/// Generate a curl command to run the ES query.
pub fn curl_command(es_host: &str, index: &str, job_id: &str) -> String {
    let query = phase_timing_query(job_id, 200);
    format!(
        "curl -s -X POST '{}/{}/_search' -H 'Content-Type: application/json' -d '{}'",
        es_host,
        index,
        serde_json::to_string(&query).unwrap_or_default()
    )
}
