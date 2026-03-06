//! Session management for controller-worker communication.
//!
//! Manages the bidirectional gRPC stream lifecycle, worker runtime state,
//! and periodic heartbeat reporting.
pub mod stream_handler;
pub mod worker_state;
