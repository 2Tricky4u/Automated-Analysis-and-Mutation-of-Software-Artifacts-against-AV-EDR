//! Instrumentation module - Code instrumentation for tracing and coverage
//!
//! Provides line-level tracing and basic-block coverage instrumentation.

pub mod line_tracer;
pub mod instrumenter;

// Re-exports
pub use line_tracer::{inject_line_traces, inject_line_traces_with_opts, SourceLanguage, TraceFormat};
pub use instrumenter::Instrumenter;