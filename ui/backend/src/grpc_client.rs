//! gRPC Client wrapper for Controller service.
//!
//! Provides typed methods for all Controller gRPC endpoints.

use crate::generated::controller::{
    BuildRequest, BuildResponse, CompareRunsRequest, CompareRunsResponse, DeployRequest,
    DeployResponse, GetOrchestratorStatusRequest, GetOrchestratorStatusResponse, GetRoundRequest,
    GetRoundResponse, JobProgressRequest, JobProgressResponse, JobRequest, JobResponse,
    JobStatusRequest, JobStatusResponse, ListWorkersRequest, ListWorkersResponse, ModuleSelection,
    PingRequest, PingResponse, QueryRequest, QueryResponse, StopJobRequest, StopJobResponse,
    TriageRequest, TriageResponse, controller_client::ControllerClient,
};
use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::sync::RwLock;
use tonic::transport::Channel;
use tracing::{debug, info};

/// Configuration for Controller gRPC connection
#[derive(Clone, Debug)]
pub struct ControllerConfig {
    pub address: String,
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

/// gRPC client wrapper with connection management
#[derive(Clone)]
pub struct ControllerGrpcClient {
    config: ControllerConfig,
    client: Arc<RwLock<Option<ControllerClient<Channel>>>>,
}

impl ControllerGrpcClient {
    /// Create new client with config
    pub fn new(config: ControllerConfig) -> Self {
        Self {
            config,
            client: Arc::new(RwLock::new(None)),
        }
    }

    /// Create with default config (localhost:50051)
    #[allow(dead_code)]
    pub fn default_local() -> Self {
        Self::new(ControllerConfig::default())
    }

    /// Get or create gRPC client connection
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

    /// Clear cached connection (for reconnect)
    #[allow(dead_code)]
    pub async fn disconnect(&self) {
        let mut guard = self.client.write().await;
        *guard = None;
        debug!("Disconnected from Controller");
    }

    // ========================================================================
    // Health Check
    // ========================================================================

    /// Ping the controller
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

    /// Check if controller is healthy
    pub async fn is_healthy(&self) -> bool {
        self.ping().await.is_ok()
    }

    // ========================================================================
    // Job Management
    // ========================================================================

    /// Schedule a new job
    pub async fn schedule_job(
        &self,
        source: String,
        max_rounds: u32,
        target_os: Option<String>,
        required_capabilities: Vec<String>,
        modules: Option<ModuleSelection>,
        encoding: Option<String>,
        stop_on_evasion: bool,
    ) -> Result<JobResponse> {
        let mut client = self.get_client().await?;

        let request = JobRequest {
            source,
            max_rounds,
            target_os: target_os.unwrap_or_default(),
            required_capabilities,
            modules,
            encoding: encoding.unwrap_or_else(|| "xor".to_string()),
            stop_on_evasion,
            ..Default::default()
        };

        let response = client
            .schedule_job(request)
            .await
            .map_err(|e| anyhow!("ScheduleJob failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Get job status
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

    /// Get detailed job progress with rounds
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

    /// Stop a running job
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

    /// Get round details
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

    /// Compare baseline vs instrumented runs
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

    // ========================================================================
    // Workers
    // ========================================================================

    /// List connected workers
    pub async fn list_workers(&self) -> Result<ListWorkersResponse> {
        let mut client = self.get_client().await?;

        let request = ListWorkersRequest {};

        let response = client
            .list_workers(request)
            .await
            .map_err(|e| anyhow!("ListWorkers failed: {}", e))?;

        Ok(response.into_inner())
    }

    /// Get orchestrator status (active jobs, queue depth, pool metrics)
    pub async fn get_orchestrator_status(&self) -> Result<GetOrchestratorStatusResponse> {
        let mut client = self.get_client().await?;

        let request = GetOrchestratorStatusRequest {};

        let response = client
            .get_orchestrator_status(request)
            .await
            .map_err(|e| anyhow!("GetOrchestratorStatus failed: {}", e))?;

        Ok(response.into_inner())
    }

    // ========================================================================
    // Artifacts
    // ========================================================================

    /// Build artifact from template
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

    /// Deploy artifact to worker
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

    /// Query Elasticsearch results
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

    /// Submit triage request
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

    /// Stream telemetry (returns the raw client for streaming)
    #[allow(dead_code)]
    pub async fn get_streaming_client(&self) -> Result<ControllerClient<Channel>> {
        self.get_client().await
    }
}
