//! Build System - Unified artifact compilation, mutation, and instrumentation
//!
//! This crate provides a complete build pipeline for creating Windows PE executables
//! with support for AST/IR mutations, instrumentation, and template-based assembly.
//!
//! # Architecture
//!
//! ```text
//! Source → Transform (AST/IR) → Instrument → Compile → Artifact
//! ```
//!
//! # Modules
//!
//! - [`transform`] - AST and IR mutations
//! - [`instrument`] - Line tracing, BB coverage
//! - [`template`] - @MODULE assembly, payload encoding
//! - [`mutator`] - Mutation specification and registry
//! - [`builder`] - High-level artifact builder (main API)
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use build::{ArtifactBuilder, BuilderConfig, BuildInput};
//!
//! let config = BuilderConfig::default();
//! let builder = ArtifactBuilder::new(config)?;
//!
//! let artifact = builder.build(BuildInput::SourceFile {
//!     template_name: "eicar_test".to_string(),
//!     source_file: "eicar_test.c".to_string(),
//!     mutations: vec![],
//!     trace_mode: "off".to_string(),
//! }).await?;
//! ```

use serde::{Deserialize, Serialize};

// ============================================================================
// Submodules (new organization)
// ============================================================================

pub mod builder;
pub mod compiler;
pub mod instrument;
pub mod mutator;
pub mod template;
pub mod transform;

// ============================================================================
// Re-exports from submodules
// ============================================================================

// Transform module
pub use transform::{AstMutator, IrMutator};

// Instrument module
pub use instrument::{
    Instrumenter, SourceLanguage, TraceFormat, inject_line_traces, inject_line_traces_with_opts,
};

// Template module
pub use template::{
    Assembler, EncodedPayload, EncodingType, ModuleSelection, MutationMarker, PayloadEncoder,
};
pub use template::{extract_mutation_markers, generate_test_payload, strip_mutation_markers};

// Builder module (main API)
pub use builder::{ArtifactBuilder, BuildInput, BuilderConfig, BuiltArtifact};

// ============================================================================
// Core Types
// ============================================================================

/// Trace instrumentation mode (CLAUDE.md Section 4)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceMode {
    /// No instrumentation
    Off,
    /// API tracing only
    Api,
    /// Basic-block coverage only
    BB,
    /// API tracing + BB coverage (DEFAULT for mutation loop)
    #[serde(rename = "api+bb")]
    ApiPlusBB,
    /// Line-level tracing (diagnostic mode, baseline only)
    Lines,
    /// Targeted line tracing around specific BB (narrowing mode)
    #[serde(rename = "lines-around-bb")]
    LinesAroundBB(u32),
    /// All instrumentation (debug mode)
    All,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_mode_serialization() {
        let modes = vec![
            TraceMode::Off,
            TraceMode::Api,
            TraceMode::BB,
            TraceMode::ApiPlusBB,
            TraceMode::Lines,
            TraceMode::All,
        ];
        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: TraceMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, deserialized);
        }
    }
}
