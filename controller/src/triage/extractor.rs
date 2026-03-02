//! Token extraction from round data and ES telemetry.
//!
//! Two sources:
//! - **In-memory** (`extract_round_tokens`): module + mutation tokens from `RoundSummary`
//! - **ES telemetry** (`extract_telemetry_tokens`): tokens from all non-trace events:
//!   - `api:*` — syscall/lifecycle function names (dll + kernel events)
//!   - `api_arg:*` — memory protection arguments (e.g. `api_arg:NtAllocateVirtualMemory:protect=RW-`)
//!   - `etw:*` — ETW provider/event pairs (e.g. `etw:Microsoft-Windows-Kernel-Process/6`)
//!   - `image:*` — loaded DLL basenames (e.g. `image:ntdll.dll`)
//!   - `seq2:*` — bigrams from consecutive `payload_func` calls
//!
//! `extract_and_score` is the top-level async function spawned in the background
//! by `finalize_round()`. It combines both sources, indexes to ES, and produces
//! `TriageGuidance` for the selector.

use crate::dispatch::types::RoundSummary;
use crate::storage::EsStorage;
use crate::triage::TriageGuidance;
use crate::triage::scorer;
use serde_json::{Value, json};
use std::collections::HashSet;
use tracing::{debug, warn};

// ============================================================================
// In-memory token extraction (pure, no IO)
// ============================================================================

/// Extract `module:*` and `mutation:*` tokens from a RoundSummary.
///
/// Module tokens: one per category (carrier, decoder, antiemulation,
/// deconditioner, guardrail, virtualprotect, decoy).
/// Mutation tokens: one per applied mutation ID.
pub fn extract_round_tokens(summary: &RoundSummary) -> Vec<String> {
    let mut tokens = Vec::with_capacity(7 + summary.mutations.len());

    // Module tokens — 7 categories
    tokens.push(format!("module:carrier={}", summary.modules.carrier));
    tokens.push(format!("module:decoder={}", summary.modules.decoder));
    tokens.push(format!(
        "module:antiemulation={}",
        summary.modules.antiemulation
    ));
    tokens.push(format!(
        "module:deconditioner={}",
        summary.modules.deconditioner
    ));
    tokens.push(format!("module:guardrail={}", summary.modules.guardrail));
    tokens.push(format!(
        "module:virtualprotect={}",
        summary.modules.virtualprotect
    ));
    tokens.push(format!("module:decoy={}", summary.modules.decoy));

    // Mutation tokens
    for mutation_id in &summary.mutations {
        tokens.push(format!("mutation:{}", mutation_id));
    }

    tokens
}

// ============================================================================
// ES telemetry token extraction (async)
// ============================================================================

/// Extract telemetry tokens from ES for a specific run.
///
/// Queries all non-trace telemetry sorted by `payload_id` and delegates to
/// [`extract_tokens_from_docs`] for the pure extraction logic.
pub async fn extract_telemetry_tokens(
    storage: &EsStorage,
    _job_id: &str,
    run_id: &str,
) -> anyhow::Result<Vec<String>> {
    let docs = storage.query_api_telemetry(run_id).await;
    Ok(extract_tokens_from_docs(&docs))
}

/// Pure token extraction from sorted telemetry docs (no IO).
///
/// Handles three event categories:
/// - **dll/kernel** (`payload_func` present): `api:<func>`, `seq2:<prev>→<func>`,
///   `api_arg:<func>:protect=<val>`, `image:<basename>`
/// - **etw** (`payload_event` present, `payload_func` null): `etw:<provider>/<event_id>`
///
/// Returns deduplicated tokens in insertion order.
pub fn extract_tokens_from_docs(docs: &[Value]) -> Vec<String> {
    if docs.is_empty() {
        return Vec::new();
    }

    let mut tokens = Vec::new();
    let mut seen = HashSet::new();
    let mut prev_func: Option<String> = None;

    for doc in docs {
        // --- dll / kernel events (payload_func is non-null) ---
        if let Some(func) = doc["payload_func"].as_str() {
            // api:<func>
            let api_token = format!("api:{func}");
            if seen.insert(api_token.clone()) {
                tokens.push(api_token);
            }

            // seq2:<prev>→<func> bigram from consecutive payload_func calls
            if let Some(ref prev) = prev_func {
                let seq_token = format!("seq2:{prev}→{func}");
                if seen.insert(seq_token.clone()) {
                    tokens.push(seq_token);
                }
            }
            prev_func = Some(func.to_string());

            // api_arg:<func>:protect=<val> (memory protection — top detection signal)
            if let Some(protect) = doc["payload_protect"].as_str() {
                let arg_token = format!("api_arg:{func}:protect={protect}");
                if seen.insert(arg_token.clone()) {
                    tokens.push(arg_token);
                }
            }

            // image:<dll_basename> from kernel image_load events
            if func == "image_load"
                && let Some(path) = doc["payload_image"].as_str()
            {
                let basename = path.rsplit('\\').next().unwrap_or(path);
                let img_token = format!("image:{}", basename.to_lowercase());
                if seen.insert(img_token.clone()) {
                    tokens.push(img_token);
                }
            }

            continue;
        }

        // --- etw events (payload_func is null, payload_event is set) ---
        if let Some(event) = doc["payload_event"].as_str() {
            let event = event.trim();
            if let Some(provider) = doc["payload_etw_provider_name"].as_str() {
                let event_id = doc["payload_event_id"]
                    .as_u64()
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "?".to_string());
                let etw_token = format!("etw:{provider}/{event_id}");
                if seen.insert(etw_token.clone()) {
                    tokens.push(etw_token);
                }
            }
            // Also emit a descriptive etw event token
            if !event.is_empty() {
                let evt_token = format!("etw_event:{event}");
                if seen.insert(evt_token.clone()) {
                    tokens.push(evt_token);
                }
            }
        }
    }

    tokens
}

// ============================================================================
// Combined extract + index + score (async background task)
// ============================================================================

/// Background task: extract tokens, index to ES, compute scores, return guidance.
///
/// Called by `finalize_round()` via `tokio::spawn`. Non-fatal — errors are logged
/// and the guidance channel simply doesn't receive an update.
pub async fn extract_and_score(
    storage: &EsStorage,
    job_id: &str,
    round_id: &str,
    baseline_run_id: &str,
    summary: &RoundSummary,
    all_summaries: &[(RoundSummary, bool)], // (summary, detected) for all prior rounds
) -> anyhow::Result<TriageGuidance> {
    // 1. Extract in-memory tokens from this round
    let mut tokens = extract_round_tokens(summary);

    // 2. Extract telemetry tokens from ES (api + seq2)
    match extract_telemetry_tokens(storage, job_id, baseline_run_id).await {
        Ok(telemetry_tokens) => {
            debug!(
                "[Triage:{}] Extracted {} telemetry tokens for round {}",
                job_id,
                telemetry_tokens.len(),
                round_id
            );
            tokens.extend(telemetry_tokens);
        }
        Err(e) => {
            warn!(
                "[Triage:{}] Failed to extract telemetry tokens: {}",
                job_id, e
            );
            // Continue with in-memory tokens only
        }
    }

    // 3. Index combined token set to `tokens-YYYY.MM`
    let doc = json!({
        "job_id": job_id,
        "round_id": round_id,
        "run_id": baseline_run_id,
        "detected": summary.detected,
        "differential_category": format!("{:?}", summary.differential_category),
        "evasion_score": summary.evasion_score,
        "modules": {
            "carrier": summary.modules.carrier,
            "decoder": summary.modules.decoder,
            "antiemulation": summary.modules.antiemulation,
            "deconditioner": summary.modules.deconditioner,
            "guardrail": summary.modules.guardrail,
            "virtualprotect": summary.modules.virtualprotect,
            "decoy": summary.modules.decoy,
        },
        "mutations": summary.mutations,
        "tokens": tokens,
        "token_count": tokens.len(),
        "timestamp": crate::storage::helpers::now_rfc3339(),
    });
    if let Err(e) = storage.index_token_set(doc).await {
        warn!("[Triage:{}] Failed to index token set: {}", job_id, e);
    }

    // 4. Build token-round matrix from all prior rounds + current
    let mut round_tokens: Vec<(Vec<String>, bool)> = Vec::with_capacity(all_summaries.len() + 1);

    for (s, detected) in all_summaries {
        // Skip InstrumentationArtifact rounds — not trustworthy
        if !s.differential_category.is_trustworthy() {
            continue;
        }
        let s_tokens = extract_round_tokens(s);
        round_tokens.push((s_tokens, *detected));
    }

    // Add current round (if trustworthy)
    if summary.differential_category.is_trustworthy() {
        round_tokens.push((tokens, summary.detected));
    }

    // 5. Compute scores and build guidance
    let scores = scorer::compute_token_scores(&round_tokens);
    let guidance = scorer::build_guidance(&scores, 1.5, 0.3);

    debug!(
        "[Triage:{}] Guidance: {} avoid, {} seek tokens",
        job_id,
        guidance.avoid_tokens.len(),
        guidance.seek_tokens.len()
    );

    Ok(guidance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::types::{
        DifferentialCategory, ModuleSelectionSpec, MutationSpec, RoundId,
    };
    use std::time::SystemTime;

    fn make_summary(modules: ModuleSelectionSpec, mutations: Vec<String>) -> RoundSummary {
        RoundSummary {
            round_id: RoundId("test-round".to_string()),
            round_number: 1,
            mutation_specs: mutations
                .iter()
                .map(|id| MutationSpec {
                    id: id.clone(),
                    params: None,
                })
                .collect(),
            mutations,
            modules,
            detected: false,
            behavior_match: true,
            evasion_score: 0.5,
            differential_category: DifferentialCategory::Evasion,
            completed_at: SystemTime::now(),
            dry_run_exit_code: None,
            has_dryrun: false,
            detection_verdict: String::new(),
            coverage_percent: None,
            time_factor: 0.0,
        }
    }

    #[test]
    fn test_extract_round_tokens() {
        let summary = make_summary(
            ModuleSelectionSpec {
                carrier: "alloc_rw_rx".to_string(),
                decoder: "xor".to_string(),
                antiemulation: "none".to_string(),
                deconditioner: "alloc_loop".to_string(),
                guardrail: "none".to_string(),
                virtualprotect: "standard".to_string(),
                decoy: "none".to_string(),
            },
            vec![
                "ast.string_xor".to_string(),
                "binary.rich_header".to_string(),
            ],
        );

        let tokens = extract_round_tokens(&summary);

        // 7 module tokens + 2 mutation tokens
        assert_eq!(tokens.len(), 9);
        assert!(tokens.contains(&"module:carrier=alloc_rw_rx".to_string()));
        assert!(tokens.contains(&"module:deconditioner=alloc_loop".to_string()));
        assert!(tokens.contains(&"mutation:ast.string_xor".to_string()));
        assert!(tokens.contains(&"mutation:binary.rich_header".to_string()));
    }

    #[test]
    fn test_extract_round_tokens_no_mutations() {
        let summary = make_summary(ModuleSelectionSpec::default(), vec![]);
        let tokens = extract_round_tokens(&summary);

        // Only 7 module tokens, no mutation tokens
        assert_eq!(tokens.len(), 7);
        assert!(tokens.iter().all(|t| t.starts_with("module:")));
    }

    #[test]
    fn test_extract_telemetry_tokens_from_docs() {
        // Build mock ES docs matching real RedEDR telemetry schema
        let docs = vec![
            // dll event: NtAllocateVirtualMemory with protect=RW-
            json!({
                "event_type": "dll",
                "payload_func": "NtAllocateVirtualMemory",
                "payload_id": 1,
                "payload_protect": "RW-",
                "payload_size": 4096,
                "payload_alloc_type": 12288,
            }),
            // dll event: NtProtectVirtualMemory with protect=R-X
            json!({
                "event_type": "dll",
                "payload_func": "NtProtectVirtualMemory",
                "payload_id": 2,
                "payload_protect": "R-X",
                "payload_size": 4096,
            }),
            // kernel event: image_load with DLL path
            json!({
                "event_type": "kernel",
                "payload_func": "image_load",
                "payload_id": 3,
                "payload_image": "\\Device\\HarddiskVolume3\\Windows\\System32\\ntdll.dll",
                "payload_type": "kernel",
            }),
            // kernel event: thread_create (no payload_protect, no payload_image)
            json!({
                "event_type": "kernel",
                "payload_func": "thread_create",
                "payload_id": 4,
                "payload_type": "kernel",
            }),
            // etw event: payload_func is null
            json!({
                "event_type": "etw",
                "payload_func": null,
                "payload_id": 5,
                "payload_event": "ImageUnloadInfo ",
                "payload_etw_provider_name": "Microsoft-Windows-Kernel-Process",
                "payload_event_id": 6,
                "payload_type": "etw",
            }),
            // etw event: ProcessStopStop
            json!({
                "event_type": "etw",
                "payload_func": null,
                "payload_id": 6,
                "payload_event": "ProcessStopStop",
                "payload_etw_provider_name": "Microsoft-Windows-Kernel-Process",
                "payload_event_id": 2,
                "payload_type": "etw",
            }),
            // Another dll event: duplicate NtAllocateVirtualMemory (should be deduplicated)
            json!({
                "event_type": "dll",
                "payload_func": "NtAllocateVirtualMemory",
                "payload_id": 7,
                "payload_protect": "RW-",
                "payload_size": 8192,
            }),
        ];

        let tokens = extract_tokens_from_docs(&docs);

        // --- api: tokens ---
        assert!(tokens.contains(&"api:NtAllocateVirtualMemory".to_string()));
        assert!(tokens.contains(&"api:NtProtectVirtualMemory".to_string()));
        assert!(tokens.contains(&"api:image_load".to_string()));
        assert!(tokens.contains(&"api:thread_create".to_string()));

        // --- api_arg: tokens ---
        assert!(tokens.contains(&"api_arg:NtAllocateVirtualMemory:protect=RW-".to_string()));
        assert!(tokens.contains(&"api_arg:NtProtectVirtualMemory:protect=R-X".to_string()));

        // --- seq2: bigrams (from consecutive payload_func calls) ---
        assert!(
            tokens.contains(&"seq2:NtAllocateVirtualMemory→NtProtectVirtualMemory".to_string())
        );
        assert!(tokens.contains(&"seq2:NtProtectVirtualMemory→image_load".to_string()));
        assert!(tokens.contains(&"seq2:image_load→thread_create".to_string()));
        // After ETW events (no payload_func), next dll event continues bigram from thread_create
        assert!(tokens.contains(&"seq2:thread_create→NtAllocateVirtualMemory".to_string()));

        // --- image: tokens ---
        assert!(tokens.contains(&"image:ntdll.dll".to_string()));

        // --- etw: tokens ---
        assert!(tokens.contains(&"etw:Microsoft-Windows-Kernel-Process/6".to_string()));
        assert!(tokens.contains(&"etw:Microsoft-Windows-Kernel-Process/2".to_string()));

        // --- etw_event: tokens ---
        assert!(tokens.contains(&"etw_event:ImageUnloadInfo".to_string()));
        assert!(tokens.contains(&"etw_event:ProcessStopStop".to_string()));

        // --- deduplication: api:NtAllocateVirtualMemory appears only once ---
        let alloc_count = tokens
            .iter()
            .filter(|t| *t == "api:NtAllocateVirtualMemory")
            .count();
        assert_eq!(
            alloc_count, 1,
            "api:NtAllocateVirtualMemory should be deduplicated"
        );
    }

    #[test]
    fn test_extract_tokens_empty_docs() {
        let tokens = extract_tokens_from_docs(&[]);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_extract_tokens_etw_only() {
        let docs = vec![json!({
            "event_type": "etw",
            "payload_func": null,
            "payload_id": 1,
            "payload_event": "ThreadStart",
            "payload_etw_provider_name": "Microsoft-Windows-Kernel-Process",
            "payload_event_id": 3,
        })];

        let tokens = extract_tokens_from_docs(&docs);
        assert_eq!(tokens.len(), 2); // etw: + etw_event:
        assert!(tokens.contains(&"etw:Microsoft-Windows-Kernel-Process/3".to_string()));
        assert!(tokens.contains(&"etw_event:ThreadStart".to_string()));
    }
}
