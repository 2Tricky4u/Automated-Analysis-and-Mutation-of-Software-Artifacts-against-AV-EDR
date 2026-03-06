//! Telemetry collector implementations.
//!
//! Each collector targets a specific telemetry source and normalizes events
//! into the common [`TelemetryData`](crate::automutate::common::TelemetryData) protobuf envelope.
pub mod rededr;
pub mod trace;
