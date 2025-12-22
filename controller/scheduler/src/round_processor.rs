use crate::job::{Job, MutationSpec};
use crate::round::{Round, RoundStatus, RoundSummary, BehaviorComparison, Feedback, RunType};
use crate::run_result::{RunResult, RunOutcome};
use crate::worker_pool::WorkerPool;
use anyhow::{Result, Context};
use tracing::{info, warn};
use tracing::error;

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
    /// 1. Select mutations (currently uses job.mutations, Phase 5 adds selector)
    /// 2. Build & run baseline (trace_mode=off)
    /// 3. Build & run instrumented (trace_mode=lines)
    /// 4. Compare behavior (ensure identical outcomes)
    /// 5. Analyze feedback (Phase 5 adds triage integration)
    /// 6. Return round summary
    pub async fn process_round(
        &self,
        round: &mut Round,
        job: &Job,
        pool: &WorkerPool,
    ) -> Result<RoundSummary> {
        info!("[{}][{}] Starting round processor", job.id, round.round_id);

        // Step 1: Select mutations
        // If Selector service is configured, use it for feedback-driven selection
        // Otherwise, fall back to job.mutations
        let mutations = if let Some(selector_addr) = &self.selector_address {
            match self.select_mutations_from_selector(
                selector_addr,
                &job.id,
                &round.round_id,
                &job.rounds,
            ).await {
                Ok(selected) => {
                    info!("[{}][{}] Selector chose {} mutations", job.id, round.round_id, selected.len());
                    selected
                }
                Err(e) => {
                    warn!("[{}][{}] Selector call failed ({}), using job mutations",
                        job.id, round.round_id, e);
                    job.mutations.clone()
                }
            }
        } else {
            // No Selector configured - use job mutations directly
            job.mutations.clone()
        };

        round.mutations = mutations;
        let mutation_ids: Vec<String> = round.mutations.iter().map(|m| m.id.clone()).collect();
        info!("[{}][{}] Mutations: {:?}", job.id, round.round_id, mutation_ids);

        // Step 2 & 3: Execute baseline AND instrumented runs
        // OS-aware worker selection: ensure both runs use workers with SAME OS
        //
        // Strategy:
        // - BUILDS always run in PARALLEL (major speedup, no conflicts with UUID temp files)
        // - EXECUTION runs in PARALLEL if different workers, SEQUENTIAL if same worker

        // Select workers with matching OS
        let workers_by_os = pool.get_available_workers_by_os();
        let (baseline_worker_id, instrumented_worker_id, selected_os) = Self::select_os_matched_workers(&workers_by_os)?;

        let same_worker = baseline_worker_id == instrumented_worker_id;

        if same_worker {
            warn!("[{}][{}] Using SAME worker for both runs - execution will be SEQUENTIAL",
                job.id, round.round_id);
            info!("[{}][{}]   Worker: {} (OS: {})", job.id, round.round_id, baseline_worker_id, selected_os);
        } else {
            info!("[{}][{}] Using DIFFERENT workers - execution will be PARALLEL",
                job.id, round.round_id);
            info!("[{}][{}]   Baseline worker:     {} (OS: {})", job.id, round.round_id, baseline_worker_id, selected_os);
            info!("[{}][{}]   Instrumented worker: {} (OS: {})", job.id, round.round_id, instrumented_worker_id, selected_os);
        }

        let (baseline_result, instrumented_result) = if same_worker {
            // SEQUENTIAL execution: run baseline, WAIT for completion, then run instrumented
            // This prevents "Worker is busy" errors when only one worker is available
            info!("[{}][{}] Starting SEQUENTIAL dual-run execution", job.id, round.round_id);

            let baseline = self.execute_run(
                &job.id,
                &round.round_id,
                RunType::Baseline,
                &job.template_name,
                &job.source_file,
                &round.mutations,
                "off",  // No tracing for baseline
                pool,
                Some(&baseline_worker_id),
            ).await?;

            info!("[{}][{}] Baseline complete, starting instrumented run", job.id, round.round_id);

            let instrumented = self.execute_run(
                &job.id,
                &round.round_id,
                RunType::Instrumented,
                &job.template_name,
                &job.source_file,
                &round.mutations,
                "lines",  // Full tracing for instrumented
                pool,
                Some(&instrumented_worker_id),
            ).await?;

            (baseline, instrumented)
        } else {
            // PARALLEL execution: both workers available, run concurrently
            info!("[{}][{}] Starting PARALLEL dual-run execution", job.id, round.round_id);

            tokio::try_join!(
                self.execute_run(
                    &job.id,
                    &round.round_id,
                    RunType::Baseline,
                    &job.template_name,
                    &job.source_file,
                    &round.mutations,
                    "off",  // No tracing for baseline
                    pool,
                    Some(&baseline_worker_id),
                ),
                self.execute_run(
                    &job.id,
                    &round.round_id,
                    RunType::Instrumented,
                    &job.template_name,
                    &job.source_file,
                    &round.mutations,
                    "lines",  // Full tracing for instrumented
                    pool,
                    Some(&instrumented_worker_id),
                )
            )?
        };

        round.status = RoundStatus::BaselineComplete;

        let exec_mode = if same_worker { "SEQUENTIAL" } else { "PARALLEL" };
        info!("[{}][{}] {} dual-run execution complete", job.id, round.round_id, exec_mode);
        info!("[{}][{}]   Baseline:     detected={}, exit_code={}",
            job.id, round.round_id, baseline_result.detected, baseline_result.exit_code);
        info!("[{}][{}]   Instrumented: detected={}, exit_code={}",
            job.id, round.round_id, instrumented_result.detected, instrumented_result.exit_code);

        // Step 4: Compare behavior
        round.status = RoundStatus::ComparisonInProgress;
        let behavior_comparison = self.compare_behavior(&baseline_result, &instrumented_result)?;
        round.behavior_match = Some(behavior_comparison.clone());

        if !behavior_comparison.outcome_match {
            warn!("[{}][{}] Behavior mismatch detected! Differences: {:?}",
                job.id, round.round_id, behavior_comparison.differences);
            round.status = RoundStatus::BehaviorMismatch;
            round.mark_failed("Baseline and instrumented runs have different behavior".to_string());
            return Ok(round.to_summary());
        }

        info!("[{}][{}] Behavior comparison: MATCH (confidence: {:.2})",
            job.id, round.round_id, behavior_comparison.confidence);

        // Step 5: Generate feedback
        // For Phase 2, create simple feedback based on detection status
        // Phase 5 will integrate with Triage service for advanced analysis
        let feedback = Feedback {
            detected: baseline_result.detected,
            avoid_features: vec![],  // Phase 5: from triage analysis
            seek_features: vec![],   // Phase 5: from coverage analysis
            evasion_score: if baseline_result.detected { 0.0 } else { 1.0 },
        };
        round.feedback = Some(feedback);

        // Step 6: Mark round as completed
        round.mark_completed();
        info!("[{}][{}] Round complete: detected={}, behavior_match={}, evasion_score={:.2}",
            job.id, round.round_id, baseline_result.detected,
            behavior_comparison.outcome_match, round.feedback.as_ref().unwrap().evasion_score);

        Ok(round.to_summary())
    }

    /// Select two workers with matching OS for dual-run protocol
    ///
    /// # Strategy
    /// 1. Prefer OS with 2+ workers (true parallel execution)
    /// 2. Fallback to OS with 1 worker (sequential execution, but same OS)
    /// 3. Return (baseline_worker, instrumented_worker, os_name)
    ///
    /// # Returns
    /// (baseline_worker_id, instrumented_worker_id, os)
    fn select_os_matched_workers(
        workers_by_os: &std::collections::HashMap<String, Vec<String>>
    ) -> Result<(String, String, String)> {
        use anyhow::Context;

        if workers_by_os.is_empty() {
            return Err(anyhow::anyhow!("No available workers in pool"));
        }

        // Strategy 1: Find OS with 2+ workers (best case - true parallelism)
        if let Some((os, workers)) = workers_by_os.iter()
            .find(|(_, workers)| workers.len() >= 2)
        {
            info!("Found OS '{}' with {} workers - enabling true parallel execution",
                os, workers.len());
            return Ok((
                workers[0].clone(),
                workers[1].clone(),
                os.clone(),
            ));
        }

        // Strategy 2: Fallback to any OS with 1 worker (same OS, sequential execution)
        if let Some((os, workers)) = workers_by_os.iter().next() {
            // Note: Caller will detect same worker and run sequentially
            return Ok((
                workers[0].clone(),
                workers[0].clone(),  // Same worker for both
                os.clone(),
            ));
        }

        Err(anyhow::anyhow!("No workers available"))
    }

    /// Execute a single run (baseline or instrumented)
    ///
    /// # Workflow (following test-e2e-eicar.sh pattern):
    /// 1. Build artifact using builder crate
    /// 2. Use specified worker (or select from pool if None)
    /// 3. Deploy artifact to worker via gRPC streaming
    /// 4. Execute artifact on worker via RunSample RPC (BLOCKING)
    /// 5. Parse response and populate RunResult
    ///
    /// # Parameters
    /// - `specific_worker_id`: If Some, use this specific worker (for OS-matching)
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
        pool: &WorkerPool,
        specific_worker_id: Option<&str>,  // NEW: specific worker for OS-matching
    ) -> Result<RunResult> {
        use crate::automutate::common::{ArtifactChunk, SampleRequest};
        use crate::automutate::worker::worker_agent_client::WorkerAgentClient;
        use futures::stream;
        use std::time::Instant;

        let run_id = format!("{}/{}/{}", job_id, round_id, run_type.as_str());
        let start_time = Instant::now();

        // Step 1: Build artifact
        info!("[{}] Building artifact (template: {}, trace_mode: {})",
            run_id, template_name, trace_mode);

        let builder_config = builder::BuilderConfig::default();
        let artifact_builder = builder::ArtifactBuilder::new(builder_config.clone())?;

        // Convert mutations
        let builder_mutations: Vec<builder::mutator::MutationSpec> = mutations
            .iter()
            .map(|m| {
                let params = m.params.as_ref()
                    .and_then(|v| v.as_object().map(|obj| {
                        obj.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect::<std::collections::HashMap<String, String>>()
                    }))
                    .unwrap_or_default();
                builder::mutator::MutationSpec {
                    id: m.id.clone(),
                    params,
                }
            })
            .collect();

        let built = artifact_builder.build(builder::BuildInput::SourceFile {
            template_name: template_name.to_string(),
            source_file: source_file.to_string(),
            mutations: builder_mutations,
            trace_mode: trace_mode.to_string(),
        }).await?;

        let artifact_id = built.artifact_id.clone();
        info!("[{}] Build complete: artifact_id={}, size={} bytes",
            run_id, artifact_id, built.size_bytes);

        // Step 2: Select worker (use specific worker if provided, otherwise select from pool)
        let worker_id_to_use = if let Some(worker_id) = specific_worker_id {
            worker_id.to_string()
        } else {
            let worker_ids = pool.get_available_workers();
            if worker_ids.is_empty() {
                return Err(anyhow::anyhow!("No available workers in pool"));
            }
            worker_ids[0].clone()
        };

        let worker = pool.get_worker(&worker_id_to_use)
            .ok_or_else(|| anyhow::anyhow!("Worker {} not found", worker_id_to_use))?;
        let worker_address = worker.address.clone();
        info!("[{}] Using worker: {} at {}", run_id, worker.id, worker_address);

        // Step 3: Deploy artifact to worker
        info!("[{}] Deploying artifact to worker...", run_id);
        let artifact_path = builder_config.output_dir.join(format!("{}.exe", artifact_id));
        let artifact_data = tokio::fs::read(&artifact_path).await?;

        let worker_url = format!("http://{}", worker_address);
        let endpoint = tonic::transport::Endpoint::try_from(worker_url.clone())?;
        let mut client = WorkerAgentClient::connect(endpoint.clone()).await?;

        // Stream artifact in chunks (4MB each)
        let chunk_size = 4 * 1024 * 1024;
        let total_chunks = ((artifact_data.len() + chunk_size - 1) / chunk_size) as u32;
        let chunks: Vec<ArtifactChunk> = artifact_data
            .chunks(chunk_size)
            .enumerate()
            .map(|(i, chunk)| ArtifactChunk {
                artifact_id: artifact_id.clone(),
                data: chunk.to_vec(),
                chunk_index: i as u32,
                total_chunks,
                sha256: artifact_id.clone(),
            })
            .collect();

        client.send_artifact(stream::iter(chunks)).await?;
        info!("[{}] Deployment complete", run_id);

        // Step 4: Execute artifact (BLOCKING - wait for completion)
        info!("[{}] Executing artifact on worker...", run_id);
        let mut exec_client = WorkerAgentClient::connect(endpoint).await?;

        let request = tonic::Request::new(SampleRequest {
            job_id: run_id.clone(),
            artifact_id: artifact_id.clone(),
            timeout_seconds: 60,
            enable_etw: trace_mode != "off",
        });

        let response = exec_client.run_sample(request).await?;
        let exec_result = response.into_inner();

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
        result.worker_id = worker.id.clone();

        info!("[{}] Execution complete: detected={}, exit_code={}, elapsed={}s, telemetry_events={}",
            run_id, detected, exit_code, elapsed, result.telemetry_events_count);

        // Step 6: Pull telemetry events from worker and forward to controller for Elasticsearch indexing
        if result.telemetry_events_count > 0 {
            info!("[{}] Pulling {} telemetry events from worker...", run_id, result.telemetry_events_count);

            match Self::pull_and_forward_telemetry(&mut exec_client, &run_id, &worker).await {
                Ok(actual_count) => {
                    info!("[{}] Successfully pulled and indexed {} telemetry events", run_id, actual_count);
                    result.telemetry_events_count = actual_count as u64;
                }
                Err(e) => {
                    warn!("[{}] Failed to pull/index telemetry: {}", run_id, e);
                    warn!("[{}] Telemetry may be lost", run_id);
                }
            }
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
            comparison.outcome_match, comparison.confidence, comparison.differences.len()
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
        use crate::automutate::controller::{FeedbackProto, SelectionRequest};
        use crate::automutate::controller::selector_client::SelectorClient;

        info!("[{}][{}] Calling Selector service at {}", job_id, round_id, selector_address);

        // Get feedback from previous round (if any)
        let previous_feedback = previous_rounds.last().map(|round| FeedbackProto {
            detected: round.detected,
            avoid_features: vec![],  // TODO: Extract from triage analysis
            seek_features: vec![],   // TODO: Extract from coverage analysis
            evasion_score: round.evasion_score,
        });

        if let Some(ref feedback) = previous_feedback {
            info!("[{}][{}] Previous round: detected={}, evasion_score={:.2}",
                job_id, round_id, feedback.detected, feedback.evasion_score);
        } else {
            info!("[{}][{}] First round - no previous feedback", job_id, round_id);
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
            job_id: Some(JobId { value: job_id.to_string() }),
            round_id: round_id.to_string(),
            previous_feedback,
        });

        let response = client.select_mutation(request)
            .await
            .context("Selector RPC failed")?;

        let selection = response.into_inner();

        info!("[{}][{}] Selector returned {} mutations (exploration_prob={:.2})",
            job_id, round_id, selection.mutations.len(), selection.exploration_probability);
        info!("[{}][{}] Rationale: {}", job_id, round_id, selection.rationale);

        // Convert protobuf Mutations to MutationSpec
        let mutations: Vec<MutationSpec> = selection.mutations.iter()
            .map(|m| MutationSpec {
                id: m.id.clone(),
                params: Some(serde_json::to_value(&m.params).unwrap_or(serde_json::Value::Null)),
            })
            .collect();

        Ok(mutations)
    }

    /// Pull telemetry from worker and forward to controller for Elasticsearch indexing
    async fn pull_and_forward_telemetry(
        worker_client: &mut crate::automutate::worker::worker_agent_client::WorkerAgentClient<tonic::transport::Channel>,
        run_id: &str,
        worker: &crate::worker_pool::WorkerState,
    ) -> Result<usize> {
        use crate::automutate::worker::TelemetryRequest;
        use crate::automutate::controller::controller_client::ControllerClient;
        use crate::automutate::common::TelemetryData;
        use futures::stream;
        use tokio_stream::StreamExt;

        // Step 1: Pull telemetry from worker via GetTelemetry RPC
        let request = tonic::Request::new(TelemetryRequest {
            job_id: run_id.to_string(),
            since_timestamp: 0, // Get all events
            max_events: 0,      // No limit
        });

        let mut telemetry_stream = worker_client.get_telemetry(request).await
            .context("Failed to call GetTelemetry RPC on worker")?
            .into_inner();

        // Step 2: Collect telemetry events from stream
        let mut telemetry_events: Vec<TelemetryData> = Vec::new();
        while let Some(event) = telemetry_stream.next().await {
            match event {
                Ok(telemetry_data) => {
                    telemetry_events.push(telemetry_data);
                }
                Err(e) => {
                    warn!("[{}] Error receiving telemetry event: {}", run_id, e);
                    break;
                }
            }
        }

        let event_count = telemetry_events.len();
        info!("[{}] Pulled {} telemetry events from worker {}", run_id, event_count, worker.id);

        if telemetry_events.is_empty() {
            return Ok(0);
        }

        // Step 3: Forward telemetry to controller's StreamTelemetry RPC for Elasticsearch indexing
        // Connect to controller (loopback to ourselves)
        let controller_addr = std::env::var("CONTROLLER_ADDRESS")
            .unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());

        let mut controller_client = ControllerClient::connect(controller_addr).await
            .context("Failed to connect to controller for telemetry forwarding")?;

        // Stream telemetry to controller
        let telemetry_stream = stream::iter(telemetry_events);
        let request = tonic::Request::new(telemetry_stream);

        controller_client.stream_telemetry(request).await
            .context("Failed to forward telemetry to controller")?;

        info!("[{}] Successfully forwarded {} telemetry events to controller for indexing", run_id, event_count);

        Ok(event_count)
    }
}

impl Default for RoundProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{Job, MutationSpec};
    use crate::round::Round;
    use crate::worker_pool::WorkerPool;

    // NOTE: Full process_round() tests require actual builder + worker infrastructure
    // Use test-e2e-eicar.sh for end-to-end testing
    // Unit tests below cover individual components (compare_behavior, feedback extraction)

    #[test]
    fn test_compare_behavior_identical_runs() {
        let processor = RoundProcessor::new();

        // Create two identical run results
        let baseline = RunResult::new(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "artifact-123".to_string(),
            vec![],
        );

        let mut instrumented = baseline.clone();
        instrumented.run_type = RunType::Instrumented;

        // Compare
        let comparison = processor.compare_behavior(&baseline, &instrumented);

        assert!(comparison.is_ok(), "Comparison should succeed");
        let comp = comparison.unwrap();

        assert!(comp.outcome_match, "Identical runs should match");
        assert_eq!(comp.confidence, 1.0, "Perfect match = confidence 1.0");
        assert_eq!(comp.differences.len(), 0, "No differences expected");
    }

    #[test]
    fn test_compare_behavior_different_detection() {
        let processor = RoundProcessor::new();

        // Create baseline (not detected)
        let baseline = RunResult::new(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "artifact-123".to_string(),
            vec![],
        );

        // Create instrumented (detected)
        let mut instrumented = baseline.clone();
        instrumented.run_type = RunType::Instrumented;
        instrumented.detected = true;
        instrumented.outcome = RunOutcome::Detected;

        // Compare
        let comparison = processor.compare_behavior(&baseline, &instrumented);

        assert!(comparison.is_ok(), "Comparison should succeed");
        let comp = comparison.unwrap();

        assert!(!comp.outcome_match, "Different detection should not match");
        assert_eq!(comp.differences.len(), 1, "Should have 1 difference");
        assert!(
            comp.differences[0].contains("Detection mismatch"),
            "Should mention detection mismatch"
        );
        assert_eq!(comp.confidence, 0.75, "One difference = confidence 0.75");
    }

    #[test]
    fn test_compare_behavior_different_exit_codes() {
        let processor = RoundProcessor::new();

        // Create baseline (exit code 0)
        let baseline = RunResult::new(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "artifact-123".to_string(),
            vec![],
        );

        // Create instrumented (exit code 1)
        let mut instrumented = baseline.clone();
        instrumented.run_type = RunType::Instrumented;
        instrumented.exit_code = 1;

        // Compare
        let comparison = processor.compare_behavior(&baseline, &instrumented);

        assert!(comparison.is_ok(), "Comparison should succeed");
        let comp = comparison.unwrap();

        assert!(!comp.outcome_match, "Different exit codes should not match");
        assert_eq!(comp.differences.len(), 1, "Should have 1 difference");
        assert!(
            comp.differences[0].contains("Exit code mismatch"),
            "Should mention exit code mismatch"
        );
    }

    #[test]
    fn test_compare_behavior_multiple_differences() {
        let processor = RoundProcessor::new();

        // Create baseline
        let baseline = RunResult::new(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "artifact-123".to_string(),
            vec![],
        );

        // Create instrumented with multiple differences
        let mut instrumented = baseline.clone();
        instrumented.run_type = RunType::Instrumented;
        instrumented.detected = true;
        instrumented.exit_code = 1;

        // Compare
        let comparison = processor.compare_behavior(&baseline, &instrumented);

        assert!(comparison.is_ok(), "Comparison should succeed");
        let comp = comparison.unwrap();

        assert!(!comp.outcome_match, "Multiple differences should not match");
        assert_eq!(comp.differences.len(), 2, "Should have 2 differences");
        assert_eq!(comp.confidence, 0.5, "Two differences = confidence 0.5");
    }

    #[test]
    fn test_round_processor_with_selector() {
        // Test that RoundProcessor can be created with Selector address
        let processor = RoundProcessor::with_selector("localhost:50054".to_string());

        assert_eq!(processor.selector_address, Some("localhost:50054".to_string()));
    }

    #[test]
    fn test_round_processor_without_selector() {
        // Test that RoundProcessor works without Selector (fallback mode)
        let processor = RoundProcessor::new();

        assert_eq!(processor.selector_address, None);
    }

    #[test]
    fn test_feedback_extraction_from_previous_rounds() {
        // Test that feedback is correctly extracted from previous rounds
        use crate::round::RoundSummary;
        use std::time::SystemTime;

        // Create a round summary (simulates a completed previous round)
        let previous_round = RoundSummary {
            round_id: "round-1".to_string(),
            round_number: 1,
            mutations: vec!["ast.import_reshape".to_string()],
            detected: true,  // Was detected
            behavior_match: true,
            evasion_score: 0.0,  // No evasion (detected)
            completed_at: SystemTime::now(),
        };

        // Verify fields that would be passed to Selector
        assert!(previous_round.detected, "Should be detected");
        assert_eq!(previous_round.evasion_score, 0.0, "No evasion when detected");

        // Create another round summary (not detected)
        let successful_round = RoundSummary {
            round_id: "round-2".to_string(),
            round_number: 2,
            mutations: vec!["beh.preamble.fs".to_string()],
            detected: false,  // Not detected
            behavior_match: true,
            evasion_score: 1.0,  // Full evasion (not detected)
            completed_at: SystemTime::now(),
        };

        // Verify fields that would be passed to Selector
        assert!(!successful_round.detected, "Should not be detected");
        assert_eq!(successful_round.evasion_score, 1.0, "Full evasion when not detected");
    }
}
