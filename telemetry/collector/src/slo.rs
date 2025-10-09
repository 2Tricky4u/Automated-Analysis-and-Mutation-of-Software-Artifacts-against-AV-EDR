/// SLO metrics and collector config facts
///
/// Implements CLAUDE.md Section 6: Collector Config Facts
/// Records ETW buffer sizes, lost events, parser configuration, etc.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorConfigFacts {
    pub etw: EtwConfig,
    pub collector: CollectorConfig,
    pub semantic_enrichment: SemanticConfig,
    pub stack: StackConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EtwConfig {
    pub buffersize_kb: u32,
    pub lost_events: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorConfig {
    pub threads: u32,
    pub cache_pools: u32,
    pub parser: String, // "sliding", "batch", etc.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticConfig {
    pub fixups: Vec<String>, // ["filekey->name", "thread->process"]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackConfig {
    pub user_hash: String,
    pub kernel_hash: String,
}

impl Default for CollectorConfigFacts {
    fn default() -> Self {
        Self {
            etw: EtwConfig {
                buffersize_kb: 1024,
                lost_events: 0,
            },
            collector: CollectorConfig {
                threads: 4,
                cache_pools: 4,
                parser: "sliding".to_string(),
            },
            semantic_enrichment: SemanticConfig {
                fixups: vec!["filekey->name".to_string(), "thread->process".to_string()],
            },
            stack: StackConfig {
                user_hash: String::new(),
                kernel_hash: String::new(),
            },
        }
    }
}

/// SLO metrics for collector performance monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SloMetrics {
    /// P95 latency from event generation to record in Elasticsearch
    pub event_to_record_ms_p95: u32,

    /// Number of events dropped due to buffer overflow
    pub dropped_events: u64,

    /// Parser throughput (events/second)
    pub events_per_second: u32,
}

impl Default for SloMetrics {
    fn default() -> Self {
        Self {
            event_to_record_ms_p95: 0,
            dropped_events: 0,
            events_per_second: 0,
        }
    }
}
