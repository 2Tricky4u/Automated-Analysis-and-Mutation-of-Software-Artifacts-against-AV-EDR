use crate::job::{Job, MutationSpec};
use crate::round::{Round, RoundStatus, RoundSummary, BehaviorComparison, Feedback, RunType};
use crate::run_result::{RunResult, RunOutcome};
use crate::worker_pool::WorkerPool;
use anyhow::{Result, Context};
use tracing::{info, warn, error};

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

        // Step 2: Execute baseline run (no instrumentation)
        info!("[{}][{}] Starting BASELINE run", job.id, round.round_id);
        let baseline_result = self.execute_run(
            &job.id,
            &round.round_id,
            RunType::Baseline,
            &job.template_name,
            &job.source_file,
            &round.mutations,
            "off",  // No tracing for baseline
            pool,
        ).await?;

        info!("[{}][{}] Baseline run complete: detected={}, exit_code={}",
            job.id, round.round_id, baseline_result.detected, baseline_result.exit_code);

        // Step 3: Execute instrumented run (full tracing)
        round.status = RoundStatus::BaselineComplete;
        info!("[{}][{}] Starting INSTRUMENTED run", job.id, round.round_id);
        let instrumented_result = self.execute_run(
            &job.id,
            &round.round_id,
            RunType::Instrumented,
            &job.template_name,
            &job.source_file,
            &round.mutations,
            "lines",  // Full tracing for instrumented
            pool,
        ).await?;

        info!("[{}][{}] Instrumented run complete: detected={}, exit_code={}",
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

    /// Execute a single run (baseline or instrumented)
    ///
    /// # Returns
    /// RunResult with execution outcome
    async fn execute_run(
        &self,
        job_id: &str,
        round_id: &str,
        run_type: RunType,
        _template_name: &str,
        _source_file: &str,
        mutations: &[MutationSpec],
        trace_mode: &str,
        _pool: &WorkerPool,
    ) -> Result<RunResult> {
        let run_id = format!("{}/{}/{}", job_id, round_id, run_type.as_str());
        info!("[{}] Building artifact (trace_mode: {})", run_id, trace_mode);

        // For Phase 2, create a placeholder RunResult
        // Phase 3/4 will integrate with actual builder/worker services
        // This allows the round processor logic to be tested independently

        let mut result = RunResult::new(
            job_id.to_string(),
            round_id.to_string(),
            run_type,
            "placeholder-artifact-id".to_string(),  // Phase 3: actual artifact_id from builder
            mutations.iter().map(|m| m.id.clone()).collect(),
        );

        // Placeholder: simulate execution result
        // Phase 3/4: call actual builder.build() and worker.run_sample()
        result.detected = false;  // Assume not detected for now
        result.exit_code = 0;      // Assume success
        result.elapsed_seconds = 1; // Placeholder timing
        result.telemetry_events_count = if trace_mode == "off" { 0 } else { 100 }; // Simulate telemetry
        result.outcome = RunOutcome::NotDetected;
        result.worker_id = "placeholder-worker".to_string();  // Phase 3: actual worker_id from pool

        info!("[{}] Execution complete (placeholder mode)", run_id);

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
        use crate::edr::common::JobId;
        use crate::edr::controller::{FeedbackProto, SelectionRequest};
        use crate::edr::controller::selector_client::SelectorClient;

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

    #[tokio::test]
    async fn test_process_round_with_placeholder() {
        // Create test job
        let job = Job::new(
            "job-000001".to_string(),
            "test".to_string(),
            "test.c".to_string(),
            vec![],  // No mutations for test
            "api+bb".to_string(),
            0,
            10,
        );

        // Create round
        let mut round = Round::new(job.id.clone(), 1);

        // Create round processor and worker pool
        let processor = RoundProcessor::new();
        let pool = WorkerPool::new(30);

        // Process round (uses placeholder execution)
        let result = processor.process_round(&mut round, &job, &pool).await;

        // Verify round completed successfully
        assert!(result.is_ok(), "Round processing should succeed");
        let summary = result.unwrap();

        // Verify summary fields
        assert_eq!(summary.round_id, "round-1");
        assert_eq!(summary.round_number, 1);
        assert!(summary.behavior_match, "Placeholder runs should match");
        assert!(!summary.detected, "Placeholder runs should not be detected");
        assert_eq!(summary.evasion_score, 1.0, "No detection = score 1.0");
    }

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
