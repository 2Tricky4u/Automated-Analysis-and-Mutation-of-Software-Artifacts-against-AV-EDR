//! C2: Selector Strategy Ablation (Campaign Runner)
//!
//! Orchestrates 4 independent campaigns (one per selector strategy)
//! through the existing controller gRPC API.
//!
//! Each campaign runs N rounds with a different selector and collects:
//! - Evasion rate
//! - Mean evasion score
//! - Time-to-first-evasion
//! - Config diversity
//! - Convergence round
//!
//! **RQ:** Does token-guided selection outperform baselines?
//!
//! Usage:
//!   The campaign runner connects to a running controller via gRPC.
//!   This is the most expensive experiment and should be run last.

use serde::{Deserialize, Serialize};
use serde_json::json;

/// Selector strategies to compare.
pub const SELECTOR_STRATEGIES: &[&str] = &[
    "coverage", // CoverageSelector (epsilon-greedy, default)
    "fuzzer",   // FuzzerSelector (genetic algorithm)
    "token",    // TokenSelector (token-biased epsilon-greedy)
    "random",   // RandomSelector (uniform random baseline)
];

/// Configuration for a selector ablation campaign.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignConfig {
    /// Controller gRPC address (e.g., "http://localhost:50051")
    pub controller_addr: String,
    /// Number of rounds per campaign
    pub rounds_per_campaign: u32,
    /// Payload shellcode path
    pub payload_path: String,
    /// Target VM OS
    pub target_os: String,
    /// EDR name for labeling
    pub edr_name: String,
}

/// Results from one selector campaign.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignResult {
    pub selector: String,
    pub rounds_completed: u32,
    pub evasion_rate: f64,
    pub mean_score: f64,
    pub time_to_first_evasion: Option<u32>,
    pub unique_configs: usize,
    pub convergence_round: Option<u32>,
    pub evasion_scores: Vec<f64>,
}

/// Summary comparing all 4 selectors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AblationSummary {
    pub campaigns: Vec<CampaignResult>,
    pub best_selector: String,
    pub improvement_over_random: f64,
}

/// Generate the comparison table for all campaigns.
pub fn comparison_table(campaigns: &[CampaignResult]) -> serde_json::Value {
    let random_rate = campaigns
        .iter()
        .find(|c| c.selector == "random")
        .map(|c| c.evasion_rate)
        .unwrap_or(0.0);

    let rows: Vec<serde_json::Value> = campaigns
        .iter()
        .map(|c| {
            let delta = c.evasion_rate - random_rate;
            json!({
                "selector": c.selector,
                "rounds": c.rounds_completed,
                "evasion_rate": c.evasion_rate,
                "mean_score": c.mean_score,
                "ttfe": c.time_to_first_evasion,
                "unique_configs": c.unique_configs,
                "convergence_round": c.convergence_round,
                "delta_vs_random": delta,
                "relative_improvement": if random_rate > 0.0 { delta / random_rate } else { 0.0 },
            })
        })
        .collect();

    json!({
        "columns": [
            "selector", "rounds", "evasion_rate", "mean_score",
            "ttfe", "unique_configs", "convergence_round",
            "delta_vs_random", "relative_improvement"
        ],
        "rows": rows,
    })
}

/// Generate learning curve data for plotting.
pub fn learning_curves(campaigns: &[CampaignResult]) -> serde_json::Value {
    let curves: Vec<serde_json::Value> = campaigns
        .iter()
        .map(|c| {
            // Cumulative evasion rate over rounds
            let cumulative: Vec<f64> = c
                .evasion_scores
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let window = &c.evasion_scores[..=i];
                    window.iter().sum::<f64>() / window.len() as f64
                })
                .collect();

            json!({
                "selector": c.selector,
                "scores": c.evasion_scores,
                "cumulative_mean": cumulative,
            })
        })
        .collect();

    json!({ "curves": curves })
}

/// Placeholder: Run the campaign via gRPC.
///
/// In practice, this would:
/// 1. Connect to the controller
/// 2. Create a job with the specified selector
/// 3. Wait for N rounds to complete
/// 4. Collect results
///
/// For now, returns a template that shows expected output structure.
pub fn campaign_template(config: &CampaignConfig) -> serde_json::Value {
    json!({
        "status": "template",
        "description": "Run 4 campaigns through controller gRPC API",
        "config": config,
        "selectors": SELECTOR_STRATEGIES,
        "steps": [
            "1. Start controller with ElasticSearch backend",
            "2. For each selector strategy:",
            "   a. Create job with selector override",
            "   b. Submit payload and module pool",
            "   c. Wait for N rounds to complete",
            "   d. Export EvalDataset for each campaign",
            "3. Run evaluation on each dataset",
            "4. Generate comparison table and learning curves",
        ],
        "grpc_calls": [
            "CreateJob(selector_strategy, rounds_per_campaign)",
            "SubmitPayload(job_id, payload_bytes)",
            "GetJobStatus(job_id) [poll until complete]",
            "ExportRoundSummaries(job_id)",
        ],
    })
}
