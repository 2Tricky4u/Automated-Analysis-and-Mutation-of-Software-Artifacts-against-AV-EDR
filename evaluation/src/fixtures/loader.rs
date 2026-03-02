//! Load EvalDataset from JSON files for offline analysis.

use crate::EvalDataset;
use std::path::Path;

/// Load an EvalDataset from a JSON file.
pub fn load_dataset(path: &Path) -> anyhow::Result<EvalDataset> {
    let content = std::fs::read_to_string(path)?;
    let dataset: EvalDataset = serde_json::from_str(&content)?;
    Ok(dataset)
}

/// Save an EvalDataset to a JSON file.
pub fn save_dataset(dataset: &EvalDataset, path: &Path) -> anyhow::Result<()> {
    let content = serde_json::to_string_pretty(dataset)?;
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::round_factory::RoundSequenceBuilder;
    use crate::fixtures::token_factory::build_token_matrix;

    #[test]
    fn test_roundtrip_json() {
        let mut b = RoundSequenceBuilder::new();
        b.random_rounds(5, 42);
        let rounds = b.build();
        let token_matrices = build_token_matrix(&rounds);

        let dataset = EvalDataset {
            job_id: "test-job".to_string(),
            rounds,
            selections: vec![],
            token_matrices,
            telemetry_tokens: None,
        };

        let json = serde_json::to_string_pretty(&dataset).unwrap();
        let restored: EvalDataset = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.job_id, dataset.job_id);
        assert_eq!(restored.rounds.len(), dataset.rounds.len());
        assert_eq!(restored.token_matrices.len(), dataset.token_matrices.len());
    }
}
