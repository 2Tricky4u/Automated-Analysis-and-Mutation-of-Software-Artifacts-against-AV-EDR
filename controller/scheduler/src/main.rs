use edr_config::ControllerConfig;
use elasticsearch::{Elasticsearch, IndexParts, http::transport::Transport};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status, transport::Server};
use tracing::{error, info, warn};

mod job;
mod queue;
mod worker_pool;
mod scheduler_core;

use scheduler_core::{SchedulerConfig as CoreSchedulerConfig, create_scheduler_core};

pub mod edr {
    pub mod common {
        tonic::include_proto!("edr.common");
    }
    pub mod controller {
        tonic::include_proto!("edr.controller");
    }
    pub mod worker {
        tonic::include_proto!("edr.worker");
    }
}

use edr::common::{JobId, TelemetryAck, TelemetryData};
use edr::controller::{
    BuildRequest, BuildResponse, DeployRequest, DeployResponse, JobRequest, JobResponse,
    JobStatusRequest, JobStatusResponse, ListWorkersRequest, ListWorkersResponse,
    PingRequest, PingResponse, QueryRequest, QueryResponse,
    StatusAck, StatusReport, TriageRequest, TriageResponse, WorkerInfo,
    controller_server::{Controller, ControllerServer},
};

const DELAY: u64 = 20;

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct Job {
    id: String,
    name: String,
    status: String,
    progress: i32,
    phase: String,
    logs: Vec<String>,
}

#[derive(Debug, Default)]
struct SchedulerState {
    jobs: HashMap<String, Job>,
    job_counter: u64,
}

#[derive(Clone)]
pub struct SchedulerService {
    state: Arc<Mutex<SchedulerState>>,
    es_client: Elasticsearch,
    controller_ip: String,
    scheduler_core: Option<Arc<scheduler_core::SchedulerCore>>,
}

impl SchedulerService {
    pub fn new(es_client: Elasticsearch, controller_ip: String) -> Self {
        Self {
            state: Arc::new(Mutex::new(SchedulerState::default())),
            es_client,
            controller_ip,
            scheduler_core: None,
        }
    }

    pub fn set_scheduler_core(&mut self, core: Arc<scheduler_core::SchedulerCore>) {
        self.scheduler_core = Some(core);
    }
}

#[tonic::async_trait]
impl Controller for SchedulerService {
    async fn ping(&self, request: Request<PingRequest>) -> Result<Response<PingResponse>, Status> {
        let req = request.into_inner();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        info!("Ping received: {}", req.message);

        Ok(Response::new(PingResponse {
            message: format!("pong: {}", req.message),
            timestamp,
            server: "controller/scheduler".to_string(),
        }))
    }

    async fn schedule_job(
        &self,
        request: Request<JobRequest>,
    ) -> Result<Response<JobResponse>, Status> {
        let req = request.into_inner();

        // Check if scheduler core is available
        let scheduler_core = match &self.scheduler_core {
            Some(core) => core,
            None => {
                warn!("Scheduler core not available, job submission rejected");
                return Ok(Response::new(JobResponse {
                    job_id: None,
                    accepted: false,
                    message: "Scheduler core not initialized. Check that worker configs exist in automation/generated/".to_string(),
                    estimated_duration_seconds: 0,
                }));
            }
        };

        // Parse mutations from mutation_strategies field
        // Format: "ast.import_reshape,beh.preamble.fs"
        let mutations: Vec<job::MutationSpec> = req
            .mutation_strategies
            .iter()
            .map(|s| job::MutationSpec {
                id: s.clone(),
                params: None,
            })
            .collect();

        // Submit job to scheduler queue
        match scheduler_core.queue().submit_job(
            req.artifact_type.clone(),  // template_name
            req.source.clone(),          // source_file
            mutations,
            "all".to_string(),        // Default trace mode (Phase 1) //TODO make modular
            req.priority,
        ) {
            Ok(job_id) => {
                info!(
                    "Job {} submitted to scheduler queue: {} ({})",
                    job_id, req.name, req.artifact_type
                );

                Ok(Response::new(JobResponse {
                    job_id: Some(JobId {
                        value: job_id.clone(),
                    }),
                    accepted: true,
                    message: format!("Job {} queued for execution", job_id),
                    estimated_duration_seconds: 60, // Estimated from config timeout
                }))
            }
            Err(e) => {
                error!("Failed to submit job: {}", e);
                Ok(Response::new(JobResponse {
                    job_id: None,
                    accepted: false,
                    message: format!("Failed to queue job: {}", e),
                    estimated_duration_seconds: 0,
                }))
            }
        }
    }

    async fn get_job_status(
        &self,
        request: Request<JobStatusRequest>,
    ) -> Result<Response<JobStatusResponse>, Status> {
        let req = request.into_inner();

        // Check if scheduler core is available
        let scheduler_core = match &self.scheduler_core {
            Some(core) => core,
            None => {
                return Err(Status::unavailable("Scheduler core not initialized"));
            }
        };

        // Query job from scheduler queue
        match scheduler_core.queue().get_job(&req.job_id) {
            Some(job) => {
                let progress_percent = match job.status {
                    job::JobStatus::Queued => 0,
                    job::JobStatus::Building => 25,
                    job::JobStatus::Deployed => 50,
                    job::JobStatus::Running => 75,
                    job::JobStatus::Completed => 100,
                    job::JobStatus::Failed | job::JobStatus::Timeout => 100,
                };

                Ok(Response::new(JobStatusResponse {
                    job_id: job.id.clone(),
                    status: job.status.to_string(),
                    progress_percent,
                    current_phase: format!("{:?}", job.status),
                    logs: vec![format!("Job {} is {}", job.id, job.status)],
                }))
            }
            None => Err(Status::not_found(format!("Job {} not found", req.job_id))),
        }
    }

    async fn submit_triage(
        &self,
        request: Request<TriageRequest>,
    ) -> Result<Response<TriageResponse>, Status> {
        let req = request.into_inner();
        let triage_id = format!(
            "triage-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );

        info!(
            "Triage submitted for job {}: detected={}",
            req.job_id, req.detected
        );

        Ok(Response::new(TriageResponse {
            job_id: req.job_id,
            stored: true,
            triage_id,
        }))
    }

    async fn query_results(
        &self,
        request: Request<QueryRequest>,
    ) -> Result<Response<QueryResponse>, Status> {
        let _req = request.into_inner();

        Ok(Response::new(QueryResponse {
            results: vec![],
            total_count: 0,
        }))
    }

    async fn stream_telemetry(
        &self,
        request: Request<tonic::Streaming<TelemetryData>>,
    ) -> Result<Response<TelemetryAck>, Status> {
        use tokio::time::{timeout, Duration};

        // Get remote_addr BEFORE into_inner() consumes the request
        let remote_addr = request.remote_addr().map(|a| a.to_string()).unwrap_or_else(|| "unknown".to_string());

        let mut stream = request.into_inner();
        let mut events_count = 0;
        let mut first_job_id = String::new();
        let mut batch = Vec::new();
        const MAX_BATCH_SIZE: usize = 10000; // Prevent memory exhaustion

        info!("[RECV]  Telemetry stream opened from worker: {}", remote_addr);

        // Collect all events from stream with timeout
        let collection_result = timeout(Duration::from_secs(30), async {
            while let Some(telemetry) = stream.message().await? {
                if events_count == 0 {
                    first_job_id = telemetry.job_id.clone();
                }

                events_count += 1;
                batch.push(telemetry);

                // Prevent unbounded memory growth
                if batch.len() >= MAX_BATCH_SIZE {
                    warn!(
                        "[WARN]  Telemetry batch size limit reached ({} events), stopping collection [worker: {}]",
                        MAX_BATCH_SIZE, remote_addr
                    );
                    break;
                }
            }
            Ok::<(), tonic::Status>(())
        })
        .await;

        match collection_result {
            Ok(Ok(())) => {
                info!(
                    "[OK] Telemetry batch collected: job={}, events_count={}, worker={}",
                    first_job_id, events_count, remote_addr
                );
            }
            Ok(Err(e)) => {
                error!("[ERROR] STREAM ERROR: Failed to collect telemetry from worker: {}", remote_addr);
                error!("   Error details: {:?}", e);
                error!("   Status code: {}, Message: {}", e.code(), e.message());
                return Err(e);
            }
            Err(_) => {
                error!(
                    "[TIMEOUT]  TIMEOUT: Telemetry stream collection exceeded 30s limit [worker: {}]",
                    remote_addr
                );
                error!("   Collected {} events before timeout (partial batch)", events_count);
                warn!("   Possible causes: slow network, large payload, worker stalled");
                // Continue with partial batch rather than failing
            }
        }

        // Index batch to Elasticsearch with timeout
        if !batch.is_empty() {
            info!("[UPLOAD]Indexing {} events to Elasticsearch [job: {}]", events_count, first_job_id);
            match timeout(
                Duration::from_secs(10),
                self.index_telemetry_batch(&batch)
            )
            .await
            {
                Ok(Ok(())) => {
                    info!(
                        "[OK] Successfully indexed {} telemetry events to Elasticsearch [job: {}]",
                        events_count, first_job_id
                    );
                }
                Ok(Err(e)) => {
                    error!("[ERROR] ELASTICSEARCH ERROR: Failed to index telemetry batch");
                    error!("   Job: {}, Events: {}, Worker: {}", first_job_id, events_count, remote_addr);
                    error!("   Error details: {}", e);
                    error!("   [WARN]  Telemetry received but NOT INDEXED (Elasticsearch may be down/unreachable)");
                    warn!("   Possible causes: Elasticsearch down, network issue, mapping conflict, disk full");
                    // Don't fail the RPC - telemetry was received, just not indexed
                }
                Err(_) => {
                    error!(
                        "[TIMEOUT]  ELASTICSEARCH TIMEOUT: Indexing exceeded 10s limit [job: {}]",
                        first_job_id
                    );
                    error!(
                        "   Events: {}, Worker: {}", events_count, remote_addr
                    );
                    error!(
                        "   [WARN]  Telemetry received but NOT INDEXED (Elasticsearch is slow/unavailable)"
                    );
                    warn!("   Possible causes: Elasticsearch overloaded, slow disk I/O, large batch size");
                    // Don't fail the RPC - telemetry was received, just not indexed
                }
            }
        } else {
            warn!("[WARN]  Telemetry stream closed with ZERO events [worker: {}]", remote_addr);
            warn!("   This may indicate: worker had no telemetry to send, or stream failed early");
        }

        Ok(Response::new(TelemetryAck {
            received: true,
            events_count,
        }))
    }

    async fn report_status(
        &self,
        request: Request<StatusReport>,
    ) -> Result<Response<StatusAck>, Status> {
        use tokio::time::Duration;

        let remote_addr = request.remote_addr().map(|a| a.to_string()).unwrap_or_else(|| "unknown".to_string());
        let report = request.into_inner();

        // Update worker health and job status in scheduler core (if available)
        if let Some(ref scheduler_core) = self.scheduler_core {
            // Update worker health timestamp
            if let Err(e) = scheduler_core.pool().update_health(&report.worker_id) {
                warn!("Failed to update worker health for {}: {}", report.worker_id, e);
            }

            // Update job status based on status report
            if let Some(mut job) = scheduler_core.queue().get_job(&report.job_id) {
                match report.event_type.as_str() {
                    "success" => {
                        job.mark_completed();
                        let _ = scheduler_core.queue().update_job(&job);
                        // Release worker
                        let _ = scheduler_core.pool().release_worker(&report.worker_id);
                    }
                    "error" => {
                        job.mark_failed(report.details.clone());
                        let _ = scheduler_core.queue().update_job(&job);
                        // Release worker
                        let _ = scheduler_core.pool().release_worker(&report.worker_id);
                    }
                    "timeout" => {
                        job.mark_timeout();
                        let _ = scheduler_core.queue().update_job(&job);
                        // Release worker
                        let _ = scheduler_core.pool().release_worker(&report.worker_id);
                    }
                    _ => {
                        // Heartbeat or other status - job is still running, just update
                        let _ = scheduler_core.queue().update_job(&job);
                    }
                }
            }
        }

        // Format status display: [WORKER: ip] [PID: pid] [JOB: job_id] [STATUS: event_type] details
        let status_line = format!(
            "[WORKER: {} ({})] [PID: {}] [JOB: {}] [RUN: {}] [STATUS: {}] [ARTIFACT: {}] {}",
            report.worker_ip,
            report.worker_id,
            report.pid,
            report.job_id,
            report.run_id,
            report.event_type.to_uppercase(),
            report.artifact_name,
            report.details
        );

        // Use appropriate log level based on status type
        match report.event_type.as_str() {
            "error" | "timeout" | "stuck" | "crashed" => {
                tracing::warn!("[WARN]  {}", status_line);
            }
            _ => {
                info!("[STATUS] {}", status_line);
            }
        }

        // Store RunResult to Elasticsearch when final status received
        // Use timeout to prevent blocking RPC if Elasticsearch is slow/unreachable
        if matches!(report.event_type.as_str(), "success" | "error" | "timeout") {
            info!("[SAVE]Storing final run result for job: {} [worker: {}]", report.job_id, remote_addr);
            match tokio::time::timeout(
                Duration::from_secs(DELAY),
                self.store_run_result(&report)
            ).await {
                Ok(Ok(())) => {
                    info!("[OK] Run result stored successfully [job: {}, run: {}]", report.job_id, report.run_id);
                }
                Ok(Err(e)) => {
                    error!("[ERROR] ELASTICSEARCH ERROR: Failed to store run result");
                    error!("   Job: {}, Run: {}, Worker: {}", report.job_id, report.run_id, remote_addr);
                    error!("   Error details: {}", e);
                    error!("   [WARN]  Status report received but NOT INDEXED (Elasticsearch may be down/unreachable)");
                    warn!("   Possible causes: Elasticsearch down, network issue, mapping conflict, disk full");
                    // Don't fail the RPC - status was received, just not indexed
                }
                Err(_) => {
                    error!(
                        "[TIMEOUT]  ELASTICSEARCH TIMEOUT: Storing run result exceeded {}s limit", DELAY
                    );
                    error!("   Job: {}, Run: {}, Worker: {}", report.job_id, report.run_id, remote_addr);
                    error!(
                        "   [WARN]  Status report received but NOT INDEXED (Elasticsearch is slow/unavailable)"
                    );
                    warn!("   Possible causes: Elasticsearch overloaded, slow disk I/O, query queue backlog");
                    // Don't fail the RPC - status was received, just not indexed
                }
            }
        }

        Ok(Response::new(StatusAck { received: true }))
    }

    async fn build_artifact(
        &self,
        request: Request<BuildRequest>,
    ) -> Result<Response<BuildResponse>, Status> {
        let req = request.into_inner();

        // Extract trace_mode with default (backwards compatibility)
        let trace_mode = if req.trace_mode.is_empty() {
            "api+bb".to_string() // Default: API + BB coverage for mutation loop
        } else {
            req.trace_mode.clone()
        };

        info!(
            "Build request: template={}, source={}, mutations={}, trace_mode={}",
            req.template_name,
            req.source_file,
            req.mutations.len(),
            trace_mode
        );

        // Create builder with default config
        let builder_config = builder::BuilderConfig::default();
        let artifact_builder = builder::ArtifactBuilder::new(builder_config).map_err(|e| {
            error!("Failed to create artifact builder: {}", e);
            Status::internal(format!("Failed to create builder: {}", e))
        })?;

        // Convert proto mutations to builder::mutator::MutationSpec
        let mutations: Vec<builder::mutator::MutationSpec> = req
            .mutations
            .into_iter()
            .map(|m| builder::mutator::MutationSpec {
                id: m.id,
                params: m.params,
            })
            .collect();

        if !mutations.is_empty() {
            info!(
                "Applying mutations: {:?}",
                mutations.iter().map(|m| &m.id).collect::<Vec<_>>()
            );
        }

        // Build the artifact with mutations
        // NOTE: trace_mode is passed to builder; emitter support pending
        let built = artifact_builder
            .build(builder::BuildInput::SourceFile {
                template_name: req.template_name.clone(),
                source_file: req.source_file.clone(),
                mutations,
                trace_mode: trace_mode.clone(),
            })
            .await
            .map_err(|e| {
                error!("Build failed: {}", e);
                Status::internal(format!("Build failed: {}", e))
            })?;

        info!(
            "Build successful: artifact_id={}, size={} bytes, mutations_applied={:?}, trace_mode={}",
            built.artifact_id, built.size_bytes, built.mutations_applied, trace_mode
        );

        // TODO (Phase 3): Index artifact metadata to Elasticsearch

        Ok(Response::new(BuildResponse {
            artifact_id: built.artifact_id,
            size_bytes: built.size_bytes,
            build_status: "success".to_string(),
            error: String::new(),
            storage_path: built.output_path.to_string_lossy().to_string(),
            build_timestamp: built.build_timestamp.timestamp(),
            mutations_applied: built.mutations_applied,
            trace_mode,  // Echo back the trace_mode that was used
        }))
    }

    async fn deploy_artifact(
        &self,
        request: Request<DeployRequest>,
    ) -> Result<Response<DeployResponse>, Status> {
        use edr::worker::{ArtifactChunk, worker_agent_client::WorkerAgentClient};
        use futures::stream;
        use sha2::{Digest, Sha256};

        let req = request.into_inner();

        info!(
            "Deploy request: artifact_id={}, worker={}",
            req.artifact_id, req.worker_address
        );

        // 1. Read artifact from disk
        let builder_config = builder::BuilderConfig::default();
        let artifact_path = builder_config
            .output_dir
            .join(format!("{}.exe", req.artifact_id));

        if !artifact_path.exists() {
            return Err(Status::not_found(format!(
                "Artifact {} not found. Build it first using BuildArtifact RPC.",
                req.artifact_id
            )));
        }

        let artifact_data = tokio::fs::read(&artifact_path)
            .await
            .map_err(|e| Status::internal(format!("Failed to read artifact: {}", e)))?;

        info!(
            "Read artifact: {} bytes from {:?}",
            artifact_data.len(),
            artifact_path
        );

        // 2. Verify SHA256 matches artifact_id
        let mut hasher = Sha256::new();
        hasher.update(&artifact_data);
        let actual_sha256 = format!("{:x}", hasher.finalize());

        if actual_sha256 != req.artifact_id {
            return Err(Status::internal(format!(
                "Artifact SHA256 mismatch: expected {}, got {}",
                req.artifact_id, actual_sha256
            )));
        }

        // 3. Connect to worker
        let worker_url = format!("http://{}", req.worker_address);
        info!("Attempting connection to worker: {}", worker_url);

        // Create endpoint with explicit configuration
        let endpoint = match tonic::transport::Endpoint::try_from(worker_url.clone()) {
            Ok(ep) => ep,
            Err(e) => {
                error!("Invalid endpoint URL '{}': {}", worker_url, e);
                return Err(Status::invalid_argument(format!(
                    "Invalid worker address: {}",
                    e
                )));
            }
        };

        info!("Endpoint created, connecting...");
        let mut client = WorkerAgentClient::connect(endpoint).await.map_err(|e| {
            error!("=== CONNECT TO WORKER FAILED ===");
            error!("Worker URL: {}", worker_url);
            error!("Error type: {}", std::any::type_name_of_val(&e));
            error!("Error: {}", e);
            error!("Debug: {:?}", e);
            Status::unavailable(format!("Failed to connect to worker: {}", e))
        })?;

        info!("Successfully connected to worker");

        // 4. Split into chunks (4MB per chunk)
        let chunk_size = 4 * 1024 * 1024; // 4MB
        let total_chunks = (artifact_data.len() + chunk_size - 1) / chunk_size;

        let chunks: Vec<ArtifactChunk> = artifact_data
            .chunks(chunk_size)
            .enumerate()
            .map(|(i, chunk)| ArtifactChunk {
                artifact_id: req.artifact_id.clone(),
                data: chunk.to_vec(),
                chunk_index: i as u32,
                total_chunks: total_chunks as u32,
                sha256: req.artifact_id.clone(),
            })
            .collect();

        info!(
            "Streaming {} chunks ({} bytes total) to worker",
            chunks.len(),
            artifact_data.len()
        );

        // 5. Stream chunks to worker
        let chunk_stream = stream::iter(chunks.clone());

        info!("Calling send_artifact RPC with {} chunks...", chunks.len());
        let response = client
            .send_artifact(chunk_stream)
            .await
            .map_err(|e| {
                error!("=== send_artifact RPC FAILED ===");
                error!("Error: {}", e);
                error!("Error code: {:?}", e.code());
                error!("Error message: {}", e.message());
                error!("Worker URL: {}", worker_url);
                error!("Full debug: {:?}", e);
                Status::internal(format!(
                    "Failed to send artifact: {} (code: {:?})",
                    e.message(),
                    e.code()
                ))
            })?
            .into_inner();

        info!("send_artifact RPC completed successfully");

        if !response.received {
            return Err(Status::internal(format!(
                "Worker rejected artifact: {}",
                response.error
            )));
        }

        info!(
            "Artifact deployed successfully: {} chunks sent to {}",
            response.chunks_received, req.worker_address
        );

        Ok(Response::new(DeployResponse {
            success: true,
            artifact_id: req.artifact_id,
            worker_address: req.worker_address,
            worker_storage_path: response.storage_path,
            chunks_sent: response.chunks_received,
            error: String::new(),
        }))
    }

    async fn list_workers(
        &self,
        _request: Request<ListWorkersRequest>,
    ) -> Result<Response<ListWorkersResponse>, Status> {
        let scheduler_core = match &self.scheduler_core {
            Some(core) => core,
            None => {
                return Err(Status::unavailable("Scheduler core not initialized"));
            }
        };

        let workers = scheduler_core.pool().list_workers();
        let worker_infos: Vec<WorkerInfo> = workers
            .iter()
            .map(|w| {
                let last_ping_secs = w
                    .last_ping
                    .elapsed()
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);

                WorkerInfo {
                    worker_id: w.id.clone(),
                    address: w.address.clone(),
                    status: w.status.to_string(),
                    current_job_id: w.current_job.clone().unwrap_or_default(),
                    last_ping_seconds_ago: last_ping_secs,
                    enabled: w.enabled,
                }
            })
            .collect();

        Ok(Response::new(ListWorkersResponse {
            workers: worker_infos,
        }))
    }
}

impl SchedulerService {
    /// Index telemetry batch to Elasticsearch
    async fn index_telemetry_batch(
        &self,
        batch: &[TelemetryData],
    ) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let index_name = format!("telemetry-{}", chrono::Utc::now().format("%Y.%m.%d"));
        let mut indexed = 0;

        // Index events individually (simpler API usage)
        for event in batch {
            // Parse payload to extract searchable fields, but keep original as string
            let mut payload_fields = if let Ok(payload_json) =
                serde_json::from_slice::<serde_json::Value>(&event.payload)
            {
                payload_json.as_object().cloned().unwrap_or_default()
            } else {
                Default::default()
            };

            // Handle typed_event variants (for events using structured proto instead of JSON payload)
            // This is critical for trace events which use typed_event.trace instead of payload
            if let Some(ref typed_event) = event.typed_event {
                use edr::common::telemetry_data::TypedEvent;
                match typed_event {
                    TypedEvent::Trace(trace) => {
                        // Extract trace event fields into payload_fields for indexing
                        payload_fields.insert("seq".to_string(), json!(trace.seq));
                        payload_fields.insert("file".to_string(), json!(&trace.file));
                        payload_fields.insert("line".to_string(), json!(trace.line));
                        payload_fields.insert("func".to_string(), json!(&trace.func));
                        payload_fields.insert("ts_us".to_string(), json!(trace.ts_us));
                    }
                    TypedEvent::Coverage(cov) => {
                        // Extract BB coverage fields into payload_fields for indexing
                        payload_fields.insert("total_bbs".to_string(), json!(cov.total_bbs));
                        payload_fields.insert("bb_ids".to_string(), json!(&cov.bb_ids));
                        payload_fields.insert("hit_counts".to_string(), json!(&cov.hit_counts));
                        payload_fields.insert("bitmap_size".to_string(), json!(cov.bitmap.len()));

                        // Store bitmap as Base64 for Elasticsearch (more efficient than raw bytes)
                        use base64::{engine::general_purpose, Engine as _};
                        let bitmap_b64 = general_purpose::STANDARD.encode(&cov.bitmap);
                        payload_fields.insert("bitmap_b64".to_string(), json!(bitmap_b64));
                    }
                    TypedEvent::Checkpoint(cp) => {
                        // Extract checkpoint fields into payload_fields for indexing
                        payload_fields.insert("checkpoint_name".to_string(), json!(&cp.name));
                        payload_fields.insert("ts_us".to_string(), json!(cp.ts_us));
                    }
                }
            }

            // Build document with flattened payload fields at top level for searchability
            let mut doc = json!({
                "job_id": event.job_id,
                "event_type": event.event_type,
                "timestamp": event.timestamp,
                "metadata": event.metadata,
                "indexed_at": chrono::Utc::now().to_rfc3339(),
            });

            // Merge payload fields into top level (makes them searchable)
            if let Some(obj) = doc.as_object_mut() {
                for (key, value) in payload_fields {
                    // Detect pointer/address fields by name pattern
                    let key_lower = key.to_lowercase();
                    let is_pointer_field = key_lower.contains("address")
                        || key_lower.contains("pointer")
                        || key_lower.contains("stack")
                        || key_lower.contains("base")
                        || key_lower.contains("limit")
                        || key_lower.contains("rva")
                        || key_lower.contains("offset") && value.is_number();

                    // Detect fields that should be numeric (daddr, saddr, port, etc.)
                    let should_be_numeric = key_lower.contains("addr")
                        || key_lower.contains("port")
                        || key_lower.contains("pid")
                        || key_lower.contains("tid")
                        || key_lower.contains("size");

                    // Smart conversion: keep small numbers as numbers, convert problematic ones
                    let converted_value = match value {
                        serde_json::Value::Number(n) => {
                            if is_pointer_field {
                                // Pointer/address field: always convert to hex string
                                if let Some(u) = n.as_u64() {
                                    json!(format!("0x{:X}", u))
                                } else if let Some(i) = n.as_i64() {
                                    json!(format!("0x{:X}", i))
                                } else {
                                    json!(n.to_string())
                                }
                            } else {
                                // Non-pointer field: check range
                                if let Some(u) = n.as_u64() {
                                    if u > i64::MAX as u64 {
                                        // Exceeds i64 range - convert to string
                                        json!(format!("0x{:X}", u))
                                    } else {
                                        // Safe range - keep as number for Kibana aggregations
                                        json!(u)
                                    }
                                } else if let Some(i) = n.as_i64() {
                                    // Signed integer in safe range
                                    json!(i)
                                } else if let Some(f) = n.as_f64() {
                                    // Float - keep as-is
                                    json!(f)
                                } else {
                                    // Fallback to string
                                    json!(n.to_string())
                                }
                            }
                        }
                        serde_json::Value::String(s) => {
                            // If field should be numeric but contains "unsupported" or other non-numeric string,
                            // convert to null to avoid Elasticsearch type conflicts
                            if should_be_numeric && (s == "unsupported" || s.parse::<i64>().is_err()) {
                                json!(null)
                            } else {
                                json!(s)
                            }
                        }
                        serde_json::Value::Bool(b) => json!(b),
                        serde_json::Value::Null => json!(null),
                        serde_json::Value::Array(arr) => json!(arr),
                        serde_json::Value::Object(obj) => json!(obj),
                    };

                    // Prefix payload fields to avoid conflicts with top-level fields
                    obj.insert(format!("payload_{}", key), converted_value);
                }
            }

            let doc_str = serde_json::to_string(&doc).unwrap_or_default();

            let response = self
                .es_client
                .index(IndexParts::Index(&index_name))
                .body(doc)
                .send()
                .await;

            match response {
                Ok(resp) if resp.status_code().is_success() => {
                    indexed += 1;
                }
                Ok(resp) => {
                    let status = resp.status_code();
                    let body = resp.text().await.unwrap_or_default();
                    warn!("Failed to index event: status {} - {}", status, body);

                    // Log the problematic document for debugging (first occurrence only)
                    if indexed == 0 {
                        warn!("Problematic document: {}", doc_str);
                    }
                }
                Err(e) => {
                    warn!("Failed to index event: {}", e);
                }
            }
        }

        info!(
            "Indexed {}/{} telemetry events to {}",
            indexed,
            batch.len(),
            index_name
        );

        Ok(())
    }

    /// Store run result to Elasticsearch
    /// Tries worker IP first, falls back to configured localhost if that fails
    async fn store_run_result(
        &self,
        report: &StatusReport,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use serde_json::json;

        let index_name = format!("runs-{}", chrono::Utc::now().format("%Y.%m"));

        let doc = json!({
            "run_id": report.run_id,
            "job_id": report.job_id,
            "worker_id": report.worker_id,
            "worker_ip": report.worker_ip,
            "artifact_name": report.artifact_name,
            "pid": report.pid,
            "status": report.event_type,
            "elapsed_seconds": report.elapsed_seconds,
            "telemetry_events_count": report.telemetry_events_count,
            "details": report.details,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        // Strategy: Try controller IP first (Elasticsearch runs on controller),
        // then fall back to configured localhost
        let controller_es_url = format!("http://{}:9200", self.controller_ip);

        // Try controller IP first (as seen from worker's network perspective)
        info!("Attempting to store run result to Elasticsearch at controller IP: {}", controller_es_url);
        match self.try_index_to_es(&controller_es_url, &index_name, &report.run_id, &doc).await {
            Ok(()) => {
                info!(
                    "[OK] Stored run result to controller ES ({}): {} (status: {})",
                    self.controller_ip, report.run_id, report.event_type
                );
                return Ok(());
            }
            Err(e) => {
                warn!(
                    "Failed to store to controller ES ({}): {} - falling back to localhost",
                    controller_es_url, e
                );
            }
        }

        // Fallback to configured localhost client
        info!("Falling back to configured Elasticsearch client (localhost)");
        let response = self
            .es_client
            .index(IndexParts::IndexId(&index_name, &report.run_id))
            .body(doc)
            .send()
            .await?;

        if response.status_code().is_success() {
            info!(
                "[OK] Stored run result to localhost ES: {} (status: {})",
                report.run_id, report.event_type
            );
        } else {
            warn!(
                "Index run result returned non-success status: {}",
                response.status_code()
            );
        }

        Ok(())
    }

    /// Try to index document to a specific Elasticsearch URL
    async fn try_index_to_es(
        &self,
        es_url: &str,
        index_name: &str,
        doc_id: &str,
        doc: &serde_json::Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Create temporary ES client for this specific URL
        let es_transport = Transport::single_node(es_url)?;
        let es_client = Elasticsearch::new(es_transport);

        let response = es_client
            .index(IndexParts::IndexId(index_name, doc_id))
            .body(doc.clone())
            .send()
            .await?;

        if response.status_code().is_success() {
            Ok(())
        } else {
            Err(format!(
                "Elasticsearch returned non-success status: {}",
                response.status_code()
            )
            .into())
        }
    }
}

/// Detect the controller's IP address for network communication
/// Returns the first non-loopback IPv4 address found
fn detect_controller_ip() -> Option<String> {
    use std::net::IpAddr;

    // Try to detect by connecting to a known external address
    // This works even without actual internet connectivity
    // The connect() call doesn't send packets, just selects the appropriate local interface
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        // Connect to a public DNS server (doesn't actually send packets)
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(local_addr) = socket.local_addr() {
                if let IpAddr::V4(ipv4) = local_addr.ip() {
                    if !ipv4.is_loopback() {
                        info!("Detected controller IP via routing table: {}", ipv4);
                        return Some(ipv4.to_string());
                    }
                }
            }
        }
    }

    // If all else fails, fallback to localhost
    warn!("Could not auto-detect controller IP, falling back to localhost");
    None
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing with INFO level (visible in both debug and release builds)
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Load generated TOML config (auto-finds in standard locations)
    // Search order:
    //   1. AUTOMUTATE_CONTROLLER_CONFIG env var
    //   2. ~/automutate/config/controller.toml (WSL2 deployment default)
    //   3. automation/generated/controller.toml (generated from config.yaml)
    //   4. config/controller.toml (local development)
    //   5. automation/templates/controller.toml (template fallback)
    let config = ControllerConfig::load().unwrap_or_else(|e| {
        eprintln!("Failed to load controller.toml: {}", e);
        eprintln!("Run 'automation/scripts/generate-configs.ps1' to create config files");
        eprintln!("Or set AUTOMUTATE_CONTROLLER_CONFIG environment variable");
        std::process::exit(1);
    });

    info!(
        "Loaded controller config successfully from {}",
        ControllerConfig::find_config_path()
    );
    info!("Bind address: {}", config.server.bind_address);
    info!("Elasticsearch: {}", config.elasticsearch.url);
    info!(
        "Triage model: {} (threshold: {})",
        config.triage.model_type, config.triage.confidence_threshold
    );

    // Create Elasticsearch client
    let es_transport = Transport::single_node(&config.elasticsearch.url)?;
    let es_client = Elasticsearch::new(es_transport);

    info!(
        "Elasticsearch client initialized: {}",
        config.elasticsearch.url
    );

    let addr = config.server.bind_address.parse()?;

    // Extract controller IP from bind address (e.g., "0.0.0.0:50051" or "10.200.200.1:50051")
    // If bind address is 0.0.0.0, try to detect actual IP
    let controller_ip = if config.server.bind_address.starts_with("0.0.0.0") {
        // Try to detect the actual network IP
        detect_controller_ip().unwrap_or_else(|| "127.0.0.1".to_string())
    } else {
        // Extract IP from bind address
        config.server.bind_address
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string()
    };

    info!("Controller IP for Elasticsearch access: {}", controller_ip);

    let mut scheduler = SchedulerService::new(es_client, controller_ip);

    info!("Controller/Scheduler starting...");

    // Create scheduler core configuration from controller config
    let scheduler_core_config = CoreSchedulerConfig {
        poll_interval_seconds: 5,
        max_concurrent_jobs: config.scheduler.max_concurrent_runs_per_worker as usize,
        default_timeout_seconds: config.scheduler.run_timeout_secs,
        health_timeout_seconds: 30, // Default health check timeout
    };

    // Create and spawn scheduler core
    match create_scheduler_core(scheduler_core_config) {
        Ok(scheduler_core) => {
            info!("Scheduler core initialized successfully");

            // Set scheduler core in service (for gRPC methods)
            scheduler.set_scheduler_core(Arc::clone(&scheduler_core));

            // Spawn scheduler core in background task
            tokio::spawn(async move {
                scheduler_core.run().await;
            });
        }
        Err(e) => {
            warn!("Failed to initialize scheduler core: {}", e);
            warn!("gRPC server will still start, but job scheduling will not be available");
        }
    }

    // gRPC reflection for grpcurl
    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(tonic::include_file_descriptor_set!("edr_descriptor"))
        .build_v1()?;

    Server::builder()
        .add_service(ControllerServer::new(scheduler))
        .add_service(reflection_service)
        .serve(addr)
        .await?;

    Ok(())
}
