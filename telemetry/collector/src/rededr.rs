/// RedEDR JSON parser and normalizer
///
/// Parses RedEDR output files and converts to canonical telemetry format

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedEdrEvent {
    #[serde(rename = "@timestamp")]
    pub timestamp: String,
    pub event_type: String,
    pub metadata: serde_json::Value,
}

pub struct RedEdrParser {}

impl RedEdrParser {
    pub fn new() -> Self {
        Self {}
    }

    pub fn parse(&self, _json_content: &str) -> Result<RedEdrEvent, Box<dyn std::error::Error>> {
        // Placeholder
        Err("Not implemented".into())
    }
}
