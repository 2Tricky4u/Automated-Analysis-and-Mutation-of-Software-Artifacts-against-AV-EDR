//! WebSocket handler for real-time telemetry streaming.
//!
//! Proxies telemetry events from Controller to WebSocket clients.

use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::Response,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// Telemetry event for WebSocket clients
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub event_type: String,
    pub job_id: String,
    pub timestamp: String,
    pub payload: serde_json::Value,
}

/// Manages WebSocket connections and broadcasts telemetry
#[derive(Clone)]
pub struct TelemetryBroadcaster {
    sender: broadcast::Sender<TelemetryEvent>,
}

impl TelemetryBroadcaster {
    /// Create new broadcaster with given channel capacity
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Subscribe to telemetry events
    pub fn subscribe(&self) -> broadcast::Receiver<TelemetryEvent> {
        self.sender.subscribe()
    }

    /// Broadcast telemetry event to all connected clients
    pub fn broadcast(&self, event: TelemetryEvent) {
        // send() returns error if no receivers, which is fine
        let _ = self.sender.send(event);
    }

    /// Get number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

/// WebSocket upgrade handler
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(broadcaster): State<Arc<TelemetryBroadcaster>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, broadcaster))
}

/// Handle individual WebSocket connection
async fn handle_socket(socket: WebSocket, broadcaster: Arc<TelemetryBroadcaster>) {
    let (mut sender, mut receiver) = socket.split();

    // Subscribe to telemetry broadcast
    let mut rx = broadcaster.subscribe();

    info!(
        "WebSocket client connected (total: {})",
        broadcaster.subscriber_count()
    );

    // Spawn task to forward telemetry to this client
    let send_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            match serde_json::to_string(&event) {
                Ok(json) => {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break; // Client disconnected
                    }
                }
                Err(e) => {
                    error!("Failed to serialize telemetry event: {}", e);
                }
            }
        }
    });

    // Handle incoming messages (for filtering, commands, etc.)
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                debug!("WebSocket received: {}", text);
                // Could handle filter commands here
                // e.g., {"filter": {"job_id": "job-123"}}
            }
            Ok(Message::Close(_)) => {
                debug!("WebSocket client requested close");
                break;
            }
            Ok(Message::Ping(_)) => {
                // Pong is sent automatically by axum
                debug!("WebSocket ping received");
            }
            Err(e) => {
                warn!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    // Clean up
    send_task.abort();
    info!(
        "WebSocket client disconnected (remaining: {})",
        broadcaster.subscriber_count().saturating_sub(1)
    );
}

/// WebSocket handler with job_id filter
#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub job_id: Option<String>,
}

pub async fn ws_handler_filtered(
    ws: WebSocketUpgrade,
    axum::extract::Query(query): axum::extract::Query<WsQuery>,
    State(broadcaster): State<Arc<TelemetryBroadcaster>>,
) -> Response {
    let job_id_filter = query.job_id;
    ws.on_upgrade(move |socket| handle_socket_filtered(socket, broadcaster, job_id_filter))
}

async fn handle_socket_filtered(
    socket: WebSocket,
    broadcaster: Arc<TelemetryBroadcaster>,
    job_id_filter: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = broadcaster.subscribe();

    info!(
        "WebSocket client connected with filter: {:?} (total: {})",
        job_id_filter,
        broadcaster.subscriber_count()
    );

    let filter = job_id_filter.clone();
    let send_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            // Apply filter if specified
            if let Some(ref filter_id) = filter
                && event.job_id != *filter_id {
                    continue;
                }

            match serde_json::to_string(&event) {
                Ok(json) => {
                    if sender.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    error!("Failed to serialize telemetry event: {}", e);
                }
            }
        }
    });

    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Close(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }

    send_task.abort();
    info!("WebSocket client disconnected");
}
