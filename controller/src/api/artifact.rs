use crate::api::SchedulerService;
use crate::automutate::controller::{BuildRequest, BuildResponse, DeployRequest, DeployResponse};
use tonic::{Request, Response, Status};
use tracing::{debug, error, info};

pub async fn build_artifact(
    _service: &SchedulerService,
    request: Request<BuildRequest>,
) -> Result<Response<BuildResponse>, Status> {
    let req = request.into_inner();

    // Require modular_build spec — legacy SourceFile path has been removed
    let modular = req.modular_build.ok_or_else(|| {
        Status::invalid_argument(
            "modular_build field is required. Legacy template_name/source_file builds are no longer supported.",
        )
    })?;

    // Extract trace_mode with default (backwards compatibility)
    let trace_mode = if req.trace_mode.is_empty() {
        "lines".to_string()
    } else {
        req.trace_mode.clone()
    };

    info!(
        "Build request (modular): encoding={}, mutations={}, trace_mode={}",
        modular.encoding,
        req.mutations.len(),
        trace_mode
    );

    // Create builder with default config
    let builder_config = build::BuilderConfig::default();
    let artifact_builder = build::ArtifactBuilder::new(builder_config).map_err(|e| {
        error!("Failed to create artifact builder: {}", e);
        Status::internal(format!("Failed to create builder: {}", e))
    })?;

    // Convert proto mutations to build::mutator::MutationSpec
    let mutations: Vec<build::mutator::MutationSpec> = req
        .mutations
        .into_iter()
        .map(|m| build::mutator::MutationSpec {
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

    // Convert proto ModuleSelection to build crate's ModuleSelection
    let proto_modules = modular.modules.unwrap_or_default();
    let modules = build::ModuleSelection {
        carrier: if proto_modules.carrier.is_empty() {
            "change_rw_rx".into()
        } else {
            proto_modules.carrier
        },
        decoder: if proto_modules.decoder.is_empty() {
            "xor".into()
        } else {
            proto_modules.decoder
        },
        antiemulation: if proto_modules.antiemulation.is_empty() {
            "none".into()
        } else {
            proto_modules.antiemulation
        },
        deconditioner: if proto_modules.deconditioner.is_empty() {
            "none".into()
        } else {
            proto_modules.deconditioner
        },
        guardrail: if proto_modules.guardrail.is_empty() {
            "none".into()
        } else {
            proto_modules.guardrail
        },
        virtualprotect: if proto_modules.virtualprotect.is_empty() {
            "standard".into()
        } else {
            proto_modules.virtualprotect
        },
        decoy: if proto_modules.decoy.is_empty() {
            "none".into()
        } else {
            proto_modules.decoy
        },
    };

    // Parse encoding type (default to XOR)
    let encoding = std::str::FromStr::from_str(if modular.encoding.is_empty() {
        "xor"
    } else {
        &modular.encoding
    })
    .unwrap_or(build::EncodingType::Xor);

    // Build the artifact using ModularTemplate
    let built = artifact_builder
        .build(build::BuildInput::ModularTemplate {
            modules,
            payload: modular.payload,
            encoding,
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

    // TODO Index artifact metadata to Elasticsearch

    Ok(Response::new(BuildResponse {
        artifact_id: built.artifact_id,
        size_bytes: built.size_bytes,
        build_status: "success".to_string(),
        error: String::new(),
        storage_path: built.output_path.to_string_lossy().to_string(),
        build_timestamp: built.build_timestamp.timestamp(),
        mutations_applied: built.mutations_applied,
        trace_mode,
    }))
}

pub async fn deploy_artifact(
    _service: &SchedulerService,
    request: Request<DeployRequest>,
) -> Result<Response<DeployResponse>, Status> {
    use crate::automutate::worker::worker_agent_client::WorkerAgentClient;
    use futures::stream;
    use sha2::{Digest, Sha256};

    let req = request.into_inner();

    info!(
        "Deploy request: artifact_id={}, worker={}",
        req.artifact_id, req.worker_address
    );

    // 1. Read artifact from disk
    let builder_config = build::BuilderConfig::default();
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

    debug!("Successfully connected to worker");

    // 4. Split into chunks (4MB per chunk)
    let chunks = crate::dispatch::types::chunk_artifact(&req.artifact_id, &artifact_data);

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
