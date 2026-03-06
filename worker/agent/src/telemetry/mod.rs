//! Telemetry collection and packaging.
//!
//! Collects execution telemetry from multiple sources:
//! - **RedEDR**: ETW kernel callbacks via HTTP API polling
//! - **Named pipe trace**: Line-level execution path from instrumented artifacts
//! - **BB coverage**: Basic-block hit counts from SanitizerCoverage
//! - **API checkpoints**: Runtime milestone events from the artifact
pub mod collectors;
pub mod pipeline;
// TODO: finish trace_compressor pipeline — connect to telemetry collector output
//       and wire compressed traces into triage token extraction
#[allow(dead_code)]
pub mod trace_compressor;
