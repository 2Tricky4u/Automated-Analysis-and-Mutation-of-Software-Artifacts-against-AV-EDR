use edr_config::ControllerConfig;
use elasticsearch::{Elasticsearch, IndexParts, http::transport::Transport};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status, transport::Server};
use tracing::{error, info, warn};

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
    JobStatusRequest, JobStatusResponse, PingRequest, PingResponse, QueryRequest, QueryResponse,
    StatusAck, StatusReport, TriageRequest, TriageResponse,
    controller_server::{Controller, ControllerServer},
};

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

#[derive(Debug, Clone)]
pub struct SchedulerService {
    state: Arc<Mutex<SchedulerState>>,
    es_client: Elasticsearch,
}

impl SchedulerService {
    pub fn new(es_client: Elasticsearch) -> Self {
        Self {
            state: Arc::new(Mutex::new(SchedulerState::default())),
            es_client,
        }
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
        let mut state = self.state.lock().await;

        state.job_counter += 1;
        let job_id = format!("job-{:06}", state.job_counter);

        let job = Job {
            id: job_id.clone(),
            name: req.name.clone(),
            status: "queued".to_string(),
            progress: 0,
            phase: "initializing".to_string(),
            logs: vec![format!("Job {} created", job_id)],
        };

        state.jobs.insert(job_id.clone(), job);

        info!("Scheduled job: {} ({})", job_id, req.name);

        Ok(Response::new(JobResponse {
            job_id: Some(JobId {
                value: job_id.clone(),
            }),
            accepted: true,
            message: format!("Job {} scheduled successfully", job_id),
            estimated_duration_seconds: 300,
        }))
    }

    async fn get_job_status(
        &self,
        request: Request<JobStatusRequest>,
    ) -> Result<Response<JobStatusResponse>, Status> {
        let req = request.into_inner();
        let state = self.state.lock().await;

        match state.jobs.get(&req.job_id) {
            Some(job) => Ok(Response::new(JobStatusResponse {
                job_id: job.id.clone(),
                status: job.status.clone(),
                progress_percent: job.progress,
                current_phase: job.phase.clone(),
                logs: job.logs.clone(),
            })),
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
        let mut stream = request.into_inner();
        let mut events_count = 0;
        let mut first_job_id = String::new();
        let mut batch = Vec::new();

        info!("Telemetry stream opened");

        // Collect all events from stream
        while let Some(telemetry) = stream.message().await? {
            if events_count == 0 {
                first_job_id = telemetry.job_id.clone();
            }

            events_count += 1;
            batch.push(telemetry);
        }

        info!(
            "Telemetry batch received: job={}, events_count={}",
            first_job_id, events_count
        );

        // Index batch to Elasticsearch
        if !batch.is_empty() {
            if let Err(e) = self.index_telemetry_batch(&batch).await {
                error!("Failed to index telemetry batch: {}", e);
                // Don't fail the RPC - telemetry is logged even if indexing fails
            }
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
        let report = request.into_inner();

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
                tracing::warn!("{}", status_line);
            }
            _ => {
                info!("{}", status_line);
            }
        }

        // Store RunResult to Elasticsearch when final status received
        if matches!(report.event_type.as_str(), "success" | "error" | "timeout") {
            if let Err(e) = self.store_run_result(&report).await {
                error!("Failed to store run result: {}", e);
            }
        }

        Ok(Response::new(StatusAck { received: true }))
    }

    async fn build_artifact(
        &self,
        request: Request<BuildRequest>,
    ) -> Result<Response<BuildResponse>, Status> {
        let req = request.into_inner();

        info!(
            "Build request: template={}, source={}",
            req.template_name, req.source_file
        );

        // Create builder with default config
        let builder_config = builder::BuilderConfig::default();
        let artifact_builder = builder::ArtifactBuilder::new(builder_config).map_err(|e| {
            error!("Failed to create artifact builder: {}", e);
            Status::internal(format!("Failed to create builder: {}", e))
        })?;

        // Build the artifact
        let built = artifact_builder
            .build_template(&req.template_name, &req.source_file)
            .await
            .map_err(|e| {
                error!("Build failed: {}", e);
                Status::internal(format!("Build failed: {}", e))
            })?;

        info!(
            "Build successful: artifact_id={}, size={} bytes",
            built.artifact_id, built.size_bytes
        );

        // TODO (Phase 2): Optionally apply mutations via Selector + Mutator
        // TODO (Phase 3): Index artifact metadata to Elasticsearch

        Ok(Response::new(BuildResponse {
            artifact_id: built.artifact_id,
            size_bytes: built.size_bytes,
            build_status: "success".to_string(),
            error: String::new(),
            storage_path: built.output_path.to_string_lossy().to_string(),
            build_timestamp: built.build_timestamp.timestamp(),
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
                return Err(Status::invalid_argument(format!("Invalid worker address: {}", e)));
            }
        };

        info!("Endpoint created, connecting...");
        let mut client = WorkerAgentClient::connect(endpoint)
            .await
            .map_err(|e| {
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
            let payload_fields = if let Ok(payload_json) =
                serde_json::from_slice::<serde_json::Value>(&event.payload)
            {
                payload_json.as_object().cloned().unwrap_or_default()
            } else {
                Default::default()
            };

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
                        serde_json::Value::String(s) => json!(s),
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

        let response = self
            .es_client
            .index(IndexParts::IndexId(&index_name, &report.run_id))
            .body(doc)
            .send()
            .await?;

        if response.status_code().is_success() {
            info!(
                "Stored run result: {} (status: {})",
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
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

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
    let scheduler = SchedulerService::new(es_client);

    info!("Controller/Scheduler starting...");

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
