//! GroupId - identifies a pool group by OS and capabilities.
//!
//! Workers with the same GroupId share a run pool, allowing parallel
//! execution of runs from the same job across multiple VMs.

use super::types::WorkerInfo;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Identifies a pool group by OS and sorted capabilities.
///
/// Workers with matching GroupId share a run pool. This allows:
/// - Multiple VMs with same capabilities to process runs in parallel
/// - Job isolation: different capability sets have separate pools
///
/// # Examples
/// ```ignore
/// GroupId("windows-defender")
/// GroupId("windows-cortex+rededr")
/// GroupId("linux-")  // no capabilities
/// ```
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct GroupId(String);

impl GroupId {
    /// Create GroupId from raw string (for testing/manual creation).
    #[allow(dead_code)]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Create GroupId from worker info.
    ///
    /// Format: `{os}-{cap1}+{cap2}+...` where capabilities are sorted.
    pub fn from_worker_info(info: &WorkerInfo) -> Self {
        let os = info.os.to_lowercase();

        if info.capabilities.is_empty() {
            return Self(format!("{}-", os));
        }

        let mut caps: Vec<String> = info
            .capabilities
            .iter()
            .map(|c| c.to_lowercase())
            .collect();
        caps.sort();

        Self(format!("{}-{}", os, caps.join("+")))
    }

    /// Create GroupId from OS and capabilities directly.
    #[allow(dead_code)]
    pub fn from_os_and_caps(os: &str, capabilities: &[String]) -> Self {
        let os = os.to_lowercase();

        if capabilities.is_empty() {
            return Self(format!("{}-", os));
        }

        let mut caps: Vec<String> = capabilities
            .iter()
            .map(|c| c.to_lowercase())
            .collect();
        caps.sort();

        Self(format!("{}-{}", os, caps.join("+")))
    }

    /// Get the raw string value.
    #[allow(dead_code)]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Extract OS from the GroupId.
    pub fn os(&self) -> &str {
        self.0.split('-').next().unwrap_or("")
    }

    /// Extract capabilities from the GroupId.
    pub fn capabilities(&self) -> Vec<String> {
        let caps_part = self.0.split('-').nth(1).unwrap_or("");
        if caps_part.is_empty() {
            Vec::new()
        } else {
            caps_part.split('+').map(String::from).collect()
        }
    }

    /// Check if this group can satisfy the required capabilities.
    ///
    /// A group satisfies requirements if it has ALL required capabilities.
    pub fn satisfies(&self, required_os: Option<&str>, required_caps: &[String]) -> bool {
        // Check OS match if specified
        if let Some(req_os) = required_os {
            if !self.os().eq_ignore_ascii_case(req_os) {
                return false;
            }
        }

        // Check capabilities: group must have ALL required
        let group_caps = self.capabilities();
        required_caps.iter().all(|req| {
            group_caps.iter().any(|cap| cap.eq_ignore_ascii_case(req))
        })
    }
}

impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::WorkerId;

    #[test]
    fn test_from_worker_info() {
        let info = WorkerInfo {
            id: WorkerId("vm-01".into()),
            os: "Windows10".into(),
            capabilities: vec!["Defender".into(), "RedEDR".into()],
        };

        let group_id = GroupId::from_worker_info(&info);
        // Capabilities should be sorted and lowercased
        assert_eq!(group_id.as_str(), "windows10-defender+rededr");
    }

    #[test]
    fn test_from_worker_info_no_caps() {
        let info = WorkerInfo {
            id: WorkerId("vm-01".into()),
            os: "Linux".into(),
            capabilities: vec![],
        };

        let group_id = GroupId::from_worker_info(&info);
        assert_eq!(group_id.as_str(), "linux-");
    }

    #[test]
    fn test_equality() {
        let info1 = WorkerInfo {
            id: WorkerId("vm-01".into()),
            os: "windows".into(),
            capabilities: vec!["B".into(), "A".into()],
        };
        let info2 = WorkerInfo {
            id: WorkerId("vm-02".into()),
            os: "WINDOWS".into(),
            capabilities: vec!["a".into(), "b".into()],
        };

        // Same OS and caps (sorted, lowercased) = same GroupId
        assert_eq!(
            GroupId::from_worker_info(&info1),
            GroupId::from_worker_info(&info2)
        );
    }

    #[test]
    fn test_satisfies() {
        let group = GroupId::new("windows-defender+rededr");

        // No requirements = satisfied
        assert!(group.satisfies(None, &[]));

        // OS match
        assert!(group.satisfies(Some("windows"), &[]));
        assert!(!group.satisfies(Some("linux"), &[]));

        // Capability match
        assert!(group.satisfies(None, &["defender".into()]));
        assert!(group.satisfies(None, &["defender".into(), "rededr".into()]));
        assert!(!group.satisfies(None, &["cortex".into()]));

        // Both
        assert!(group.satisfies(Some("windows"), &["defender".into()]));
        assert!(!group.satisfies(Some("linux"), &["defender".into()]));
    }

    #[test]
    fn test_extract_os_and_caps() {
        let group = GroupId::new("windows10-defender+rededr");
        assert_eq!(group.os(), "windows10");
        assert_eq!(group.capabilities(), vec!["defender", "rededr"]);

        let empty_caps = GroupId::new("linux-");
        assert_eq!(empty_caps.os(), "linux");
        assert!(empty_caps.capabilities().is_empty());
    }
}
