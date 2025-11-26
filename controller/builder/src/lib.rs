/// Artifact Builder - Clang cross-compilation wrapper
///
/// Implements BUILD-DEPLOY-EXECUTE-PIPELINE.md Section 1: Build on Controller (WSL)
///
/// Compiles C templates to Windows PE executables using Clang with xwin SDK.
/// Uses the same flags and dependencies as corpus/templates/build_all.sh.
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

pub mod mutator;

/// Configuration for the artifact builder
#[derive(Debug, Clone)]
pub struct BuilderConfig {
    /// Path to templates directory (e.g., "corpus/templates")
    pub templates_dir: PathBuf,
    /// Path to output directory for artifacts (e.g., "artifacts")
    pub output_dir: PathBuf,
    /// Path to xwin SDK (e.g., "/root/.xwin")
    pub xwin_dir: PathBuf,
}

impl Default for BuilderConfig {
    fn default() -> Self {
        Self {
            templates_dir: PathBuf::from("corpus/templates"),
            output_dir: PathBuf::from("artifacts"),
            xwin_dir: PathBuf::from("/root/.xwin"),
        }
    }
}

/// Metadata about a built artifact
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuiltArtifact {
    /// SHA256 hash of the artifact (artifact_id)
    pub artifact_id: String,
    /// Path to source file
    pub source_path: PathBuf,
    /// Path to output executable
    pub output_path: PathBuf,
    /// Size in bytes
    pub size_bytes: u64,
    /// SHA256 hash (same as artifact_id)
    pub sha256: String,
    /// When it was built
    pub build_timestamp: chrono::DateTime<chrono::Utc>,
    /// Compiler version
    pub compiler_version: String,
    /// Compiler flags used
    pub compiler_flags: Vec<String>,
    /// Mutations successfully applied
    pub mutations_applied: Vec<String>,
}

/// Template-specific library dependencies
/// Matches the extra_libs in build_all.sh
fn get_template_libs(template_name: &str) -> &'static [&'static str] {
    match template_name {
        "loader_v1" => &[],
        "rwx_direct" => &["-Wl,-defaultlib:advapi32", "-Wl,-defaultlib:wininet"],
        "process_injection" => &["-Wl,-defaultlib:user32"],
        "network_beacon" => &["-Wl,-defaultlib:ws2_32"],
        "eicar_test" => &["-Wl,-defaultlib:advapi32"], // GetUserNameA requires advapi32.lib
        _ => {
            warn!("Unknown template '{}', using no extra libs", template_name);
            &[]
        }
    }
}

/// Input format for artifact building
#[derive(Debug, Clone)]
pub enum BuildInput {
    /// Build from C source file
    SourceFile {
        template_name: String,
        source_file: String,
        mutations: Vec<mutator::MutationSpec>,
        /// Instrumentation mode: "off" | "lines" | "api" | "bb" | "api+bb" | "all" | "lines-around-bb=<id>"
        /// Default: "api+bb" if empty
        trace_mode: String,
    },
    /// Build from LLVM IR (post-mutation)
    LlvmIr {
        ir_code: Vec<u8>,
        artifact_name: String,
        mutations: Vec<mutator::MutationSpec>,
        /// Instrumentation mode
        trace_mode: String,
    },
    /// Build from in-memory C source (text mutations)
    SourceCode {
        source_code: Vec<u8>,
        artifact_name: String,
        mutations: Vec<mutator::MutationSpec>,
        /// Instrumentation mode
        trace_mode: String,
    },
}

/// Artifact builder
pub struct ArtifactBuilder {
    config: BuilderConfig,
}

impl ArtifactBuilder {
    pub fn new(config: BuilderConfig) -> Result<Self> {
        // Validate xwin SDK exists
        if !config.xwin_dir.exists() {
            anyhow::bail!(
                "xwin SDK not found at {:?}. Run xwin to set it up.",
                config.xwin_dir
            );
        }

        // Create output directory if it doesn't exist
        std::fs::create_dir_all(&config.output_dir).context("Failed to create output directory")?;

        Ok(Self { config })
    }

    /// Build artifact from various input formats (unified API)
    ///
    /// # Arguments
    /// * `input` - Source input (file, LLVM IR, or in-memory source)
    /// * `template_name` - Optional template name for library lookup (required for SourceFile)
    ///
    /// # Returns
    /// Metadata about the built artifact
    pub async fn build(&self, input: BuildInput) -> Result<BuiltArtifact> {
        match input {
            BuildInput::SourceFile {
                template_name,
                source_file,
                mutations,
                trace_mode,
            } => {
                info!("Building {} with trace_mode: {}", source_file, trace_mode);

                // Build artifact (with or without mutations)
                let mut built = if mutations.is_empty() {
                    self.build_template(&template_name, &source_file).await?
                } else {
                    self.build_template_with_mutations(&template_name, &source_file, &mutations).await?
                };

                // Apply instrumentation if trace_mode is not "off"
                if trace_mode != "off" && !trace_mode.is_empty() {
                    info!("Applying instrumentation: trace_mode={}", trace_mode);
                    built = self.apply_instrumentation(built, &trace_mode).await?;
                }

                Ok(built)
            }

            BuildInput::LlvmIr {
                ir_code,
                artifact_name,
                mutations,
                trace_mode,
            } => {
                info!("Building LLVM IR {} with trace_mode: {}", artifact_name, trace_mode);
                self.build_from_llvm_ir_with_mutations(&ir_code, &artifact_name, &mutations)
                    .await
            }

            BuildInput::SourceCode {
                source_code,
                artifact_name,
                mutations,
                trace_mode,
            } => {
                info!("Building source code {} with trace_mode: {}", artifact_name, trace_mode);

                self.build_from_source_code_with_mutations(&source_code, &artifact_name, &mutations)
                    .await
            }
        }
    }

    /// Build a template from source file (original method, kept for backward compatibility)
    ///
    /// # Arguments
    /// * `template_name` - Template directory name (e.g., "rwx_direct")
    /// * `source_file` - Source filename (e.g., "rwx_direct.c")
    ///
    /// # Returns
    /// Metadata about the built artifact
    pub async fn build_template(
        &self,
        template_name: &str,
        source_file: &str,
    ) -> Result<BuiltArtifact> {
        // 1. Validate source exists
        let source_path = self
            .config
            .templates_dir
            .join(template_name)
            .join(source_file);

        if !source_path.exists() {
            anyhow::bail!("Source file not found: {:?}", source_path);
        }

        info!(
            "Building template: {} from {:?}",
            template_name, source_path
        );

        // 2. Invoke clang to build
        let output_name = source_file.replace(".c", ".exe");
        let temp_output = self
            .config
            .templates_dir
            .join(template_name)
            .join(&output_name);

        self.invoke_clang(template_name, &source_path, &temp_output)
            .await?;

        // 3. Read the built artifact
        let artifact_data = tokio::fs::read(&temp_output)
            .await
            .context("Failed to read built artifact")?;

        // 4. Compute SHA256 artifact_id
        let artifact_id = self.compute_sha256(&artifact_data);

        // 5. Move to artifacts directory with SHA256 name
        let final_output = self.config.output_dir.join(format!("{}.exe", artifact_id));
        tokio::fs::rename(&temp_output, &final_output)
            .await
            .context("Failed to move artifact to output directory")?;

        info!(
            "Artifact built: {} ({} bytes) -> {:?}",
            artifact_id,
            artifact_data.len(),
            final_output
        );

        // 6. Get compiler version
        let compiler_version = self.get_clang_version()?;

        // 7. Return metadata
        Ok(BuiltArtifact {
            artifact_id: artifact_id.clone(),
            source_path,
            output_path: final_output,
            size_bytes: artifact_data.len() as u64,
            sha256: artifact_id,
            build_timestamp: chrono::Utc::now(),
            compiler_version,
            compiler_flags: self.get_compiler_flags(template_name),
            mutations_applied: vec![],
        })
    }

    /// Build template with mutations applied to source
    ///
    /// # Arguments
    /// * `template_name` - Template directory name
    /// * `source_file` - Source filename
    /// * `mutations` - List of mutations to apply
    pub async fn build_template_with_mutations(
        &self,
        template_name: &str,
        source_file: &str,
        mutations: &[mutator::MutationSpec],
    ) -> Result<BuiltArtifact> {
        if mutations.is_empty() {
            // No mutations - use original build path
            return self.build_template(template_name, source_file).await;
        }

        info!(
            "Building template {} with {} mutations",
            template_name,
            mutations.len()
        );

        // 1. Read original source
        let source_path = self
            .config
            .templates_dir
            .join(template_name)
            .join(source_file);

        if !source_path.exists() {
            anyhow::bail!("Source file not found: {:?}", source_path);
        }

        let original_source = tokio::fs::read(&source_path)
            .await
            .context("Failed to read source file")?;

        // 2. Separate AST and LLVM mutations
        let has_llvm_mutations = mutations.iter().any(|m| m.id.starts_with("llvm."));
        let has_ast_mutations = mutations.iter().any(|m| m.id.starts_with("ast."));

        // 3. Apply AST mutations to source first (if any)
        let mut working_source = original_source.clone();
        let mut all_mutations_applied = Vec::new();

        if has_ast_mutations {
            let ast_mutations: Vec<_> = mutations
                .iter()
                .filter(|m| m.id.starts_with("ast."))
                .cloned()
                .collect();

            let (mutated, applied) = mutator::Mutator::apply(&working_source, &ast_mutations)?;
            working_source = mutated;
            all_mutations_applied.extend(applied);
        }

        // 4. If we have LLVM mutations, use IR path
        if has_llvm_mutations {
            // 4a. Write (possibly AST-mutated) source to temp file
            let temp_source_filename = format!("temp_mutated_{}", source_file);
            let temp_source_path = self
                .config
                .templates_dir
                .join(template_name)
                .join(&temp_source_filename);

            tokio::fs::write(&temp_source_path, &working_source)
                .await
                .context("Failed to write temp source")?;

            // 4b. Compile source → LLVM IR
            let ir_filename = source_file.replace(".c", ".ll");
            let ir_path = self
                .config
                .templates_dir
                .join(template_name)
                .join(&ir_filename);

            self.compile_source_to_ir(&temp_source_path, &ir_path, template_name)
                .await?;

            // Clean up temp source
            let _ = tokio::fs::remove_file(&temp_source_path).await;

            // 4c. Apply LLVM mutations to IR
            let ir_content = tokio::fs::read(&ir_path)
                .await
                .context("Failed to read LLVM IR")?;

            let llvm_mutations: Vec<_> = mutations
                .iter()
                .filter(|m| m.id.starts_with("llvm."))
                .cloned()
                .collect();

            let (mutated_ir, llvm_applied) = mutator::Mutator::apply(&ir_content, &llvm_mutations)?;
            all_mutations_applied.extend(llvm_applied);

            // 4d. Write mutated IR
            tokio::fs::write(&ir_path, &mutated_ir)
                .await
                .context("Failed to write mutated IR")?;

            info!(
                "Applied {} mutations: {:?}",
                all_mutations_applied.len(),
                all_mutations_applied
            );

            // 4e. Compile IR → binary
            let output_name = source_file.replace(".c", ".exe");
            let temp_output = self
                .config
                .templates_dir
                .join(template_name)
                .join(&output_name);

            self.compile_ir_to_exe(&ir_path, &temp_output, template_name)
                .await?;

            // Clean up IR file
            let _ = tokio::fs::remove_file(&ir_path).await;

            // Continue to step 5 (artifact finalization)
            let artifact_data = tokio::fs::read(&temp_output)
                .await
                .context("Failed to read built artifact")?;

            let artifact_id = self.compute_sha256(&artifact_data);
            let final_output = self.config.output_dir.join(format!("{}.exe", artifact_id));
            tokio::fs::rename(&temp_output, &final_output)
                .await
                .context("Failed to move artifact to output directory")?;

            info!(
                "Mutated artifact built (IR path): {} ({} bytes) -> {:?}",
                artifact_id,
                artifact_data.len(),
                final_output
            );

            let compiler_version = self.get_clang_version()?;

            return Ok(BuiltArtifact {
                artifact_id: artifact_id.clone(),
                source_path,
                output_path: final_output,
                size_bytes: artifact_data.len() as u64,
                sha256: artifact_id,
                build_timestamp: chrono::Utc::now(),
                compiler_version,
                compiler_flags: self.get_compiler_flags(template_name),
                mutations_applied: all_mutations_applied,
            });
        }

        // 5. AST-only mutations: write mutated source and compile directly
        info!(
            "Applied {} mutations: {:?}",
            all_mutations_applied.len(),
            all_mutations_applied
        );

        let mutated_filename = format!("mutated_{}", source_file);
        let mutated_path = self
            .config
            .templates_dir
            .join(template_name)
            .join(&mutated_filename);

        tokio::fs::write(&mutated_path, &working_source)
            .await
            .context("Failed to write mutated source")?;

        let output_name = source_file.replace(".c", ".exe");
        let temp_output = self
            .config
            .templates_dir
            .join(template_name)
            .join(&output_name);

        self.invoke_clang(template_name, &mutated_path, &temp_output)
            .await?;

        // 6. Clean up mutated source file
        let _ = tokio::fs::remove_file(&mutated_path).await;

        // 7. Read artifact and compute hash
        let artifact_data = tokio::fs::read(&temp_output)
            .await
            .context("Failed to read built artifact")?;

        let artifact_id = self.compute_sha256(&artifact_data);

        // 8. Move to artifacts directory
        let final_output = self.config.output_dir.join(format!("{}.exe", artifact_id));
        tokio::fs::rename(&temp_output, &final_output)
            .await
            .context("Failed to move artifact to output directory")?;

        info!(
            "Mutated artifact built (AST path): {} ({} bytes) -> {:?}",
            artifact_id,
            artifact_data.len(),
            final_output
        );

        // 9. Return metadata with mutations applied
        let compiler_version = self.get_clang_version()?;

        Ok(BuiltArtifact {
            artifact_id: artifact_id.clone(),
            source_path,
            output_path: final_output,
            size_bytes: artifact_data.len() as u64,
            sha256: artifact_id,
            build_timestamp: chrono::Utc::now(),
            compiler_version,
            compiler_flags: self.get_compiler_flags(template_name),
            mutations_applied: all_mutations_applied,
        })
    }

    /// Build from LLVM IR with mutations
    async fn build_from_llvm_ir_with_mutations(
        &self,
        ir_code: &[u8],
        artifact_name: &str,
        mutations: &[mutator::MutationSpec],
    ) -> Result<BuiltArtifact> {
        // Apply mutations to LLVM IR
        let (mutated_ir, mutations_applied) = if !mutations.is_empty() {
            mutator::Mutator::apply(ir_code, mutations)?
        } else {
            (ir_code.to_vec(), vec![])
        };

        // Delegate to existing build_from_llvm_ir logic
        // (this method needs to be implemented in the builder)
        self.build_from_llvm_ir_internal(&mutated_ir, artifact_name, mutations_applied)
            .await
    }

    /// Build from source code with mutations
    async fn build_from_source_code_with_mutations(
        &self,
        source_code: &[u8],
        artifact_name: &str,
        mutations: &[mutator::MutationSpec],
    ) -> Result<BuiltArtifact> {
        // Apply mutations to source code
        let (mutated_source, mutations_applied) = if !mutations.is_empty() {
            mutator::Mutator::apply(source_code, mutations)?
        } else {
            (source_code.to_vec(), vec![])
        };

        // Delegate to existing build_from_source_code logic
        self.build_from_source_code_internal(&mutated_source, artifact_name, mutations_applied)
            .await
    }

    /// Internal helper to build from source code with mutations tracking
    async fn build_from_source_code_internal(
        &self,
        source_code: &[u8],
        artifact_name: &str,
        mutations_applied: Vec<String>,
    ) -> Result<BuiltArtifact> {
        // Write source to temporary file
        let temp_source = self
            .config
            .templates_dir
            .join(format!("temp_{}.c", artifact_name));

        tokio::fs::write(&temp_source, source_code)
            .await
            .context("Failed to write temporary source")?;

        // Build
        let temp_output = self
            .config
            .templates_dir
            .join(format!("temp_{}.exe", artifact_name));

        self.invoke_clang("", &temp_source, &temp_output).await?;

        // Clean up temp source
        let _ = tokio::fs::remove_file(&temp_source).await;

        // Read artifact
        let artifact_data = tokio::fs::read(&temp_output)
            .await
            .context("Failed to read built artifact")?;

        let artifact_id = self.compute_sha256(&artifact_data);

        // Move to artifacts directory
        let final_output = self.config.output_dir.join(format!("{}.exe", artifact_id));
        tokio::fs::rename(&temp_output, &final_output)
            .await
            .context("Failed to move artifact")?;

        info!(
            "Source-code artifact built: {} ({} bytes)",
            artifact_id,
            artifact_data.len()
        );

        let compiler_version = self.get_clang_version()?;

        Ok(BuiltArtifact {
            artifact_id: artifact_id.clone(),
            source_path: PathBuf::from(format!("in-memory:{}", artifact_name)),
            output_path: final_output,
            size_bytes: artifact_data.len() as u64,
            sha256: artifact_id,
            build_timestamp: chrono::Utc::now(),
            compiler_version,
            compiler_flags: self.get_compiler_flags(""),
            mutations_applied,
        })
    }

    /// Internal helper to build from LLVM IR with mutations tracking
    async fn build_from_llvm_ir_internal(
        &self,
        ir_code: &[u8],
        artifact_name: &str,
        mutations_applied: Vec<String>,
    ) -> Result<BuiltArtifact> {
        // Write IR to temporary file
        let temp_ir = self
            .config
            .templates_dir
            .join(format!("temp_{}.ll", artifact_name));

        tokio::fs::write(&temp_ir, ir_code)
            .await
            .context("Failed to write temporary IR")?;

        // Compile LLVM IR to executable using clang
        let temp_output = self
            .config
            .templates_dir
            .join(format!("temp_{}.exe", artifact_name));

        self.invoke_clang_on_ir(&temp_ir, &temp_output).await?;

        // Clean up temp IR
        let _ = tokio::fs::remove_file(&temp_ir).await;

        // Read artifact
        let artifact_data = tokio::fs::read(&temp_output)
            .await
            .context("Failed to read built artifact")?;

        let artifact_id = self.compute_sha256(&artifact_data);

        // Move to artifacts directory
        let final_output = self.config.output_dir.join(format!("{}.exe", artifact_id));
        tokio::fs::rename(&temp_output, &final_output)
            .await
            .context("Failed to move artifact")?;

        info!(
            "LLVM IR artifact built: {} ({} bytes)",
            artifact_id,
            artifact_data.len()
        );

        let compiler_version = self.get_clang_version()?;

        Ok(BuiltArtifact {
            artifact_id: artifact_id.clone(),
            source_path: PathBuf::from(format!("llvm-ir:{}", artifact_name)),
            output_path: final_output,
            size_bytes: artifact_data.len() as u64,
            sha256: artifact_id,
            build_timestamp: chrono::Utc::now(),
            compiler_version,
            compiler_flags: vec!["-x".to_string(), "ir".to_string()],
            mutations_applied,
        })
    }

    /// Compile LLVM IR to Windows executable
    async fn invoke_clang_on_ir(&self, ir_path: &Path, output_path: &Path) -> Result<()> {
        let xwin_dir = &self.config.xwin_dir;

        let output = tokio::process::Command::new("clang")
            .args([
                "-x",
                "ir",
                ir_path.to_str().unwrap(),
                "-o",
                output_path.to_str().unwrap(),
                "--target=x86_64-pc-windows-msvc",
                &format!("--sysroot={}", xwin_dir.display()),
                "-fuse-ld=lld",
                "-Wno-unused-command-line-argument",
            ])
            .output()
            .await
            .context("Failed to execute clang on LLVM IR")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Clang IR compilation failed:\n{}", stderr);
        }

        Ok(())
    }

    /// Invoke Clang with xwin SDK to cross-compile C to Windows PE
    ///
    /// Uses the same flags as corpus/templates/build_all.sh:
    /// - Target: x86_64-pc-windows-msvc
    /// - SDK includes: ucrt, shared, um, winrt
    /// - Libraries: kernel32, libcmt
    /// - Template-specific libs (advapi32, wininet, ws2_32, etc.)
    async fn invoke_clang(&self, template_name: &str, source: &Path, output: &Path) -> Result<()> {
        let xwin = &self.config.xwin_dir;

        // Pre-format paths to avoid lifetime issues
        let crt_include = format!("{}/crt/include", xwin.display());
        let sdk_ucrt_include = format!("{}/sdk/include/ucrt", xwin.display());
        let sdk_shared_include = format!("{}/sdk/include/shared", xwin.display());
        let sdk_um_include = format!("{}/sdk/include/um", xwin.display());
        let sdk_winrt_include = format!("{}/sdk/include/winrt", xwin.display());
        let crt_lib = format!("{}/crt/lib/x86_64", xwin.display());
        let sdk_ucrt_lib = format!("{}/sdk/lib/ucrt/x86_64", xwin.display());
        let sdk_um_lib = format!("{}/sdk/lib/um/x86_64", xwin.display());

        let output_str = output.to_str().context("Invalid output path")?;
        let source_str = source.to_str().context("Invalid source path")?;

        // Base flags (from build_all.sh COMMON_FLAGS + BASE_LIBS)
        let mut args = vec![
            "-target",
            "x86_64-pc-windows-msvc",
            "-isystem",
            crt_include.as_str(),
            "-isystem",
            sdk_ucrt_include.as_str(),
            "-isystem",
            sdk_shared_include.as_str(),
            "-isystem",
            sdk_um_include.as_str(),
            "-isystem",
            sdk_winrt_include.as_str(),
            "-L",
            crt_lib.as_str(),
            "-L",
            sdk_ucrt_lib.as_str(),
            "-L",
            sdk_um_lib.as_str(),
            "-fuse-ld=lld",
            "-Wl,/subsystem:console",
            "-O2",
            "-Wl,-defaultlib:libcmt",
            "-Wl,-defaultlib:kernel32",
        ];

        // Add template-specific libraries
        let extra_libs = get_template_libs(template_name);
        for lib in extra_libs {
            args.push(lib);
        }

        // Add output and source
        args.push("-o");
        args.push(output_str);
        args.push(source_str);

        info!("Invoking: clang {}", args.join(" "));

        // Execute clang
        let output_result = tokio::process::Command::new("clang")
            .args(&args)
            .output()
            .await
            .context("Failed to execute clang")?;

        if !output_result.status.success() {
            let stderr = String::from_utf8_lossy(&output_result.stderr);
            let stdout = String::from_utf8_lossy(&output_result.stdout);
            anyhow::bail!(
                "Clang build failed:\nSTDOUT:\n{}\nSTDERR:\n{}",
                stdout,
                stderr
            );
        }

        // Verify output file was created
        if !output.exists() {
            anyhow::bail!("Build succeeded but output file not found: {:?}", output);
        }

        Ok(())
    }

    /// Compute SHA256 hash of bytes
    fn compute_sha256(&self, data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// Compile C source to LLVM IR
    async fn compile_source_to_ir(
        &self,
        source_path: &Path,
        ir_path: &Path,
        template_name: &str,
    ) -> Result<()> {
        let xwin_root = PathBuf::from("/root/.xwin");
        let crt_include = xwin_root.join("crt/include");
        let sdk_ucrt_include = xwin_root.join("sdk/include/ucrt");
        let sdk_shared_include = xwin_root.join("sdk/include/shared");
        let sdk_um_include = xwin_root.join("sdk/include/um");
        let sdk_winrt_include = xwin_root.join("sdk/include/winrt");

        let args = vec![
            "-target",
            "x86_64-pc-windows-msvc",
            "-isystem",
            crt_include.to_str().unwrap(),
            "-isystem",
            sdk_ucrt_include.to_str().unwrap(),
            "-isystem",
            sdk_shared_include.to_str().unwrap(),
            "-isystem",
            sdk_um_include.to_str().unwrap(),
            "-isystem",
            sdk_winrt_include.to_str().unwrap(),
            "-S",         // Emit assembly (LLVM IR in this case)
            "-emit-llvm", // Output LLVM IR instead of native assembly
            "-O0",        // No optimization to preserve all instructions for mutation
            "-o",
            ir_path.to_str().unwrap(),
            source_path.to_str().unwrap(),
        ];

        info!("Compiling source → IR: clang {}", args.join(" "));

        let output = tokio::process::Command::new("clang")
            .args(&args)
            .output()
            .await
            .context("Failed to execute clang for IR generation")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Clang IR generation failed:\n{}", stderr);
        }

        Ok(())
    }

    /// Compile LLVM IR to executable
    async fn compile_ir_to_exe(
        &self,
        ir_path: &Path,
        exe_path: &Path,
        template_name: &str,
    ) -> Result<()> {
        let xwin_root = PathBuf::from("/root/.xwin");
        let crt_lib = xwin_root.join("crt/lib/x86_64");
        let sdk_ucrt_lib = xwin_root.join("sdk/lib/ucrt/x86_64");
        let sdk_um_lib = xwin_root.join("sdk/lib/um/x86_64");

        let mut args = vec![
            "-target",
            "x86_64-pc-windows-msvc",
            "-L",
            crt_lib.to_str().unwrap(),
            "-L",
            sdk_ucrt_lib.to_str().unwrap(),
            "-L",
            sdk_um_lib.to_str().unwrap(),
            "-fuse-ld=lld",
            "-Wl,/subsystem:console",
            "-O0", // Keep -O0 to preserve mutated NOPs (don't optimize them out)
            "-Wl,-defaultlib:libcmt",
            "-Wl,-defaultlib:kernel32",
        ];

        // Add template-specific libraries
        let extra_libs = get_template_libs(template_name);
        for lib in extra_libs {
            args.push(lib);
        }

        args.push("-o");
        args.push(exe_path.to_str().unwrap());
        args.push(ir_path.to_str().unwrap());

        info!("Compiling IR → EXE: clang {}", args.join(" "));

        let output = tokio::process::Command::new("clang")
            .args(&args)
            .output()
            .await
            .context("Failed to execute clang for linking")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Clang linking failed:\n{}", stderr);
        }

        Ok(())
    }

    /// Get Clang version for metadata
    fn get_clang_version(&self) -> Result<String> {
        let output = std::process::Command::new("clang")
            .arg("--version")
            .output()
            .context("Failed to get clang version")?;

        if output.status.success() {
            let version_output = String::from_utf8_lossy(&output.stdout);
            // Extract first line (e.g., "clang version 17.0.6")
            Ok(version_output
                .lines()
                .next()
                .unwrap_or("unknown")
                .to_string())
        } else {
            Ok("unknown".to_string())
        }
    }

    /// Get compiler flags used for this template (for metadata)
    fn get_compiler_flags(&self, template_name: &str) -> Vec<String> {
        let mut flags = vec![
            "-target x86_64-pc-windows-msvc".to_string(),
            "-O2".to_string(),
            "-fuse-ld=lld".to_string(),
        ];

        // Add template-specific libs
        for lib in get_template_libs(template_name) {
            flags.push(lib.to_string());
        }

        flags
    }

    /// Build from LLVM IR (post-mutation)
    ///
    /// # Arguments
    /// * `ir_code` - LLVM IR bitcode or text IR
    /// * `artifact_name` - Name for the artifact (used for temp files and logging)
    ///
    /// # Returns
    /// Metadata about the built artifact
    ///
    /// # Workflow
    /// 1. Write IR to temporary .ll file
    /// 2. Compile IR → object file (clang -c)
    /// 3. Link object → exe (clang with xwin libs)
    /// 4. Compute SHA256 and move to artifacts/
    async fn build_from_llvm_ir(
        &self,
        ir_code: &[u8],
        artifact_name: &str,
    ) -> Result<BuiltArtifact> {
        info!("Building from LLVM IR: {}", artifact_name);

        // 1. Write IR to temp file
        let temp_ir = self.config.output_dir.join(format!("{}.ll", artifact_name));
        tokio::fs::write(&temp_ir, ir_code)
            .await
            .context("Failed to write IR to temp file")?;

        // 2. Compile IR to object file
        let temp_obj = self.config.output_dir.join(format!("{}.o", artifact_name));
        self.compile_ir_to_object(&temp_ir, &temp_obj).await?;

        // 3. Link object to executable
        let temp_exe = self
            .config
            .output_dir
            .join(format!("{}_temp.exe", artifact_name));
        self.link_object_to_exe(&temp_obj, &temp_exe, &[]).await?;

        // 4. Read, hash, and finalize
        let artifact_data = tokio::fs::read(&temp_exe)
            .await
            .context("Failed to read built artifact")?;
        let artifact_id = self.compute_sha256(&artifact_data);
        let final_output = self.config.output_dir.join(format!("{}.exe", artifact_id));

        tokio::fs::rename(&temp_exe, &final_output)
            .await
            .context("Failed to move artifact")?;

        // Cleanup temp files
        let _ = tokio::fs::remove_file(&temp_ir).await;
        let _ = tokio::fs::remove_file(&temp_obj).await;

        info!(
            "Artifact built from IR: {} ({} bytes)",
            artifact_id,
            artifact_data.len()
        );

        Ok(BuiltArtifact {
            artifact_id: artifact_id.clone(),
            source_path: temp_ir, // IR file path
            output_path: final_output,
            size_bytes: artifact_data.len() as u64,
            sha256: artifact_id,
            build_timestamp: chrono::Utc::now(),
            compiler_version: self.get_clang_version()?,
            compiler_flags: vec!["-target x86_64-pc-windows-msvc".to_string()],
            mutations_applied: vec![],
        })
    }

    /// Build from in-memory C source code
    async fn build_from_source_code(
        &self,
        source_code: &[u8],
        artifact_name: &str,
    ) -> Result<BuiltArtifact> {
        info!("Building from in-memory source: {}", artifact_name);

        // Write source to temp file
        let temp_source = self.config.output_dir.join(format!("{}.c", artifact_name));
        tokio::fs::write(&temp_source, source_code)
            .await
            .context("Failed to write source to temp file")?;

        let temp_exe = self
            .config
            .output_dir
            .join(format!("{}_temp.exe", artifact_name));

        // Compile (no template libs - caller must provide flags if needed)
        self.invoke_clang("", &temp_source, &temp_exe).await?;

        // Finalize
        let artifact_data = tokio::fs::read(&temp_exe).await?;
        let artifact_id = self.compute_sha256(&artifact_data);
        let final_output = self.config.output_dir.join(format!("{}.exe", artifact_id));

        tokio::fs::rename(&temp_exe, &final_output).await?;
        let _ = tokio::fs::remove_file(&temp_source).await;

        Ok(BuiltArtifact {
            artifact_id: artifact_id.clone(),
            source_path: temp_source,
            output_path: final_output,
            size_bytes: artifact_data.len() as u64,
            sha256: artifact_id,
            build_timestamp: chrono::Utc::now(),
            compiler_version: self.get_clang_version()?,
            compiler_flags: vec![],
            mutations_applied: vec![],
        })
    }

    /// Compile LLVM IR to object file
    async fn compile_ir_to_object(&self, ir_path: &Path, obj_path: &Path) -> Result<()> {
        let output = tokio::process::Command::new("clang")
            .args(&[
                "-target",
                "x86_64-pc-windows-msvc",
                "-c", // Compile only, don't link
                "-o",
                obj_path.to_str().context("Invalid obj path")?,
                ir_path.to_str().context("Invalid IR path")?,
            ])
            .output()
            .await
            .context("Failed to compile IR to object")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("IR compilation failed:\n{}", stderr);
        }

        Ok(())
    }

    /// Link object file to executable
    async fn link_object_to_exe(
        &self,
        obj_path: &Path,
        exe_path: &Path,
        extra_libs: &[&str],
    ) -> Result<()> {
        let xwin = &self.config.xwin_dir;

        let crt_lib = format!("{}/crt/lib/x86_64", xwin.display());
        let sdk_ucrt_lib = format!("{}/sdk/lib/ucrt/x86_64", xwin.display());
        let sdk_um_lib = format!("{}/sdk/lib/um/x86_64", xwin.display());
        let obj_str = obj_path.to_str().context("Invalid obj path")?;
        let exe_str = exe_path.to_str().context("Invalid exe path")?;

        let mut args = vec![
            "-target",
            "x86_64-pc-windows-msvc",
            "-fuse-ld=lld",
            "-Wl,/subsystem:console",
            "-L",
            crt_lib.as_str(),
            "-L",
            sdk_ucrt_lib.as_str(),
            "-L",
            sdk_um_lib.as_str(),
            "-Wl,-defaultlib:libcmt",
            "-Wl,-defaultlib:kernel32",
        ];

        for lib in extra_libs {
            args.push(lib);
        }

        args.push("-o");
        args.push(exe_str);
        args.push(obj_str);

        let output = tokio::process::Command::new("clang")
            .args(&args)
            .output()
            .await
            .context("Failed to link object to exe")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Linking failed:\n{}", stderr);
        }

        Ok(())
    }

    /// Apply instrumentation to an already-built artifact
    ///
    /// This re-instruments the binary by:
    /// 1. Reading the original source
    /// 2. Compiling to LLVM IR
    /// 3. Instrumenting the IR with the emitter
    /// 4. Compiling instrumented IR to object file
    /// 5. Linking with instrumentation runtime
    async fn apply_instrumentation(
        &self,
        built: BuiltArtifact,
        trace_mode_str: &str,
    ) -> Result<BuiltArtifact> {
        info!("Applying instrumentation: trace_mode={}", trace_mode_str);

        // Parse trace_mode string to build_emitter::TraceMode enum
        let trace_mode = match trace_mode_str {
            "off" => build_emitter::TraceMode::Off,
            "api" => build_emitter::TraceMode::Api,
            "bb" => build_emitter::TraceMode::BB,
            "api+bb" => build_emitter::TraceMode::ApiPlusBB,
            "lines" => build_emitter::TraceMode::Lines,
            "all" => build_emitter::TraceMode::All,
            _ => {
                warn!("Unknown trace_mode '{}', defaulting to 'api+bb'", trace_mode_str);
                build_emitter::TraceMode::ApiPlusBB
            }
        };

        if trace_mode == build_emitter::TraceMode::Off {
            info!("Instrumentation disabled (trace_mode=off)");
            return Ok(built);
        }

        // Step 1: Verify source exists
        if !built.source_path.exists() {
            anyhow::bail!("Source file not found for instrumentation: {:?}", built.source_path);
        }

        // Step 2: Compile source → LLVM IR
        let ir_path = built.source_path.with_extension("instrumented.ll");

        info!("Compiling source to LLVM IR for instrumentation...");
        let template_name = built.source_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        self.compile_source_to_ir(&built.source_path, &ir_path, template_name)
            .await
            .context("Failed to compile source to IR for instrumentation")?;

        // Step 3: Instrument the IR
        let instrumented_ir_path = built.source_path.with_extension("instrumented_final.ll");

        info!("Instrumenting LLVM IR with trace_mode={:?}...", trace_mode);
        let mut instrumenter = build_emitter::Instrumenter::new();
        instrumenter
            .instrument(&ir_path, trace_mode, &instrumented_ir_path)
            .await
            .context("Failed to instrument IR")?;

        // Clean up intermediate IR
        let _ = tokio::fs::remove_file(&ir_path).await;

        // Step 4: Compile instrumented IR → object file
        let obj_path = built.source_path.with_extension("instrumented.o");

        info!("Compiling instrumented IR to object file...");
        self.compile_ir_to_object(&instrumented_ir_path, &obj_path)
            .await
            .context("Failed to compile instrumented IR to object")?;

        // Clean up instrumented IR
        let _ = tokio::fs::remove_file(&instrumented_ir_path).await;

        // Step 5: Compile instrumentation runtime to object file (if not already compiled)
        let runtime_src = PathBuf::from("build/emitter/runtime/instrumentation_runtime.c");
        let runtime_obj = self.config.output_dir.join("instrumentation_runtime.o");

        if !runtime_obj.exists() {
            info!("Compiling instrumentation runtime...");
            self.compile_runtime(&runtime_src, &runtime_obj)
                .await
                .context("Failed to compile instrumentation runtime")?;
        }

        // Step 6: Link instrumented object + runtime → final executable
        let instrumented_exe_path = built.source_path.with_extension("instrumented.exe");

        info!("Linking instrumented binary with runtime...");
        self.link_instrumented_exe(&obj_path, &runtime_obj, &instrumented_exe_path, template_name)
            .await
            .context("Failed to link instrumented executable")?;

        // Clean up object file
        let _ = tokio::fs::remove_file(&obj_path).await;

        // Step 7: Move to output directory and update artifact metadata
        let instrumented_data = tokio::fs::read(&instrumented_exe_path)
            .await
            .context("Failed to read instrumented artifact")?;

        let instrumented_id = self.compute_sha256(&instrumented_data);
        let final_output = self.config.output_dir.join(format!("{}.exe", instrumented_id));

        tokio::fs::rename(&instrumented_exe_path, &final_output)
            .await
            .context("Failed to move instrumented artifact to output directory")?;

        info!(
            "Instrumented artifact built: {} ({} bytes) -> {:?}",
            instrumented_id,
            instrumented_data.len(),
            final_output
        );

        // Return new BuiltArtifact with updated paths and ID
        Ok(BuiltArtifact {
            artifact_id: instrumented_id.clone(),
            source_path: built.source_path,
            output_path: final_output,
            size_bytes: instrumented_data.len() as u64,
            sha256: instrumented_id,
            build_timestamp: chrono::Utc::now(),
            compiler_version: built.compiler_version,
            compiler_flags: built.compiler_flags,
            mutations_applied: built.mutations_applied,
        })
    }

    /// Compile instrumentation runtime C file to object file
    async fn compile_runtime(
        &self,
        runtime_src: &Path,
        runtime_obj: &Path,
    ) -> Result<()> {
        let output = tokio::process::Command::new("clang")
            .arg("-c") // Compile only (don't link)
            .arg(runtime_src)
            .arg("-o")
            .arg(runtime_obj)
            .arg("-target")
            .arg("x86_64-pc-windows-msvc")
            .arg("-fms-compatibility")
            .arg("-fms-extensions")
            .arg("-D_CRT_SECURE_NO_WARNINGS")
            .arg("-O2")
            .arg(format!("--sysroot={}", self.config.xwin_dir.display()))
            .output()
            .await
            .context("Failed to run clang for runtime compilation")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Runtime compilation failed:\n{}", stderr);
        }

        info!("Instrumentation runtime compiled: {:?}", runtime_obj);
        Ok(())
    }

    /// Link instrumented object file with runtime to create final executable
    async fn link_instrumented_exe(
        &self,
        obj_path: &Path,
        runtime_obj: &Path,
        output_exe: &Path,
        template_name: &str,
    ) -> Result<()> {
        let template_libs = get_template_libs(template_name);

        let mut cmd = tokio::process::Command::new("lld-link");
        cmd.arg(obj_path)
            .arg(runtime_obj) // Link with instrumentation runtime
            .arg("/out:".to_owned() + output_exe.to_str().unwrap())
            .arg("/subsystem:console")
            .arg("/machine:x64")
            .arg(format!("/libpath:{}/lib", self.config.xwin_dir.display()));

        // Add template-specific libraries
        for lib in template_libs {
            cmd.arg(format!("{}.lib", lib));
        }

        // Add standard Windows libraries
        cmd.arg("kernel32.lib")
            .arg("user32.lib")
            .arg("advapi32.lib")
            .arg("ws2_32.lib");

        let output = cmd
            .output()
            .await
            .context("Failed to link instrumented executable")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Linking instrumented executable failed:\n{}", stderr);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_libs() {
        assert_eq!(get_template_libs("loader_v1").len(), 0);
        assert_eq!(get_template_libs("rwx_direct").len(), 2);
        assert_eq!(get_template_libs("network_beacon").len(), 1);
    }

    #[test]
    fn test_compute_sha256() {
        let config = BuilderConfig::default();
        let builder = ArtifactBuilder { config };
        let hash = builder.compute_sha256(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
