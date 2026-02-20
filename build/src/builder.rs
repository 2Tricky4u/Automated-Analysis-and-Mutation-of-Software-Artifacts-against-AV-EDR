/// Artifact Builder - Clang cross-compilation wrapper
///
/// Implements BUILD-DEPLOY-EXECUTE-PIPELINE.md Section 1: Build on Controller (WSL)
///
/// Compiles C templates to Windows PE executables using Clang with xwin SDK.
/// Uses the same flags and dependencies as corpus/templates/build_all.sh.
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

// Use modules from crate root
use crate::mutator;
use crate::template::assembler::{Assembler, ModuleSelection};
use crate::template::payload::{EncodingType, PayloadEncoder};
use crate::transform::BinaryMutator;

/// Configuration for the artifact builder
#[derive(Debug, Clone)]
pub struct BuilderConfig {
    /// Path to output directory for artifacts (e.g., "artifacts")
    pub output_dir: PathBuf,
    /// Path to xwin SDK (e.g., "/root/.xwin")
    pub xwin_dir: PathBuf,
    /// Path to instrumentation runtime source
    pub runtime_src: PathBuf,
    /// Path to minimal runtime source
    pub minimal_runtime_src: PathBuf,
    /// Path to modular template directory
    /// Contains loader_template.c and modules/ subdirectory
    pub modular_template_dir: PathBuf,
}

impl Default for BuilderConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("artifacts"),
            xwin_dir: PathBuf::from("/root/.xwin"),
            runtime_src: PathBuf::from("build/runtime/instrumentation_runtime.c"),
            minimal_runtime_src: PathBuf::from("build/runtime/minimal_runtime.c"),
            modular_template_dir: PathBuf::from("build/templates"),
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
/// Returns base library names (without .lib extension or -Wl prefix)
/// Caller is responsible for formatting for specific linker
fn get_template_libs(_template_name: &str) -> &'static [&'static str] {
    // Modular templates link all needed libs via the standard flags;
    // no per-template overrides are required.
    &[]
}

/// Format template-specific library names as clang `-Wl,-defaultlib:name` args
fn format_template_lib_args(template_name: &str) -> Vec<String> {
    get_template_libs(template_name)
        .iter()
        .map(|lib| format!("-Wl,-defaultlib:{}", lib))
        .collect()
}

/// Input format for artifact building
#[derive(Debug, Clone)]
pub enum BuildInput {
    /// Build from modular template assembly using @MODULE markers
    ModularTemplate {
        /// Module selection (carrier, decoder, antiemulation, etc.)
        modules: ModuleSelection,
        /// Raw payload bytes to encode
        payload: Vec<u8>,
        /// Encoding type for the payload
        encoding: EncodingType,
        /// AST mutations to apply at @MUTATE marker locations
        mutations: Vec<mutator::MutationSpec>,
        /// Instrumentation mode: "off" | "lines" | "api" | "bb" | "api+bb" | "all"
        trace_mode: String,
    },
}

/// Artifact builder
pub struct ArtifactBuilder {
    config: BuilderConfig,
    xwin: XwinPaths,
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

        let xwin = XwinPaths::new(&config.xwin_dir);
        Ok(Self { config, xwin })
    }

    /// Build artifact from modular template input
    ///
    /// # Arguments
    /// * `input` - Modular template specification (modules, payload, encoding, mutations, trace)
    ///
    /// # Returns
    /// Metadata about the built artifact
    pub async fn build(&self, input: BuildInput) -> Result<BuiltArtifact> {
        let BuildInput::ModularTemplate {
            modules,
            payload,
            encoding,
            mutations,
            trace_mode,
        } = input;

        debug!(
            "Building modular template with carrier={}, decoder={}, trace_mode={}",
            modules.carrier, modules.decoder, trace_mode
        );

        self.build_modular_template(modules, &payload, encoding, &mutations, &trace_mode)
            .await
    }

    /// Compile minimal_runtime.o if it doesn't already exist. Returns the object path.
    async fn ensure_minimal_runtime(&self) -> Result<PathBuf> {
        let obj = self.config.output_dir.join("minimal_runtime.o");
        if !obj.exists() {
            debug!("Compiling minimal runtime (for __runtime_exit)...");
            self.compile_runtime(&self.config.minimal_runtime_src, &obj)
                .await
                .context("Failed to compile minimal runtime")?;
        }
        Ok(obj)
    }

    /// Invoke Clang with xwin SDK to cross-compile C to Windows PE with optional runtime linking
    async fn invoke_clang_internal(
        &self,
        template_name: &str,
        source: &Path,
        output: &Path,
        link_runtime: bool,
    ) -> Result<()> {
        let xwin = &self.xwin;

        let output_str = output.to_str().context("Invalid output path")?;
        let source_str = source.to_str().context("Invalid source path")?;

        // Pre-format runtime include path (always needed for instrumentation.h header)
        let runtime_include_str = format!(
            "{}",
            self.config
                .runtime_src
                .parent()
                .context("Invalid runtime source path")?
                .display()
        );

        // Base flags
        let mut args = vec!["-target", "x86_64-pc-windows-msvc"];
        args.extend(xwin.include_args());
        args.extend(xwin.lib_args());
        args.extend_from_slice(&[
            // TODO could use MSVC linker (link.exe , WINE?), not lld-link
            //"-fuse-ld=link",
            "-fuse-ld=lld",
            // Codegen
            "-O2",
            // --- link.exe flags for "MSVC-ish" release ---
            "-Wl,/subsystem:console",
            // no debug directory / no PDB
            "-Wl,/DEBUG:NONE",
            // reproducible-ish + no incremental
            "-Wl,/Brepro",
            "-Wl,/INCREMENTAL:NO",
            // typical release link opts
            "-Wl,/OPT:REF",
            "-Wl,/OPT:ICF",
            // Keep consistent runtime model: /MT == libcmt (static CRT)
            // (If you prefer /MD, replace libcmt with msvcrt and ensure matching libs everywhere.)
            "-Wl,-defaultlib:libcmt",
            // Keep the system libs you truly need
            "-Wl,-defaultlib:kernel32",
        ]);

        // Always add instrumentation header path (needed for instrumentation.h)
        args.push("-I");
        args.push(runtime_include_str.as_str());

        // Define ENABLE_INSTRUMENTATION macro only when runtime will be linked
        if link_runtime {
            args.push("-DENABLE_INSTRUMENTATION");
        }

        // Add template-specific libraries (format for clang wrapper: -Wl,-defaultlib:name)
        let formatted_lib_args = format_template_lib_args(template_name);
        for lib_arg in &formatted_lib_args {
            args.push(lib_arg.as_str());
        }

        // Compile minimal_runtime.o only when instrumentation is enabled.
        // It provides __runtime_exit() (direct syscall termination + telemetry flush).
        // For trace=off (baseline) builds, we skip it so the binary doesn't contain
        // direct syscall patterns that taint ground-truth EDR measurements.
        let minimal_runtime_str = if link_runtime {
            let obj = self.ensure_minimal_runtime().await?;
            Some(obj.to_string_lossy().into_owned())
        } else {
            None
        };

        // If linking with instrumentation, compile instrumentation_runtime too
        let runtime_obj_str = if link_runtime {
            let runtime_obj = self.config.output_dir.join("instrumentation_runtime.o");

            // Compile runtime if not already compiled
            if !runtime_obj.exists() {
                debug!("Compiling instrumentation runtime for direct linking...");
                self.compile_runtime(&self.config.runtime_src, &runtime_obj)
                    .await
                    .context("Failed to compile instrumentation runtime")?;
            }

            Some(runtime_obj.to_string_lossy().into_owned())
        } else {
            None
        };

        // Add output and source
        args.push("-o");
        args.push(output_str);
        args.push(source_str);

        // Add minimal_runtime.o when instrumentation is enabled (telemetry flush + syscall exit)
        if let Some(ref minimal_str) = minimal_runtime_str {
            args.push(minimal_str.as_str());
        }

        // Add instrumentation_runtime.o if instrumentation is enabled
        if let Some(ref runtime_str) = runtime_obj_str {
            args.push(runtime_str.as_str());
        }

        debug!("Invoking: clang {}", args.join(" "));

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

    /// Read a temp artifact, compute its SHA256, and rename it to `<sha256>.exe` in output_dir.
    ///
    /// Returns `(artifact_id, final_path, size_bytes)`.
    async fn finalize_artifact(&self, temp_path: &Path) -> Result<(String, PathBuf, u64)> {
        let data = tokio::fs::read(temp_path)
            .await
            .context("Failed to read built artifact")?;

        let artifact_id = self.compute_sha256(&data);
        let final_output = self.config.output_dir.join(format!("{}.exe", artifact_id));

        tokio::fs::rename(temp_path, &final_output)
            .await
            .context("Failed to move artifact to output directory")?;

        Ok((artifact_id, final_output, data.len() as u64))
    }

    /// Compile C source to LLVM IR
    async fn compile_source_to_ir(
        &self,
        source_path: &Path,
        ir_path: &Path,
        enable_instrumentation: bool,
    ) -> Result<()> {
        let xwin = &self.xwin;

        // Get runtime include path for instrumentation.h
        let runtime_include = self
            .config
            .runtime_src
            .parent()
            .context("Invalid runtime source path")?
            .to_str()
            .context("Runtime include path is not valid UTF-8")?;

        let mut args = vec!["-target", "x86_64-pc-windows-msvc"];
        args.extend(xwin.include_args());
        args.extend_from_slice(&[
            "-I",
            runtime_include, // Add instrumentation header path
        ]);
        if enable_instrumentation {
            args.push("-DENABLE_INSTRUMENTATION");
        }
        args.extend_from_slice(&[
            "-S",         // Emit assembly (LLVM IR in this case)
            "-emit-llvm", // Output LLVM IR instead of native assembly
            "-O0",        // No optimization to preserve all instructions for mutation
            "-o",
            ir_path.to_str().unwrap(),
            source_path.to_str().unwrap(),
        ]);

        debug!("Compiling source -> IR: clang {}", args.join(" "));

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
        let xwin = &self.xwin;

        let mut args = vec!["-target", "x86_64-pc-windows-msvc"];
        args.extend(xwin.lib_args());
        args.extend_from_slice(&[
            "-fuse-ld=lld",
            "-Wl,/subsystem:console",
            "-O0", // Keep -O0 to preserve mutated NOPs (don't optimize them out)
            "-Wl,-defaultlib:libcmt",
            "-Wl,-defaultlib:kernel32",
        ]);

        // Add template-specific libraries (format for clang wrapper: -Wl,-defaultlib:name)
        let formatted_lib_args = format_template_lib_args(template_name);
        for lib_arg in &formatted_lib_args {
            args.push(lib_arg.as_str());
        }

        args.push("-o");
        args.push(exe_path.to_str().unwrap());
        args.push(ir_path.to_str().unwrap());

        debug!("Compiling IR -> EXE: clang {}", args.join(" "));

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

    /// Compile LLVM IR to object file
    async fn compile_ir_to_object(&self, ir_path: &Path, obj_path: &Path) -> Result<()> {
        let output = tokio::process::Command::new("clang")
            .args([
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
        debug!("Applying instrumentation: trace_mode={}", trace_mode_str);

        // Parse trace_mode string to crate::TraceMode enum
        let trace_mode = match trace_mode_str {
            "off" => crate::TraceMode::Off,
            "api" => crate::TraceMode::Api,
            "bb" => crate::TraceMode::BB,
            "api+bb" => crate::TraceMode::ApiPlusBB,
            "lines" => crate::TraceMode::Lines,
            "all" => crate::TraceMode::All,
            _ => {
                warn!(
                    "Unknown trace_mode '{}', defaulting to 'lines'",
                    trace_mode_str
                );
                crate::TraceMode::Lines
            }
        };

        if trace_mode == crate::TraceMode::Off {
            debug!("Instrumentation disabled (trace_mode=off)");
            return Ok(built);
        }

        // Step 1: Verify source exists
        if !built.source_path.exists() {
            anyhow::bail!(
                "Source file not found for instrumentation: {:?}",
                built.source_path
            );
        }

        // Step 1.5: Apply AST-level line tracing (if enabled)
        let source_for_compilation = if trace_mode == crate::TraceMode::Lines
            || trace_mode == crate::TraceMode::All
        {
            debug!("Applying AST-level line tracing to source code (Binary protocol format)...");

            // Read original source
            let original_source = tokio::fs::read_to_string(&built.source_path)
                .await
                .context("Failed to read source file for line tracing")?;

            // Detect language from file extension
            let language = crate::SourceLanguage::from_path(&built.source_path);

            // Convert source path to string for embedding in trace calls
            let file_path_str = built.source_path.to_string_lossy();

            // Inject line traces at AST level with actual file path
            let instrumented_source = crate::inject_line_traces_with_opts(
                &original_source,
                language,
                &file_path_str,
                crate::TraceFormat::default(),
            )
            .context("Failed to inject line traces at AST level")?;

            // Count how many trace calls were injected
            let trace_call_count = instrumented_source.matches("__trace_line_binary(").count();

            // Write instrumented source to temporary file
            let instrumented_source_path = built.source_path.with_extension("line_traced.c");
            tokio::fs::write(&instrumented_source_path, &instrumented_source)
                .await
                .context("Failed to write line-traced source")?;

            debug!(
                "AST line tracing complete: injected {} trace calls into {:?}",
                trace_call_count, instrumented_source_path
            );
            instrumented_source_path
        } else {
            // No line tracing, use original source
            built.source_path.clone()
        };

        // Step 2: Compile source -> LLVM IR
        let ir_path = source_for_compilation.with_extension("instrumented.ll");

        debug!("Compiling source to LLVM IR for instrumentation...");
        let template_name = built
            .source_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        self.compile_source_to_ir(&source_for_compilation, &ir_path, true)
            .await
            .context("Failed to compile source to IR for instrumentation")?;

        // Step 3: Instrument the IR
        let instrumented_ir_path = built.source_path.with_extension("instrumented_final.ll");

        debug!("Instrumenting LLVM IR with trace_mode={:?}...", trace_mode);
        let mut instrumenter = crate::Instrumenter::new();
        instrumenter
            .instrument(&ir_path, trace_mode, &instrumented_ir_path)
            .await
            .context("Failed to instrument IR")?;

        // Clean up intermediate IR
        let _ = tokio::fs::remove_file(&ir_path).await;

        // Clean up line-traced source (if it was created)
        if source_for_compilation != built.source_path {
            let _ = tokio::fs::remove_file(&source_for_compilation).await;
        }

        // Step 4: Compile instrumented IR -> object file
        let obj_path = built.source_path.with_extension("instrumented.o");

        debug!("Compiling instrumented IR to object file...");
        self.compile_ir_to_object(&instrumented_ir_path, &obj_path)
            .await
            .context("Failed to compile instrumented IR to object")?;

        // Clean up instrumented IR
        let _ = tokio::fs::remove_file(&instrumented_ir_path).await;

        // Step 5: Compile instrumentation runtime to object file (if not already compiled)
        let runtime_src = &self.config.runtime_src;
        let runtime_obj = self.config.output_dir.join("instrumentation_runtime.o");

        // Check if runtime source exists
        if !runtime_src.exists() {
            anyhow::bail!(
                "Instrumentation runtime source not found: {:?}\nExpected at: {:?}\nCurrent working directory: {:?}",
                runtime_src,
                runtime_src
                    .canonicalize()
                    .unwrap_or_else(|_| runtime_src.clone()),
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("unknown"))
            );
        }

        if !runtime_obj.exists() {
            debug!("Compiling instrumentation runtime...");
            self.compile_runtime(runtime_src, &runtime_obj)
                .await
                .context("Failed to compile instrumentation runtime")?;
        }

        // Step 5.5: Verify runtime has required symbols for line tracing (non-fatal)
        if (trace_mode == crate::TraceMode::Lines || trace_mode == crate::TraceMode::All)
            && let Err(e) = self.verify_runtime_symbols(&runtime_obj, trace_mode).await
        {
            warn!("Runtime symbol verification failed (non-fatal): {}", e);
            warn!("Build will continue, but linking may fail if symbols are missing");
        }

        // Step 6: Link instrumented object + runtime -> final executable
        let instrumented_exe_path = built.source_path.with_extension("instrumented.exe");

        debug!("Linking instrumented binary with runtime...");
        self.link_instrumented_exe(
            &obj_path,
            &runtime_obj,
            &instrumented_exe_path,
            template_name,
        )
        .await
        .context("Failed to link instrumented executable")?;

        // Clean up object file
        let _ = tokio::fs::remove_file(&obj_path).await;

        // Step 7: Verify instrumented executable exists and has reasonable size
        if !instrumented_exe_path.exists() {
            anyhow::bail!(
                "Instrumented executable not found at {:?}",
                instrumented_exe_path
            );
        }

        let instrumented_data = tokio::fs::read(&instrumented_exe_path)
            .await
            .context("Failed to read instrumented artifact")?;

        // Sanity check: instrumented binary should be larger than original, not smaller
        if instrumented_data.len() < built.size_bytes as usize {
            error!(
                "WARNING: Instrumented binary ({} bytes) is SMALLER than original ({} bytes)! This indicates a build error.",
                instrumented_data.len(),
                built.size_bytes
            );
            error!("Instrumented path: {:?}", instrumented_exe_path);
            error!("Original path: {:?}", built.output_path);

            // Check if object file exists (might have been left behind)
            if obj_path.exists() {
                let obj_size = tokio::fs::metadata(&obj_path).await?.len();
                error!(
                    "Object file still exists: {:?} ({} bytes)",
                    obj_path, obj_size
                );
            }

            anyhow::bail!(
                "Instrumented binary is suspiciously small ({} bytes vs {} bytes original). Check linker output.",
                instrumented_data.len(),
                built.size_bytes
            );
        }

        // Finalize: hash, rename, metadata
        // (can't use finalize_artifact because data is already read for size check)
        let instrumented_id = self.compute_sha256(&instrumented_data);
        let final_output = self
            .config
            .output_dir
            .join(format!("{}.exe", instrumented_id));

        tokio::fs::rename(&instrumented_exe_path, &final_output)
            .await
            .context("Failed to move instrumented artifact to output directory")?;

        info!(
            "Instrumented artifact built: {} ({} bytes, original was {} bytes) -> {:?}",
            instrumented_id,
            instrumented_data.len(),
            built.size_bytes,
            final_output
        );

        Ok(build_artifact_metadata(
            instrumented_id,
            built.source_path,
            final_output,
            instrumented_data.len() as u64,
            built.compiler_version,
            built.compiler_flags,
            built.mutations_applied,
        ))
    }

    /// Compile instrumentation runtime C file to object file
    async fn compile_runtime(&self, runtime_src: &Path, runtime_obj: &Path) -> Result<()> {
        // Build the command
        let mut cmd = tokio::process::Command::new("clang");
        cmd.arg("-c") // Compile only (don't link)
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
            // Add explicit include paths for xwin SDK
            .arg(format!("-I{}/crt/include", self.config.xwin_dir.display()))
            .arg(format!(
                "-I{}/sdk/include/ucrt",
                self.config.xwin_dir.display()
            ))
            .arg(format!(
                "-I{}/sdk/include/um",
                self.config.xwin_dir.display()
            ))
            .arg(format!(
                "-I{}/sdk/include/shared",
                self.config.xwin_dir.display()
            ));

        // Log the command for debugging
        debug!(
            "Compiling runtime: clang -c {} -o {} -target x86_64-pc-windows-msvc --sysroot={} -I.../crt/include -I.../sdk/include/{{ucrt,um,shared}}",
            runtime_src.display(),
            runtime_obj.display(),
            self.config.xwin_dir.display()
        );

        let output = cmd
            .output()
            .await
            .context("Failed to run clang for runtime compilation")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            anyhow::bail!(
                "Runtime compilation failed:\nSTDOUT:\n{}\nSTDERR:\n{}\nRuntime source: {:?}\nXwin dir: {:?}",
                stdout,
                stderr,
                runtime_src,
                self.config.xwin_dir
            );
        }

        debug!("Instrumentation runtime compiled: {:?}", runtime_obj);
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

        // Use full path to lld-link (WSL has it at /usr/lib/llvm-17/bin/lld-link)
        let lld_link_path = if cfg!(target_os = "linux") {
            "/usr/lib/llvm-17/bin/lld-link"
        } else {
            "lld-link" // Fallback to PATH lookup on other platforms
        };

        // ALWAYS compile minimal_runtime.o (provides __runtime_exit)
        let minimal_runtime_obj = self.ensure_minimal_runtime().await?;

        let mut cmd = tokio::process::Command::new(lld_link_path);
        cmd.arg(obj_path)
            .arg(runtime_obj) // Link with instrumentation runtime
            .arg(&minimal_runtime_obj) // ALWAYS link minimal runtime
            .arg("/out:".to_owned() + output_exe.to_str().unwrap())
            .arg("/subsystem:console")
            .arg("/machine:x64");
        // Add xwin library paths (CRT and Windows SDK)
        for arg in self.xwin.lld_lib_args() {
            cmd.arg(arg);
        }

        // Add template-specific libraries
        for lib in template_libs {
            cmd.arg(format!("{}.lib", lib));
        }

        // Add standard Windows libraries
        cmd.arg("kernel32.lib")
            .arg("user32.lib")
            .arg("advapi32.lib")
            .arg("ws2_32.lib")
            .arg("libcmt.lib") // Static C runtime (must match clang builds that use -Wl,-defaultlib:libcmt)
            .arg("libucrt.lib"); // Universal CRT

        // Log the FULL command for debugging (including all library arguments)
        let full_cmd = format!("{:?}", cmd);
        debug!("Full linking command: {}", full_cmd);

        let output = cmd.output().await.context("Failed to run lld-link")?;

        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);

        // Always log linker output (even if empty) to diagnose issues
        debug!(
            "Linker stderr: [{}]",
            if stderr.is_empty() { "empty" } else { &stderr }
        );
        debug!(
            "Linker stdout: [{}]",
            if stdout.is_empty() { "empty" } else { &stdout }
        );

        if !output.status.success() {
            anyhow::bail!(
                "Linking instrumented executable failed:\nSTDERR:\n{}\nSTDOUT:\n{}",
                stderr,
                stdout
            );
        }

        // Verify output file was created and has reasonable size
        if !output_exe.exists() {
            anyhow::bail!(
                "Linker succeeded but output file not found: {:?}",
                output_exe
            );
        }

        let output_size = tokio::fs::metadata(output_exe).await?.len();
        debug!(
            "Linked executable created: {:?} ({} bytes)",
            output_exe, output_size
        );

        if output_size < 10000 {
            warn!(
                "WARNING: Linked executable is very small ({} bytes). This may indicate a linker issue.",
                output_size
            );
        }

        Ok(())
    }

    /// Build artifact from modular template assembly
    ///
    /// This is the preferred method for new artifacts. It uses the two-level mutation system:
    /// 1. @MODULE markers in loader_template.c are replaced with selected module code
    /// 2. @MUTATE markers within modules guide AST transformations
    ///
    /// # Arguments
    /// * `modules` - Module selection (carrier, decoder, antiemulation, etc.)
    /// * `payload` - Raw payload bytes to encode
    /// * `encoding` - Encoding type for the payload (XOR or English)
    /// * `mutations` - AST mutations to apply at @MUTATE marker locations
    /// * `trace_mode` - Instrumentation mode
    ///
    /// # Returns
    /// Metadata about the built artifact
    async fn build_modular_template(
        &self,
        mut modules: ModuleSelection,
        payload: &[u8],
        encoding: EncodingType,
        mutations: &[mutator::MutationSpec],
        trace_mode: &str,
    ) -> Result<BuiltArtifact> {
        // Step 0: Sync decoder module to match encoding type
        let expected_decoder = encoding.decoder_module();
        if modules.decoder != expected_decoder {
            info!(
                "Auto-syncing decoder '{}' → '{}' to match encoding",
                modules.decoder, expected_decoder
            );
            modules.decoder = expected_decoder.to_string();
        }

        // Step 1: Encode the payload
        let encoder = PayloadEncoder::new();
        let encoded = encoder.encode(payload, encoding);
        let payload_header = encoder.generate_c_header(&encoded);

        debug!(
            "Encoded payload: {} bytes -> {} encoded bytes ({})",
            payload.len(),
            encoded.data.len(),
            match encoding {
                EncodingType::Xor => "XOR",
                EncodingType::English => "English",
            }
        );

        // Step 2: Use template directory from config
        let template_dir = &self.config.modular_template_dir;

        if !template_dir.exists() {
            anyhow::bail!(
                "Modular template directory not found at {:?}.",
                template_dir
            );
        }

        // Step 3: Assemble the template with selected modules
        let mut assembler = Assembler::new(template_dir).context("Failed to create assembler")?;

        let assembled_source = assembler
            .assemble(&modules, &payload_header)
            .context("Failed to assemble template")?;

        info!(
            "Assembled template: {} bytes (carrier={}, decoder={}, antiemulation={}, deconditioner={}, guardrail={})",
            assembled_source.len(),
            modules.carrier,
            modules.decoder,
            modules.antiemulation,
            modules.deconditioner,
            modules.guardrail
        );

        // Step 4: Partition mutations into AST and LLVM, apply AST mutations
        let ast_mutations: Vec<_> = mutations
            .iter()
            .filter(|m| !m.id.starts_with("llvm."))
            .cloned()
            .collect();
        let llvm_mutations: Vec<_> = mutations
            .iter()
            .filter(|m| m.id.starts_with("llvm."))
            .cloned()
            .collect();
        let has_llvm_mutations = !llvm_mutations.is_empty();

        // Apply AST mutations to C source (or just strip markers)
        let (final_source, mut mutations_applied) = if !ast_mutations.is_empty() {
            let source_bytes = assembled_source.as_bytes().to_vec();
            let (mutated, applied) = mutator::Mutator::apply(&source_bytes, &ast_mutations)?;
            debug!("Applied {} AST mutations: {:?}", applied.len(), applied);
            let stripped = crate::template::assembler::strip_mutation_markers(
                &String::from_utf8_lossy(&mutated),
            );
            (stripped, applied)
        } else {
            (
                crate::template::assembler::strip_mutation_markers(&assembled_source),
                Vec::new(),
            )
        };

        // Step 5: Build the assembled source
        let artifact_name = format!(
            "modular_{}_{}_{}",
            modules.carrier,
            modules.decoder,
            Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("unknown")
        );

        let temp_source = self.config.output_dir.join(format!("{}.c", artifact_name));

        tokio::fs::write(&temp_source, &final_source)
            .await
            .context("Failed to write assembled source")?;

        debug!(
            "Wrote assembled source to {:?} ({} bytes)",
            temp_source,
            final_source.len()
        );

        let needs_runtime = trace_mode != "off" && !trace_mode.is_empty();

        let unique_id = Uuid::new_v4();
        let temp_output = self
            .config
            .output_dir
            .join(format!("{}.{}.exe", artifact_name, unique_id));

        if has_llvm_mutations {
            // --- IR mutation path: C → IR → mutate IR → EXE ---
            let temp_ir = self.config.output_dir.join(format!("{}.ll", artifact_name));

            // Compile C source to LLVM IR (without instrumentation flag —
            // ARTIFACT_CHECKPOINT expands to ((void)0) so no unresolved symbols)
            self.compile_source_to_ir(&temp_source, &temp_ir, false)
                .await
                .context("Failed to compile source to IR for LLVM mutations")?;

            // Apply LLVM mutations to the IR
            let ir_content = tokio::fs::read(&temp_ir)
                .await
                .context("Failed to read LLVM IR for mutation")?;

            let (mutated_ir, llvm_applied) = mutator::Mutator::apply(&ir_content, &llvm_mutations)?;
            debug!(
                "Applied {} LLVM mutations: {:?}",
                llvm_applied.len(),
                llvm_applied
            );
            mutations_applied.extend(llvm_applied);

            // Write mutated IR back
            tokio::fs::write(&temp_ir, &mutated_ir)
                .await
                .context("Failed to write mutated IR")?;

            // Compile IR → EXE at -O0 (preserves NOP inserts, opaque predicates)
            self.compile_ir_to_exe(&temp_ir, &temp_output, "modular")
                .await
                .context("Failed to compile mutated IR to executable")?;

            // Clean up IR file (keep .c for potential instrumentation)
            let _ = tokio::fs::remove_file(&temp_ir).await;
        } else {
            // --- Direct path: C → EXE (unchanged from original) ---
            self.invoke_clang_internal("modular", &temp_source, &temp_output, needs_runtime)
                .await
                .context("Failed to compile assembled template")?;

            // Clean up temp source (no IR path needs it)
            let _ = tokio::fs::remove_file(&temp_source).await;
        }

        // Step 5b: Apply binary mutations (post-link PE transforms)
        let binary_mutations: Vec<&mutator::MutationSpec> = mutations
            .iter()
            .filter(|m| m.id.starts_with("binary."))
            .collect();

        if !binary_mutations.is_empty() {
            let pe_bytes = tokio::fs::read(&temp_output)
                .await
                .context("Failed to read PE for binary mutations")?;

            let mutator = BinaryMutator::new(pe_bytes);
            let (mutated_pe, binary_applied) = mutator
                .apply(&binary_mutations)
                .context("Binary mutations failed")?;

            debug!(
                "Applied {} binary mutations: {:?}",
                binary_applied.len(),
                binary_applied
            );
            mutations_applied.extend(binary_applied);

            tokio::fs::write(&temp_output, &mutated_pe)
                .await
                .context("Failed to write binary-mutated PE")?;
        }

        // Step 6: Finalize artifact
        let (artifact_id, final_output, size_bytes) = self.finalize_artifact(&temp_output).await?;

        info!(
            "Modular artifact built: {} ({} bytes) -> {:?}",
            artifact_id, size_bytes, final_output
        );

        let compiler_version = self.get_clang_version()?;

        let built = build_artifact_metadata(
            artifact_id,
            temp_source.clone(),
            final_output,
            size_bytes,
            compiler_version,
            self.get_compiler_flags("modular"),
            mutations_applied,
        );

        // Step 7: Apply instrumentation if needed
        if needs_runtime {
            if has_llvm_mutations {
                warn!(
                    "LLVM mutations + instrumentation: the instrumented binary will be \
                     re-compiled from AST-mutated source without IR mutations. \
                     Per the two-run differential protocol, Run A (instrumented) is for \
                     observation; Run B (baseline with IR mutations) is ground truth."
                );
            }

            // Write source for instrumentation (IR path kept .c, direct path deleted it)
            if !tokio::fs::try_exists(&temp_source).await.unwrap_or(false) {
                tokio::fs::write(&temp_source, &final_source)
                    .await
                    .context("Failed to re-write source for instrumentation")?;
            }

            let mut built_with_source = built;
            built_with_source.source_path = temp_source;

            let instrumented = self
                .apply_instrumentation(built_with_source, trace_mode)
                .await?;

            let _ = tokio::fs::remove_file(&instrumented.source_path).await;

            Ok(instrumented)
        } else {
            // Clean up .c if IR path left it around
            if has_llvm_mutations {
                let _ = tokio::fs::remove_file(&temp_source).await;
            }
            Ok(built)
        }
    }

    /// Verify that the runtime object file contains required symbols for the trace mode
    async fn verify_runtime_symbols(
        &self,
        runtime_obj: &Path,
        trace_mode: crate::TraceMode,
    ) -> Result<()> {
        debug!(
            "Verifying runtime object has required symbols for trace mode: {:?}",
            trace_mode
        );

        // Use llvm-nm to list symbols in the object file (full path to LLVM 17 tools)
        let nm_path = if cfg!(target_os = "linux") {
            "/usr/lib/llvm-17/bin/llvm-nm"
        } else {
            "llvm-nm"
        };

        let output = tokio::process::Command::new(nm_path)
            .arg(runtime_obj)
            .output()
            .await
            .context("Failed to run llvm-nm to check runtime symbols")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("llvm-nm failed to read runtime object:\n{}", stderr);
        }

        let symbols = String::from_utf8_lossy(&output.stdout);

        // Check for required symbols based on trace mode
        let mut missing_symbols = Vec::new();

        if trace_mode == crate::TraceMode::Lines || trace_mode == crate::TraceMode::All {
            // Binary protocol is the default, check for __trace_line_binary
            if !symbols.contains("__trace_line_binary") {
                missing_symbols.push("__trace_line_binary");
            }
        }

        if !missing_symbols.is_empty() {
            warn!(
                "Runtime object file is missing required symbols: {:?}",
                missing_symbols
            );
            warn!("Runtime path: {:?}", runtime_obj);
            warn!("This usually means the runtime source is outdated or wasn't recompiled.");
            warn!("Solution: Remove the cached runtime object to force recompilation:");
            warn!("  rm -f {:?}", runtime_obj);
            anyhow::bail!("Runtime object file missing symbols: {:?}", missing_symbols);
        }

        debug!("Runtime symbol verification passed: all required symbols present");
        Ok(())
    }
}

/// Resolved xwin SDK paths for cross-compilation
#[derive(Debug, Clone)]
pub struct XwinPaths {
    pub crt_include: String,
    pub sdk_ucrt_include: String,
    pub sdk_shared_include: String,
    pub sdk_um_include: String,
    pub sdk_winrt_include: String,
    pub crt_lib: String,
    pub sdk_ucrt_lib: String,
    pub sdk_um_lib: String,
}

impl XwinPaths {
    /// Build all xwin SDK paths from the root directory
    pub fn new(xwin_dir: &Path) -> Self {
        let xwin = xwin_dir.display();
        Self {
            crt_include: format!("{}/crt/include", xwin),
            sdk_ucrt_include: format!("{}/sdk/include/ucrt", xwin),
            sdk_shared_include: format!("{}/sdk/include/shared", xwin),
            sdk_um_include: format!("{}/sdk/include/um", xwin),
            sdk_winrt_include: format!("{}/sdk/include/winrt", xwin),
            crt_lib: format!("{}/crt/lib/x86_64", xwin),
            sdk_ucrt_lib: format!("{}/sdk/lib/ucrt/x86_64", xwin),
            sdk_um_lib: format!("{}/sdk/lib/um/x86_64", xwin),
        }
    }

    /// Return the include `-isystem` args in order
    pub fn include_args(&self) -> Vec<&str> {
        vec![
            "-isystem",
            &self.crt_include,
            "-isystem",
            &self.sdk_ucrt_include,
            "-isystem",
            &self.sdk_shared_include,
            "-isystem",
            &self.sdk_um_include,
            "-isystem",
            &self.sdk_winrt_include,
        ]
    }

    /// Return the library `-L` args in order
    pub fn lib_args(&self) -> Vec<&str> {
        vec![
            "-L",
            &self.crt_lib,
            "-L",
            &self.sdk_ucrt_lib,
            "-L",
            &self.sdk_um_lib,
        ]
    }

    /// Return `/libpath:` args for lld-link
    pub fn lld_lib_args(&self) -> Vec<String> {
        vec![
            format!("/libpath:{}", self.crt_lib),
            format!("/libpath:{}", self.sdk_ucrt_lib),
            format!("/libpath:{}", self.sdk_um_lib),
        ]
    }
}

/// Build `BuiltArtifact` metadata from raw artifact data and context.
///
/// This is the common pattern extracted from 5 finalization sites.
fn build_artifact_metadata(
    artifact_id: String,
    source_path: PathBuf,
    output_path: PathBuf,
    size_bytes: u64,
    compiler_version: String,
    compiler_flags: Vec<String>,
    mutations_applied: Vec<String>,
) -> BuiltArtifact {
    BuiltArtifact {
        artifact_id: artifact_id.clone(),
        source_path,
        output_path,
        size_bytes,
        sha256: artifact_id,
        build_timestamp: chrono::Utc::now(),
        compiler_version,
        compiler_flags,
        mutations_applied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_sha256() {
        let config = BuilderConfig::default();
        let xwin = XwinPaths::new(&config.xwin_dir);
        let builder = ArtifactBuilder { config, xwin };
        let hash = builder.compute_sha256(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    // ── XwinPaths tests ──────────────────────────────────────────────────

    #[test]
    fn test_xwin_paths_default() {
        let paths = XwinPaths::new(Path::new("/root/.xwin"));
        assert_eq!(paths.crt_include, "/root/.xwin/crt/include");
        assert_eq!(paths.sdk_ucrt_include, "/root/.xwin/sdk/include/ucrt");
        assert_eq!(paths.sdk_shared_include, "/root/.xwin/sdk/include/shared");
        assert_eq!(paths.sdk_um_include, "/root/.xwin/sdk/include/um");
        assert_eq!(paths.sdk_winrt_include, "/root/.xwin/sdk/include/winrt");
        assert_eq!(paths.crt_lib, "/root/.xwin/crt/lib/x86_64");
        assert_eq!(paths.sdk_ucrt_lib, "/root/.xwin/sdk/lib/ucrt/x86_64");
        assert_eq!(paths.sdk_um_lib, "/root/.xwin/sdk/lib/um/x86_64");
    }

    #[test]
    fn test_xwin_paths_custom_dir() {
        let paths = XwinPaths::new(Path::new("/opt/xwin-sdk"));
        assert_eq!(paths.crt_include, "/opt/xwin-sdk/crt/include");
        assert_eq!(paths.sdk_um_lib, "/opt/xwin-sdk/sdk/lib/um/x86_64");
    }

    #[test]
    fn test_xwin_paths_include_args() {
        let paths = XwinPaths::new(Path::new("/root/.xwin"));
        let args = paths.include_args();
        assert_eq!(args.len(), 10); // 5 pairs of (-isystem, path)
        assert_eq!(args[0], "-isystem");
        assert_eq!(args[1], "/root/.xwin/crt/include");
        assert_eq!(args[8], "-isystem");
        assert_eq!(args[9], "/root/.xwin/sdk/include/winrt");
    }

    #[test]
    fn test_xwin_paths_lib_args() {
        let paths = XwinPaths::new(Path::new("/root/.xwin"));
        let args = paths.lib_args();
        assert_eq!(args.len(), 6); // 3 pairs of (-L, path)
        assert_eq!(args[0], "-L");
        assert_eq!(args[1], "/root/.xwin/crt/lib/x86_64");
        assert_eq!(args[4], "-L");
        assert_eq!(args[5], "/root/.xwin/sdk/lib/um/x86_64");
    }

    // ── build_artifact_metadata tests ────────────────────────────────────

    #[test]
    fn test_build_artifact_metadata_basic() {
        let artifact = build_artifact_metadata(
            "abc123".to_string(),
            PathBuf::from("/tmp/source.c"),
            PathBuf::from("/tmp/output.exe"),
            1024,
            "clang 17.0.6".to_string(),
            vec!["-O2".to_string()],
            vec!["ast.string_xor".to_string()],
        );

        assert_eq!(artifact.artifact_id, "abc123");
        assert_eq!(artifact.sha256, "abc123");
        assert_eq!(artifact.source_path, PathBuf::from("/tmp/source.c"));
        assert_eq!(artifact.output_path, PathBuf::from("/tmp/output.exe"));
        assert_eq!(artifact.size_bytes, 1024);
        assert_eq!(artifact.compiler_version, "clang 17.0.6");
        assert_eq!(artifact.compiler_flags, vec!["-O2"]);
        assert_eq!(artifact.mutations_applied, vec!["ast.string_xor"]);
    }

    #[test]
    fn test_build_artifact_metadata_empty_mutations() {
        let artifact = build_artifact_metadata(
            "def456".to_string(),
            PathBuf::from("src.c"),
            PathBuf::from("out.exe"),
            512,
            "unknown".to_string(),
            vec![],
            vec![],
        );

        assert!(artifact.mutations_applied.is_empty());
        assert!(artifact.compiler_flags.is_empty());
    }

    #[test]
    fn test_build_artifact_metadata_sha256_matches_id() {
        let artifact = build_artifact_metadata(
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".to_string(),
            PathBuf::from("s.c"),
            PathBuf::from("o.exe"),
            0,
            "".to_string(),
            vec![],
            vec![],
        );

        assert_eq!(artifact.artifact_id, artifact.sha256);
    }
}
