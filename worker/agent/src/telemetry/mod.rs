/// Telemetry collection and export module
///
/// Handles:
/// - Local telemetry collection (ETW, Event Logs, Defender)
/// - External telemetry export (Cortex, MDE, custom HTTP)

pub mod exporters;
