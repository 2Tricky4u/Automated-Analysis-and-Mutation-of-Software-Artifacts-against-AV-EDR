//! Build Engine Benchmarks (A1, A3, A4)
//!
//! External timing harness that imports the `build` crate as a library
//! and wraps public API calls with `Instant::now()`.
//!
//! These experiments require the build crate and cross-compilation toolchain
//! (Clang/LLVM + xwin SDK) to be available. They are gated behind the
//! `build-bench` feature.
//!
//! ## Experiments
//!
//! | ID | Experiment                  | Function                        |
//! |----|-----------------------------|---------------------------------|
//! | A1 | Pipeline Stage Latency      | `pipeline_stage_latency()`      |
//! | A3 | Instrumentation Overhead    | `instrumentation_overhead()`    |
//! | A4 | Build Reproducibility       | `build_reproducibility()`       |

#[cfg(feature = "build-bench")]
pub mod bench {
    use anyhow::Result;
    use serde_json::json;
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use build::builder::{ArtifactBuilder, BuildInput, BuilderConfig, PreparedPayload};
    use build::mutator::MutationSpec;
    use build::template::assembler::ModuleSelection;
    use build::template::payload::EncodingType;

    /// Result of a single timed stage.
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct StageTimings {
        pub stage: String,
        pub duration_ms: f64,
    }

    /// Result of the A1 pipeline stage latency experiment.
    #[derive(Debug, Clone, serde::Serialize)]
    pub struct PipelineLatencyResult {
        pub iterations: usize,
        pub stages: Vec<StageTimings>,
        pub total_ms: f64,
    }

    /// Default module selection for benchmarks.
    fn default_modules() -> ModuleSelection {
        ModuleSelection {
            carrier: "alloc_rw_rx".to_string(),
            decoder: "xor".to_string(),
            antiemulation: "none".to_string(),
            deconditioner: "none".to_string(),
            guardrail: "none".to_string(),
            virtualprotect: "standard".to_string(),
            decoy: "none".to_string(),
        }
    }

    /// Dummy shellcode (NOP sled + ret) for benchmarking.
    fn bench_payload() -> Vec<u8> {
        let mut payload = vec![0x90; 256]; // NOP sled
        payload.push(0xC3); // RET
        payload
    }

    /// A1: Pipeline Stage Latency Breakdown
    ///
    /// Measures time for each stage of the build pipeline by calling
    /// individual sub-APIs. Falls back to measuring the monolithic
    /// `build()` call if sub-APIs are not individually callable.
    ///
    /// # Arguments
    /// * `config` - Builder configuration with paths to xwin SDK, templates, etc.
    /// * `iterations` - Number of iterations to average over (default: 50)
    pub async fn pipeline_stage_latency(
        config: BuilderConfig,
        iterations: usize,
    ) -> Result<PipelineLatencyResult> {
        let builder = ArtifactBuilder::new(config.clone())?;
        let payload = bench_payload();

        let mut stage_totals: Vec<(String, f64)> = Vec::new();
        let mut total_elapsed = 0.0;

        for _ in 0..iterations {
            // Stage 1: Payload encoding
            let t0 = Instant::now();
            let prepared =
                ArtifactBuilder::prepare_payload(&payload, EncodingType::Xor, "off", None)?;
            let payload_ms = t0.elapsed().as_secs_f64() * 1000.0;

            // Stage 2-8: Full build (includes assemble + mutate + compile + link + binary mutate)
            let input = BuildInput::ModularTemplate {
                modules: default_modules(),
                payload: payload.clone(),
                encoding: EncodingType::Xor,
                mutations: vec![],
                trace_mode: "off".to_string(),
                mutation_targets: vec![],
                sc_checkpoint_count: None,
                precomputed_payload: Some(prepared),
            };

            let t1 = Instant::now();
            let _artifact = builder.build(input).await?;
            let build_ms = t1.elapsed().as_secs_f64() * 1000.0;

            let iter_total = payload_ms + build_ms;
            total_elapsed += iter_total;

            // Accumulate
            if stage_totals.is_empty() {
                stage_totals.push(("payload_encode".to_string(), payload_ms));
                stage_totals.push(("build_pipeline".to_string(), build_ms));
            } else {
                stage_totals[0].1 += payload_ms;
                stage_totals[1].1 += build_ms;
            }
        }

        // Average
        let stages: Vec<StageTimings> = stage_totals
            .iter()
            .map(|(name, total)| StageTimings {
                stage: name.clone(),
                duration_ms: total / iterations as f64,
            })
            .collect();

        Ok(PipelineLatencyResult {
            iterations,
            stages,
            total_ms: total_elapsed / iterations as f64,
        })
    }

    /// A3: Instrumentation Overhead
    ///
    /// Builds each configuration twice (baseline + instrumented) and compares
    /// PE sizes. Reports size_ratio = instrumented_bytes / baseline_bytes.
    ///
    /// # Arguments
    /// * `config` - Builder configuration
    /// * `carrier_variants` - Carrier modules to test
    pub async fn instrumentation_overhead(
        config: BuilderConfig,
        carrier_variants: &[&str],
    ) -> Result<Vec<serde_json::Value>> {
        let builder = ArtifactBuilder::new(config)?;
        let payload = bench_payload();

        let mut results = Vec::new();

        for &carrier in carrier_variants {
            let modules = ModuleSelection {
                carrier: carrier.to_string(),
                ..default_modules()
            };

            // Baseline build (trace_mode = "off")
            let baseline_input = BuildInput::ModularTemplate {
                modules: modules.clone(),
                payload: payload.clone(),
                encoding: EncodingType::Xor,
                mutations: vec![],
                trace_mode: "off".to_string(),
                mutation_targets: vec![],
                sc_checkpoint_count: None,
                precomputed_payload: None,
            };

            let baseline = builder.build(baseline_input).await?;
            let baseline_size = baseline.size_bytes;

            // Instrumented build (trace_mode = "lines")
            let instr_input = BuildInput::ModularTemplate {
                modules,
                payload: payload.clone(),
                encoding: EncodingType::Xor,
                mutations: vec![],
                trace_mode: "lines".to_string(),
                mutation_targets: vec![],
                sc_checkpoint_count: None,
                precomputed_payload: None,
            };

            let instrumented = builder.build(instr_input).await?;
            let instr_size = instrumented.size_bytes;

            let size_ratio = instr_size as f64 / baseline_size.max(1) as f64;

            results.push(json!({
                "carrier": carrier,
                "baseline_bytes": baseline_size,
                "instrumented_bytes": instr_size,
                "size_ratio": size_ratio,
                "overhead_bytes": instr_size as i64 - baseline_size as i64,
                "overhead_percent": (size_ratio - 1.0) * 100.0,
            }));
        }

        Ok(results)
    }

    /// A4: Build Reproducibility
    ///
    /// Builds the same config N times and compares output SHA-256 hashes.
    ///
    /// # Arguments
    /// * `config` - Builder configuration
    /// * `repetitions` - Number of identical builds (default: 10)
    pub async fn build_reproducibility(
        config: BuilderConfig,
        repetitions: usize,
    ) -> Result<serde_json::Value> {
        let builder = ArtifactBuilder::new(config)?;
        let payload = bench_payload();

        let mut hashes: Vec<String> = Vec::new();
        let mut sizes: Vec<u64> = Vec::new();

        for _ in 0..repetitions {
            let input = BuildInput::ModularTemplate {
                modules: default_modules(),
                payload: payload.clone(),
                encoding: EncodingType::Xor,
                mutations: vec![],
                trace_mode: "off".to_string(),
                mutation_targets: vec![],
                sc_checkpoint_count: None,
                precomputed_payload: None,
            };

            let artifact = builder.build(input).await?;
            hashes.push(artifact.sha256.clone());
            sizes.push(artifact.size_bytes);
        }

        // Check hash consistency
        let first_hash = &hashes[0];
        let all_match = hashes.iter().all(|h| h == first_hash);
        let unique_hashes: std::collections::HashSet<&String> = hashes.iter().collect();

        Ok(json!({
            "repetitions": repetitions,
            "all_match": all_match,
            "unique_hashes": unique_hashes.len(),
            "hashes": hashes,
            "sizes": sizes,
            "reproducible": all_match,
        }))
    }
}
