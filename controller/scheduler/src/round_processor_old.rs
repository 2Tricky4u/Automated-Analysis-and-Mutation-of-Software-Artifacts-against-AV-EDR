use crate::job::{Job, ModularBuildSpec, MutationSpec};
use crate::round::{BehaviorComparison, Feedback, Round, RoundStatus, RoundSummary, RunType};
use crate::run_result::{RunOutcome, RunResult};
use crate::target_manager::TargetManager;
use anyhow::{Context, Result};
use tracing::{info, warn};

/// Round processor orchestrates dual-run protocol for each round
#[derive(Clone)]
pub struct RoundProcessor {
    /// Selector service address (optional - if None, uses job mutations directly)
    selector_address: Option<String>,
}

impl RoundProcessor {
    pub fn new() -> Self {
        RoundProcessor {
            selector_address: None,
        }
    }

    /// Create with Selector service integration
    pub fn with_selector(selector_address: String) -> Self {
        RoundProcessor {
            selector_address: Some(selector_address),
        }
    }

    /// Process a complete round with dual-run protocol
    ///
    /// # Workflow
    /// 1. Select mutations
    /// 2. Build & run baseline (trace_mode=off)
    /// 3. Build & run instrumented (trace_mode=lines)
    /// 4. Compare behavior (ensure identical outcomes)
    /// 5. Analyze feedback
    /// 6. Return round summary
    pub async fn process_round(
        &self,
        round: &mut Round,
        job: &Job,
        targets: &TargetManager,
    ) -> Result<RoundSummary> {
        info!("[{}][{}] Starting round processor", job.id, round.round_id);

        // Step 1: Select mutations
        // If Selector service is configured, use it for feedback-driven selection
        // Otherwise, fall back to job.mutations
        let mutations = if let Some(selector_addr) = &self.selector_address {
            match self
                .select_mutations_from_selector(
                    selector_addr,
                    &job.id,
                    &round.round_id,
                    &job.rounds,
                )
                .await
            {
                Ok(selected) => {
                    info!(
                        "[{}][{}] Selector chose {} mutations",
                        job.id,
                        round.round_id,
                        selected.len()
                    );
                    selected
                }
                Err(e) => {
                    warn!(
                        "[{}][{}] Selector call failed ({}), using job mutations",
                        job.id, round.round_id, e
                    );
                    job.mutations.clone()
                }
            }
        } else {
            // No Selector configured - use job mutations directly
            job.mutations.clone()
        };

        round.mutations = mutations;
        let mutation_ids: Vec<String> = round.mutations.iter().map(|m| m.id.clone()).collect();
        info!(
            "[{}][{}] Mutations: {:?}",
            job.id, round.round_id, mutation_ids
        );

        // Step 2 & 3: Execute baseline AND instrumented runs
        // OS-aware and capability-aware target selection:
        // - Ensure both runs use targets with SAME OS AND required capabilities
        // - If job.required_capabilities is ["mde"], only select targets with MDE
        // - If job.required_capabilities is ["cortex"], only select targets with Cortex
        //
        // Strategy:
        // - BUILDS always run in PARALLEL (major speedup, no conflicts with UUID temp files)
        // - EXECUTION runs in PARALLEL if different targets, SEQUENTIAL if same target

        // Select targets with matching OS AND capabilities
        let targets_by_os = targets.get_available_by_os_and_capabilities(&job.required_capabilities);

        if !job.required_capabilities.is_empty() {
            info!(
                "[{}][{}] Filtering targets by capabilities: {:?}",
                job.id, round.round_id, job.required_capabilities
            );
        }

        let (baseline_target_id, instrumented_target_id, selected_os) =
            Self::select_os_matched_targets(&targets_by_os)?;

        let same_target = baseline_target_id == instrumented_target_id;

        let caps_str = if job.required_capabilities.is_empty() {
            "any".to_string()
        } else {
            job.required_capabilities.join(",")
        };

        if same_target {
            warn!(
                "[{}][{}] Using SAME target for both runs - execution will be SEQUENTIAL",
                job.id, round.round_id
            );
            info!(
                "[{}][{}]   Target: {} (OS: {}, Caps: {})",
                job.id, round.round_id, baseline_target_id, selected_os, caps_str
            );
        } else {
            info!(
                "[{}][{}] Using DIFFERENT targets - execution will be PARALLEL",
                job.id, round.round_id
            );
            info!(
                "[{}][{}]   Baseline target:     {} (OS: {}, Caps: {})",
                job.id, round.round_id, baseline_target_id, selected_os, caps_str
            );
            info!(
                "[{}][{}]   Instrumented target: {} (OS: {}, Caps: {})",
                job.id, round.round_id, instrumented_target_id, selected_os, caps_str
            );
        }

        // Get modular build spec reference (if any)
        let modular_build_ref = job.modular_build.as_ref();

        let (baseline_result, instrumented_result) = if same_target {
            // SEQUENTIAL execution: run baseline, WAIT for completion, then run instrumented
            // This prevents "Target is busy" errors when only one target is available
            info!(
                "[{}][{}] Starting SEQUENTIAL dual-run execution",
                job.id, round.round_id
            );

            let baseline = self
                .execute_run(
                    &job.id,
                    &round.round_id,
                    RunType::Baseline,
                    &job.template_name,
                    &job.source_file,
                    &round.mutations,
                    "off", // No tracing for baseline
                    targets,
                    Some(&baseline_target_id),
                    modular_build_ref,
                )
                .await?;

            info!(
                "[{}][{}] Baseline complete, starting instrumented run",
                job.id, round.round_id
            );

            let instrumented = self
                .execute_run(
                    &job.id,
                    &round.round_id,
                    RunType::Instrumented,
                    &job.template_name,
                    &job.source_file,
                    &round.mutations,
                    "lines", // Full tracing for instrumented
                    targets,
                    Some(&instrumented_target_id),
                    modular_build_ref,
                )
                .await?;

            (baseline, instrumented)
        } else {
            // PARALLEL execution: both targets available, run concurrently
            info!(
                "[{}][{}] Starting PARALLEL dual-run execution",
                job.id, round.round_id
            );

            tokio::try_join!(
                self.execute_run(
                    &job.id,
                    &round.round_id,
                    RunType::Baseline,
                    &job.template_name,
                    &job.source_file,
                    &round.mutations,
                    "off", // No tracing for baseline
                    targets,
                    Some(&baseline_target_id),
                    modular_build_ref,
                ),
                self.execute_run(
                    &job.id,
                    &round.round_id,
                    RunType::Instrumented,
                    &job.template_name,
                    &job.source_file,
                    &round.mutations,
                    "lines", // Full tracing for instrumented
                    targets,
                    Some(&instrumented_target_id),
                    modular_build_ref,
                )
            )?
        };

        round.status = RoundStatus::BaselineComplete;

        let exec_mode = if same_target {
            "SEQUENTIAL"
        } else {
            "PARALLEL"
        };
        info!(
            "[{}][{}] {} dual-run execution complete",
            job.id, round.round_id, exec_mode
        );
        info!(
            "[{}][{}]   Baseline:     detected={}, exit_code={}",
            job.id, round.round_id, baseline_result.detected, baseline_result.exit_code
        );
        info!(
            "[{}][{}]   Instrumented: detected={}, exit_code={}",
            job.id, round.round_id, instrumented_result.detected, instrumented_result.exit_code
        );

        // Step 4: Compare behavior
        round.status = RoundStatus::ComparisonInProgress;
        let behavior_comparison = self.compare_behavior(&baseline_result, &instrumented_result)?;
        round.behavior_match = Some(behavior_comparison.clone());

        if !behavior_comparison.outcome_match {
            warn!(
                "[{}][{}] Behavior mismatch detected! Differences: {:?}",
                job.id, round.round_id, behavior_comparison.differences
            );
            round.status = RoundStatus::BehaviorMismatch;
            round.mark_failed("Baseline and instrumented runs have different behavior".to_string());
            return Ok(round.to_summary());
        }

        info!(
            "[{}][{}] Behavior comparison: MATCH (confidence: {:.2})",
            job.id, round.round_id, behavior_comparison.confidence
        );

        // Generate feedback
        // create simple feedback based on detection status
        // will integrate with Triage service for advanced analysis
        let feedback = Feedback {
            detected: baseline_result.detected,
            avoid_features: vec![], // from triage analysis
            seek_features: vec![],  // from coverage analysis
            evasion_score: if baseline_result.detected { 0.0 } else { 1.0 },
        };
        round.feedback = Some(feedback);

        // Step 6: Mark round as completed
        round.mark_completed();
        info!(
            "[{}][{}] Round complete: detected={}, behavior_match={}, evasion_score={:.2}",
            job.id,
            round.round_id,
            baseline_result.detected,
            behavior_comparison.outcome_match,
            round.feedback.as_ref().unwrap().evasion_score
        );

        Ok(round.to_summary())
    }

    /// Select two targets with matching OS for dual-run protocol
    ///
    /// # Strategy
    /// 1. Prefer OS with 2+ targets (true parallel execution)
    /// 2. Fallback to OS with 1 target (sequential execution, but same OS)
    /// 3. Return (baseline_target, instrumented_target, os_name)
    ///
    /// # Returns
    /// (baseline_target_id, instrumented_target_id, os)
    fn select_os_matched_targets(
        targets_by_os: &std::collections::HashMap<String, Vec<String>>,
    ) -> Result<(String, String, String)> {
        if targets_by_os.is_empty() {
            return Err(anyhow::anyhow!("No available targets"));
        }

        // Strategy 1: Find OS with 2+ targets (best case - true parallelism)
        if let Some((os, targets)) = targets_by_os.iter().find(|(_, targets)| targets.len() >= 2) {
            info!(
                "Found OS '{}' with {} targets - enabling true parallel execution",
                os,
                targets.len()
            );
            return Ok((targets[0].clone(), targets[1].clone(), os.clone()));
        }

        // Strategy 2: Fallback to any OS with 1 target (same OS, sequential execution)
        if let Some((os, targets)) = targets_by_os.iter().next() {
            // Note: Caller will detect same target and run sequentially
            return Ok((
                targets[0].clone(),
                targets[0].clone(), // Same target for both
                os.clone(),
            ));
        }

        Err(anyhow::anyhow!("No targets available"))
    }

    /// Execute a single run (baseline or instrumented)
    ///
    /// # Workflow (following test-e2e-eicar.sh pattern):
    /// 1. Build artifact using builder crate (modular or legacy mode)
    /// 2. Use specified target (or select from available if None)
    /// 3. Deploy artifact to target via gRPC streaming
    /// 4. Execute artifact on target via RunSample RPC (BLOCKING)
    /// 5. Parse response and populate RunResult
    ///
    /// # Parameters
    /// - `specific_target_id`: If Some, use this specific target (for OS-matching)
    /// - `modular_build`: If Some, use modular template build; otherwise legacy SourceFile
    ///
    /// # Returns
    /// RunResult with execution outcome
    async fn execute_run(
        &self,
        job_id: &str,
        round_id: &str,
        run_type: RunType,
        template_name: &str,
        source_file: &str,
        mutations: &[MutationSpec],
        trace_mode: &str,
        targets: &TargetManager,
        specific_target_id: Option<&str>,
        modular_build: Option<&ModularBuildSpec>,
    ) -> Result<RunResult> {
        use crate::automutate::common::SampleRequest;
        use std::time::Instant;

        let run_id = format!("{}/{}/{}", job_id, round_id, run_type.as_str());
        let start_time = Instant::now();

        // Step 1: Build artifact (modular or legacy mode)
        let builder_config = build::BuilderConfig::default();
        let artifact_builder = build::ArtifactBuilder::new(builder_config.clone())?;

        // Convert mutations to builder format
        let builder_mutations: Vec<build::mutator::MutationSpec> = mutations
            .iter()
            .map(|m| {
                let params = m
                    .params
                    .as_ref()
                    .and_then(|v| {
                        v.as_object().map(|obj| {
                            obj.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect::<std::collections::HashMap<String, String>>()
                        })
                    })
                    .unwrap_or_default();
                build::mutator::MutationSpec {
                    id: m.id.clone(),
                    params,
                }
            })
            .collect();

        // Choose build mode based on modular_build parameter
        let built = if let Some(modular) = modular_build {
            // NEW: Modular template build using @MODULE marker system
            info!(
                "[{}] Building artifact (MODULAR: carrier={}, decoder={}, encoding={}, trace_mode={})",
                run_id, modular.modules.carrier, modular.modules.decoder, modular.encoding, trace_mode
            );

            // Convert module selection to builder format
            let modules = build::ModuleSelection {
                carrier: modular.modules.carrier.clone(),
                decoder: modular.modules.decoder.clone(),
                antiemulation: modular.modules.antiemulation.clone(),
                guardrail: modular.modules.guardrail.clone(),
                virtualprotect: modular.modules.virtualprotect.clone(),
                decoy: modular.modules.decoy.clone(),
            };

            // Parse encoding type
            let encoding = build::EncodingType::from_str(&modular.encoding)
                .unwrap_or(build::EncodingType::Xor);

            artifact_builder
                .build(build::BuildInput::ModularTemplate {
                    modules,
                    payload: modular.payload.clone(),
                    encoding,
                    mutations: builder_mutations,
                    trace_mode: trace_mode.to_string(),
                })
                .await?
        } else {
            // Legacy: SourceFile build mode
            info!(
                "[{}] Building artifact (LEGACY: template={}, trace_mode={})",
                run_id, template_name, trace_mode
            );

            artifact_builder
                .build(build::BuildInput::SourceFile {
                    template_name: template_name.to_string(),
                    source_file: source_file.to_string(),
                    mutations: builder_mutations,
                    trace_mode: trace_mode.to_string(),
                })
                .await?
        };

        let artifact_id = built.artifact_id.clone();
        info!(
            "[{}] Build complete: artifact_id={}, size={} bytes",
            run_id, artifact_id, built.size_bytes
        );

        // Step 2: Select target (use specific target if provided, otherwise select from available)
        let target_id_to_use = if let Some(target_id) = specific_target_id {
            target_id.to_string()
        } else {
            let available = targets.get_available();
            if available.is_empty() {
                return Err(anyhow::anyhow!("No available targets"));
            }
            available[0].clone()
        };

        let target = targets
            .get(&target_id_to_use)
            .ok_or_else(|| anyhow::anyhow!("Target {} not found", target_id_to_use))?;

        info!(
            "[{}] Using target: {} at {}",
            run_id, target.id, target.address
        );

        // Step 3: Deploy artifact to target via TargetManager
        info!("[{}] Deploying artifact to target...", run_id);
        let artifact_path = builder_config
            .output_dir
            .join(format!("{}.exe", artifact_id));

        targets
            .send_artifact(&target_id_to_use, &artifact_id, &artifact_path)
            .await?;
        info!("[{}] Deployment complete", run_id);

        // Step 4: Execute artifact via TargetManager (BLOCKING - wait for completion)
        info!("[{}] Executing artifact on target...", run_id);

        let request = SampleRequest {
            job_id: run_id.clone(),
            artifact_id: artifact_id.clone(),
            timeout_seconds: 60,
            enable_etw: trace_mode != "off",
        };

        // Route execution through TargetManager via bidirectional stream
        let exec_result = targets
            .execute_artifact(&target_id_to_use, request)
            .await?;

        let elapsed = start_time.elapsed().as_secs();

        // Step 5: Parse response and populate RunResult
        let detected = !exec_result.success;
        let exit_code = exec_result.exit_code;

        let outcome = if detected {
            RunOutcome::Detected
        } else if exit_code != 0 {
            RunOutcome::Crashed
        } else {
            RunOutcome::NotDetected
        };

        let mut result = RunResult::new(
            job_id.to_string(),
            round_id.to_string(),
            run_type,
            artifact_id.clone(),
            mutations.iter().map(|m| m.id.clone()).collect(),
        );

        result.detected = detected;
        result.exit_code = exit_code;
        result.elapsed_seconds = elapsed;
        result.telemetry_events_count = exec_result.telemetry_ids.len() as u64;
        result.outcome = outcome;
        result.worker_id = target.id.clone();

        info!(
            "[{}] Execution complete: detected={}, exit_code={}, elapsed={}s, telemetry_events={}",
            run_id, detected, exit_code, elapsed, result.telemetry_events_count
        );

        // Step 6: Telemetry handling
        // NOTE: Telemetry is now pushed automatically via bidirectional stream during execution
        // The telemetry_events_count in SampleResponse is just a reference count
        // Actual telemetry data is streamed in real-time and handled by main.rs event loop
        // No need to pull telemetry here - it's already been pushed and indexed
        if result.telemetry_events_count > 0 {
            info!(
                "[{}] Telemetry: {} events were streamed during execution (already indexed)",
                run_id, result.telemetry_events_count
            );
        }

        Ok(result)
    }

    /// Compare baseline and instrumented run behavior
    ///
    /// Ensures instrumentation doesn't alter artifact behavior.
    ///
    /// # Returns
    /// BehaviorComparison with outcome_match=true if behaviors are identical
    pub fn compare_behavior(
        &self,
        baseline: &RunResult,
        instrumented: &RunResult,
    ) -> Result<BehaviorComparison> {
        info!("Comparing baseline and instrumented behavior");

        let baseline_detected = baseline.detected;
        let baseline_exit_code = baseline.exit_code;
        let instrumented_detected = instrumented.detected;
        let instrumented_exit_code = instrumented.exit_code;

        // Collect differences
        let mut differences = Vec::new();

        if baseline_detected != instrumented_detected {
            differences.push(format!(
                "Detection mismatch: baseline={}, instrumented={}",
                baseline_detected, instrumented_detected
            ));
        }

        if baseline_exit_code != instrumented_exit_code {
            differences.push(format!(
                "Exit code mismatch: baseline={}, instrumented={}",
                baseline_exit_code, instrumented_exit_code
            ));
        }

        // Determine if outcomes match
        let outcome_match = differences.is_empty();

        // Calculate confidence
        // 1.0 = perfect match (both detected and exit_code identical)
        // 0.75 = one metric matches, one differs
        // 0.5 = both metrics differ but runs completed normally
        // 0.0 = runs couldn't be compared (errors, crashes, etc.)
        let confidence = if outcome_match {
            1.0
        } else if differences.len() == 1 {
            0.75
        } else {
            // Check if both runs completed (non-error outcomes)
            if baseline.outcome != RunOutcome::Error && instrumented.outcome != RunOutcome::Error {
                0.5
            } else {
                0.0
            }
        };

        let comparison = BehaviorComparison {
            outcome_match,
            baseline_detected,
            baseline_exit_code,
            instrumented_detected,
            instrumented_exit_code,
            differences,
            confidence,
        };

        info!(
            "Behavior comparison complete: outcome_match={}, confidence={:.2}, differences={}",
            comparison.outcome_match,
            comparison.confidence,
            comparison.differences.len()
        );

        Ok(comparison)
    }

    /// Call Selector service to get mutations for next round
    async fn select_mutations_from_selector(
        &self,
        selector_address: &str,
        job_id: &str,
        round_id: &str,
        previous_rounds: &[RoundSummary],
    ) -> Result<Vec<MutationSpec>> {
        use crate::automutate::common::JobId;
        use crate::automutate::controller::selector_client::SelectorClient;
        use crate::automutate::controller::{FeedbackProto, SelectionRequest};

        info!(
            "[{}][{}] Calling Selector service at {}",
            job_id, round_id, selector_address
        );

        // Get feedback from previous round (if any)
        let previous_feedback = previous_rounds.last().map(|round| FeedbackProto {
            detected: round.detected,
            avoid_features: vec![], // TODO: Extract from triage analysis
            seek_features: vec![],  // TODO: Extract from coverage analysis
            evasion_score: round.evasion_score,
        });

        if let Some(ref feedback) = previous_feedback {
            info!(
                "[{}][{}] Previous round: detected={}, evasion_score={:.2}",
                job_id, round_id, feedback.detected, feedback.evasion_score
            );
        } else {
            info!(
                "[{}][{}] First round - no previous feedback",
                job_id, round_id
            );
        }

        // Connect to Selector service
        let selector_url = format!("http://{}", selector_address);
        let endpoint = tonic::transport::Endpoint::try_from(selector_url.clone())
            .context("Invalid Selector URL")?;

        let mut client = SelectorClient::connect(endpoint)
            .await
            .context("Failed to connect to Selector service")?;

        // Send selection request
        let request = tonic::Request::new(SelectionRequest {
            job_id: Some(JobId {
                value: job_id.to_string(),
            }),
            round_id: round_id.to_string(),
            previous_feedback,
        });

        let response = client
            .select_mutation(request)
            .await
            .context("Selector RPC failed")?;

        let selection = response.into_inner();

        info!(
            "[{}][{}] Selector returned {} mutations (exploration_prob={:.2})",
            job_id,
            round_id,
            selection.mutations.len(),
            selection.exploration_probability
        );
        info!(
            "[{}][{}] Rationale: {}",
            job_id, round_id, selection.rationale
        );

        // Convert protobuf Mutations to MutationSpec
        let mutations: Vec<MutationSpec> = selection
            .mutations
            .iter()
            .map(|m| MutationSpec {
                id: m.id.clone(),
                params: Some(serde_json::to_value(&m.params).unwrap_or(serde_json::Value::Null)),
            })
            .collect();

        Ok(mutations)
    }
}

impl Default for RoundProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;