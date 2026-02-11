use crate::automutate::common::{ControllerMessage, WorkerMessage};
use crate::{WorkerAgentService, capabilities, session};
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{error, info};

/// Bidirectional stream for real-time communication
/// Controller initiates the stream, both sides can send messages
pub async fn establish_stream(
    service: &WorkerAgentService,
    request: Request<tonic::Streaming<ControllerMessage>>,
) -> Result<Response<tokio_stream::wrappers::ReceiverStream<Result<WorkerMessage, Status>>>, Status>
{
    use crate::session::worker_state::WorkerState;
    use session::stream_handler::StreamHandler;
    use tokio::sync::RwLock;

    info!("EstablishStream RPC called - establishing bidirectional stream with controller");

    // Detect current capabilities
    let capabilities_data = capabilities::detect_capabilities()
        .await
        .map_err(|e| Status::internal(format!("Failed to detect capabilities: {}", e)))?;

    // Create worker state
    let worker_state = WorkerState::new(service.worker_id.clone(), capabilities_data);
    let worker_state = Arc::new(RwLock::new(worker_state));

    // Create stream handler with individual fields (no Arc<WorkerAgentService> → breaks cycle)
    let (handler, rx) = StreamHandler::new(
        worker_state.clone(),
        service.worker_id.clone(),
        service.config.clone(),
        service.execution_lock.clone(),
    );
    let handler = Arc::new(handler);

    // Store handler in service for use by run_sample() (unary RPC path)
    {
        let mut stream_handler_lock = service.stream_handler.write().await;
        *stream_handler_lock = Some(handler.clone());
    }
    info!("StreamHandler stored in service - available for ExecutionMonitor");

    // Send registration immediately
    if let Err(e) = handler.send_registration().await {
        error!("Failed to send registration: {}", e);
        return Err(Status::internal(format!(
            "Failed to send registration: {}",
            e
        )));
    }

    info!("Sent worker registration to controller");

    // Spawn task to handle incoming controller messages
    let stream = request.into_inner();
    let handler_clone = handler.clone();
    tokio::spawn(async move {
        if let Err(e) = handler_clone.handle_stream(stream).await {
            error!("Stream handler error: {}", e);
        }
        info!("Controller stream handler terminated");
    });

    // Spawn heartbeat task
    let handler_clone = handler.clone();
    tokio::spawn(async move {
        session::stream_handler::heartbeat_loop(handler_clone, 30).await;
    });

    info!("Bidirectional stream established successfully");

    // Return outgoing message stream
    let out_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Ok(Response::new(out_stream))
}
