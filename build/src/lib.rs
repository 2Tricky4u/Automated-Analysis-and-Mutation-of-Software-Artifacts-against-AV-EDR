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
//! use build::{ArtifactBuilder, BuilderConfig, BuildInput, EncodingType, ModuleSelection};
//!
//! let config = BuilderConfig::default();
//! let builder = ArtifactBuilder::new(config)?;
//!
//! let artifact = builder.build(BuildInput::ModularTemplate {
//!     modules: ModuleSelection::default(),
//!     payload: shellcode_bytes,
//!     encoding: EncodingType::Xor,
//!     mutations: vec![],
//!     trace_mode: "off".to_string(),
//!     mutation_targets: vec![],
//!     sc_checkpoint_count: None,
//!     precomputed_payload: None,
//! }).await?;
//! ```

use serde::{Deserialize, Serialize};

// --- Submodules ---

pub mod builder;
pub mod instrument;
pub mod msvc_compat;
pub mod mutator;
//pub mod pe_inject;
pub mod template;
pub mod transform;

// --- Re-exports ---

// Transform module
pub use transform::{AstMutator, BinaryMutator, IrMutator};

// Instrument module
pub use instrument::{
    DEFAULT_DELAY_ITERATIONS, Instrumenter, SourceLanguage, TraceFormat, inject_line_traces,
    inject_line_traces_with_delay, inject_line_traces_with_opts,
};

// Template module
pub use template::{
    Assembler, EncodedPayload, EncodingType, ModuleSelection, MutationMarker, PayloadEncoder,
};
pub use template::{extract_mutation_markers, generate_test_payload, strip_mutation_markers};

// Builder module (main API)
pub use builder::{
    ArtifactBuilder, BuildInput, BuilderConfig, BuiltArtifact, PreparedPayload, prepare_payload,
};

// MSVC compat module
pub use msvc_compat::MsvcCompat;

// --- Core Types ---

/// Trace instrumentation mode for the two-run differential protocol.
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

impl std::str::FromStr for TraceMode {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "off" => Ok(TraceMode::Off),
            "api" => Ok(TraceMode::Api),
            "bb" => Ok(TraceMode::BB),
            "api+bb" => Ok(TraceMode::ApiPlusBB),
            "lines" => Ok(TraceMode::Lines),
            "all" => Ok(TraceMode::All),
            other => {
                // Try "lines-around-bb:<N>" format
                if let Some(n) = other.strip_prefix("lines-around-bb:") {
                    n.parse::<u32>()
                        .map(TraceMode::LinesAroundBB)
                        .map_err(|e| format!("Invalid BB id in '{}': {}", other, e))
                } else {
                    Err(format!("Unknown trace mode: '{}'", other))
                }
            }
        }
    }
}

impl std::fmt::Display for TraceMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceMode::Off => write!(f, "off"),
            TraceMode::Api => write!(f, "api"),
            TraceMode::BB => write!(f, "bb"),
            TraceMode::ApiPlusBB => write!(f, "api+bb"),
            TraceMode::Lines => write!(f, "lines"),
            TraceMode::LinesAroundBB(n) => write!(f, "lines-around-bb:{}", n),
            TraceMode::All => write!(f, "all"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_mode_from_str() {
        assert_eq!("off".parse::<TraceMode>().unwrap(), TraceMode::Off);
        assert_eq!("api".parse::<TraceMode>().unwrap(), TraceMode::Api);
        assert_eq!("bb".parse::<TraceMode>().unwrap(), TraceMode::BB);
        assert_eq!("api+bb".parse::<TraceMode>().unwrap(), TraceMode::ApiPlusBB);
        assert_eq!("lines".parse::<TraceMode>().unwrap(), TraceMode::Lines);
        assert_eq!("all".parse::<TraceMode>().unwrap(), TraceMode::All);
        assert_eq!(
            "lines-around-bb:42".parse::<TraceMode>().unwrap(),
            TraceMode::LinesAroundBB(42)
        );
        assert!("unknown".parse::<TraceMode>().is_err());
    }

    #[test]
    fn test_trace_mode_display_roundtrip() {
        let modes = vec![
            TraceMode::Off,
            TraceMode::Api,
            TraceMode::BB,
            TraceMode::ApiPlusBB,
            TraceMode::Lines,
            TraceMode::LinesAroundBB(7),
            TraceMode::All,
        ];
        for mode in modes {
            let s = mode.to_string();
            let parsed: TraceMode = s.parse().unwrap();
            assert_eq!(mode, parsed);
        }
    }

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
