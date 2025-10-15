/// External telemetry exporters for VPN/RPC integration
///
/// Allows worker VMs to send telemetry to external systems over VPN:
/// - Cortex: Prometheus-compatible time series database
/// - MDE: Microsoft Defender for Endpoint custom detections
/// - Custom HTTP: Generic HTTP endpoint for custom collectors

pub mod cortex;
pub mod mde;

pub use cortex::CortexExporter;
pub use mde::MdeExporter;
