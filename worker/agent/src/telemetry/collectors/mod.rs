/// Telemetry collectors
///
/// Modules that collect telemetry from various sources:
/// - RedEDR HTTP API (rededr)
/// - Line-level tracing via named pipe (trace)
/// - ETW (future)
/// - Event Logs (future)
/// - Defender alerts (future)
pub mod rededr;
pub mod trace;
