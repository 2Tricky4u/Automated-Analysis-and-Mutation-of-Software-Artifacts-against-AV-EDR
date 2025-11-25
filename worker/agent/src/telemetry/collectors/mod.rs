/// Telemetry collectors
///
/// Modules that collect telemetry from various sources:
/// - RedEDR HTTP API
/// - Line-level tracing (named pipe)
/// - ETW (future)
/// - Event Logs (future)
/// - Defender alerts (future)
pub mod rededr;
pub mod trace;
