//! Instrumentation module - Code instrumentation for tracing and coverage
//!
//! Provides line-level tracing and basic-block coverage instrumentation.

pub mod instrumenter;
pub mod line_tracer;

// Re-exports
pub use instrumenter::Instrumenter;
pub use line_tracer::{
    DEFAULT_DELAY_ITERATIONS, SourceLanguage, TraceFormat, inject_line_traces,
    inject_line_traces_with_delay, inject_line_traces_with_opts,
};
