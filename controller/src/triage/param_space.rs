//! Mutation parameter registry — defines the parameter space per mutation.
//!
//! Used by `FuzzerSelector` to sample random params, perturb existing params,
//! and explore the full parameter space of each mutation.

use serde_json::{Map, Value, json};

/// A single parameter definition.
#[derive(Debug, Clone)]
pub enum ParamDef {
    /// Discrete choices: e.g., mode ∈ {"robust", "trivial"}
    Categorical {
        name: String,
        options: Vec<String>,
        default: String,
    },
    /// Integer range: e.g., count ∈ [1, 200]
    IntRange {
        name: String,
        min: i64,
        max: i64,
        default: i64,
    },
    /// Float range: e.g., density ∈ [0.0, 1.0]
    FloatRange {
        name: String,
        min: f64,
        max: f64,
        default: f64,
    },
}

impl ParamDef {
    pub fn name(&self) -> &str {
        match self {
            ParamDef::Categorical { name, .. } => name,
            ParamDef::IntRange { name, .. } => name,
            ParamDef::FloatRange { name, .. } => name,
        }
    }

    /// Default value as a string (for JSON serialization).
    pub fn default_value(&self) -> String {
        match self {
            ParamDef::Categorical { default, .. } => default.clone(),
            ParamDef::IntRange { default, .. } => default.to_string(),
            ParamDef::FloatRange { default, .. } => default.to_string(),
        }
    }

    /// Sample a random value from the parameter space.
    pub fn sample(&self, rng: &mut SeededRng) -> String {
        match self {
            ParamDef::Categorical { options, .. } => {
                let idx = rng.next_usize(options.len());
                options[idx].clone()
            }
            ParamDef::IntRange { min, max, .. } => {
                let range = (*max - *min + 1) as u64;
                let val = *min + (rng.next_u64() % range) as i64;
                val.to_string()
            }
            ParamDef::FloatRange { min, max, .. } => {
                let t = rng.next_f64();
                let val = *min + t * (*max - *min);
                format!("{:.2}", val)
            }
        }
    }

    /// Perturb a current value within the parameter space.
    ///
    /// - Categorical: random swap to a different option
    /// - Numeric: ±(intensity * range) perturbation, clamped to bounds
    pub fn perturb(&self, current: &str, rng: &mut SeededRng, intensity: f64) -> String {
        match self {
            ParamDef::Categorical { options, .. } => {
                // Pick a different option
                if options.len() <= 1 {
                    return current.to_string();
                }
                loop {
                    let idx = rng.next_usize(options.len());
                    if options[idx] != current {
                        return options[idx].clone();
                    }
                }
            }
            ParamDef::IntRange { min, max, .. } => {
                let cur: i64 = current.parse().unwrap_or(*min);
                let range = (*max - *min) as f64;
                let delta = (rng.next_f64() * 2.0 - 1.0) * intensity * range;
                let new_val = (cur as f64 + delta).round() as i64;
                new_val.clamp(*min, *max).to_string()
            }
            ParamDef::FloatRange { min, max, .. } => {
                let cur: f64 = current.parse().unwrap_or(*min);
                let range = *max - *min;
                let delta = (rng.next_f64() * 2.0 - 1.0) * intensity * range;
                let new_val = (cur + delta).clamp(*min, *max);
                format!("{:.2}", new_val)
            }
        }
    }
}

/// Full parameter space for one mutation.
#[derive(Debug, Clone)]
pub struct MutationParamSpace {
    pub mutation_id: String,
    pub params: Vec<ParamDef>,
}

impl MutationParamSpace {
    /// Sample all params as a JSON object.
    pub fn sample_params(&self, rng: &mut SeededRng) -> Option<Value> {
        if self.params.is_empty() {
            return None;
        }
        let mut map = Map::new();
        for p in &self.params {
            let val = p.sample(rng);
            map.insert(p.name().to_string(), json!(val));
        }
        Some(Value::Object(map))
    }

    /// Perturb existing params. If `current` is None, sample from scratch.
    pub fn perturb_params(
        &self,
        current: Option<&Value>,
        rng: &mut SeededRng,
        intensity: f64,
    ) -> Option<Value> {
        if self.params.is_empty() {
            return None;
        }
        let mut map = Map::new();
        for p in &self.params {
            let default = p.default_value();
            let cur_val = current
                .and_then(|v| v.get(p.name()))
                .and_then(|v| v.as_str())
                .unwrap_or(&default);
            let new_val = p.perturb(cur_val, rng, intensity);
            map.insert(p.name().to_string(), json!(new_val));
        }
        Some(Value::Object(map))
    }
}

/// Simple seeded pseudo-random number generator (xorshift64).
///
/// Deterministic given a seed — used for reproducible evolution.
#[derive(Debug, Clone)]
pub struct SeededRng {
    state: u64,
}

impl SeededRng {
    /// Create a RNG from a raw seed value (must be non-zero).
    pub fn from_raw(seed: u64) -> Self {
        SeededRng { state: seed.max(1) }
    }

    /// Create a seeded RNG from job_id and round_number.
    pub fn new(job_id: &str, round_number: u32) -> Self {
        // FNV-1a hash of job_id + round_number
        let mut h: u64 = 0xcbf29ce484222325;
        for b in job_id.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h ^= round_number as u64;
        h = h.wrapping_mul(0x100000001b3);
        // Ensure non-zero
        if h == 0 {
            h = 1;
        }
        SeededRng { state: h }
    }

    /// Next u64 via xorshift64.
    pub fn next_u64(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }

    /// Next usize in [0, n). Panics if n == 0.
    pub fn next_usize(&mut self, n: usize) -> usize {
        if n == 0 {
            panic!("next_usize called with n=0");
        }
        (self.next_u64() % n as u64) as usize
    }

    /// Next f64 in [0.0, 1.0).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }

    /// Random bool with probability `p` of being true.
    pub fn coin(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }
}

/// Full registry of parameter spaces for all implemented mutations.
///
/// Derived from the build crate's actual param parsing.
pub fn default_registry() -> Vec<MutationParamSpace> {
    vec![
        MutationParamSpace {
            mutation_id: "ast.decon_rounds".to_string(),
            params: vec![
                ParamDef::IntRange {
                    name: "count".to_string(),
                    min: 5,
                    max: 500,
                    default: 20,
                },
                ParamDef::Categorical {
                    name: "method".to_string(),
                    options: vec!["fixed".to_string(), "runtime".to_string()],
                    default: "fixed".to_string(),
                },
            ],
        },
        MutationParamSpace {
            mutation_id: "ast.fill_pattern".to_string(),
            params: vec![ParamDef::Categorical {
                name: "pattern".to_string(),
                options: vec![
                    "xor".to_string(),
                    "nop_sled".to_string(),
                    "random".to_string(),
                    "zero".to_string(),
                ],
                default: "xor".to_string(),
            }],
        },
        MutationParamSpace {
            mutation_id: "ast.exec_decoy".to_string(),
            params: vec![ParamDef::Categorical {
                name: "method".to_string(),
                options: vec![
                    "none".to_string(),
                    "direct".to_string(),
                    "thread".to_string(),
                ],
                default: "none".to_string(),
            }],
        },
        MutationParamSpace {
            mutation_id: "ast.timing_pattern".to_string(),
            params: vec![
                ParamDef::IntRange {
                    name: "min_ms".to_string(),
                    min: 0,
                    max: 500,
                    default: 10,
                },
                ParamDef::IntRange {
                    name: "max_ms".to_string(),
                    min: 10,
                    max: 2000,
                    default: 100,
                },
            ],
        },
        MutationParamSpace {
            mutation_id: "ast.protection_transition".to_string(),
            params: vec![ParamDef::Categorical {
                name: "pattern".to_string(),
                options: vec![
                    "rw_rx".to_string(),
                    "rw_rwx".to_string(),
                    "rw_r_rx".to_string(),
                ],
                default: "rw_rx".to_string(),
            }],
        },
        MutationParamSpace {
            mutation_id: "ast.string_xor".to_string(),
            params: vec![ParamDef::IntRange {
                name: "xor_key".to_string(),
                min: 1,
                max: 255,
                default: 170,
            }],
        },
        MutationParamSpace {
            mutation_id: "llvm.nop_insert".to_string(),
            params: vec![ParamDef::FloatRange {
                name: "density".to_string(),
                min: 0.0,
                max: 1.0,
                default: 0.3,
            }],
        },
        MutationParamSpace {
            mutation_id: "llvm.opaque_predicate".to_string(),
            params: vec![
                ParamDef::FloatRange {
                    name: "density".to_string(),
                    min: 0.0,
                    max: 1.0,
                    default: 0.3,
                },
                ParamDef::Categorical {
                    name: "mode".to_string(),
                    options: vec!["robust".to_string()],
                    default: "robust".to_string(),
                },
            ],
        },
        MutationParamSpace {
            mutation_id: "llvm.junk_block".to_string(),
            params: vec![ParamDef::IntRange {
                name: "count".to_string(),
                min: 1,
                max: 10,
                default: 2,
            }],
        },
        MutationParamSpace {
            mutation_id: "binary.rich_header".to_string(),
            params: vec![ParamDef::Categorical {
                name: "donor".to_string(),
                options: vec![
                    "notepad".to_string(),
                    "calc".to_string(),
                    "explorer".to_string(),
                ],
                default: "notepad".to_string(),
            }],
        },
        MutationParamSpace {
            mutation_id: "binary.import_pad".to_string(),
            params: vec![ParamDef::IntRange {
                name: "count".to_string(),
                min: 5,
                max: 100,
                default: 50,
            }],
        },
        MutationParamSpace {
            mutation_id: "binary.string_inject".to_string(),
            params: vec![ParamDef::IntRange {
                name: "count".to_string(),
                min: 5,
                max: 50,
                default: 20,
            }],
        },
        MutationParamSpace {
            mutation_id: "binary.size_pad".to_string(),
            params: vec![ParamDef::IntRange {
                name: "target_kb".to_string(),
                min: 64,
                max: 1024,
                default: 256,
            }],
        },
        MutationParamSpace {
            mutation_id: "binary.entropy_normalize".to_string(),
            params: vec![ParamDef::FloatRange {
                name: "target".to_string(),
                min: 4.0,
                max: 7.5,
                default: 6.0,
            }],
        },
        MutationParamSpace {
            mutation_id: "binary.timestamp".to_string(),
            params: vec![ParamDef::IntRange {
                name: "age_days".to_string(),
                min: 30,
                max: 1825,
                default: 365,
            }],
        },
        // Mutations with no tunable params — empty param list
        MutationParamSpace {
            mutation_id: "binary.resource_inject".to_string(),
            params: vec![],
        },
        MutationParamSpace {
            mutation_id: "binary.section_rename".to_string(),
            params: vec![],
        },
        MutationParamSpace {
            mutation_id: "binary.debug_dir".to_string(),
            params: vec![],
        },
    ]
}

/// Look up the param space for a mutation by ID.
pub fn find_param_space<'a>(
    registry: &'a [MutationParamSpace],
    mutation_id: &str,
) -> Option<&'a MutationParamSpace> {
    registry.iter().find(|m| m.mutation_id == mutation_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::coverage_selector::MUTATION_CATALOG;

    #[test]
    fn test_registry_covers_all_mutations() {
        let registry = default_registry();
        let registry_ids: Vec<&str> = registry.iter().map(|m| m.mutation_id.as_str()).collect();
        for mutation in MUTATION_CATALOG {
            assert!(
                registry_ids.contains(mutation),
                "Missing param space for mutation: {}",
                mutation
            );
        }
    }

    #[test]
    fn test_sample_covers_range() {
        let mut rng = SeededRng::new("test-job", 42);
        let int_param = ParamDef::IntRange {
            name: "count".to_string(),
            min: 5,
            max: 500,
            default: 20,
        };
        for _ in 0..100 {
            let val: i64 = int_param.sample(&mut rng).parse().unwrap();
            assert!(
                (5..=500).contains(&val),
                "IntRange sample out of bounds: {}",
                val
            );
        }

        let float_param = ParamDef::FloatRange {
            name: "density".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.3,
        };
        for _ in 0..100 {
            let val: f64 = float_param.sample(&mut rng).parse().unwrap();
            assert!(
                (0.0..=1.0).contains(&val),
                "FloatRange sample out of bounds: {}",
                val
            );
        }
    }

    #[test]
    fn test_perturb_categorical_changes_option() {
        let mut rng = SeededRng::new("test-job", 42);
        let param = ParamDef::Categorical {
            name: "mode".to_string(),
            options: vec!["robust".to_string(), "trivial".to_string()],
            default: "robust".to_string(),
        };
        let mut changed = false;
        for _ in 0..10 {
            let result = param.perturb("robust", &mut rng, 0.3);
            if result != "robust" {
                changed = true;
                assert_eq!(result, "trivial");
                break;
            }
        }
        assert!(
            changed,
            "Categorical perturb should eventually change the option"
        );
    }

    #[test]
    fn test_perturb_numeric_small_delta() {
        let mut rng = SeededRng::new("test-job", 42);
        let param = ParamDef::IntRange {
            name: "count".to_string(),
            min: 5,
            max: 500,
            default: 20,
        };
        for _ in 0..50 {
            let result: i64 = param.perturb("250", &mut rng, 0.1).parse().unwrap();
            assert!(
                (5..=500).contains(&result),
                "Perturbed value out of bounds: {}",
                result
            );
        }
    }

    #[test]
    fn test_seeded_rng_deterministic() {
        let mut rng1 = SeededRng::new("job-abc", 5);
        let mut rng2 = SeededRng::new("job-abc", 5);
        for _ in 0..20 {
            assert_eq!(rng1.next_u64(), rng2.next_u64());
        }
    }

    #[test]
    fn test_seeded_rng_different_seeds() {
        let mut rng1 = SeededRng::new("job-abc", 5);
        let mut rng2 = SeededRng::new("job-abc", 6);
        let mut same = true;
        for _ in 0..10 {
            if rng1.next_u64() != rng2.next_u64() {
                same = false;
                break;
            }
        }
        assert!(!same, "Different seeds should produce different sequences");
    }

    #[test]
    fn test_mutation_param_space_sample() {
        let mut rng = SeededRng::new("test", 1);
        let space = MutationParamSpace {
            mutation_id: "test.mutation".to_string(),
            params: vec![
                ParamDef::IntRange {
                    name: "count".to_string(),
                    min: 1,
                    max: 10,
                    default: 5,
                },
                ParamDef::Categorical {
                    name: "mode".to_string(),
                    options: vec!["a".to_string(), "b".to_string()],
                    default: "a".to_string(),
                },
            ],
        };
        let params = space.sample_params(&mut rng);
        assert!(params.is_some());
        let obj = params.unwrap();
        assert!(obj.get("count").is_some());
        assert!(obj.get("mode").is_some());
    }

    #[test]
    fn test_empty_param_space_returns_none() {
        let mut rng = SeededRng::new("test", 1);
        let space = MutationParamSpace {
            mutation_id: "binary.debug_dir".to_string(),
            params: vec![],
        };
        assert!(space.sample_params(&mut rng).is_none());
    }
}
