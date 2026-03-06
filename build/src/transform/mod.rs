//! AST, IR, and binary mutation transforms.
//!
//! Mutations are applied in three ordered layers during the build pipeline:
//!
//! ```text
//! Source (.c)  →  AST mutations  →  Compile  →  IR mutations  →  Link  →  Binary mutations
//!                 └─ AstMutator                 └─ IrMutator              └─ BinaryMutator
//! ```
//!
//! Mutation dispatch is based on the `id` prefix of each [`crate::mutator::MutationSpec`]:
//! - `ast.*`    → [`AstMutator`] (tree-sitter, marker-based + global transforms)
//! - `llvm.*`   → [`IrMutator`] (text-based `.ll` transforms, no LLVM C-API)
//! - `binary.*` → [`BinaryMutator`] (post-link PE structure changes)
//!
//! # Submodules
//!
//! - [`ast_mutator`] — Tree-sitter C parser for source-level mutations
//! - [`ir_mutator`] — NOP insertion, opaque predicates, junk blocks in LLVM IR
//! - [`binary_mutator`] — PE section renaming, import reshaping, entropy normalization
//! - [`binary_data`] — Binary data tables (Rich header, section name pools)
//! - [`benign_catalog`] — Benign Windows API call sequences for N-gram dilution

pub mod ast_mutator;
pub mod benign_catalog;
pub mod binary_data;
pub mod binary_mutator;
pub mod ir_mutator;

// Re-exports
pub use ast_mutator::AstMutator;
pub use binary_mutator::BinaryMutator;
pub use ir_mutator::IrMutator;
