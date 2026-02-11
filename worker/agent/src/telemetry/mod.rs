/// Telemetry collection and export module
///
/// Handles:
/// - Local telemetry collection (ETW, Event Logs, Defender, RedEDR)
/// - External telemetry export (Cortex, MDE, custom HTTP)
pub mod collectors;
pub mod pipeline;
pub mod trace_compressor;
