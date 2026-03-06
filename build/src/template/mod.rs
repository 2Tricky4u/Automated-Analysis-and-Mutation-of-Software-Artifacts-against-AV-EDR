//! Template assembly and payload encoding.
//!
//! This module implements the first phase of the build pipeline:
//!
//! ```text
//! loader_template.c + ModuleSelection → Assembler → assembled .c file
//! payload bytes + EncodingType → PayloadEncoder → payload.h header
//! ```
//!
//! The [`Assembler`] reads `loader_template.c`, finds `// @MODULE:<name>`
//! markers, and replaces each with the corresponding module file from the
//! `modules/` directory. The result is a single, self-contained C source
//! file ready for mutation and compilation.
//!
//! The [`PayloadEncoder`] encodes raw shellcode bytes into a C header
//! (`payload.h`) using one of several encoding schemes (XOR, English
//! word mapping, sub-byte nibble mapping, or no encoding).
//!
//! # Submodules
//!
//! - [`assembler`] — `@MODULE` marker replacement and module validation
//! - [`payload`] — Payload encoding and C header generation
//! - [`sc_checkpoints`] — INT3 shellcode checkpoint patching
//! - [`shellcode_stub`] — Instrumentation stub prepended to payloads

pub mod assembler;
pub mod payload;
pub mod sc_checkpoints;
pub mod shellcode_stub;

// Re-exports
pub use assembler::{
    Assembler, ModuleSelection, MutationMarker, extract_mutation_markers, strip_mutation_markers,
};
pub use payload::{EncodedPayload, EncodingType, PayloadEncoder, generate_test_payload};
