use rand::Rng;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
/// Selector gRPC Service
///
/// Implements CLAUDE.md Section 2: Selector component
/// "Selector: feedback-driven mutation selection with exploration vs exploitation"
///
/// gRPC service that:
/// - Selects mutations based on triage feedback
/// - Implements exploration/exploitation tradeoff
/// - Tracks outcome history for adaptive selection
use tonic::{Request, Response, Status, transport::Server};
use tracing::{debug, info};

pub mod automutate {
    pub mod common {
        tonic::include_proto!("automutate.common");
    }
    pub mod controller {
        tonic::include_proto!("automutate.controller");
    }
    pub mod worker {
        tonic::include_proto!("automutate.worker");
    }
}

use automutate::common::Mutation;
use automutate::controller::{
    OutcomeAck, OutcomeReport, SelectionRequest, SelectionResponse,
    selector_server::{Selector, SelectorServer},
};

/// Mutation pool with predefined mutation strategies
#[derive(Debug, Clone)]
struct MutationPool {
    /// Available mutations grouped by category
    mutations: Vec<MutationInfo>,
}

#[derive(Debug, Clone)]
struct MutationInfo {
    id: String,
    category: String,
    params: HashMap<String, String>,
    success_rate: f64, // Tracks historical success (0.0 - 1.0)
    total_attempts: u32,
}

impl MutationPool {
    fn new() -> Self {
        // Initialize with common mutation strategies from CLAUDE.md
        let mutations = vec![
            MutationInfo {
                id: "ast.import_reshape".to_string(),
                category: "ast".to_string(),
                params: [("delay_load".to_string(), "true".to_string())]
                    .iter()
                    .cloned()
                    .collect(),
                success_rate: 0.5,
                total_attempts: 0,
            },
            MutationInfo {
                id: "ast.opaque_predicate".to_string(),
                category: "ast".to_string(),
                params: [("complexity".to_string(), "2".to_string())]
                    .iter()
                    .cloned()
                    .collect(),
                success_rate: 0.5,
                total_attempts: 0,
            },
            MutationInfo {
                id: "beh.preamble.fs".to_string(),
                category: "behavioral".to_string(),
                params: [
                    ("fs_ops".to_string(), "3".to_string()),
                    ("hkcu_queries".to_string(), "2".to_string()),
                ]
                .iter()
                .cloned()
                .collect(),
                success_rate: 0.5,
                total_attempts: 0,
            },
            MutationInfo {
                id: "source.rename_identifiers".to_string(),
                category: "source".to_string(),
                params: [("strategy".to_string(), "random".to_string())]
                    .iter()
                    .cloned()
                    .collect(),
                success_rate: 0.5,
                total_attempts: 0,
            },
            MutationInfo {
                id: "source.api_wrapper".to_string(),
                category: "source".to_string(),
                params: [("depth".to_string(), "1".to_string())]
                    .iter()
                    .cloned()
                    .collect(),
                success_rate: 0.5,
                total_attempts: 0,
            },
        ];

        MutationPool { mutations }
    }

    /// Filter mutations that avoid flagged features
    fn filter_avoid_features(&self, avoid_features: &[String]) -> Vec<MutationInfo> {
        self.mutations
            .iter()
            .filter(|m| {
                // Check if mutation ID contains any avoid feature
                !avoid_features.iter().any(|avoid| m.id.contains(avoid))
            })
            .cloned()
            .collect()
    }

    /// Select mutations using epsilon-greedy strategy
    fn select_epsilon_greedy(
        &self,
        filtered: &[MutationInfo],
        epsilon: f64,
        count: usize,
    ) -> Vec<MutationInfo> {
        let mut rng = rand::thread_rng();
        let mut selected = Vec::new();

        for _ in 0..count.min(filtered.len()) {
            if rng.r#gen::<f64>() < epsilon {
                // Explore: random selection
                let idx = rng.gen_range(0..filtered.len());
                selected.push(filtered[idx].clone());
            } else {
                // Exploit: select highest success rate
                let best = filtered
                    .iter()
                    .max_by(|a, b| a.success_rate.partial_cmp(&b.success_rate).unwrap())
                    .cloned();
                if let Some(mutation) = best {
                    selected.push(mutation);
                }
            }
        }

        selected
    }
}

/// Selector service state
#[derive(Debug)]
pub struct SelectorService {
    /// Mutation pool
    pool: Arc<Mutex<MutationPool>>,
    /// Exploration probability (0.0-1.0)
    epsilon: f64,
    /// Outcome history per job
    outcome_history: Arc<Mutex<HashMap<String, Vec<OutcomeRecord>>>>,
}

#[derive(Debug, Clone)]
struct OutcomeRecord {
    mutation_id: String,
    detected: bool,
    evasion_score: f64,
}

impl SelectorService {
    pub fn new() -> Self {
        Self {
            pool: Arc::new(Mutex::new(MutationPool::new())),
            epsilon: 0.3, // 30% exploration by default
            outcome_history: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for SelectorService {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl Selector for SelectorService {
    async fn select_mutation(
        &self,
        request: Request<SelectionRequest>,
    ) -> Result<Response<SelectionResponse>, Status> {
        let req = request.into_inner();
        let job_id = req.job_id.map(|jid| jid.value).unwrap_or_default();
        let round_id = req.round_id;

        info!(
            "Selecting mutations for job: {}, round: {}",
            job_id, round_id
        );

        // Extract avoid features from previous feedback
        let avoid_features = req
            .previous_feedback
            .as_ref()
            .map(|f| f.avoid_features.clone())
            .unwrap_or_default();

        let evasion_score = req
            .previous_feedback
            .as_ref()
            .map(|f| f.evasion_score)
            .unwrap_or(0.5);

        debug!("Avoid features: {:?}", avoid_features);
        debug!("Previous evasion score: {:.2}", evasion_score);

        // Get mutation pool and filter
        let pool = self.pool.lock().unwrap();
        let filtered = pool.filter_avoid_features(&avoid_features);

        debug!("Filtered mutation pool: {} candidates", filtered.len());

        if filtered.is_empty() {
            return Err(Status::failed_precondition(
                "No mutations available after filtering avoid features",
            ));
        }

        // Select mutations using epsilon-greedy
        // For Phase 5, select 1-2 mutations per round
        let mutation_count = 2;
        let selected = pool.select_epsilon_greedy(&filtered, self.epsilon, mutation_count);

        // Convert to protobuf Mutation messages
        let mutations: Vec<Mutation> = selected
            .iter()
            .map(|m| Mutation {
                id: m.id.clone(),
                params: m.params.clone(),
            })
            .collect();

        let rationale = if selected.iter().any(|m| m.success_rate > 0.7) {
            format!(
                "Exploitation: selected {} high-success mutations",
                selected.len()
            )
        } else {
            format!(
                "Exploration: trying {} new mutation strategies",
                selected.len()
            )
        };

        info!(
            "Selected mutations: {:?}",
            selected.iter().map(|m| &m.id).collect::<Vec<_>>()
        );
        info!("Rationale: {}", rationale);

        Ok(Response::new(SelectionResponse {
            mutations,
            exploration_probability: self.epsilon as f32,
            rationale,
        }))
    }

    async fn report_outcome(
        &self,
        request: Request<OutcomeReport>,
    ) -> Result<Response<OutcomeAck>, Status> {
        let req = request.into_inner();
        let run_id = req.run_id.map(|rid| rid.uuid).unwrap_or_default();

        info!("Outcome reported for run: {}", run_id);

        // Extract outcome data from RunResult
        if let Some(result) = req.result {
            let detected = result.status == "detected";
            let mutations: Vec<String> = result.mutations.iter().map(|m| m.id.clone()).collect();

            debug!(
                "Run outcome: detected={}, mutations={:?}",
                detected, mutations
            );

            // Update mutation success rates in pool
            let mut pool = self.pool.lock().unwrap();
            for mutation in &mutations {
                if let Some(mut_info) = pool.mutations.iter_mut().find(|m| &m.id == mutation) {
                    mut_info.total_attempts += 1;

                    // Update success rate: not detected = success
                    let success = !detected;
                    mut_info.success_rate = (mut_info.success_rate
                        * (mut_info.total_attempts - 1) as f64
                        + if success { 1.0 } else { 0.0 })
                        / mut_info.total_attempts as f64;

                    debug!(
                        "Updated {}: success_rate={:.2}, attempts={}",
                        mutation, mut_info.success_rate, mut_info.total_attempts
                    );
                }
            }

            // Store outcome in history
            let job_id = result.worker_id.map(|wid| wid.value).unwrap_or_default();
            let mut history = self.outcome_history.lock().unwrap();
            let job_history = history.entry(job_id).or_default();

            for mutation in mutations {
                job_history.push(OutcomeRecord {
                    mutation_id: mutation,
                    detected,
                    evasion_score: if detected { 0.0 } else { 1.0 },
                });
            }
        }

        Ok(Response::new(OutcomeAck { received: true }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let addr = "0.0.0.0:50054".parse()?;
    let selector = SelectorService::default();

    info!("Selector gRPC service starting on {}", addr);

    Server::builder()
        .add_service(SelectorServer::new(selector))
        .serve(addr)
        .await?;

    Ok(())
}
