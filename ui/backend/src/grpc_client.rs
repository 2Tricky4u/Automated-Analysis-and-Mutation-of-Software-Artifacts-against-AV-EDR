//! gRPC client wrapper for the Controller service.
//!
//! Provides typed, async methods for every Controller RPC endpoint used by the
//! REST layer. The underlying [`tonic`] channel is created lazily on first use
//! and cached behind an [`Arc<RwLock>`] so that all Axum handlers share a
//! single connection.
//!
//! ## Connection Strategy
//!
//! - **Lazy connect:** No TCP connection is attempted until the first RPC call.
//! - **Double-checked locking:** [`get_client`](ControllerGrpcClient::get_client)
//!   acquires a read lock first; only if no connection exists does it upgrade to
//!   a write lock and re-check before connecting. This avoids contention on the
//!   hot path.
//! - **Reconnection:** Call [`disconnect`](ControllerGrpcClient::disconnect) to
//!   clear the cached channel; the next RPC call will transparently reconnect.
//!   On RPC failure the caller (REST handler) returns `SERVICE_UNAVAILABLE` and
//!   the frontend retries.
//!
//! ## RPC Method Groups
//!
//! | Group              | Methods                                                                 |
//! |--------------------|-------------------------------------------------------------------------|
//! | Health             | `ping`, `is_healthy`                                                    |
//! | Jobs               | `schedule_job`, `get_job_status`, `get_job_progress`, `stop_job`, `get_round` |
//! | Runs               | `compare_runs`, `get_trace_lines`                                       |
//! | Tokens             | `compare_tokens`                                                        |
//! | Workers            | `list_workers`, `get_worker_metadata`, `get_available_workers`, `ping_worker`, `disconnect_worker`, `disconnect_all_workers` |
//! | Orchestrator       | `get_orchestrator_status`                                               |
//! | Artifacts          | `build_artifact`, `deploy_artifact` (unused by UI)                      |
//! | Query & Triage     | `query_results`, `submit_triage`                                        |
//! | Streaming          | `get_streaming_client` (reserved for future WebSocket bridge)           |

use crate::generated::controller::{
    BuildRequest, BuildResponse, CompareRunsRequest, CompareRunsResponse, CompareTokensRequest,
    CompareTokensResponse, DeployRequest, DeployResponse, DisconnectAllWorkersRequest,
    DisconnectAllWorkersResponse, DisconnectWorkerRequest, DisconnectWorkerResponse,
    GetAvailableWorkersRequest, GetAvailableWorkersResponse, GetOrchestratorStatusRequest,
    GetOrchestratorStatusResponse, GetRoundCoverageRequest, GetRoundCoverageResponse,
    GetRoundRequest, GetRoundResponse, GetRoundSourceRequest, GetRoundSourceResponse,
    GetTraceLinesRequest, GetTraceLinesResponse, GetWorkerMetadataRequest,
    GetWorkerMetadataResponse, JobProgressRequest, JobProgressResponse, JobRequest, JobResponse,
    JobStatusRequest, JobStatusResponse, ListWorkersRequest, ListWorkersResponse, ModuleSelection,
    PingRequest, PingResponse, PingWorkerRequest, PingWorkerResponse, QueryRequest, QueryResponse,
    StopJobRequest, StopJobResponse, TriageRequest, TriageResponse,
    controller_client::ControllerClient,
};
use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::transport::Channel;
use tracing::{debug, info};

/// Configuration for the Controller gRPC connection.
#[derive(Clone, Debug)]
pub struct ControllerConfig {
    /// gRPC endpoint URI, e.g. `"http://10.200.200.1:50051"`.
    pub address: String,
    /// Per-RPC deadline in seconds. Applied to the [`tonic`] channel at
    /// construction time.
    pub timeout_secs: u64,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            address: "http://127.0.0.1:50051".to_string(),
            timeout_secs: 30,
        }
    }
}

/// Parameters for scheduling a new mutation job.
///
/// Maps 1-to-1 to the protobuf `JobRequest` message. Fields use Rust-native
/// types; the [`schedule_job`](ControllerGrpcClient::schedule_job) method
/// converts them to proto before sending.
pub struct ScheduleJobParams {
    /// Path to the raw shellcode `.bin` file on the build host.
    pub source: String,
    /// Upper bound on mutation rounds. The job stops early if
    /// `stop_on_evasion` is set and evasion is achieved.
    pub max_rounds: u32,
    /// Target OS filter, e.g. `"win10"`, `"win11"`. Empty means any.
    pub target_os: Option<String>,
    /// Required worker capabilities (e.g. `["defender", "rededr"]`).
    pub required_capabilities: Vec<String>,
    /// Explicit loader module selection. `None` lets the controller pick
    /// defaults.
    pub modules: Option<ModuleSelection>,
    /// Payload encoding: `"xor"`, `"english"`, `"subbyte"`, or `"none"`.
    pub encoding: Option<String>,
    /// Stop the job after the first round that achieves full evasion.
    pub stop_on_evasion: bool,
    /// Instrumented-run trace granularity. Valid: `"off"`, `"api"`, `"bb"`,
    /// `"api+bb"`, `"lines"`, `"all"`.
    pub trace_mode: Option<String>,
    /// Selector algorithm: `"coverage"` (default) or `"fuzzer"`.
    pub selector_type: Option<String>,
    /// Module categories the selector may vary across rounds.
    pub variable_categories: Vec<String>,
    /// Variation strategy: `"mutation"` (default) or `"full"`.
    pub variation_strategy: Option<String>,
    /// Mutation IDs to explore. Empty means the full catalogue.
    pub mutation_pool: Vec<String>,
    /// Module names to apply mutations to. Empty means all modules.
    pub mutation_targets: Vec<String>,
    /// Mutations always applied every round after the baseline.
    pub fixed_mutations: Vec<String>,
    /// Number of INT3 shellcode checkpoints (`0` = disabled).
    pub sc_checkpoint_count: u32,
    /// Cache the encoded payload across rounds instead of re-encoding.
    pub cache_payload: bool,
    /// Use MSVC `link.exe` instead of `lld-link` for the final link step.
    pub msvc_compat: bool,
    /// Keep dryrun in pool after grace period expires (default: false).
    pub keep_late_dryrun: bool,
}

/// Thread-safe gRPC client wrapper with lazy connection management.
///
/// Cheaply cloneable (`Arc` interior) and safe to share across Axum handlers.
/// The inner [`tonic`] `ControllerClient` is stored behind
/// `Arc<RwLock<Option<...>>>` so that connection establishment is serialized
/// while concurrent reads proceed without contention.
#[derive(Clone)]
pub struct ControllerGrpcClient {
    config: ControllerConfig,
    client: Arc<RwLock<Option<ControllerClient<Channel>>>>,
}

impl ControllerGrpcClient {
    /// Create a new client from the given config.
    ///
    /// No connection is opened until the first RPC call.
    pub fn new(config: ControllerConfig) -> Self {
        Self {
            config,
            client: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a client targeting `localhost:50051` with a 30 s timeout.
    #[allow(dead_code)]
    pub fn default_local() -> Self {
        Self::new(ControllerConfig::default())
    }

    /// Return an existing connection or create a new one.
    ///
    /// Uses double-checked locking: a read lock is tried first, and only if the
    /// cached client is `None` does the method upgrade to a write lock and
    /// re-check before dialling.
    ///
    /// # Errors
    ///
    /// Returns an error if the TCP/HTTP2 connection to the Controller cannot be
    /// established (network unreachable, DNS failure, TLS mismatch, etc.).
    async fn get_client(&self) -> Result<ControllerClient<Channel>> {
        // Check if we have a connection
        {
            let guard = self.client.read().await;
            if let Some(ref client) = *guard {
                return Ok(client.clone());
            }
        }

        // Create new connection
        let mut guard = self.client.write().await;

        // Double-check after acquiring write lock
        if let Some(ref client) = *guard {
            return Ok(client.clone());
        }

        info!("Connecting to Controller at {}", self.config.address);

        let channel = Channel::from_shared(self.config.address.clone())?
            .timeout(std::time::Duration::from_secs(self.config.timeout_secs))
            .connect()
            .await
            .map_err(|e| anyhow!("Failed to connect to Controller: {}", e))?;

        let client = ControllerClient::new(channel);
        *guard = Some(client.clone());

        info!("Connected to Controller");
        Ok(client)
    }

    /// Clear the cached connection so the next RPC call reconnects.
    #[allow(dead_code)]
    pub async fn disconnect(&self) {
        let mut guard = self.client.write().await;
        *guard = None;
        debug!("Disconnected from Controller");
    }

    // ========================================================================
    // Health Check
    // ========================================================================

    /// Send a `Ping` RPC to the Controller.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established or the
    /// Controller does not respond within the configured timeout.
    pub async fn ping(&self) -> Result<PingResponse> {
        let mut client = self.get_client().await?;
        let response = client
            .ping(PingRequest {
                message: "ping".to_string(),
            })
            .await
            .map_err(|e| anyhow!("Ping failed: {}", e))?;
        Ok(response.into_inner())
    }

    /// Return `true` if a `Ping` RPC succeeds within the timeout.
    pub async fn is_healthy(&self) -> bool {
        self.ping().await.is_ok()
    }

    // ========================================================================
    // Job Management
    // ========================================================================

    /// Schedule a new mutation job on the Controller.
    ///
    /// Converts [`ScheduleJobParams`] to a protobuf `JobRequest` and sends
    /// `ScheduleJob`. The Controller validates the request, enqueues the job,
    /// and returns a `job_id` on acceptance.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the Controller rejects the
    /// request (invalid source, no available workers, etc.).
    pub async fn schedule_job(&self, params: ScheduleJobParams) -> Result<JobResponse> {
        let mut client = self.get_client().await?;

        let request = JobRequest {
            source: params.source,
            max_rounds: params.max_rounds,
            target_os: params.target_os.unwrap_or_default(),
            required_capabilities: params.required_capabilities,
            modules: params.modules,
            encoding: params.encoding.unwrap_or_else(|| "xor".to_string()),
            stop_on_evasion: params.stop_on_evasion,
            trace_mode: params.trace_mode.unwrap_or_default(),
            variable_categories: params.variable_categories,
            selector_type: params.selector_type.unwrap_or_default(),
            variation_strategy: params.variation_strategy.unwrap_or_default(),
            mutation_pool: params.mutation_pool,
            mutation_targets: params.mutation_targets,
            fixed_mutations: params.fixed_mutations,
            sc_checkpoint_count: params.sc_checkpoint_count,
            cache_payload: params.cache_payload,
            msvc_compat: params.msvc_compat,
            keep_late_dryrun: params.keep_late_dryrun,
            ..Default::default()
        };

        let response = client
            .schedule_job(request)
            .await
            .map_err(|e| anyhow!("ScheduleJob failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Fetch the current status of a job.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the job ID is unknown.
    pub async fn get_job_status(&self, job_id: &str) -> Result<JobStatusResponse> {
        let mut client = self.get_client().await?;

        let request = JobStatusRequest {
            job_id: job_id.to_string(),
        };

        let response = client
            .get_job_status(request)
            .await
            .map_err(|e| anyhow!("GetJobStatus failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Fetch detailed job progress including per-round summaries.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the job ID is unknown.
    pub async fn get_job_progress(&self, job_id: &str) -> Result<JobProgressResponse> {
        let mut client = self.get_client().await?;

        let request = JobProgressRequest {
            job_id: job_id.to_string(),
        };

        let response = client
            .get_job_progress(request)
            .await
            .map_err(|e| anyhow!("GetJobProgress failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Request the Controller to stop a running job.
    ///
    /// Idempotent — stopping an already-stopped job is not an error.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the job ID is unknown.
    pub async fn stop_job(&self, job_id: &str) -> Result<StopJobResponse> {
        let mut client = self.get_client().await?;

        let request = StopJobRequest {
            job_id: job_id.to_string(),
        };

        let response = client
            .stop_job(request)
            .await
            .map_err(|e| anyhow!("StopJob failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Fetch full details for a single round within a job.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the job/round ID pair is
    /// unknown.
    pub async fn get_round(&self, job_id: &str, round_id: &str) -> Result<GetRoundResponse> {
        let mut client = self.get_client().await?;

        let request = GetRoundRequest {
            job_id: job_id.to_string(),
            round_id: round_id.to_string(),
        };

        let response = client
            .get_round(request)
            .await
            .map_err(|e| anyhow!("GetRound failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Fetch assembled source for a round (lazy-loaded by source viewer).
    pub async fn get_round_source(
        &self,
        job_id: &str,
        round_id: &str,
    ) -> Result<GetRoundSourceResponse> {
        let mut client = self.get_client().await?;

        let request = GetRoundSourceRequest {
            job_id: job_id.to_string(),
            round_id: round_id.to_string(),
        };

        let response = client
            .get_round_source(request)
            .await
            .map_err(|e| anyhow!("GetRoundSource failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Fetch function coverage for a round (lazy-loaded on section expand).
    pub async fn get_round_coverage(
        &self,
        job_id: &str,
        round_id: &str,
    ) -> Result<GetRoundCoverageResponse> {
        let mut client = self.get_client().await?;

        let request = GetRoundCoverageRequest {
            job_id: job_id.to_string(),
            round_id: round_id.to_string(),
        };

        let response = client
            .get_round_coverage(request)
            .await
            .map_err(|e| anyhow!("GetRoundCoverage failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Compare a baseline (trace-off) run against an instrumented (trace-on)
    /// run from the same round, implementing the two-run differential protocol.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if either run ID is unknown.
    pub async fn compare_runs(
        &self,
        baseline_run_id: &str,
        instrumented_run_id: &str,
    ) -> Result<CompareRunsResponse> {
        let mut client = self.get_client().await?;

        let request = CompareRunsRequest {
            baseline_run_id: baseline_run_id.to_string(),
            instrumented_run_id: instrumented_run_id.to_string(),
        };

        let response = client
            .compare_runs(request)
            .await
            .map_err(|e| anyhow!("CompareRuns failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Compute the symmetric token-set difference between two runs.
    ///
    /// Returns tokens only in A, only in B, in common, per-mutation parameter
    /// distances, and a Jaccard distance metric.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if either run ID is unknown.
    pub async fn compare_tokens(
        &self,
        run_id_a: &str,
        run_id_b: &str,
    ) -> Result<CompareTokensResponse> {
        let mut client = self.get_client().await?;

        let request = CompareTokensRequest {
            run_id_a: run_id_a.to_string(),
            run_id_b: run_id_b.to_string(),
        };

        let response = client
            .compare_tokens(request)
            .await
            .map_err(|e| anyhow!("CompareTokens failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Retrieve the most recent trace lines for a run.
    ///
    /// `last` controls how many lines to return (most recent N). Pass `0` to
    /// use the server default.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the run ID is unknown.
    pub async fn get_trace_lines(&self, run_id: &str, last: u32) -> Result<GetTraceLinesResponse> {
        let mut client = self.get_client().await?;

        let request = GetTraceLinesRequest {
            run_id: run_id.to_string(),
            last,
        };

        let response = client
            .get_trace_lines(request)
            .await
            .map_err(|e| anyhow!("GetTraceLines failed: {}", e))?;

        Ok(response.into_inner())
    }

    // ========================================================================
    // Workers
    // ========================================================================

    /// List all workers currently registered with the Controller.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure.
    pub async fn list_workers(&self) -> Result<ListWorkersResponse> {
        let mut client = self.get_client().await?;

        let request = ListWorkersRequest {};

        let response = client
            .list_workers(request)
            .await
            .map_err(|e| anyhow!("ListWorkers failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Fetch aggregated orchestrator metrics: active jobs, queue depth, and
    /// worker pool utilization.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure.
    pub async fn get_orchestrator_status(&self) -> Result<GetOrchestratorStatusResponse> {
        let mut client = self.get_client().await?;

        let request = GetOrchestratorStatusRequest {};

        let response = client
            .get_orchestrator_status(request)
            .await
            .map_err(|e| anyhow!("GetOrchestratorStatus failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Fetch enhanced metadata for a worker (health, tool versions, last_seen).
    ///
    /// Pass an empty `worker_id` to retrieve metadata for **all** workers.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the specified worker ID is
    /// unknown.
    pub async fn get_worker_metadata(&self, worker_id: &str) -> Result<GetWorkerMetadataResponse> {
        let mut client = self.get_client().await?;

        let request = GetWorkerMetadataRequest {
            worker_id: worker_id.to_string(),
        };

        let response = client
            .get_worker_metadata(request)
            .await
            .map_err(|e| anyhow!("GetWorkerMetadata failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Return workers that match the given OS and capability filters.
    ///
    /// Pass empty strings / vectors to skip filtering.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure.
    pub async fn get_available_workers(
        &self,
        target_os: &str,
        capabilities: Vec<String>,
    ) -> Result<GetAvailableWorkersResponse> {
        let mut client = self.get_client().await?;

        let request = GetAvailableWorkersRequest {
            target_os: target_os.to_string(),
            required_capabilities: capabilities,
        };

        let response = client
            .get_available_workers(request)
            .await
            .map_err(|e| anyhow!("GetAvailableWorkers failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Ping a specific worker through the Controller relay.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the worker is unreachable.
    pub async fn ping_worker(&self, worker_id: &str) -> Result<PingWorkerResponse> {
        let mut client = self.get_client().await?;

        let request = PingWorkerRequest {
            worker_id: worker_id.to_string(),
        };

        let response = client
            .ping_worker(request)
            .await
            .map_err(|e| anyhow!("PingWorker failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Ask the Controller to disconnect a specific worker.
    ///
    /// `reason` is stored in the worker's disconnect log. The worker may or
    /// may not be allowed to reconnect depending on Controller policy.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the worker ID is unknown.
    pub async fn disconnect_worker(
        &self,
        worker_id: &str,
        reason: &str,
    ) -> Result<DisconnectWorkerResponse> {
        let mut client = self.get_client().await?;

        let request = DisconnectWorkerRequest {
            worker_id: worker_id.to_string(),
            reason: reason.to_string(),
        };

        let response = client
            .disconnect_worker(request)
            .await
            .map_err(|e| anyhow!("DisconnectWorker failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Ask the Controller to disconnect every registered worker.
    ///
    /// `reason` is logged for each disconnection. When `reconnect_allowed` is
    /// `true`, workers may re-register after the disconnect.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure.
    pub async fn disconnect_all_workers(
        &self,
        reason: &str,
        reconnect_allowed: bool,
    ) -> Result<DisconnectAllWorkersResponse> {
        let mut client = self.get_client().await?;

        let request = DisconnectAllWorkersRequest {
            reason: reason.to_string(),
            reconnect_allowed,
        };

        let response = client
            .disconnect_all_workers(request)
            .await
            .map_err(|e| anyhow!("DisconnectAllWorkers failed: {}", e))?;

        Ok(response.into_inner())
    }

    // ========================================================================
    // Artifacts
    // ========================================================================

    /// Build an artifact from a template via the Controller.
    ///
    /// Not currently used by the UI — the build pipeline is triggered
    /// internally by the job worker. Retained for future direct-build
    /// workflows.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or build failure.
    #[allow(dead_code)]
    pub async fn build_artifact(
        &self,
        template_name: &str,
        source_file: &str,
        trace_mode: &str,
    ) -> Result<BuildResponse> {
        let mut client = self.get_client().await?;

        let request = BuildRequest {
            template_name: template_name.to_string(),
            source_file: source_file.to_string(),
            trace_mode: trace_mode.to_string(),
            ..Default::default()
        };

        let response = client
            .build_artifact(request)
            .await
            .map_err(|e| anyhow!("BuildArtifact failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Deploy an already-built artifact to a worker VM.
    ///
    /// Not currently used by the UI — deployment is handled by the job worker.
    /// Retained for future direct-deploy workflows.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or deployment failure.
    #[allow(dead_code)]
    pub async fn deploy_artifact(
        &self,
        artifact_id: &str,
        worker_address: &str,
    ) -> Result<DeployResponse> {
        let mut client = self.get_client().await?;

        let request = DeployRequest {
            artifact_id: artifact_id.to_string(),
            worker_address: worker_address.to_string(),
        };

        let response = client
            .deploy_artifact(request)
            .await
            .map_err(|e| anyhow!("DeployArtifact failed: {}", e))?;

        Ok(response.into_inner())
    }

    // ========================================================================
    // Query & Triage
    // ========================================================================

    /// Query ElasticSearch analysis results through the Controller.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the ES query fails
    /// server-side.
    pub async fn query_results(
        &self,
        job_ids: Vec<String>,
        date_from: Option<String>,
        date_to: Option<String>,
    ) -> Result<QueryResponse> {
        let mut client = self.get_client().await?;

        let request = QueryRequest {
            job_ids,
            date_from: date_from.unwrap_or_default(),
            date_to: date_to.unwrap_or_default(),
            filters: Default::default(),
        };

        let response = client
            .query_results(request)
            .await
            .map_err(|e| anyhow!("QueryResults failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Submit a triage verdict for a job.
    ///
    /// # Errors
    ///
    /// Returns an error on connection failure or if the job ID is unknown.
    pub async fn submit_triage(
        &self,
        job_id: &str,
        detected: bool,
        av_product: &str,
    ) -> Result<TriageResponse> {
        let mut client = self.get_client().await?;

        let request = TriageRequest {
            job_id: job_id.to_string(),
            detected,
            av_product: av_product.to_string(),
            ..Default::default()
        };

        let response = client
            .submit_triage(request)
            .await
            .map_err(|e| anyhow!("SubmitTriage failed: {}", e))?;

        Ok(response.into_inner())
    }

    // ========================================================================
    // Telemetry Streaming
    // ========================================================================

    /// Return the raw [`ControllerClient`] for streaming RPCs.
    ///
    /// Reserved for a future WebSocket bridge that streams telemetry events to
    /// the frontend in real time.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established.
    #[allow(dead_code)]
    pub async fn get_streaming_client(&self) -> Result<ControllerClient<Channel>> {
        self.get_client().await
    }
}
