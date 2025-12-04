/// Trace log compression with loop detection
/// Reduces repetitive line traces from tight loops

/// Compressed trace representation
#[derive(Debug)]
pub struct CompressedTrace {
    pub original_size: usize,
    pub compressed_size: usize,
    pub content: String,
    pub compression_ratio: f64,
}

/// Simple loop detection: if we see the same sequence of N lines repeated M times,
/// compress it as [LOOP M times] ... [END LOOP]
pub fn compress_trace_log(content: &str, min_loop_iterations: usize) -> CompressedTrace {
    let lines: Vec<&str> = content.lines().collect();
    let original_size = content.len();

    if lines.len() < 10 {
        // Too short to compress
        return CompressedTrace {
            original_size,
            compressed_size: original_size,
            content: content.to_string(),
            compression_ratio: 1.0,
        };
    }

    let mut compressed = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        // Try to detect a loop starting at position i
        if let Some((loop_body, iterations, loop_len)) = detect_loop(&lines[i..], min_loop_iterations) {
            // Found a loop!
            compressed.push(format!("[LOOP {} iterations]", iterations));
            for line in loop_body {
                compressed.push(line.to_string());
            }
            compressed.push("[END_LOOP]".to_string());
            i += loop_len;
        } else {
            // No loop, just copy the line
            compressed.push(lines[i].to_string());
            i += 1;
        }
    }

    let compressed_content = compressed.join("\n");
    let compressed_size = compressed_content.len();

    CompressedTrace {
        original_size,
        compressed_size,
        content: compressed_content,
        compression_ratio: original_size as f64 / compressed_size as f64,
    }
}

/// Detect if there's a repeating loop pattern starting at `lines`
/// Returns: (loop_body, iterations, total_lines_consumed)
fn detect_loop<'a>(lines: &'a [&'a str], min_iterations: usize) -> Option<(Vec<&'a str>, usize, usize)> {
    // Try different loop body sizes (2 to 50 lines)
    for body_size in 2..=50.min(lines.len() / 2) {
        if body_size * min_iterations > lines.len() {
            break;
        }

        let loop_body = &lines[0..body_size];
        let mut iterations = 1;

        // Check how many times this pattern repeats
        let mut pos = body_size;
        while pos + body_size <= lines.len() {
            if &lines[pos..pos + body_size] == loop_body {
                iterations += 1;
                pos += body_size;
            } else {
                break;
            }
        }

        // If we found enough iterations, return this as a loop
        if iterations >= min_iterations {
            return Some((loop_body.to_vec(), iterations, iterations * body_size));
        }
    }

    None
}

/// Apply gzip compression if needed
pub fn gzip_compress(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loop_detection() {
        let content = r#"loop.c:10:func
loop.c:11:func
loop.c:12:func
loop.c:10:func
loop.c:11:func
loop.c:12:func
loop.c:10:func
loop.c:11:func
loop.c:12:func
main.c:50:main"#;

        let compressed = compress_trace_log(content, 3);
        assert!(compressed.content.contains("[LOOP 3 iterations]"));
        assert!(compressed.content.contains("[END_LOOP]"));
        assert!(compressed.compression_ratio > 1.0);
    }
}
