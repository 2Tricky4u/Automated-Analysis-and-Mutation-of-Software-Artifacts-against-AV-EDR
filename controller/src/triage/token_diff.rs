//! Token set comparison — computes set differences and mutation param distances.
//!
//! Pure functions, no I/O. Used by the `CompareTokens` gRPC handler.

use std::collections::{HashMap, HashSet};

use super::param_space::{MutationComparison, MutationParamSpace, find_param_space};

/// A parsed `mutation:id:k=v:k=v` token.
///
/// Produced by [`parse_mutation_token`] when a raw token string starts with
/// `"mutation:"`. The `params` map contains the key-value pairs extracted from
/// the colon-separated segments after the mutation ID.
#[derive(Debug, Clone)]
pub struct ParsedMutationToken {
    /// Mutation identifier (e.g. `"ast.decon_rounds"`).
    pub mutation_id: String,
    /// Parameter key-value pairs parsed from `k=v` segments.
    pub params: HashMap<String, String>,
    /// Original unmodified token string.
    pub raw_token: String,
}

/// Comparison result for a single mutation across two token sets.
///
/// When both sets contain the same `mutation_id`, a per-parameter distance is
/// computed via the [`MutationParamSpace`]
/// registry. When the mutation appears in only one set, `overall_distance` is 1.0.
#[derive(Debug, Clone)]
pub struct MutationTokenComparison {
    /// Mutation identifier shared (or unique) across the two sets.
    pub mutation_id: String,
    /// Presence flag: `"both"`, `"only_a"`, or `"only_b"`.
    pub presence: String,
    /// Raw mutation token from set A (empty string if absent).
    pub token_a: String,
    /// Raw mutation token from set B (empty string if absent).
    pub token_b: String,
    /// Per-parameter distance breakdown, present only when both sets contain
    /// this mutation **and** a matching [`MutationParamSpace`]
    /// entry exists in the registry.
    pub param_comparison: Option<MutationComparison>,
    /// Aggregate distance in [0.0, 1.0]. Mean of per-parameter normalized
    /// distances when a registry entry exists; 0.0 for identical raw tokens
    /// or 1.0 for missing/differing tokens without a registry entry.
    pub overall_distance: f64,
}

/// Full token set comparison result.
///
/// Combines set-level differences (Jaccard distance, symmetric difference)
/// with per-mutation parameter distances. Produced by [`compare_token_sets`].
#[derive(Debug, Clone)]
pub struct TokenSetComparison {
    /// Tokens present in set A but not in set B.
    pub only_in_a: Vec<String>,
    /// Tokens present in set B but not in set A.
    pub only_in_b: Vec<String>,
    /// Tokens present in both sets.
    pub common: Vec<String>,
    /// Per-mutation comparisons (sorted by `mutation_id`).
    pub mutation_comparisons: Vec<MutationTokenComparison>,
    /// Jaccard distance: `1 - |A ∩ B| / |A ∪ B|`. 0.0 for identical sets,
    /// 1.0 for completely disjoint sets.
    pub jaccard_distance: f64,
    /// Total token count in set A.
    pub count_a: usize,
    /// Total token count in set B.
    pub count_b: usize,
}

/// Parse a `mutation:id:k=v:k=v` token string.
///
/// Returns `None` if the token doesn't start with `mutation:`.
pub fn parse_mutation_token(token: &str) -> Option<ParsedMutationToken> {
    if !token.starts_with("mutation:") {
        return None;
    }
    let rest = &token["mutation:".len()..];
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.is_empty() {
        return None;
    }
    let mutation_id = parts[0].to_string();
    let mut params = HashMap::new();
    for &part in &parts[1..] {
        if let Some((k, v)) = part.split_once('=') {
            params.insert(k.to_string(), v.to_string());
        }
    }
    Some(ParsedMutationToken {
        mutation_id,
        params,
        raw_token: token.to_string(),
    })
}

/// Compare two token sets, computing set differences and mutation param distances.
pub fn compare_token_sets(
    a: &[String],
    b: &[String],
    registry: &[MutationParamSpace],
) -> TokenSetComparison {
    let set_a: HashSet<&str> = a.iter().map(|s| s.as_str()).collect();
    let set_b: HashSet<&str> = b.iter().map(|s| s.as_str()).collect();

    let only_in_a: Vec<String> = set_a.difference(&set_b).map(|s| s.to_string()).collect();
    let only_in_b: Vec<String> = set_b.difference(&set_a).map(|s| s.to_string()).collect();
    let common: Vec<String> = set_a.intersection(&set_b).map(|s| s.to_string()).collect();

    // Jaccard distance = 1 - |A ∩ B| / |A ∪ B|
    let union_size = set_a.union(&set_b).count();
    let jaccard_distance = if union_size == 0 {
        0.0
    } else {
        1.0 - (common.len() as f64 / union_size as f64)
    };

    // Parse mutation tokens from each set, group by mutation_id
    let mut mutations_a: HashMap<String, ParsedMutationToken> = HashMap::new();
    let mut mutations_b: HashMap<String, ParsedMutationToken> = HashMap::new();

    for token in a {
        if let Some(parsed) = parse_mutation_token(token) {
            mutations_a.insert(parsed.mutation_id.clone(), parsed);
        }
    }
    for token in b {
        if let Some(parsed) = parse_mutation_token(token) {
            mutations_b.insert(parsed.mutation_id.clone(), parsed);
        }
    }

    // Collect all mutation IDs
    let all_ids: HashSet<&str> = mutations_a
        .keys()
        .chain(mutations_b.keys())
        .map(|s| s.as_str())
        .collect();

    let mut mutation_comparisons = Vec::new();

    for id in all_ids {
        let in_a = mutations_a.get(id);
        let in_b = mutations_b.get(id);

        match (in_a, in_b) {
            (Some(ta), Some(tb)) => {
                // Both present — compute param distance
                let param_cmp = find_param_space(registry, id)
                    .map(|space| space.compare_params(&ta.params, &tb.params));
                let overall = param_cmp
                    .as_ref()
                    .map(|c| c.overall_distance)
                    .unwrap_or_else(|| {
                        if ta.raw_token == tb.raw_token {
                            0.0
                        } else {
                            1.0
                        }
                    });

                mutation_comparisons.push(MutationTokenComparison {
                    mutation_id: id.to_string(),
                    presence: "both".to_string(),
                    token_a: ta.raw_token.clone(),
                    token_b: tb.raw_token.clone(),
                    param_comparison: param_cmp,
                    overall_distance: overall,
                });
            }
            (Some(ta), None) => {
                mutation_comparisons.push(MutationTokenComparison {
                    mutation_id: id.to_string(),
                    presence: "only_a".to_string(),
                    token_a: ta.raw_token.clone(),
                    token_b: String::new(),
                    param_comparison: None,
                    overall_distance: 1.0,
                });
            }
            (None, Some(tb)) => {
                mutation_comparisons.push(MutationTokenComparison {
                    mutation_id: id.to_string(),
                    presence: "only_b".to_string(),
                    token_a: String::new(),
                    token_b: tb.raw_token.clone(),
                    param_comparison: None,
                    overall_distance: 1.0,
                });
            }
            (None, None) => unreachable!(),
        }
    }

    // Sort by mutation_id for stable output
    mutation_comparisons.sort_by(|a, b| a.mutation_id.cmp(&b.mutation_id));

    TokenSetComparison {
        only_in_a,
        only_in_b,
        common,
        mutation_comparisons,
        jaccard_distance,
        count_a: a.len(),
        count_b: b.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triage::param_space::{MutationParamSpace, ParamDef};

    #[test]
    fn test_parse_mutation_token_basic() {
        let tok = "mutation:ast.decon_rounds:count=20:method=fixed";
        let parsed = parse_mutation_token(tok).unwrap();
        assert_eq!(parsed.mutation_id, "ast.decon_rounds");
        assert_eq!(parsed.params.get("count").unwrap(), "20");
        assert_eq!(parsed.params.get("method").unwrap(), "fixed");
    }

    #[test]
    fn test_parse_mutation_token_no_params() {
        let tok = "mutation:binary.debug_dir";
        let parsed = parse_mutation_token(tok).unwrap();
        assert_eq!(parsed.mutation_id, "binary.debug_dir");
        assert!(parsed.params.is_empty());
    }

    #[test]
    fn test_parse_non_mutation_token() {
        assert!(parse_mutation_token("api:VirtualAlloc").is_none());
        assert!(parse_mutation_token("etw:Microsoft-Windows-Kernel-Process/1").is_none());
    }

    #[test]
    fn test_compare_token_sets_basic() {
        let a = vec![
            "api:VirtualAlloc".to_string(),
            "api:VirtualProtect".to_string(),
            "etw:foo/1".to_string(),
        ];
        let b = vec![
            "api:VirtualAlloc".to_string(),
            "api:HeapAlloc".to_string(),
            "etw:foo/1".to_string(),
        ];

        let result = compare_token_sets(&a, &b, &[]);

        assert_eq!(result.count_a, 3);
        assert_eq!(result.count_b, 3);
        assert_eq!(result.common.len(), 2); // VirtualAlloc, etw:foo/1
        assert_eq!(result.only_in_a.len(), 1); // VirtualProtect
        assert_eq!(result.only_in_b.len(), 1); // HeapAlloc
        assert!(result.jaccard_distance > 0.0);
        assert!(result.jaccard_distance < 1.0);
    }

    #[test]
    fn test_compare_token_sets_identical() {
        let a = vec!["api:X".to_string(), "api:Y".to_string()];
        let result = compare_token_sets(&a, &a, &[]);
        assert!((result.jaccard_distance).abs() < 1e-9);
        assert!(result.only_in_a.is_empty());
        assert!(result.only_in_b.is_empty());
    }

    #[test]
    fn test_compare_token_sets_disjoint() {
        let a = vec!["api:X".to_string()];
        let b = vec!["api:Y".to_string()];
        let result = compare_token_sets(&a, &b, &[]);
        assert!((result.jaccard_distance - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_compare_mutation_tokens_with_registry() {
        let registry = vec![MutationParamSpace {
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
        }];

        let a = vec![
            "mutation:ast.decon_rounds:count=100:method=fixed".to_string(),
            "api:VirtualAlloc".to_string(),
        ];
        let b = vec![
            "mutation:ast.decon_rounds:count=200:method=runtime".to_string(),
            "api:VirtualAlloc".to_string(),
        ];

        let result = compare_token_sets(&a, &b, &registry);
        assert_eq!(result.mutation_comparisons.len(), 1);

        let mc = &result.mutation_comparisons[0];
        assert_eq!(mc.mutation_id, "ast.decon_rounds");
        assert_eq!(mc.presence, "both");
        assert!(mc.overall_distance > 0.0);

        let pc = mc.param_comparison.as_ref().unwrap();
        assert_eq!(pc.param_distances.len(), 2);
    }

    #[test]
    fn test_compare_mutation_only_in_one() {
        let a = vec!["mutation:ast.fill_pattern:pattern=xor".to_string()];
        let b: Vec<String> = vec![];

        let result = compare_token_sets(&a, &b, &[]);
        assert_eq!(result.mutation_comparisons.len(), 1);
        assert_eq!(result.mutation_comparisons[0].presence, "only_a");
        assert!((result.mutation_comparisons[0].overall_distance - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_empty_sets() {
        let result = compare_token_sets(&[], &[], &[]);
        assert!((result.jaccard_distance).abs() < 1e-9);
        assert_eq!(result.count_a, 0);
        assert_eq!(result.count_b, 0);
    }
}
