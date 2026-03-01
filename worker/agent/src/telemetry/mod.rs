/// Telemetry collection and export module
///
/// Handles:
/// - Local telemetry collection (ETW, Event Logs, Defender, RedEDR)
/// - External telemetry export (Cortex, MDE, custom HTTP)
pub mod collectors;
pub mod pipeline;
// TODO: finish trace_compressor pipeline — connect to telemetry collector output
//       and wire compressed traces into triage token extraction
#[allow(dead_code)]
pub mod trace_compressor;
