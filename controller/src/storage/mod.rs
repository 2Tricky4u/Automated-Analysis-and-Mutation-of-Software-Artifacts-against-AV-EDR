//! Consolidated Elasticsearch storage module.
//!
//! All ES indexing logic lives here, organized by index:
//! - telemetry: payload flattening, typed_event handling
//! - jobs: job lifecycle (create, status updates)
//! - rounds: round summaries
//! - runs: run results with detection outcome derivation
//! - queries: reusable ES query helpers for API handlers
//! - templates: index template bootstrap

pub mod artifacts;
pub mod helpers;
pub mod jobs;
pub mod queries;
pub mod rounds;
pub mod runs;
pub mod telemetry;
pub mod templates;

use elasticsearch::Elasticsearch;

use crate::automutate::common::TelemetryData;
use crate::automutate::controller::StatusReport;
use crate::dispatch::types::{JobOutcome, JobSession};
pub use rounds::RoundIndexParams;
pub use runs::RunIndexParams;

/// Correlation context for enriching telemetry documents.
#[derive(Debug, Clone, Default)]
pub struct TelemetryContext {
    pub run_id: Option<String>,
    pub round_id: Option<String>,
    pub vm_id: String,
}

/// Consolidated Elasticsearch storage.
///
/// Wraps a single `Elasticsearch` client and exposes typed indexing
/// methods for each index (jobs, rounds, runs, telemetry).
#[derive(Clone)]
pub struct EsStorage {
    client: Elasticsearch,
}

impl EsStorage {
    /// Create a new storage handle wrapping the given [`Elasticsearch`] client.
    pub fn new(client: Elasticsearch) -> Self {
        Self { client }
    }

    // -- Telemetry ---------------------------------------------------------

    /// Bulk-index a batch of telemetry events, enriched with [`TelemetryContext`] correlation keys.
    ///
    /// Delegates to [`telemetry::index_telemetry_batch`]. Empty batches are no-ops.
    ///
    /// # Errors
    ///
    /// Returns an error if the Elasticsearch bulk request fails at the transport
    /// level or the bulk response has a non-success HTTP status. Individual item
    /// failures within a successful bulk response are logged but do not cause a
    /// top-level error.
    pub async fn index_telemetry_batch(
        &self,
        events: &[TelemetryData],
        context: &TelemetryContext,
    ) -> anyhow::Result<()> {
        telemetry::index_telemetry_batch(&self.client, events, context).await
    }

    // -- Jobs --------------------------------------------------------------

    /// Index a new job document when a [`JobSession`] is submitted.
    ///
    /// # Errors
    ///
    /// Returns an error if the Elasticsearch index request or response check fails.
    pub async fn index_job(&self, job: &JobSession) -> anyhow::Result<()> {
        jobs::index_job(&self.client, job).await
    }

    /// Transition a job to "running" status and record its `started_at` timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error if the job document cannot be found or the update request fails.
    pub async fn update_job_started(&self, job_id: &str) -> anyhow::Result<()> {
        jobs::update_job_started(&self.client, job_id).await
    }

    /// Advance the job's `current_round` counter after a round completes.
    ///
    /// # Errors
    ///
    /// Returns an error if the job document cannot be found or the update request fails.
    pub async fn update_job_progress(
        &self,
        job_id: &str,
        current_round: u32,
    ) -> anyhow::Result<()> {
        jobs::update_job_progress(&self.client, job_id, current_round).await
    }

    /// Set a terminal job status (completed, stopped, or failed) with an optional [`JobOutcome`].
    ///
    /// # Errors
    ///
    /// Returns an error if the job document cannot be found or the update request fails.
    pub async fn update_job_status(
        &self,
        job_id: &str,
        status: &str,
        outcome: Option<&JobOutcome>,
    ) -> anyhow::Result<()> {
        jobs::update_job_status(&self.client, job_id, status, outcome).await
    }

    // -- Rounds ------------------------------------------------------------

    /// Index a completed round summary with mutation recipe and run IDs.
    ///
    /// See [`RoundIndexParams`] for the full set of fields persisted.
    ///
    /// # Errors
    ///
    /// Returns an error if the Elasticsearch index request or response check fails.
    pub async fn index_round(&self, params: &RoundIndexParams<'_>) -> anyhow::Result<()> {
        rounds::index_round(&self.client, params).await
    }

    /// Patch an existing round document with computed source-coverage fields.
    ///
    /// Adds `coverage_percent`, `cutoff_line`, `function_coverage`, and related
    /// fields derived from the [`CoverageResult`](crate::triage::source_resolver::CoverageResult).
    ///
    /// # Errors
    ///
    /// Returns an error if the round document cannot be located or the update fails.
    pub async fn update_round_coverage(
        &self,
        job_id: &str,
        round_id: &str,
        coverage: &crate::triage::source_resolver::CoverageResult,
    ) -> anyhow::Result<()> {
        rounds::update_round_coverage(&self.client, job_id, round_id, coverage).await
    }

    /// Patch an existing round document with a blended evasion score.
    ///
    /// The score is rounded to three decimal places before storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the round document cannot be located or the update fails.
    pub async fn update_round_evasion_score(
        &self,
        job_id: &str,
        round_id: &str,
        blended_score: f64,
    ) -> anyhow::Result<()> {
        rounds::update_round_evasion_score(&self.client, job_id, round_id, blended_score).await
    }

    /// Patch an existing round document with late-arriving dryrun data and recomputed fields.
    ///
    /// # Errors
    ///
    /// Returns an error if the round document cannot be located or the update fails.
    pub async fn update_round_dryrun(
        &self,
        job_id: &str,
        round_id: &str,
        dryrun_run_id: &str,
        dry_run_exit_code: i32,
        differential_category: &str,
        detection_verdict: &str,
        detected: bool,
        evasion_score: f64,
    ) -> anyhow::Result<()> {
        rounds::update_round_dryrun(
            &self.client,
            job_id,
            round_id,
            dryrun_run_id,
            dry_run_exit_code,
            differential_category,
            detection_verdict,
            detected,
            evasion_score,
        )
        .await
    }

    // -- Runs --------------------------------------------------------------

    /// Index a run result from a `RoundCompleted` event (primary path).
    ///
    /// Derives the detection outcome from the verdict string, falling back to
    /// the legacy `detected` boolean and exit code. See [`RunIndexParams`].
    ///
    /// # Errors
    ///
    /// Returns an error if the Elasticsearch index request or response check fails.
    pub async fn index_run_result(&self, params: &RunIndexParams<'_>) -> anyhow::Result<()> {
        runs::index_run_result(&self.client, params).await
    }

    /// Index a run status from a [`StatusReport`] (legacy path).
    ///
    /// This path carries worker metadata but no exit code or detection verdict.
    ///
    /// # Errors
    ///
    /// Returns an error if the Elasticsearch index request fails.
    pub async fn index_run_status(&self, report: &StatusReport) -> anyhow::Result<()> {
        runs::index_run_status(&self.client, report).await
    }

    // -- Artifacts ---------------------------------------------------------

    /// Index artifact build metadata to the `artifacts-YYYY.MM` index.
    ///
    /// The caller is responsible for constructing the JSON document; this method
    /// only performs the index request and response validation.
    ///
    /// # Errors
    ///
    /// Returns an error if the Elasticsearch index request or response check fails.
    pub async fn index_artifact(&self, doc: serde_json::Value) -> anyhow::Result<()> {
        artifacts::index_artifact(&self.client, doc).await
    }

    // -- Queries (read-side) -----------------------------------------------

    /// Look up a single job document by `job_id`.
    ///
    /// Returns `None` if the document is not found or the query fails.
    pub async fn query_job(&self, job_id: &str) -> Option<serde_json::Value> {
        queries::query_job(&self.client, job_id).await
    }

    /// Query all rounds for a job, sorted by `round_number` ascending.
    ///
    /// Returns an empty `Vec` on query failure.
    pub async fn query_rounds(&self, job_id: &str) -> Vec<serde_json::Value> {
        queries::query_rounds(&self.client, job_id).await
    }

    /// Query a single round by `job_id` and `round_id`.
    ///
    /// Returns `None` if the document is not found or the query fails.
    pub async fn query_round(&self, job_id: &str, round_id: &str) -> Option<serde_json::Value> {
        queries::query_round(&self.client, job_id, round_id).await
    }

    /// Query a single round excluding heavy fields (`assembled_source`, `function_coverage`).
    ///
    /// Used for fast round detail loads where source and coverage are lazy-loaded separately.
    pub async fn query_round_light(&self, job_id: &str, round_id: &str) -> Option<serde_json::Value> {
        queries::query_round_light(&self.client, job_id, round_id).await
    }

    /// Query a single round returning only `assembled_source` and `instrumented_run_id`.
    ///
    /// Used by the source viewer lazy-load endpoint.
    pub async fn query_round_source_only(&self, job_id: &str, round_id: &str) -> Option<serde_json::Value> {
        queries::query_round_source_only(&self.client, job_id, round_id).await
    }

    /// Query a single round returning only function coverage fields.
    ///
    /// Used by the function coverage lazy-load endpoint.
    pub async fn query_round_coverage_only(&self, job_id: &str, round_id: &str) -> Option<serde_json::Value> {
        queries::query_round_coverage_only(&self.client, job_id, round_id).await
    }

    /// Query run documents by a list of run IDs.
    ///
    /// Returns an empty `Vec` if `run_ids` is empty or the query fails.
    pub async fn query_runs_by_ids(&self, run_ids: &[&str]) -> Vec<serde_json::Value> {
        queries::query_runs_by_ids(&self.client, run_ids).await
    }

    /// Best-effort update of a single string field on a job document.
    ///
    /// # Errors
    ///
    /// Returns an error if the job document cannot be found or the update request fails.
    pub async fn update_job_field(
        &self,
        job_id: &str,
        field: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        queries::update_job_field(&self.client, job_id, field, value).await
    }

    /// Fetch the last `last_n` trace-line records for a run.
    ///
    /// Returns a tuple of `(lines, total_count)`. Lines are sorted by sequence
    /// number descending and capped at `last_n` (defaults to 50 when zero).
    pub async fn query_trace_lines(
        &self,
        run_id: &str,
        last_n: u32,
    ) -> (Vec<serde_json::Value>, u64) {
        queries::query_trace_lines(&self.client, run_id, last_n).await
    }

    /// Fetch the raw JSONL trace content for a run without parsing.
    ///
    /// Returns `None` if no trace document exists or the content is empty.
    pub async fn query_trace_content(&self, run_id: &str) -> Option<String> {
        queries::query_trace_content(&self.client, run_id).await
    }

    // -- Analysis queries (UI) -----------------------------------------------

    /// Query run documents for the UI's `QueryResults` RPC.
    ///
    /// Filters by `job_ids` and/or date range when non-empty. Returns up to
    /// 100 documents sorted by `timestamp` descending.
    pub async fn query_analysis_results(
        &self,
        job_ids: &[String],
        date_from: &str,
        date_to: &str,
    ) -> Vec<serde_json::Value> {
        queries::query_analysis_results(&self.client, job_ids, date_from, date_to).await
    }

    // -- Triage tokens -----------------------------------------------------

    /// Query all non-trace telemetry for a run (dll + kernel + etw), sorted by `payload_id`.
    pub async fn query_api_telemetry(&self, run_id: &str) -> Vec<serde_json::Value> {
        queries::query_api_telemetry(&self.client, run_id).await
    }

    /// Query all checkpoint events for a run from `telemetry-*`.
    pub async fn query_checkpoint_events(&self, run_id: &str) -> Vec<serde_json::Value> {
        queries::query_checkpoint_events(&self.client, run_id).await
    }

    /// Query all token set documents for a job from `tokens-*`.
    ///
    /// Each document represents one round's combined token set. Returns an
    /// empty `Vec` on query failure.
    #[allow(dead_code)]
    pub async fn query_token_sets(&self, job_id: &str) -> Vec<serde_json::Value> {
        queries::query_token_sets(&self.client, job_id).await
    }

    /// Query a single token set document by `job_id` and `round_id`.
    ///
    /// Returns `None` if no matching document is found.
    pub async fn query_token_set_by_round_id(
        &self,
        job_id: &str,
        round_id: &str,
    ) -> Option<serde_json::Value> {
        queries::query_token_set_by_round_id(&self.client, job_id, round_id).await
    }

    /// Index a triage token set document to the `tokens-YYYY.MM` index.
    ///
    /// The document should contain `job_id`, `round_id`, `tokens[]`, and
    /// associated detection metadata.
    ///
    /// # Errors
    ///
    /// Returns an error if the Elasticsearch index request or response check fails.
    pub async fn index_token_set(&self, doc: serde_json::Value) -> anyhow::Result<()> {
        let index = helpers::es_index_name("tokens");
        let response = self
            .client
            .index(elasticsearch::IndexParts::Index(&index))
            .body(doc)
            .refresh(elasticsearch::params::Refresh::WaitFor)
            .send()
            .await?;
        helpers::check_index_response(response, "token_set", "").await
    }

    // -- Bootstrap ---------------------------------------------------------

    /// Create or update all Elasticsearch index templates on startup.
    ///
    /// Ensures mappings exist for `jobs`, `rounds`, `runs`, `telemetry`,
    /// `tokens`, and `artifacts` indices. Individual template failures are logged as warnings
    /// but do not abort the overall operation.
    ///
    /// # Errors
    ///
    /// Currently always returns `Ok(())` because individual failures are
    /// logged rather than propagated. The `Result` return type is retained
    /// for forward compatibility.
    pub async fn ensure_templates(&self) -> anyhow::Result<()> {
        templates::ensure_templates(&self.client).await
    }
}
