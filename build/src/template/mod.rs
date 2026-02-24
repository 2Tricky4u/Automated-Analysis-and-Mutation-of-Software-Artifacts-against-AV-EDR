//! Template module - Template assembly and payload encoding
//!
//! Provides @MODULE marker assembly and payload encoding (XOR, English).

pub mod assembler;
pub mod payload;
pub mod sc_checkpoints;
pub mod shellcode_stub;

// Re-exports
pub use assembler::{
    Assembler, ModuleSelection, MutationMarker, extract_mutation_markers, strip_mutation_markers,
};
pub use payload::{EncodedPayload, EncodingType, PayloadEncoder, generate_test_payload};
