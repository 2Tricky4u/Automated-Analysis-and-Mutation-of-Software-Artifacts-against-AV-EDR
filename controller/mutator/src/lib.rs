/// Mutation engine for AST, IR, binary, and behavioral transformations
///
/// This module implements CLAUDE.md Section 3: Fuzzer & Mutation Engine
///
/// Key capabilities:
/// - AST/IR transforms: control-flow jitter, opaque predicates, constant encoding
/// - Binary transforms: splicing, insertion, bitflip, shellcode re-encodings
/// - Behavioral: benign preambles/postambles, staged execution, randomized timing

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MutationSpec {
    pub id: String,
    pub layer: MutationLayer,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MutationLayer {
    Ast,
    Ir,
    Binary,
    Behavioral,
}

pub trait Mutator: Send + Sync {
    fn mutate(&self, input: &[u8], params: &serde_json::Value) -> Result<Vec<u8>>;
    fn name(&self) -> &str;
    fn layer(&self) -> MutationLayer;
}

pub struct MutationEngine {
    mutators: Vec<Box<dyn Mutator>>,
}

impl MutationEngine {
    pub fn new() -> Self {
        Self {
            mutators: Vec::new(),
        }
    }

    pub fn register(&mut self, mutator: Box<dyn Mutator>) {
        self.mutators.push(mutator);
    }

    pub fn apply(&self, input: &[u8], specs: &[MutationSpec]) -> Result<Vec<u8>> {
        let mut output = input.to_vec();

        for spec in specs {
            let mutator = self
                .mutators
                .iter()
                .find(|m| m.name() == spec.id)
                .ok_or_else(|| anyhow::anyhow!("Mutator not found: {}", spec.id))?;

            output = mutator.mutate(&output, &spec.params)?;
        }

        Ok(output)
    }

    pub fn list_mutators(&self) -> Vec<String> {
        self.mutators.iter().map(|m| m.name().to_string()).collect()
    }
}

impl Default for MutationEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutation_engine_creation() {
        let engine = MutationEngine::new();
        assert_eq!(engine.list_mutators().len(), 0);
    }
}
