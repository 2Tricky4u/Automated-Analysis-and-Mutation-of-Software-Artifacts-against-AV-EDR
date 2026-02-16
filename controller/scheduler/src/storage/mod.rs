//! Consolidated Elasticsearch storage module.
//!
//! All ES indexing logic lives here, organized by index:
//! - telemetry: payload flattening, typed_event handling
//! - jobs: job lifecycle (create, status updates)
//! - rounds: round summaries
//! - runs: run results with detection outcome derivation
//! - queries: reusable ES query helpers for API handlers
//! - templates: index template bootstrap

pub mod jobs;
pub mod queries;
pub mod rounds;
pub mod runs;
pub mod telemetry;
pub mod templates;

use elasticsearch::Elasticsearch;

use crate::automutate::common::TelemetryData;
use crate::automutate::controller::StatusReport;
use crate::dispatch::types::{
    JobOutcome, JobSession, MutationSpec, RoundSummary, RunOutcome,
};

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
    pub fn new(client: Elasticsearch) -> Self {
        Self { client }
    }

    /// Access the raw ES client for ad-hoc queries (used by api/job.rs handlers).
    pub fn client(&self) -> &Elasticsearch {
        &self.client
    }

    // -- Telemetry ---------------------------------------------------------

    pub async fn index_telemetry_batch(
        &self,
        events: &[TelemetryData],
        context: &TelemetryContext,
    ) -> anyhow::Result<()> {
        telemetry::index_telemetry_batch(&self.client, events, context).await
    }

    // -- Jobs --------------------------------------------------------------

    pub async fn index_job(&self, job: &JobSession) -> anyhow::Result<()> {
        jobs::index_job(&self.client, job).await
    }

    pub async fn update_job_started(&self, job_id: &str) -> anyhow::Result<()> {
        jobs::update_job_started(&self.client, job_id).await
    }

    pub async fn update_job_progress(
        &self,
        job_id: &str,
        current_round: u32,
    ) -> anyhow::Result<()> {
        jobs::update_job_progress(&self.client, job_id, current_round).await
    }

    pub async fn update_job_status(
        &self,
        job_id: &str,
        status: &str,
        outcome: Option<&JobOutcome>,
    ) -> anyhow::Result<()> {
        jobs::update_job_status(&self.client, job_id, status, outcome).await
    }

    // -- Rounds ------------------------------------------------------------

    pub async fn index_round(
        &self,
        job_id: &str,
        summary: &RoundSummary,
        mutation_specs: &[MutationSpec],
        baseline_run_id: &str,
        instrumented_run_id: &str,
        started_at: Option<&str>,
    ) -> anyhow::Result<()> {
        rounds::index_round(
            &self.client,
            job_id,
            summary,
            mutation_specs,
            baseline_run_id,
            instrumented_run_id,
            started_at,
        )
        .await
    }

    // -- Runs --------------------------------------------------------------

    pub async fn index_run_result(
        &self,
        job_id: &str,
        round_id: &str,
        run_id: &str,
        run_type: &str,
        outcome: &RunOutcome,
        mutations: &[String],
        vm_id: &str,
    ) -> anyhow::Result<()> {
        runs::index_run_result(
            &self.client,
            job_id,
            round_id,
            run_id,
            run_type,
            outcome,
            mutations,
            vm_id,
        )
        .await
    }

    pub async fn index_run_status(&self, report: &StatusReport) -> anyhow::Result<()> {
        runs::index_run_status(&self.client, report).await
    }

    // -- Queries (read-side) -----------------------------------------------

    pub async fn query_job(&self, job_id: &str) -> Option<serde_json::Value> {
        queries::query_job(&self.client, job_id).await
    }

    pub async fn query_rounds(&self, job_id: &str) -> Vec<serde_json::Value> {
        queries::query_rounds(&self.client, job_id).await
    }

    pub async fn query_round(
        &self,
        job_id: &str,
        round_id: &str,
    ) -> Option<serde_json::Value> {
        queries::query_round(&self.client, job_id, round_id).await
    }

    pub async fn query_runs_by_ids(&self, run_ids: &[&str]) -> Vec<serde_json::Value> {
        queries::query_runs_by_ids(&self.client, run_ids).await
    }

    pub async fn update_job_field(
        &self,
        job_id: &str,
        field: &str,
        value: &str,
    ) -> anyhow::Result<()> {
        queries::update_job_field(&self.client, job_id, field, value).await
    }

    // -- Bootstrap ---------------------------------------------------------

    pub async fn ensure_templates(&self) -> anyhow::Result<()> {
        templates::ensure_templates(&self.client).await
    }
}
