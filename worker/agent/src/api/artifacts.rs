use crate::automutate::common::ArtifactChunk;
use crate::automutate::worker::TransferAck;
use crate::WorkerAgentService;
use tonic::{Request, Response, Status};
use tracing::info;

pub async fn send_artifact(
    service: &WorkerAgentService,
    request: Request<tonic::Streaming<ArtifactChunk>>,
) -> Result<Response<TransferAck>, Status> {
    use sha2::{Digest, Sha256};

    let mut stream = request.into_inner();
    let mut chunks = Vec::new();
    let mut artifact_id = String::new();
    let mut expected_sha256 = String::new();

    info!("Starting artifact transfer...");

    // Receive all chunks
    while let Some(chunk) = stream.message().await? {
        if artifact_id.is_empty() {
            artifact_id = chunk.artifact_id.clone();
            expected_sha256 = chunk.sha256.clone();
            info!(
                "Receiving artifact: id={}, total_chunks={}",
                artifact_id, chunk.total_chunks
            );
        }
        chunks.push(chunk);
    }

    if chunks.is_empty() {
        return Err(Status::invalid_argument("No chunks received"));
    }

    // Sort chunks by index
    chunks.sort_by_key(|c| c.chunk_index);

    // Reassemble binary
    let file_data: Vec<u8> = chunks.iter().flat_map(|c| c.data.clone()).collect();

    info!(
        "Reassembled artifact: {} bytes from {} chunks",
        file_data.len(),
        chunks.len()
    );

    // Verify integrity
    let mut hasher = Sha256::new();
    hasher.update(&file_data);
    let actual_sha256 = format!("{:x}", hasher.finalize());

    if actual_sha256 != expected_sha256 {
        return Err(Status::data_loss(format!(
            "SHA256 mismatch: expected {}, got {}",
            expected_sha256, actual_sha256
        )));
    }

    // Write to disk (artifacts directory)
    let artifacts_dir = std::path::Path::new(&service.config.storage.artifacts_path);
    std::fs::create_dir_all(artifacts_dir).map_err(|e| {
        Status::internal(format!("Failed to create artifacts directory: {}", e))
    })?;

    let artifact_path = artifacts_dir.join(format!("{}.exe", artifact_id));
    std::fs::write(&artifact_path, &file_data)
        .map_err(|e| Status::internal(format!("Failed to write artifact to disk: {}", e)))?;

    info!(
        "Artifact stored: {} ({} bytes) at {:?}",
        artifact_id,
        file_data.len(),
        artifact_path
    );

    Ok(Response::new(TransferAck {
        received: true,
        chunks_received: chunks.len() as u32,
        error: String::new(),
        storage_path: artifact_path.to_string_lossy().to_string(),
    }))
}
