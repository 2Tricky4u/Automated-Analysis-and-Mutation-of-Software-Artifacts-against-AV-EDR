///! Build/Emitter: Multi-level mutation + cross-compilation pipeline
///!
///! Takes C/C++ source templates → applies mutations → builds Windows PE
///!
///! Architecture:
///!   Source → AST mutations → LLVM IR → IR mutations → Instrumentation → PE
///!
///! See: automation/BUILD-PIPELINE.md for detailed design
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::{debug, info, warn};

pub mod ast_mutator;
pub mod instrumenter;
pub mod ir_mutator;

// Re-exports
pub use ast_mutator::AstMutator;
pub use instrumenter::Instrumenter;
pub use ir_mutator::IrMutator;

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

/// Build configuration
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
            trace_mode: TraceMode::ApiPlusBB,
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

/// Main builder orchestrator
pub struct ArtifactBuilder {
    config: BuildConfig,
}

impl ArtifactBuilder {
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
        info!("Building artifact from: {:?}", source_path);
        info!("Applying {} mutations", mutations.len());

        // Create temp directory for intermediate files
        let temp_dir = tempfile::tempdir()?;
        let temp_path = temp_dir.path();

        // Step 1: Apply AST-level mutations
        let mutated_source = temp_path.join("mutated.c");
        self.apply_ast_mutations(source_path, mutations, &mutated_source)
            .await?;

        // Step 2: Compile to LLVM IR
        let ir_path = temp_path.join("artifact.ll");
        self.compile_to_ir(&mutated_source, &ir_path).await?;

        // Step 3: Apply IR-level mutations
        let mutated_ir = temp_path.join("mutated.ll");
        self.apply_ir_mutations(&ir_path, mutations, &mutated_ir)
            .await?;

        // Step 4: Inject instrumentation
        let instrumented_ir = temp_path.join("instrumented.ll");
        self.inject_instrumentation(&mutated_ir, &instrumented_ir)
            .await?;

        // Step 5: Compile IR → Object file
        let obj_path = temp_path.join("artifact.obj");
        self.compile_to_obj(&instrumented_ir, &obj_path).await?;

        // Step 5.5: Compile runtime library if instrumentation is enabled
        let mut obj_files = vec![obj_path];
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

    /// Apply AST-level mutations to source code
    async fn apply_ast_mutations(
        &self,
        source: &Path,
        mutations: &[Mutation],
        output: &Path,
    ) -> Result<()> {
        debug!("Applying AST mutations...");

        let ast_mutations: Vec<_> = mutations
            .iter()
            .filter(|m| m.id.starts_with("ast."))
            .collect();

        if ast_mutations.is_empty() {
            // No AST mutations, just copy source
            tokio::fs::copy(source, output).await?;
            return Ok(());
        }

        // TODO: Implement full AST mutation pipeline
        // For now, just copy source
        warn!("AST mutations not yet implemented, copying source");
        tokio::fs::copy(source, output).await?;

        Ok(())
    }

    /// Compile source to LLVM IR
    async fn compile_to_ir(&self, source: &Path, output: &Path) -> Result<()> {
        info!("Compiling to LLVM IR...");

        let mut cmd = Command::new("clang");
        cmd.arg(format!("-target={}", self.config.target))
            .arg(format!("--sysroot={}", self.config.xwin_path.display()))
            .arg("-emit-llvm")
            .arg("-S")
            .arg(format!("-O{}", self.config.optimization))
            .arg("-o")
            .arg(output)
            .arg(source)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output_result = cmd.output().context("Failed to run clang")?;

        if !output_result.status.success() {
            anyhow::bail!(
                "clang failed: {}",
                String::from_utf8_lossy(&output_result.stderr)
            );
        }

        debug!("LLVM IR generated: {:?}", output);
        Ok(())
    }

    /// Apply IR-level mutations
    async fn apply_ir_mutations(
        &self,
        ir: &Path,
        mutations: &[Mutation],
        output: &Path,
    ) -> Result<()> {
        debug!("Applying IR mutations...");

        let ir_mutations: Vec<_> = mutations
            .iter()
            .filter(|m| m.id.starts_with("ir."))
            .collect();

        if ir_mutations.is_empty() {
            // No IR mutations, just copy
            tokio::fs::copy(ir, output).await?;
            return Ok(());
        }

        // TODO: Implement IR mutation passes
        warn!("IR mutations not yet implemented, copying IR");
        tokio::fs::copy(ir, output).await?;

        Ok(())
    }

    /// Inject instrumentation (BB coverage, API tracing)
    async fn inject_instrumentation(&self, ir: &Path, output: &Path) -> Result<()> {
        debug!("Injecting instrumentation: {:?}", self.config.trace_mode);

        match self.config.trace_mode {
            TraceMode::Off => {
                // No instrumentation
                tokio::fs::copy(ir, output).await?;
            }
            _ => {
                // Use instrumenter for all instrumentation modes
                let mut instrumenter = Instrumenter::new();
                instrumenter
                    .instrument(ir, self.config.trace_mode, output)
                    .await?;
            }
        }

        Ok(())
    }

    /// Compile LLVM IR to object file
    async fn compile_to_obj(&self, ir: &Path, output: &Path) -> Result<()> {
        info!("Compiling IR to object file...");

        let mut cmd = Command::new("llc");
        cmd.arg(format!("-mtriple={}", self.config.target))
            .arg("-filetype=obj")
            .arg("-o")
            .arg(output)
            .arg(ir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output_result = cmd.output().context("Failed to run llc")?;

        if !output_result.status.success() {
            anyhow::bail!(
                "llc failed: {}",
                String::from_utf8_lossy(&output_result.stderr)
            );
        }

        debug!("Object file generated: {:?}", output);
        Ok(())
    }

    /// Compile instrumentation runtime library to object file
    async fn compile_runtime_library(&self, temp_dir: &Path) -> Result<PathBuf> {
        info!("Compiling instrumentation runtime library...");

        // Path to runtime source (relative to project root)
        let runtime_source = Path::new("build/emitter/runtime/instrumentation_runtime.c");

        if !runtime_source.exists() {
            anyhow::bail!("Runtime library source not found: {:?}", runtime_source);
        }

        let runtime_obj = temp_dir.join("instrumentation_runtime.obj");

        // Compile runtime.c to object file
        let mut cmd = Command::new("clang");
        cmd.arg(format!("-target={}", self.config.target))
            .arg(format!("--sysroot={}", self.config.xwin_path.display()))
            .arg("-c")
            .arg(format!("-O{}", self.config.optimization))
            .arg("-o")
            .arg(&runtime_obj)
            .arg(runtime_source)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output_result = cmd.output().context("Failed to compile runtime library")?;

        if !output_result.status.success() {
            anyhow::bail!(
                "Runtime compilation failed: {}",
                String::from_utf8_lossy(&output_result.stderr)
            );
        }

        info!("Runtime library compiled: {:?}", runtime_obj);
        Ok(runtime_obj)
    }

    /// Link object files to PE executable
    async fn link_to_pe(&self, obj_files: &[PathBuf], output: &Path) -> Result<()> {
        info!("Linking to PE executable...");

        let crt_lib_path = self.config.xwin_path.join("crt/lib/x86_64");
        let sdk_lib_path = self.config.xwin_path.join("sdk/lib/um/x86_64");

        let mut cmd = Command::new("ld.lld");
        cmd.arg("-flavor")
            .arg("link")
            .arg("-subsystem:console")
            .arg("-entry:mainCRTStartup")
            .arg(format!("-libpath:{}", crt_lib_path.display()))
            .arg(format!("-libpath:{}", sdk_lib_path.display()))
            .arg(format!("-out:{}", output.display()));

        // Add deterministic flag if enabled
        if self.config.deterministic {
            cmd.arg("-Brepro");
        }

        // Add object files
        for obj in obj_files {
            cmd.arg(obj);
        }

        // Add default libraries
        cmd.arg("libcmt.lib").arg("kernel32.lib");

        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output_result = cmd.output().context("Failed to run lld-link")?;

        if !output_result.status.success() {
            anyhow::bail!(
                "lld-link failed: {}",
                String::from_utf8_lossy(&output_result.stderr)
            );
        }

        info!("PE executable generated: {:?}", output);
        Ok(())
    }

    /// Generate artifact metadata (SHA256, manifest)
    fn generate_metadata(
        &self,
        source: &Path,
        mutations: &[Mutation],
        artifact: &Path,
    ) -> Result<ArtifactMetadata> {
        // Compute SHA256 of artifact
        let artifact_bytes = std::fs::read(artifact)?;
        let mut hasher = Sha256::new();
        hasher.update(&artifact_bytes);
        let hash = hasher.finalize();
        let artifact_id = format!("sha256:{}", hex::encode(hash));

        // Get toolchain versions
        let toolchain = self.get_toolchain_info()?;

        Ok(ArtifactMetadata {
            artifact_id,
            artifact_path: artifact.to_path_buf(),
            source_template: source.file_name().unwrap().to_string_lossy().to_string(),
            mutations: mutations.to_vec(),
            config: self.config.clone(),
            build_timestamp: chrono::Utc::now().to_rfc3339(),
            toolchain,
        })
    }

    /// Get toolchain version information
    fn get_toolchain_info(&self) -> Result<ToolchainInfo> {
        // Get clang version
        let clang_output = Command::new("clang").arg("--version").output()?;
        let clang_version = String::from_utf8_lossy(&clang_output.stdout)
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_string();

        // Get LLVM version
        let llvm_output = Command::new("llc").arg("--version").output()?;
        let llvm_version = String::from_utf8_lossy(&llvm_output.stdout)
            .lines()
            .nth(1)
            .unwrap_or("unknown")
            .to_string();

        Ok(ToolchainInfo {
            clang_version,
            llvm_version,
            xwin_version: "0.5.0".to_string(), // TODO: Get from xwin --version
        })
    }
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
