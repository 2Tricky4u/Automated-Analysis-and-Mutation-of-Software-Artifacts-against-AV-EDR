//! Token matrix generation for evaluation tests.

use crate::{RoundSummary, TokenMatrixEntry};
use controller::triage::extractor::extract_round_tokens;

/// Build a token matrix from round summaries.
///
/// Each entry contains the round's module+mutation tokens and detection outcome.
/// Only trustworthy rounds are marked as such.
pub fn build_token_matrix(rounds: &[RoundSummary]) -> Vec<TokenMatrixEntry> {
    rounds
        .iter()
        .map(|r| {
            let tokens = extract_round_tokens(r);
            TokenMatrixEntry {
                round_number: r.round_number,
                tokens,
                detected: r.detected,
                trustworthy: r.differential_category.is_trustworthy(),
            }
        })
        .collect()
}

/// Build a token matrix with additional synthetic telemetry tokens.
///
/// Appends synthetic API/ETW tokens based on module configuration
/// to simulate richer telemetry data.
pub fn build_enriched_token_matrix(rounds: &[RoundSummary]) -> Vec<TokenMatrixEntry> {
    rounds
        .iter()
        .map(|r| {
            let mut tokens = extract_round_tokens(r);

            // Add synthetic API tokens based on carrier type
            match r.modules.carrier.as_str() {
                "alloc_rw_rx" => {
                    tokens.push("api:NtAllocateVirtualMemory".to_string());
                    tokens.push("api:NtProtectVirtualMemory".to_string());
                    tokens.push("api_arg:NtProtectVirtualMemory:protect=R-X".to_string());
                    tokens.push("seq2:NtAllocateVirtualMemory→NtProtectVirtualMemory".to_string());
                }
                "change_rw_rx" => {
                    tokens.push("api:NtAllocateVirtualMemory".to_string());
                    tokens.push("api:NtProtectVirtualMemory".to_string());
                    tokens.push("api_arg:NtAllocateVirtualMemory:protect=RW-".to_string());
                    tokens.push("api_arg:NtProtectVirtualMemory:protect=R-X".to_string());
                }
                "peb_walk" => {
                    tokens.push("api:LdrGetProcedureAddress".to_string());
                    tokens.push("api:NtAllocateVirtualMemory".to_string());
                }
                _ => {}
            }

            // Add synthetic tokens for virtualprotect variant
            if r.modules.virtualprotect == "undersized" {
                tokens.push("api_arg:NtProtectVirtualMemory:protect=RW-".to_string());
            }

            // Add synthetic ETW tokens
            tokens.push("etw:Microsoft-Windows-Kernel-Process/1".to_string());
            if r.modules.decoy != "none" {
                tokens.push("api:CreateProcessW".to_string());
            }

            TokenMatrixEntry {
                round_number: r.round_number,
                tokens,
                detected: r.detected,
                trustworthy: r.differential_category.is_trustworthy(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::round_factory::RoundSequenceBuilder;

    #[test]
    fn test_build_token_matrix() {
        let mut b = RoundSequenceBuilder::new();
        b.random_rounds(5, 42);
        let rounds = b.build();
        let matrix = build_token_matrix(&rounds);
        assert_eq!(matrix.len(), 5);
        for entry in &matrix {
            // At least 7 module tokens
            assert!(entry.tokens.len() >= 7);
        }
    }

    #[test]
    fn test_enriched_matrix_has_more_tokens() {
        let mut b = RoundSequenceBuilder::new();
        b.random_rounds(5, 42);
        let rounds = b.build();
        let basic = build_token_matrix(&rounds);
        let enriched = build_enriched_token_matrix(&rounds);
        for (b, e) in basic.iter().zip(enriched.iter()) {
            assert!(
                e.tokens.len() >= b.tokens.len(),
                "Enriched should have >= tokens"
            );
        }
    }
}
