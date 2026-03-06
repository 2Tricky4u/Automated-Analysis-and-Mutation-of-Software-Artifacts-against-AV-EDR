//! ID newtypes for type safety.

use serde::{Deserialize, Serialize};

// ============================================================================
// ID type macro — eliminates boilerplate for newtype ID wrappers
// ============================================================================

macro_rules! impl_id_type {
    ($T:ident) => {
        impl std::fmt::Display for $T {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        #[allow(dead_code)]
        impl $T {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl AsRef<str> for $T {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
        impl std::borrow::Borrow<str> for $T {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
        impl From<String> for $T {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
        impl From<&str> for $T {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
    };
}

// ============================================================================
// ID types
// ============================================================================

/// Unique identifier for a mutation exploration job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub String);
impl_id_type!(JobId);

/// Unique identifier for a single mutation round within a job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoundId(pub String);
impl_id_type!(RoundId);

/// Unique identifier for a single artifact execution run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);
impl_id_type!(RunId);

/// WorkerId - ephemeral session identity (new ID per connection)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerId(pub String);
impl_id_type!(WorkerId);

/// TargetId - stable machine identity (persists across reconnects)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TargetId(pub String);
impl_id_type!(TargetId);
