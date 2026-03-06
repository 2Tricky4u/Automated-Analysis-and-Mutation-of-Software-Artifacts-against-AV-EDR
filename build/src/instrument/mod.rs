//! Code instrumentation for tracing and coverage.
//!
//! Provides two injection strategies used by the two-run differential protocol:
//!
//! | Run   | Trace Mode     | Purpose                              |
//! |-------|---------------|--------------------------------------|
//! | Run A | `lines` / `bb` | Execution path, truncation localization |
//! | Run B | `off`          | Ground-truth EDR behavior            |
//!
//! By comparing detection outcomes between Run A and Run B, the triage engine
//! can distinguish real detections from instrumentation artifacts.
//!
//! # Submodules
//!
//! - [`instrumenter`] — LLVM IR-level BB coverage (SanitizerCoverage) and API tracing
//! - [`line_tracer`] — AST-level line tracing via tree-sitter (C/C++ source injection)

pub mod instrumenter;
pub mod line_tracer;

// Re-exports
pub use instrumenter::Instrumenter;
pub use line_tracer::{
    DEFAULT_DELAY_ITERATIONS, SourceLanguage, TraceFormat, inject_line_traces,
    inject_line_traces_with_delay, inject_line_traces_with_opts, inject_line_traces_with_path,
};
