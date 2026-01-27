use crate::dispatch_coordinator::DispatchCoordinator;
use crate::job::{Job, MutationSpec};
use crate::round::{BehaviorComparison, Feedback, Round, RoundStatus, RoundSummary, RunType};
use crate::run_queue::RunResult as QueuedRunResult;
use crate::run_result::{RunOutcome, RunResult};
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

    /// Process a complete round with dual-run protocol (queue-based)
    ///
    /// # Workflow
    /// 1. Select mutations
    /// 2. Submit baseline run to queue
    /// 3. Submit instrumented run to queue
    /// 4. Signal dispatcher
    /// 5. Await both results (runs execute on available workers)
    /// 6. Compare behavior
    /// 7. Generate feedback
    /// 8. Return round summary
    pub async fn process_round(
        &self,
        round: &mut Round,
        job: &Job,
        coordinator: &DispatchCoordinator,
    ) -> Result<RoundSummary> {
        info!("[{}][{}] Starting round processor", job.id, round.id);

        let mutations = if let Some(selector_addr) = &self.selector_address {
            match self
                .select_mutations_from_selector(
                    selector_addr,
                    &job.id,
                    &round.id,
                    &job.rounds,
                )
                .await
            {
                Ok(selected) => {
                    info!(
                        "[{}][{}] Selector chose {} mutations",
                        job.id,
                        round.id,
                        selected.len()
                    );
                    selected
                }
                Err(e) => {
                    warn!(
                        "[{}][{}] Selector call failed ({}), using job mutations",
                        job.id, round.id, e
                    );
                    job.mutations.clone()
                }
            }
        } else {
            job.mutations.clone()
        };

        round.mutations = mutations;
        let mutation_ids: Vec<String> = round.mutations.iter().map(|m| m.id.clone()).collect();
        info!(
            "[{}][{}] Mutations: {:?}",
            job.id, round.id, mutation_ids
        );

        let required_os = job.target_os.clone().unwrap_or_default();
        let required_capabilities = job.required_capabilities.clone();

        if !required_capabilities.is_empty() {
            info!(
                "[{}][{}] Filtering workers by capabilities: {:?}",
                job.id, round.id, required_capabilities
            );
        }

        info!(
            "[{}][{}] Submitting runs to queue (OS: {}, Caps: {:?})",
            job.id, round.id, required_os, required_capabilities
        );

        let (baseline_run_id, baseline_rx) = coordinator.run_queue().submit_run(
            job.id.clone(),
            round.id.clone(),
            RunType::Baseline,
            job.template_name.clone(),
            job.source_file.clone(),
            job.modular_build.clone(),
            round.mutations.clone(),
            "off".to_string(),
            required_os.clone(),
            required_capabilities.clone(),
        );

        let (instrumented_run_id, instrumented_rx) = coordinator.run_queue().submit_run(
            job.id.clone(),
            round.id.clone(),
            RunType::Instrumented,
            job.template_name.clone(),
            job.source_file.clone(),
            job.modular_build.clone(),
            round.mutations.clone(),
            job.trace_mode.clone(),
            required_os.clone(),
            required_capabilities.clone(),
        );

        info!(
            "[{}][{}] Runs submitted: baseline={}, instrumented={}",
            job.id, round.id, baseline_run_id, instrumented_run_id
        );

        coordinator.signal_run_submitted();
        coordinator.signal_run_submitted();

        info!(
            "[{}][{}] Awaiting run results...",
            job.id, round.id
        );

        let (baseline_result, instrumented_result): (
            Result<QueuedRunResult, anyhow::Error>,
            Result<QueuedRunResult, anyhow::Error>,
        ) = tokio::join!(
            async {
                baseline_rx
                    .await
                    .map_err(|_| anyhow::anyhow!("Baseline run channel closed"))
            },
            async {
                instrumented_rx
                    .await
                    .map_err(|_| anyhow::anyhow!("Instrumented run channel closed"))
            }
        );

        let baseline_queued = baseline_result?;
        let instrumented_queued = instrumented_result?;

        round.status = RoundStatus::BaselineComplete;

        info!(
            "[{}][{}] Runs complete",
            job.id, round.id
        );
        info!(
            "[{}][{}]   Baseline:     detected={}, exit_code={:?}",
            job.id, round.id, baseline_queued.detected, baseline_queued.exit_code
        );
        info!(
            "[{}][{}]   Instrumented: detected={}, exit_code={:?}",
            job.id, round.id, instrumented_queued.detected, instrumented_queued.exit_code
        );

        let baseline_run_result = self.convert_queued_result(&baseline_queued, job, round, RunType::Baseline);
        let instrumented_run_result = self.convert_queued_result(&instrumented_queued, job, round, RunType::Instrumented);

        round.status = RoundStatus::ComparisonInProgress;
        let behavior_comparison = self.compare_behavior(&baseline_run_result, &instrumented_run_result)?;
        round.behavior_match = Some(behavior_comparison.clone());

        if !behavior_comparison.outcome_match {
            warn!(
                "[{}][{}] Behavior mismatch detected! Differences: {:?}",
                job.id, round.id, behavior_comparison.differences
            );
            round.status = RoundStatus::BehaviorMismatch;
            round.mark_failed("Baseline and instrumented runs have different behavior".to_string());
            return Ok(round.to_summary());
        }

        info!(
            "[{}][{}] Behavior comparison: MATCH (confidence: {:.2})",
            job.id, round.id, behavior_comparison.confidence
        );

        let feedback = Feedback {
            detected: baseline_run_result.detected,
            avoid_features: vec![],
            seek_features: vec![],
            evasion_score: if baseline_run_result.detected {
                0.0
            } else {
                1.0
            },
        };
        round.feedback = Some(feedback);

        round.mark_completed();
        info!(
            "[{}][{}] Round complete: detected={}, behavior_match={}, evasion_score={:.2}",
            job.id,
            round.id,
            baseline_run_result.detected,
            behavior_comparison.outcome_match,
            round.feedback.as_ref().unwrap().evasion_score
        );

        Ok(round.to_summary())
    }

    /// Convert queued run result to full RunResult
    fn convert_queued_result(
        &self,
        queued: &QueuedRunResult,
        job: &Job,
        round: &Round,
        run_type: RunType,
    ) -> RunResult {
        let outcome = if queued.detected {
            RunOutcome::Detected
        } else if queued.exit_code.map(|c| c != 0).unwrap_or(false) {
            RunOutcome::Crashed
        } else if queued.error.is_some() {
            RunOutcome::Error
        } else {
            RunOutcome::NotDetected
        };

        let mut result = RunResult::new(
            job.id.clone(),
            round.id.clone(),
            run_type,
            queued.run_id.clone(),
            round.mutations.iter().map(|m| m.id.clone()).collect(),
        );

        result.detected = queued.detected;
        result.exit_code = queued.exit_code.unwrap_or(0);
        result.outcome = outcome;

        result
    }

    /// Compare baseline and instrumented run behavior
    ///
    /// Ensures instrumentation doesn't alter artifact behavior.
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

        let outcome_match = differences.is_empty();

        let confidence = if outcome_match {
            1.0
        } else if differences.len() == 1 {
            0.75
        } else if baseline.outcome != RunOutcome::Error && instrumented.outcome != RunOutcome::Error
        {
            0.5
        } else {
            0.0
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

        let previous_feedback = previous_rounds.last().map(|round| FeedbackProto {
            detected: round.detected,
            avoid_features: vec![],
            seek_features: vec![],
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

        let selector_url = format!("http://{}", selector_address);
        let endpoint = tonic::transport::Endpoint::try_from(selector_url.clone())
            .context("Invalid Selector URL")?;

        let mut client = SelectorClient::connect(endpoint)
            .await
            .context("Failed to connect to Selector service")?;

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