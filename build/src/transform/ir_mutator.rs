//! LLVM IR-level mutations
//!
//! Semantic-preserving transformations applied to LLVM IR:
//! - Opaque predicates (always-true/false branches)
//! - CFG flattening (dispatcher-based control flow)
//! - API call indirection (via function pointers)
//! - Bogus control flow insertion

pub struct IrMutator {
    // TODO: Add LLVM context
}

impl IrMutator {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for IrMutator {
    fn default() -> Self {
        Self::new()
    }
}
