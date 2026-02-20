//! Transform module - AST, IR, and binary mutations
//!
//! Provides AST-level, LLVM IR-level, and post-link binary PE transformations.

pub mod ast_mutator;
pub mod binary_data;
pub mod binary_mutator;
pub mod ir_mutator;

// Re-exports
pub use ast_mutator::AstMutator;
pub use binary_mutator::BinaryMutator;
pub use ir_mutator::IrMutator;
