//! JSON report writer for evaluation results.

use crate::MetricResult;
use std::path::Path;

/// Write metric results to a JSON file.
pub fn write_json_report(results: &[MetricResult], path: &Path) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(results)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Format metric results as a JSON string.
pub fn format_json(results: &[MetricResult]) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(results)?)
}

/// Write a summary report grouped by axis.
pub fn write_summary_report(
    results: &[MetricResult],
    job_id: &str,
    path: &Path,
) -> anyhow::Result<()> {
    let summary = serde_json::json!({
        "job_id": job_id,
        "total_metrics": results.len(),
        "axes": {
            "input": results.iter().filter(|r| r.axis == "input").collect::<Vec<_>>(),
            "oracle": results.iter().filter(|r| r.axis == "oracle").collect::<Vec<_>>(),
            "guidance": results.iter().filter(|r| r.axis == "guidance").collect::<Vec<_>>(),
        }
    });
    let json = serde_json::to_string_pretty(&summary)?;
    std::fs::write(path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetricResult;
    use serde_json::json;

    #[test]
    fn test_format_json() {
        let results = vec![MetricResult {
            metric_id: "test.metric".to_string(),
            axis: "test".to_string(),
            category: "test".to_string(),
            label: "Test metric".to_string(),
            value: 0.75,
            details: json!({}),
            n: 10,
        }];
        let json = format_json(&results).unwrap();
        assert!(json.contains("test.metric"));
        assert!(json.contains("0.75"));
    }
}
