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

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command};
use tracing::{debug, info, warn};

// ============================================================================
// Submodules (new organization)
// ============================================================================

pub mod transform;
pub mod instrument;
pub mod template;
pub mod mutator;
pub mod builder;
pub mod compiler;

// ============================================================================
// Re-exports from submodules
// ============================================================================

// Transform module
pub use transform::{AstMutator, IrMutator};

// Instrument module
pub use instrument::{Instrumenter, inject_line_traces, inject_line_traces_with_opts, SourceLanguage, TraceFormat};

// Template module
pub use template::{Assembler, ModuleSelection, PayloadEncoder, EncodingType, EncodedPayload, MutationMarker};
pub use template::{extract_mutation_markers, strip_mutation_markers, generate_test_payload};

// Builder module (main API)
pub use builder::{ArtifactBuilder, BuilderConfig, BuildInput, BuiltArtifact};

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

/// Low-level build configuration (for direct LLVM/Clang pipeline)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// Target triple (e.g., "x86_64-pc-windows-msvc")
    pub target: String,
    /// Optimization level: "0", "1", "2", "3", "s", "z"
    pub optimization: String,
    /// Instrumentation mode
    pub trace_mode: TraceMode,
    /// Enable deterministic builds (pin timestamps, disable ASLR entropy)
    pub deterministic: bool,
    /// Path to xwin SDK (e.g., ~/.xwin)
    pub xwin_path: PathBuf,
    /// Custom LLVM passes to apply
    pub llvm_passes: Vec<String>,
    /// Mutation seed for reproducibility
    pub seed: u64,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            target: "x86_64-pc-windows-msvc".to_string(),
            optimization: "2".to_string(),
            trace_mode: TraceMode::Lines,
            deterministic: true,
            xwin_path: dirs::home_dir().unwrap().join(".xwin"),
            llvm_passes: vec![],
            seed: 0,
        }
    }
}

/// Mutation specification (CLAUDE.md Section 6)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mutation {
    /// Mutation ID (e.g., "ast.import_reshape", "ir.opaque_predicates")
    pub id: String,
    /// Mutation parameters (key-value pairs)
    pub params: std::collections::HashMap<String, String>,
}

/// Artifact metadata (stored alongside .exe)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactMetadata {
    /// SHA256 hash of final PE
    pub artifact_id: String,
    /// Path to built executable
    pub artifact_path: PathBuf,
    /// Source template used
    pub source_template: String,
    /// Applied mutations
    pub mutations: Vec<Mutation>,
    /// Build configuration
    pub config: BuildConfig,
    /// Build timestamp
    pub build_timestamp: String,
    /// Toolchain versions
    pub toolchain: ToolchainInfo,
}

/// Toolchain version info (for reproducibility)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainInfo {
    pub clang_version: String,
    pub llvm_version: String,
    pub xwin_version: String,
}

// ============================================================================
// Low-Level Builder (LLVM IR pipeline)
// ============================================================================

/// Low-level artifact builder using LLVM IR pipeline
///
/// This provides direct access to the LLVM compilation pipeline.
/// For most use cases, prefer [`ArtifactBuilder`] which provides
/// a higher-level API with template support.
pub struct LowLevelBuilder {
    config: BuildConfig,
}

impl LowLevelBuilder {
    pub fn new(config: BuildConfig) -> Self {
        Self { config }
    }

    /// Full pipeline: Source → PE executable
    pub async fn build(
        &self,
        source_path: &Path,
        mutations: &[Mutation],
        output_path: &Path,
    ) -> Result<ArtifactMetadata> {
        let abs_source = std::fs::canonicalize(source_path).unwrap_or_else(|_| source_path.to_path_buf());
        debug!("Building artifact from: {:?}", abs_source);
        debug!("Applying {} mutations", mutations.len());

        // Create temp directory for intermediate files
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path();

        // Step 1: Apply AST-level mutations
        let mutated_source = temp_path.join("mutated.c");
        self.apply_ast_mutations(source_path, mutations, &mutated_source).await?;

        // Step 2: Compile to LLVM IR
        let ir_path = temp_path.join("artifact.ll");
        self.compile_to_ir(&mutated_source, &ir_path).await?;

        // Step 3: Apply IR-level mutations
        let mutated_ir = temp_path.join("mutated.ll");
        self.apply_ir_mutations(&ir_path, mutations, &mutated_ir).await?;

        // Step 4: Inject instrumentation
        let instrumented_ir = temp_path.join("instrumented.ll");
        self.inject_instrumentation(&mutated_ir, &instrumented_ir).await?;

        // Step 5: Compile IR → Object file
        let obj_path = temp_path.join("artifact.obj");
        self.compile_to_obj(&instrumented_ir, &obj_path).await?;

        // Step 5.5: Compile runtime libraries
        let mut obj_files = vec![obj_path];
        let minimal_runtime_obj = self.compile_minimal_runtime(temp_path).await?;
        obj_files.push(minimal_runtime_obj);

        if self.config.trace_mode != TraceMode::Off {
            let runtime_obj = self.compile_runtime_library(temp_path).await?;
            obj_files.push(runtime_obj);
        }

        // Step 6: Link to PE executable
        self.link_to_pe(&obj_files, output_path).await?;

        // Step 7: Generate metadata
        let metadata = self.generate_metadata(source_path, mutations, output_path)?;
        info!("Artifact built successfully: {}", metadata.artifact_id);

        Ok(metadata)
    }

    async fn apply_ast_mutations(&self, source: &Path, mutations: &[Mutation], output: &Path) -> Result<()> {
        let ast_mutations: Vec<_> = mutations.iter().filter(|m| m.id.starts_with("ast.")).collect();
        let needs_line_tracing = matches!(
            self.config.trace_mode,
            TraceMode::Lines | TraceMode::LinesAroundBB(_) | TraceMode::All
        );

        if ast_mutations.is_empty() && !needs_line_tracing {
            tokio::fs::copy(source, output).await?;
            return Ok(());
        }

        let source_code = tokio::fs::read_to_string(source).await?;
        let processed_code = if needs_line_tracing {
            let mutator = AstMutator::new();
            mutator.inject_line_tracing(&source_code)?
        } else {
            source_code
        };

        tokio::fs::write(output, processed_code).await?;
        Ok(())
    }

    fn find_runtime_include_dir() -> Result<PathBuf> {
        let possible_paths = vec![
            PathBuf::from("build/runtime"),
            PathBuf::from("../build/runtime"),
            std::env::current_dir()?.join("build/runtime"),
            PathBuf::from("build/emitter/runtime"),
        ];
        possible_paths.iter().find(|p| p.exists()).cloned()
            .ok_or_else(|| anyhow::anyhow!("Runtime include directory not found"))
    }

    async fn compile_to_ir(&self, source: &Path, output: &Path) -> Result<()> {
        let xwin = &self.config.xwin_path;
        let runtime_include = Self::find_runtime_include_dir()?;

        let output_result = Command::new("clang")
            .arg(format!("--target={}", self.config.target))
            .arg(format!("-isystem{}/crt/include", xwin.display()))
            .arg(format!("-isystem{}/sdk/include/ucrt", xwin.display()))
            .arg(format!("-isystem{}/sdk/include/shared", xwin.display()))
            .arg(format!("-isystem{}/sdk/include/um", xwin.display()))
            .arg(format!("-I{}", runtime_include.display()))
            .arg("-emit-llvm").arg("-S")
            .arg(format!("-O{}", self.config.optimization))
            .arg("-o").arg(output).arg(source)
            .output().context("Failed to run clang")?;

        if !output_result.status.success() {
            anyhow::bail!("clang failed: {}", String::from_utf8_lossy(&output_result.stderr));
        }
        Ok(())
    }

    async fn apply_ir_mutations(&self, ir: &Path, mutations: &[Mutation], output: &Path) -> Result<()> {
        let ir_mutations: Vec<_> = mutations.iter().filter(|m| m.id.starts_with("ir.")).collect();
        if ir_mutations.is_empty() {
            tokio::fs::copy(ir, output).await?;
            return Ok(());
        }
        warn!("IR mutations not yet implemented");
        tokio::fs::copy(ir, output).await?;
        Ok(())
    }

    async fn inject_instrumentation(&self, ir: &Path, output: &Path) -> Result<()> {
        match self.config.trace_mode {
            TraceMode::Off => { tokio::fs::copy(ir, output).await?; }
            _ => {
                let mut instrumenter = Instrumenter::new();
                instrumenter.instrument(ir, self.config.trace_mode, output).await?;
            }
        }
        Ok(())
    }

    async fn compile_to_obj(&self, ir: &Path, output: &Path) -> Result<()> {
        let output_result = Command::new("llc")
            .arg(format!("-mtriple={}", self.config.target))
            .arg("-filetype=obj").arg("-o").arg(output).arg(ir)
            .output().context("Failed to run llc")?;

        if !output_result.status.success() {
            anyhow::bail!("llc failed: {}", String::from_utf8_lossy(&output_result.stderr));
        }
        Ok(())
    }

    async fn compile_runtime_library(&self, temp_dir: &Path) -> Result<PathBuf> {
        let possible_paths = vec![
            PathBuf::from("build/runtime/instrumentation_runtime.c"),
            PathBuf::from("build/emitter/runtime/instrumentation_runtime.c"),
        ];
        let runtime_source = possible_paths.iter().find(|p| p.exists())
            .ok_or_else(|| anyhow::anyhow!("Runtime source not found"))?;

        let runtime_obj = temp_dir.join("instrumentation_runtime.obj");
        let xwin = &self.config.xwin_path;

        let output_result = Command::new("clang")
            .arg(format!("--target={}", self.config.target))
            .arg(format!("-isystem{}/crt/include", xwin.display()))
            .arg(format!("-isystem{}/sdk/include/ucrt", xwin.display()))
            .arg("-c").arg(format!("-O{}", self.config.optimization))
            .arg("-o").arg(&runtime_obj).arg(runtime_source)
            .output().context("Failed to compile runtime")?;

        if !output_result.status.success() {
            anyhow::bail!("Runtime compilation failed");
        }
        Ok(runtime_obj)
    }

    async fn compile_minimal_runtime(&self, temp_dir: &Path) -> Result<PathBuf> {
        let possible_paths = vec![
            PathBuf::from("build/runtime/minimal_runtime.c"),
            PathBuf::from("build/emitter/runtime/minimal_runtime.c"),
        ];
        let runtime_source = possible_paths.iter().find(|p| p.exists())
            .ok_or_else(|| anyhow::anyhow!("Minimal runtime source not found"))?;

        let runtime_obj = temp_dir.join("minimal_runtime.obj");
        let xwin = &self.config.xwin_path;

        let output_result = Command::new("clang")
            .arg(format!("--target={}", self.config.target))
            .arg(format!("-isystem{}/crt/include", xwin.display()))
            .arg(format!("-isystem{}/sdk/include/ucrt", xwin.display()))
            .arg("-c").arg(format!("-O{}", self.config.optimization))
            .arg("-o").arg(&runtime_obj).arg(runtime_source)
            .output().context("Failed to compile minimal runtime")?;

        if !output_result.status.success() {
            anyhow::bail!("Minimal runtime compilation failed");
        }
        Ok(runtime_obj)
    }

    async fn link_to_pe(&self, obj_files: &[PathBuf], output: &Path) -> Result<()> {
        let xwin = &self.config.xwin_path;
        let mut cmd = Command::new("ld.lld");
        cmd.arg("-flavor").arg("link")
            .arg("-subsystem:console").arg("-entry:mainCRTStartup")
            .arg(format!("-libpath:{}/crt/lib/x86_64", xwin.display()))
            .arg(format!("-libpath:{}/sdk/lib/um/x86_64", xwin.display()))
            .arg(format!("-libpath:{}/sdk/lib/ucrt/x86_64", xwin.display()))
            .arg(format!("-out:{}", output.display()));

        if self.config.deterministic { cmd.arg("-Brepro"); }
        for obj in obj_files { cmd.arg(obj); }
        cmd.arg("libcmt.lib").arg("libucrt.lib").arg("kernel32.lib");

        let output_result = cmd.output().context("Failed to run lld-link")?;
        if !output_result.status.success() {
            anyhow::bail!("lld-link failed: {}", String::from_utf8_lossy(&output_result.stderr));
        }
        Ok(())
    }

    fn generate_metadata(&self, source: &Path, mutations: &[Mutation], artifact: &Path) -> Result<ArtifactMetadata> {
        let artifact_bytes = std::fs::read(artifact)?;
        let mut hasher = Sha256::new();
        hasher.update(&artifact_bytes);
        let artifact_id = format!("sha256:{}", hex::encode(hasher.finalize()));

        Ok(ArtifactMetadata {
            artifact_id,
            artifact_path: artifact.to_path_buf(),
            source_template: source.file_name().unwrap().to_string_lossy().to_string(),
            mutations: mutations.to_vec(),
            config: self.config.clone(),
            build_timestamp: chrono::Utc::now().to_rfc3339(),
            toolchain: ToolchainInfo {
                clang_version: "unknown".to_string(),
                llvm_version: "unknown".to_string(),
                xwin_version: "0.5.0".to_string(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_mode_serialization() {
        let modes = vec![TraceMode::Off, TraceMode::Api, TraceMode::BB, TraceMode::ApiPlusBB, TraceMode::Lines, TraceMode::All];
        for mode in modes {
            let json = serde_json::to_string(&mode).unwrap();
            let deserialized: TraceMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, deserialized);
        }
    }
}
