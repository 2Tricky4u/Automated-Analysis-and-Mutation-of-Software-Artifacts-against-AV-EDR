//! CSV report writer for evaluation results (for plotting).

use crate::MetricResult;
use std::io::Write;
use std::path::Path;

/// Write metric results to a CSV file.
///
/// Columns: metric_id, axis, category, label, value, n
pub fn write_csv_report(results: &[MetricResult], path: &Path) -> anyhow::Result<()> {
    let mut file = std::fs::File::create(path)?;
    writeln!(file, "metric_id,axis,category,label,value,n")?;
    for r in results {
        writeln!(
            file,
            "{},{},{},{},{},{}",
            escape_csv(&r.metric_id),
            escape_csv(&r.axis),
            escape_csv(&r.category),
            escape_csv(&r.label),
            r.value,
            r.n,
        )?;
    }
    Ok(())
}

/// Format metric results as a CSV string.
pub fn format_csv(results: &[MetricResult]) -> String {
    let mut out = String::from("metric_id,axis,category,label,value,n\n");
    for r in results {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            escape_csv(&r.metric_id),
            escape_csv(&r.axis),
            escape_csv(&r.category),
            escape_csv(&r.label),
            r.value,
            r.n,
        ));
    }
    out
}

fn escape_csv(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MetricResult;
    use serde_json::json;

    #[test]
    fn test_format_csv() {
        let results = vec![MetricResult {
            metric_id: "test.metric".to_string(),
            axis: "test".to_string(),
            category: "test".to_string(),
            label: "Test metric".to_string(),
            value: 0.75,
            details: json!({}),
            n: 10,
        }];
        let csv = format_csv(&results);
        assert!(csv.contains("metric_id,axis,category,label,value,n"));
        assert!(csv.contains("test.metric"));
    }

    #[test]
    fn test_escape_csv_with_comma() {
        assert_eq!(escape_csv("hello, world"), "\"hello, world\"");
        assert_eq!(escape_csv("simple"), "simple");
    }
}
