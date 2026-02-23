/// Advanced trace log compression inspired by CLP, Matrix Profile, and Grammar Induction
///
/// NOT INTEGRATED — the algorithms work (tests pass) but the module cannot be
/// wired into the pipeline as-is. Three things must be fixed first:
///
/// 1. **Output format**: `to_compressed_string()` emits human-readable text
///    (`RULE_0 (used 3 times): L10 L11 L12`). The controller parses JSONL via
///    `serde_json::from_str(line)` → `v["line"].as_u64()`. Either:
///    - Rewrite the output to emit JSONL (one JSON object per unique pattern +
///      a `count` and `seq_range` field), OR
///    - Add a decompression/parsing step on the controller side
///      (`controller/src/storage/queries.rs` — `query_trace_content` and
///      `query_trace_lines`).
///
/// 2. **Information loss**: The grammar discards `seq`, `ts_us`, and `thread_id`.
///    The source viewer needs `seq` to find the cutoff line (highest seq = last
///    executed). Fix: preserve `min_seq`/`max_seq` per grammar rule occurrence
///    so consumers can reconstruct ordering.
///
/// 3. **Performance**: `MatrixProfile::compute()` creates HashMap<Vec<u32>> for
///    every window size 2..50 — O(n × max_window) entries. And
///    `Grammar::from_sequence_and_motifs` line 251 does
///    `motif.occurrences.contains(&i)` which is O(n) per position. Fix:
///    - Use a HashSet<usize> for occurrence lookups
///    - Consider capping max events before pattern detection (e.g. 50K)
///
/// Current pipeline uses simple per-line deduplication instead (in pipeline.rs).
/// This module is intended for future structural loop analysis and triage token
/// extraction from execution patterns.
///
/// Three-stage pipeline:
/// 1. CLP-inspired columnar decomposition (extract line numbers, deduplicate strings)
/// 2. Matrix Profile pattern detection (find recurring motifs in line sequences)
/// 3. Sequitur-like grammar induction (hierarchical compression of patterns)
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Compressed trace representation
#[derive(Debug)]
pub struct CompressedTrace {
    pub original_size: usize,
    pub compressed_size: usize,
    pub content: String,
    pub compression_ratio: f64,
    pub statistics: CompressionStatistics,
}

/// Statistics about compression effectiveness
#[derive(Debug, Default, Serialize)]
pub struct CompressionStatistics {
    pub original_events: usize,
    pub unique_files: usize,
    pub unique_functions: usize,
    pub patterns_found: usize,
    pub max_pattern_length: usize,
    pub total_pattern_occurrences: usize,
    pub grammar_rules: usize,
}

/// Parsed trace event (columnar representation)
#[derive(Debug, Clone, Deserialize)]
struct TraceEvent {
    #[serde(default)]
    seq: u32,
    #[serde(default)]
    thread_id: u32,
    file: String,
    line: u32,
    #[serde(default)]
    func: String,
    #[serde(default)]
    ts_us: u64,
}

/// Step 1: CLP-inspired columnar decomposition
/// Extract dense integer arrays and string dictionaries
struct ColumnarTrace {
    line_sequence: Vec<u32>,  // Dense array of line numbers
    file_dict: Vec<String>,   // Dictionary of unique file names
    func_dict: Vec<String>,   // Dictionary of unique function names
    file_indices: Vec<usize>, // Map event -> file dictionary index
    func_indices: Vec<usize>, // Map event -> func dictionary index
    thread_ids: Vec<u32>,     // Thread IDs (for multi-threaded analysis)
    timestamps: Vec<u64>,     // Timestamps (for temporal analysis)
}

impl ColumnarTrace {
    fn from_jsonl(content: &str) -> Result<Self, String> {
        let mut line_sequence = Vec::new();
        let mut file_dict = Vec::new();
        let mut func_dict = Vec::new();
        let mut file_map: HashMap<String, usize> = HashMap::new();
        let mut func_map: HashMap<String, usize> = HashMap::new();
        let mut file_indices = Vec::new();
        let mut func_indices = Vec::new();
        let mut thread_ids = Vec::new();
        let mut timestamps = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            let event: TraceEvent = serde_json::from_str(line)
                .map_err(|e| format!("Failed to parse trace event: {}", e))?;

            // Extract line number (dense array)
            line_sequence.push(event.line);

            // Deduplicate file names
            let file_idx = *file_map.entry(event.file.clone()).or_insert_with(|| {
                let idx = file_dict.len();
                file_dict.push(event.file.clone());
                idx
            });
            file_indices.push(file_idx);

            // Deduplicate function names
            let func_idx = *func_map.entry(event.func.clone()).or_insert_with(|| {
                let idx = func_dict.len();
                func_dict.push(event.func.clone());
                idx
            });
            func_indices.push(func_idx);

            thread_ids.push(event.thread_id);
            timestamps.push(event.ts_us);
        }

        Ok(Self {
            line_sequence,
            file_dict,
            func_dict,
            file_indices,
            func_indices,
            thread_ids,
            timestamps,
        })
    }
}

/// Step 2: Matrix Profile - Find recurring patterns (motifs) in line sequence
/// Simplified Matrix Profile using sliding windows and distance computation
struct MatrixProfile {
    motifs: Vec<Motif>,
}

#[derive(Debug, Clone)]
struct Motif {
    start_index: usize,
    length: usize,
    occurrences: Vec<usize>, // All positions where this pattern occurs
    distance: f64,           // Average distance between occurrences
}

impl MatrixProfile {
    fn compute(
        line_sequence: &[u32],
        min_window: usize,
        max_window: usize,
        min_occurrences: usize,
    ) -> Self {
        let mut motifs = Vec::new();

        // Try different window sizes (pattern lengths)
        for window_size in min_window..=max_window.min(line_sequence.len() / 2) {
            let mut pattern_map: HashMap<Vec<u32>, Vec<usize>> = HashMap::new();

            // Sliding window to find all patterns
            for i in 0..=line_sequence.len().saturating_sub(window_size) {
                let pattern = line_sequence[i..i + window_size].to_vec();
                pattern_map.entry(pattern).or_default().push(i);
            }

            // Filter patterns that occur frequently enough
            for (pattern, occurrences) in pattern_map {
                if occurrences.len() >= min_occurrences {
                    // Compute average distance between occurrences
                    let distances: Vec<usize> =
                        occurrences.windows(2).map(|w| w[1] - w[0]).collect();
                    let avg_distance = if distances.is_empty() {
                        0.0
                    } else {
                        distances.iter().sum::<usize>() as f64 / distances.len() as f64
                    };

                    motifs.push(Motif {
                        start_index: occurrences[0],
                        length: window_size,
                        occurrences,
                        distance: avg_distance,
                    });
                }
            }
        }

        // Sort motifs by compression benefit (occurrences * length)
        motifs.sort_by(|a, b| {
            let benefit_a = a.occurrences.len() * a.length;
            let benefit_b = b.occurrences.len() * b.length;
            benefit_b.cmp(&benefit_a)
        });

        Self { motifs }
    }
}

/// Step 3: Sequitur-like Grammar Induction
/// Build a hierarchical grammar that represents the trace structure
#[derive(Debug, Clone)]
struct Grammar {
    rules: Vec<GrammarRule>,
    start_rule: Vec<Symbol>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Symbol {
    Terminal(u32),      // Line number
    NonTerminal(usize), // Rule reference
}

#[derive(Debug, Clone)]
struct GrammarRule {
    id: usize,
    expansion: Vec<Symbol>,
    usage_count: usize,
}

impl Grammar {
    fn from_sequence_and_motifs(line_sequence: &[u32], motifs: &[Motif]) -> Self {
        let mut rules = Vec::new();
        let mut start_rule = Vec::new();
        let mut covered = vec![false; line_sequence.len()];

        // Convert motifs to grammar rules (non-overlapping greedy selection)
        for (rule_id, motif) in motifs.iter().enumerate() {
            // Check if this motif's occurrences are still uncovered
            let valid_occurrences: Vec<usize> = motif
                .occurrences
                .iter()
                .copied()
                .filter(|&start| {
                    // Check if any position in this occurrence is already covered
                    (start..start + motif.length).all(|i| i < covered.len() && !covered[i])
                })
                .collect();

            if valid_occurrences.len() >= 2 {
                // Create a rule for this pattern
                let expansion: Vec<Symbol> = line_sequence
                    [motif.start_index..motif.start_index + motif.length]
                    .iter()
                    .map(|&line| Symbol::Terminal(line))
                    .collect();

                rules.push(GrammarRule {
                    id: rule_id,
                    expansion,
                    usage_count: valid_occurrences.len(),
                });

                // Mark all occurrences as covered
                for start in valid_occurrences {
                    for i in start..start + motif.length {
                        if i < covered.len() {
                            covered[i] = true;
                        }
                    }
                }
            }
        }

        // Build start rule by referencing rules or using terminals
        let mut i = 0;
        while i < line_sequence.len() {
            // Find if position i is the start of any rule
            let matching_rule = motifs.iter().enumerate().find(|(rule_id, motif)| {
                *rule_id < rules.len()
                    && motif.occurrences.contains(&i)
                    && (i..i + motif.length).all(|idx| idx < covered.len() && covered[idx])
            });

            if let Some((rule_id, motif)) = matching_rule {
                start_rule.push(Symbol::NonTerminal(rule_id));
                i += motif.length;
            } else {
                start_rule.push(Symbol::Terminal(line_sequence[i]));
                i += 1;
            }
        }

        Self { rules, start_rule }
    }

    fn to_compressed_string(&self, columnar: &ColumnarTrace) -> String {
        let mut output = Vec::new();

        // Output grammar rules
        output.push("# Grammar Rules (Sequitur-inspired)".to_string());
        for rule in &self.rules {
            output.push(format!(
                "RULE_{} (used {} times): {}",
                rule.id,
                rule.usage_count,
                symbols_to_string(&rule.expansion, columnar)
            ));
        }

        output.push("".to_string());
        output.push("# Start Sequence".to_string());

        // Output start rule (compressed trace)
        let mut current_line = String::new();
        for symbol in &self.start_rule {
            let symbol_str = match symbol {
                Symbol::Terminal(line) => {
                    // Find file/func for this line
                    // (simplified: just output line number)
                    format!("L{}", line)
                }
                Symbol::NonTerminal(rule_id) => format!("@RULE_{}", rule_id),
            };

            if current_line.len() + symbol_str.len() + 1 > 100 {
                output.push(current_line.clone());
                current_line.clear();
            }

            if !current_line.is_empty() {
                current_line.push(' ');
            }
            current_line.push_str(&symbol_str);
        }

        if !current_line.is_empty() {
            output.push(current_line);
        }

        output.join("\n")
    }
}

fn symbols_to_string(symbols: &[Symbol], columnar: &ColumnarTrace) -> String {
    symbols
        .iter()
        .map(|s| match s {
            Symbol::Terminal(line) => format!("L{}", line),
            Symbol::NonTerminal(rule_id) => format!("@R{}", rule_id),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Main compression function using the three-stage pipeline
pub fn compress_trace_log(content: &str, min_loop_iterations: usize) -> CompressedTrace {
    let original_size = content.len();

    // Early exit for small traces
    if content.lines().count() < 10 {
        return CompressedTrace {
            original_size,
            compressed_size: original_size,
            content: content.to_string(),
            compression_ratio: 1.0,
            statistics: CompressionStatistics {
                original_events: content.lines().count(),
                ..Default::default()
            },
        };
    }

    // Stage 1: CLP-inspired columnar decomposition
    let columnar = match ColumnarTrace::from_jsonl(content) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "Warning: Failed to parse JSONL, falling back to line-based: {}",
                e
            );
            return fallback_compress(content, original_size);
        }
    };

    let original_events = columnar.line_sequence.len();

    // Stage 2: Matrix Profile - Find recurring patterns
    let min_window = 2;
    let max_window = 50.min(columnar.line_sequence.len() / 3);
    let matrix_profile = MatrixProfile::compute(
        &columnar.line_sequence,
        min_window,
        max_window,
        min_loop_iterations,
    );

    // Stage 3: Sequitur-like grammar induction
    let grammar =
        Grammar::from_sequence_and_motifs(&columnar.line_sequence, &matrix_profile.motifs);

    // Generate compressed output
    let mut output = Vec::new();

    // Header with metadata
    output.push("# CLP+MatrixProfile+Sequitur Compressed Trace".to_string());
    output.push(format!("# Original Events: {}", original_events));
    output.push(format!("# Unique Files: {}", columnar.file_dict.len()));
    output.push(format!("# Unique Functions: {}", columnar.func_dict.len()));
    output.push(format!("# Patterns Found: {}", matrix_profile.motifs.len()));
    output.push("".to_string());

    // String dictionaries (CLP-inspired)
    output.push("# File Dictionary".to_string());
    for (idx, file) in columnar.file_dict.iter().enumerate() {
        output.push(format!("F{}: {}", idx, file));
    }
    output.push("".to_string());

    output.push("# Function Dictionary".to_string());
    for (idx, func) in columnar.func_dict.iter().enumerate() {
        output.push(format!("N{}: {}", idx, func));
    }
    output.push("".to_string());

    // Grammar-compressed sequence
    output.push(grammar.to_compressed_string(&columnar));

    let compressed_content = output.join("\n");
    let compressed_size = compressed_content.len();

    // Collect statistics
    let max_pattern_length = matrix_profile
        .motifs
        .iter()
        .map(|m| m.length)
        .max()
        .unwrap_or(0);
    let total_pattern_occurrences: usize = matrix_profile
        .motifs
        .iter()
        .map(|m| m.occurrences.len())
        .sum();

    CompressedTrace {
        original_size,
        compressed_size,
        content: compressed_content,
        compression_ratio: original_size as f64 / compressed_size as f64,
        statistics: CompressionStatistics {
            original_events,
            unique_files: columnar.file_dict.len(),
            unique_functions: columnar.func_dict.len(),
            patterns_found: matrix_profile.motifs.len(),
            max_pattern_length,
            total_pattern_occurrences,
            grammar_rules: grammar.rules.len(),
        },
    }
}

/// Fallback compression for non-JSONL content
fn fallback_compress(content: &str, original_size: usize) -> CompressedTrace {
    CompressedTrace {
        original_size,
        compressed_size: original_size,
        content: content.to_string(),
        compression_ratio: 1.0,
        statistics: CompressionStatistics {
            original_events: content.lines().count(),
            ..Default::default()
        },
    }
}

/// Apply gzip compression if needed
pub fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_columnar_decomposition() {
        let content = r#"{"seq":1,"thread_id":100,"file":"loop.c","line":10,"func":"main","ts_us":1000}
{"seq":2,"thread_id":100,"file":"loop.c","line":11,"func":"main","ts_us":1001}
{"seq":3,"thread_id":100,"file":"loop.c","line":10,"func":"main","ts_us":1002}"#;

        let columnar = ColumnarTrace::from_jsonl(content).unwrap();
        assert_eq!(columnar.line_sequence, vec![10, 11, 10]);
        assert_eq!(columnar.file_dict.len(), 1);
        assert_eq!(columnar.func_dict.len(), 1);
    }

    #[test]
    fn test_pattern_detection() {
        let sequence = vec![10, 11, 12, 10, 11, 12, 10, 11, 12, 20];
        let mp = MatrixProfile::compute(&sequence, 2, 5, 3);

        assert!(
            !mp.motifs.is_empty(),
            "Should find repeating pattern [10,11,12]"
        );
    }

    #[test]
    fn test_full_compression() {
        let content = r#"{"seq":1,"thread_id":100,"file":"loop.c","line":10,"func":"main","ts_us":1000}
{"seq":2,"thread_id":100,"file":"loop.c","line":11,"func":"main","ts_us":1001}
{"seq":3,"thread_id":100,"file":"loop.c","line":12,"func":"main","ts_us":1002}
{"seq":4,"thread_id":100,"file":"loop.c","line":10,"func":"main","ts_us":1003}
{"seq":5,"thread_id":100,"file":"loop.c","line":11,"func":"main","ts_us":1004}
{"seq":6,"thread_id":100,"file":"loop.c","line":12,"func":"main","ts_us":1005}
{"seq":7,"thread_id":100,"file":"loop.c","line":10,"func":"main","ts_us":1006}
{"seq":8,"thread_id":100,"file":"loop.c","line":11,"func":"main","ts_us":1007}
{"seq":9,"thread_id":100,"file":"loop.c","line":12,"func":"main","ts_us":1008}
{"seq":10,"thread_id":100,"file":"main.c","line":50,"func":"main","ts_us":1009}"#;

        let compressed = compress_trace_log(content, 3);

        assert!(compressed.compression_ratio >= 1.0);
        assert!(compressed.statistics.patterns_found > 0);
        assert!(compressed.content.contains("Grammar Rules"));
        assert!(compressed.content.contains("File Dictionary"));
    }
}
