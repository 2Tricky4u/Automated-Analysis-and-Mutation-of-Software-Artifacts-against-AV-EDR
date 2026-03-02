//! Synthetic round data builder for evaluation tests.

use controller::dispatch::types::{
    DifferentialCategory, ModuleSelectionSpec, MutationSpec, RoundId, RoundSummary,
};
use std::time::SystemTime;

/// Available module variants per category (mirrors build/templates/modules/).
const CARRIERS: &[&str] = &["alloc_rw_rx", "change_rw_rx", "peb_walk"];
const DECODERS: &[&str] = &["xor", "english"];
const ANTIEMULATIONS: &[&str] = &["none", "sirallocalot", "timeraw"];
const DECONDITIONERS: &[&str] = &["none", "alloc_loop"];
const GUARDRAILS: &[&str] = &["none", "env"];
const VIRTUALPROTECTS: &[&str] = &["standard", "undersized"];
const DECOYS: &[&str] = &["none", "calc", "winexec"];

/// Available AST mutations (from default mutation pool).
const AST_MUTATIONS: &[&str] = &[
    "ast.decon_rounds",
    "ast.fill_pattern",
    "ast.exec_decoy",
    "ast.timing_pattern",
    "ast.protection_transition",
];

/// Simple deterministic PRNG (xorshift32) for reproducible test data.
struct Rng {
    state: u32,
}

impl Rng {
    fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }

    fn next_f64(&mut self) -> f64 {
        (self.next() as f64) / (u32::MAX as f64)
    }

    fn pick<'a>(&mut self, items: &'a [&str]) -> &'a str {
        let idx = (self.next() as usize) % items.len();
        items[idx]
    }

    fn pick_n(&mut self, items: &[&str], n: usize) -> Vec<String> {
        let mut picked = Vec::new();
        for _ in 0..n {
            picked.push(self.pick(items).to_string());
        }
        // Deduplicate while preserving order
        let mut seen = std::collections::HashSet::new();
        picked.retain(|x| seen.insert(x.clone()));
        picked
    }
}

/// Builder for synthetic round sequences.
pub struct RoundSequenceBuilder {
    rounds: Vec<RoundSummary>,
    next_round: u32,
    job_id: String,
}

impl RoundSequenceBuilder {
    pub fn new() -> Self {
        Self {
            rounds: Vec::new(),
            next_round: 1,
            job_id: "eval-synthetic".to_string(),
        }
    }

    pub fn with_job_id(mut self, job_id: &str) -> Self {
        self.job_id = job_id.to_string();
        self
    }

    /// Add a single round with explicit parameters.
    pub fn add_round(
        &mut self,
        modules: ModuleSelectionSpec,
        mutations: Vec<String>,
        category: DifferentialCategory,
        evasion_score: f64,
    ) -> &mut Self {
        let n = self.next_round;
        self.next_round += 1;

        let detected = category.is_detected();
        let behavior_match = matches!(
            category,
            DifferentialCategory::RealDetection | DifferentialCategory::Evasion
        );

        let mutation_specs = mutations
            .iter()
            .map(|id| MutationSpec {
                id: id.clone(),
                params: None,
            })
            .collect();

        self.rounds.push(RoundSummary {
            round_id: RoundId(format!("{}-r{}", self.job_id, n)),
            round_number: n,
            mutations,
            mutation_specs,
            modules,
            detected,
            behavior_match,
            evasion_score,
            differential_category: category,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: if detected {
                "detected".to_string()
            } else {
                "evasion".to_string()
            },
            coverage_percent: Some(0.5 + evasion_score * 0.3),
            time_factor: 0.0,
        });

        self
    }

    /// Generate N random rounds with a seed for reproducibility.
    pub fn random_rounds(&mut self, n: usize, seed: u32) -> &mut Self {
        let mut rng = Rng::new(seed);

        for _ in 0..n {
            let modules = ModuleSelectionSpec {
                carrier: rng.pick(CARRIERS).to_string(),
                decoder: rng.pick(DECODERS).to_string(),
                antiemulation: rng.pick(ANTIEMULATIONS).to_string(),
                deconditioner: rng.pick(DECONDITIONERS).to_string(),
                guardrail: rng.pick(GUARDRAILS).to_string(),
                virtualprotect: rng.pick(VIRTUALPROTECTS).to_string(),
                decoy: rng.pick(DECOYS).to_string(),
            };

            let n_mutations = (rng.next() as usize % 4) + 1;
            let mutations = rng.pick_n(AST_MUTATIONS, n_mutations);

            let r = rng.next_f64();
            let category = if r < 0.4 {
                DifferentialCategory::RealDetection
            } else if r < 0.55 {
                DifferentialCategory::MutationFailed
            } else if r < 0.65 {
                DifferentialCategory::InstrumentationArtifact
            } else if r < 0.75 {
                DifferentialCategory::Flaky
            } else {
                DifferentialCategory::Evasion
            };

            let evasion_score = match category {
                DifferentialCategory::RealDetection => rng.next_f64() * 0.3,
                DifferentialCategory::MutationFailed => 0.0,
                DifferentialCategory::InstrumentationArtifact => 0.5 + rng.next_f64() * 0.2,
                DifferentialCategory::Flaky => rng.next_f64() * 0.3,
                DifferentialCategory::Evasion => 0.6 + rng.next_f64() * 0.4,
                _ => 0.0,
            };

            self.add_round(modules, mutations, category, evasion_score);
        }

        self
    }

    /// Scenario: gradual improvement from detected → evasion over N rounds.
    pub fn improvement_scenario(&mut self, n: usize) -> &mut Self {
        let mut rng = Rng::new(42);
        for i in 0..n {
            let progress = i as f64 / n.max(1) as f64;

            let modules = ModuleSelectionSpec {
                carrier: if progress > 0.5 {
                    "peb_walk"
                } else {
                    "alloc_rw_rx"
                }
                .to_string(),
                decoder: "xor".to_string(),
                antiemulation: if progress > 0.3 { "timeraw" } else { "none" }.to_string(),
                deconditioner: if progress > 0.4 { "alloc_loop" } else { "none" }.to_string(),
                guardrail: "none".to_string(),
                virtualprotect: if progress > 0.6 {
                    "undersized"
                } else {
                    "standard"
                }
                .to_string(),
                decoy: if progress > 0.7 { "calc" } else { "none" }.to_string(),
            };

            let n_mutations = 1 + (progress * 3.0) as usize;
            let mutations = rng.pick_n(AST_MUTATIONS, n_mutations);

            let noise = rng.next_f64() * 0.15;
            let (category, evasion_score) = if progress + noise < 0.3 {
                (DifferentialCategory::RealDetection, progress * 0.3)
            } else if progress + noise < 0.7 {
                (DifferentialCategory::RealDetection, progress * 0.4)
            } else {
                (DifferentialCategory::Evasion, 0.6 + progress * 0.3)
            };

            self.add_round(modules, mutations, category, evasion_score);
        }

        self
    }

    /// Scenario: quick gain then plateau (flat evasion score) for N rounds.
    pub fn plateau_scenario(&mut self, n: usize) -> &mut Self {
        let mut rng = Rng::new(99);
        for i in 0..n {
            let progress = i as f64 / n.max(1) as f64;

            let modules = if i < n / 4 {
                ModuleSelectionSpec::default()
            } else {
                ModuleSelectionSpec {
                    carrier: "peb_walk".to_string(),
                    decoder: "xor".to_string(),
                    antiemulation: "timeraw".to_string(),
                    deconditioner: "alloc_loop".to_string(),
                    guardrail: "none".to_string(),
                    virtualprotect: "undersized".to_string(),
                    decoy: "none".to_string(),
                }
            };

            let mutations = rng.pick_n(AST_MUTATIONS, 2);

            let noise = rng.next_f64() * 0.1;
            let (category, evasion_score) = if progress < 0.25 {
                (
                    DifferentialCategory::RealDetection,
                    0.1 + progress * 1.2 + noise,
                )
            } else {
                // Plateau: hovering around 0.45
                (DifferentialCategory::RealDetection, 0.4 + noise)
            };

            self.add_round(modules, mutations, category, evasion_score);
        }

        self
    }

    /// Scenario: all rounds detected (worst case).
    pub fn all_detected(&mut self, n: usize) -> &mut Self {
        let mut rng = Rng::new(1);
        for _ in 0..n {
            let modules = ModuleSelectionSpec::default();
            let mutations = rng.pick_n(AST_MUTATIONS, 2);
            self.add_round(
                modules,
                mutations,
                DifferentialCategory::RealDetection,
                rng.next_f64() * 0.2,
            );
        }
        self
    }

    /// Scenario: all rounds evade (best case).
    pub fn all_evasion(&mut self, n: usize) -> &mut Self {
        let mut rng = Rng::new(2);
        for _ in 0..n {
            let modules = ModuleSelectionSpec {
                carrier: rng.pick(CARRIERS).to_string(),
                decoder: rng.pick(DECODERS).to_string(),
                antiemulation: rng.pick(ANTIEMULATIONS).to_string(),
                deconditioner: rng.pick(DECONDITIONERS).to_string(),
                guardrail: rng.pick(GUARDRAILS).to_string(),
                virtualprotect: rng.pick(VIRTUALPROTECTS).to_string(),
                decoy: rng.pick(DECOYS).to_string(),
            };
            let mutations = rng.pick_n(AST_MUTATIONS, 3);
            self.add_round(
                modules,
                mutations,
                DifferentialCategory::Evasion,
                0.7 + rng.next_f64() * 0.3,
            );
        }
        self
    }

    pub fn build(self) -> Vec<RoundSummary> {
        self.rounds
    }
}

impl Default for RoundSequenceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_rounds_deterministic() {
        let mut ba = RoundSequenceBuilder::new();
        ba.random_rounds(10, 42);
        let a = ba.build();
        let mut bb = RoundSequenceBuilder::new();
        bb.random_rounds(10, 42);
        let b = bb.build();
        assert_eq!(a.len(), b.len());
        for (ra, rb) in a.iter().zip(b.iter()) {
            assert_eq!(ra.modules, rb.modules);
            assert_eq!(ra.mutations, rb.mutations);
            assert_eq!(ra.differential_category, rb.differential_category);
        }
    }

    #[test]
    fn test_improvement_scenario() {
        let mut b = RoundSequenceBuilder::new();
        b.improvement_scenario(20);
        let rounds = b.build();
        assert_eq!(rounds.len(), 20);
        // Later rounds should tend to have higher evasion scores
        let first_half_avg: f64 = rounds[..10].iter().map(|r| r.evasion_score).sum::<f64>() / 10.0;
        let second_half_avg: f64 = rounds[10..].iter().map(|r| r.evasion_score).sum::<f64>() / 10.0;
        assert!(
            second_half_avg > first_half_avg,
            "Improvement scenario: second half ({}) should score higher than first ({})",
            second_half_avg,
            first_half_avg
        );
    }
}
