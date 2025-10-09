use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// IPC message for harness communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessMessage {
    pub message_type: MessageType,
    pub payload: HashMap<String, String>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageType {
    Start,
    Stop,
    Status,
    Result,
    Error,
}

impl HarnessMessage {
    pub fn new(message_type: MessageType) -> Self {
        Self {
            message_type,
            payload: HashMap::new(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        }
    }

    pub fn with_payload(mut self, key: &str, value: &str) -> Self {
        self.payload.insert(key.to_string(), value.to_string());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_creation() {
        let msg = HarnessMessage::new(MessageType::Start).with_payload("job_id", "test-001");

        assert!(msg.payload.contains_key("job_id"));
        assert_eq!(msg.payload.get("job_id").unwrap(), "test-001");
    }
}
