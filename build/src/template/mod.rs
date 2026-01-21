//! Template module - Template assembly and payload encoding
//!
//! Provides @MODULE marker assembly and payload encoding (XOR, English).

pub mod assembler;
pub mod payload;

// Re-exports
pub use assembler::{Assembler, ModuleSelection, MutationMarker, extract_mutation_markers, strip_mutation_markers};
pub use payload::{PayloadEncoder, EncodingType, EncodedPayload, generate_test_payload};