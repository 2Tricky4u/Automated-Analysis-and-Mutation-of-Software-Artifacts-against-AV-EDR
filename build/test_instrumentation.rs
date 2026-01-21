//! Quick test binary for instrumentation features
//!
//! Usage:
//!   cargo run --example test_instrumentation
//!
//! This will:
//! 1. Create a simple test C program
//! 2. Build it with different instrumentation modes
//! 3. Execute and verify telemetry outputs
use base64::{Engine as _, engine::general_purpose};
use build_emitter::{ArtifactBuilder, BuildConfig, TraceMode};
use std::path::Path;
use std::process::Command;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter("build_emitter=debug")
        .init();

    println!("==========================================================");
    println!("  Instrumentation Quick Test");
    println!("==========================================================\n");

    // Create test directory
    let test_dir = Path::new("test_artifacts");
    std::fs::create_dir_all(test_dir)?;

    // Create temp directory for telemetry
    let temp_dir = Path::new("C:\\temp");
    std::fs::create_dir_all(temp_dir)?;

    // ====================================================================
    // Step 1: Create test C program
    // ====================================================================

    println!("📝 Step 1: Creating test C program...");

    let test_source = test_dir.join("instrumentation_test.c");
    std::fs::write(
        &test_source,
        r#"
#include <windows.h>
#include <stdio.h>

int add(int a, int b) {
    return a + b;
}

int main() {
    printf("=== Instrumentation Test Program ===\n");

    // Test 1: Simple computation
    int x = 5;
    int y = 10;
    int sum = add(x, y);
    printf("Sum: %d + %d = %d\n", x, y, sum);

    // Test 2: Loop (creates multiple basic blocks)
    printf("\nLoop test:\n");
    for (int i = 0; i < 5; i++) {
        if (i % 2 == 0) {
            printf("  Even: %d\n", i);
        } else {
            printf("  Odd: %d\n", i);
        }
    }

    // Test 3: API calls (will be tracked by API instrumentation)
    printf("\nAPI test:\n");
    void* mem = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (mem) {
        printf("  VirtualAlloc: SUCCESS (%p)\n", mem);

        DWORD oldProtect;
        if (VirtualProtect(mem, 4096, PAGE_EXECUTE_READ, &oldProtect)) {
            printf("  VirtualProtect: SUCCESS\n");
        }

        VirtualFree(mem, 0, MEM_RELEASE);
    }

    printf("\n=== Test Complete ===\n");
    return 0;
}
"#,
    )?;

    println!("✅ Created: {:?}\n", test_source);

    // ====================================================================
    // Test Mode 1: BB Coverage Only
    // ====================================================================

    println!("==========================================================");
    println!("🧪 Test 1: BB Coverage Mode");
    println!("==========================================================");

    // Clean before EACH test to ensure no cross-contamination
    clean_telemetry_files()?;

    let config_bb = BuildConfig {
        trace_mode: TraceMode::BB,
        optimization: "0".to_string(), // No optimization for easier debugging
        deterministic: true,
        ..Default::default()
    };

    let output_bb = test_dir.join("instrumentation_test_bb.exe");

    println!("Building with BB instrumentation...");
    let builder_bb = ArtifactBuilder::new(config_bb);

    match builder_bb.build(&test_source, &[], &output_bb).await {
        Ok(metadata) => {
            println!("✅ Built successfully!");
            println!("   Artifact ID: {}", metadata.artifact_id);
            println!("   Path: {:?}", metadata.artifact_path);

            // Execute from test_artifacts directory so telemetry files are written there
            println!("\nExecuting artifact...");
            let abs_exe = std::fs::canonicalize(&output_bb)?;
            let status = Command::new(&abs_exe)
                .current_dir("test_artifacts")
                .status()?;
            println!("   Exit code: {}", status.code().unwrap_or(-1));

            // Verify telemetry
            println!("\n[STATUS] Checking telemetry outputs:");
            check_coverage_file()?;
            check_checkpoints_file(false)?; // Should NOT exist in BB mode
            check_trace_file(false)?; // Should NOT exist in BB mode
        }
        Err(e) => {
            println!("❌ Build failed: {}", e);
            println!("   This is expected if clang/llc are not in PATH");
            println!("   Skipping execution tests...");
        }
    }

    // ====================================================================
    // Test Mode 2: API Checkpoints Only
    // ====================================================================

    println!("\n==========================================================");
    println!("🧪 Test 2: API Checkpoint Mode");
    println!("==========================================================");

    // Clean before this test
    clean_telemetry_files()?;

    let config_api = BuildConfig {
        trace_mode: TraceMode::Api,
        optimization: "0".to_string(),
        deterministic: true,
        ..Default::default()
    };

    let output_api = test_dir.join("instrumentation_test_api.exe");

    println!("Building with API instrumentation...");
    let builder_api = ArtifactBuilder::new(config_api);

    match builder_api.build(&test_source, &[], &output_api).await {
        Ok(metadata) => {
            println!("✅ Built successfully!");
            println!("   Artifact ID: {}", metadata.artifact_id);

            println!("\nExecuting artifact...");
            let abs_exe = std::fs::canonicalize(&output_api)?;
            let status = Command::new(&abs_exe)
                .current_dir("test_artifacts")
                .status()?;
            println!("   Exit code: {}", status.code().unwrap_or(-1));

            println!("\n[STATUS] Checking telemetry outputs:");
            check_coverage_file_expected(false)?; // Should NOT exist in API mode
            check_checkpoints_file(true)?; // Should exist
            check_trace_file(false)?;
        }
        Err(e) => {
            println!("❌ Build failed: {}", e);
            println!("   Skipping execution tests...");
        }
    }

    // ====================================================================
    // Test Mode 3: Combined (BB + API) - DEFAULT
    // ====================================================================

    println!("\n==========================================================");
    println!("🧪 Test 3: Combined Mode (BB + API) [DEFAULT]");
    println!("==========================================================");

    // Clean before this test
    clean_telemetry_files()?;

    let config_combined = BuildConfig {
        trace_mode: TraceMode::ApiPlusBB,
        optimization: "0".to_string(),
        deterministic: true,
        ..Default::default()
    };

    let output_combined = test_dir.join("instrumentation_test_combined.exe");

    println!("Building with combined instrumentation...");
    let builder_combined = ArtifactBuilder::new(config_combined);

    match builder_combined
        .build(&test_source, &[], &output_combined)
        .await
    {
        Ok(metadata) => {
            println!("✅ Built successfully!");
            println!("   Artifact ID: {}", metadata.artifact_id);

            println!("\nExecuting artifact...");
            let abs_exe = std::fs::canonicalize(&output_combined)?;
            let status = Command::new(&abs_exe)
                .current_dir("test_artifacts")
                .status()?;
            println!("   Exit code: {}", status.code().unwrap_or(-1));

            println!("\n[STATUS] Checking telemetry outputs:");
            check_coverage_file()?; // Should exist
            check_checkpoints_file(true)?; // Should exist
            check_trace_file(false)?;
        }
        Err(e) => {
            println!("❌ Build failed: {}", e);
            println!("   Skipping execution tests...");
        }
    }

    // ====================================================================
    // Test Mode 4: No Instrumentation
    // ====================================================================

    println!("\n==========================================================");
    println!("🧪 Test 4: No Instrumentation (Baseline)");
    println!("==========================================================");

    clean_telemetry_files()?;

    let config_off = BuildConfig {
        trace_mode: TraceMode::Off,
        optimization: "0".to_string(),
        deterministic: true,
        ..Default::default()
    };

    let output_off = test_dir.join("instrumentation_test_off.exe");

    println!("Building without instrumentation...");
    let builder_off = ArtifactBuilder::new(config_off);

    match builder_off.build(&test_source, &[], &output_off).await {
        Ok(metadata) => {
            println!("✅ Built successfully!");
            println!("   Artifact ID: {}", metadata.artifact_id);

            println!("\nExecuting artifact...");
            let abs_exe = std::fs::canonicalize(&output_off)?;
            let status = Command::new(&abs_exe)
                .current_dir("test_artifacts")
                .status()?;
            println!("   Exit code: {}", status.code().unwrap_or(-1));

            println!("\n[STATUS] Checking telemetry outputs:");
            check_coverage_file_expected(false)?; // Should NOT exist
            check_checkpoints_file(false)?; // Should NOT exist
            check_trace_file(false)?;
        }
        Err(e) => {
            println!("❌ Build failed: {}", e);
            println!("   Skipping execution tests...");
        }
    }

    // ====================================================================
    // Test Mode 5: Line-Level Tracing (Diagnostic Mode)
    // ====================================================================

    println!("\n==========================================================");
    println!("🧪 Test 5: Line-Level Tracing (Diagnostic Mode)");
    println!("==========================================================");

    clean_telemetry_files()?;

    let config_lines = BuildConfig {
        trace_mode: TraceMode::Lines,
        optimization: "0".to_string(),
        deterministic: true,
        ..Default::default()
    };

    let output_lines = test_dir.join("instrumentation_test_lines.exe");

    println!("Building with line-level tracing...");
    let builder_lines = ArtifactBuilder::new(config_lines);

    match builder_lines.build(&test_source, &[], &output_lines).await {
        Ok(metadata) => {
            println!("✅ Built successfully!");
            println!("   Artifact ID: {}", metadata.artifact_id);

            println!("\nExecuting artifact...");
            let abs_exe = std::fs::canonicalize(&output_lines)?;
            let output = Command::new(&abs_exe)
                .current_dir("test_artifacts")
                .output()?;
            println!("   Exit code: {}", output.status.code().unwrap_or(-1));

            // Parse stderr for b64line: traces (Lepori 2023 style)
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let trace_lines: Vec<_> = stderr_str
                .lines()
                .filter(|l| l.starts_with("b64line:"))
                .collect();

            if !trace_lines.is_empty() {
                println!(
                    "\n   📍 Captured {} Base64-encoded trace lines from stderr",
                    trace_lines.len()
                );
                println!("      Sample traces (first 3):");
                for (i, line) in trace_lines.iter().take(3).enumerate() {
                    // Decode Base64
                    if let Some(b64_data) = line.strip_prefix("b64line:")
                        && let Ok(decoded_bytes) = general_purpose::STANDARD.decode(b64_data.trim())
                        && let Ok(decoded_str) = String::from_utf8(decoded_bytes.to_owned())
                    {
                        println!("      {}. {}", i + 1, decoded_str);
                    }
                }
            }

            println!("\n[STATUS] Checking telemetry outputs:");
            check_coverage_file_expected(false)?; // Should NOT exist (Lines mode doesn't do BB coverage)
            check_checkpoints_file(false)?; // Should NOT exist
            check_trace_file(true)?; // Should exist!
        }
        Err(e) => {
            println!("❌ Build failed: {}", e);
            println!("   Skipping execution tests...");
        }
    }

    // ====================================================================
    // Summary
    // ====================================================================

    println!("\n==========================================================");
    println!("✅ Instrumentation Test Complete!");
    println!("==========================================================");
    println!("\nNext Steps:");
    println!("  1. Check test_artifacts/ for built executables");
    println!("  2. Manually run: test_artifacts/instrumentation_test_combined.exe");
    println!("  3. Inspect: test_artifacts/coverage.bin and test_artifacts/checkpoints.log");
    println!("  4. See INSTRUMENTATION-QUICKSTART.md for analysis examples");
    println!("\nFor detailed guide, see: INSTRUMENTATION-GUIDE.md\n");

    Ok(())
}

fn clean_telemetry_files() -> anyhow::Result<()> {
    // Clean files from current directory (where executables run)
    let _ = std::fs::remove_file("test_artifacts/coverage.bin");
    let _ = std::fs::remove_file("test_artifacts/coverage_bbs.txt");
    let _ = std::fs::remove_file("test_artifacts/checkpoints.log");
    let _ = std::fs::remove_file("test_artifacts/trace.log");
    Ok(())
}

fn check_coverage_file() -> anyhow::Result<()> {
    let path = Path::new("test_artifacts/coverage.bin");
    let bb_path = Path::new("test_artifacts/coverage_bbs.txt");

    if path.exists() {
        let size = std::fs::metadata(path)?.len();
        println!("   ✅ coverage.bin: {} bytes", size);

        if size == 65536 {
            // Analyze bitmap
            let data = std::fs::read(path)?;
            let edge_count = data.iter().filter(|&&b| b > 0).count();
            println!("      Edges hit: {}/65536", edge_count);

            // Show sample
            let sample: Vec<_> = data
                .iter()
                .enumerate()
                .filter(|(_, b)| **b > 0)
                .take(5)
                .map(|(i, b)| format!("Edge{}:{}", i, b))
                .collect();
            println!("      Sample: {:?}", sample);
        }

        // Check BB metadata file
        if bb_path.exists() {
            let content = std::fs::read_to_string(bb_path)?;
            let bb_lines: Vec<_> = content
                .lines()
                .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
                .collect();

            let total_bbs = bb_lines.len();
            let hit_bbs = bb_lines
                .iter()
                .filter(|line| {
                    let parts: Vec<_> = line.split_whitespace().collect();
                    parts.len() >= 2 && parts[1] != "0"
                })
                .count();

            println!(
                "   ✅ coverage_bbs.txt: {} BBs instrumented, {} hit",
                total_bbs, hit_bbs
            );

            // Show which BBs were hit
            println!("      BBs hit:");
            for line in bb_lines.iter().take(10) {
                let parts: Vec<_> = line.split_whitespace().collect();
                if parts.len() >= 2 && parts[1] != "0" {
                    println!("        BB {} - hit {} times", parts[0], parts[1]);
                }
            }
        }
    } else {
        println!("   ❌ coverage.bin: NOT FOUND (this should exist)");
    }
    Ok(())
}

fn check_coverage_file_expected(should_exist: bool) -> anyhow::Result<()> {
    let path = Path::new("test_artifacts/coverage.bin");
    if path.exists() {
        if should_exist {
            println!("   ✅ coverage.bin: EXISTS (as expected)");
        } else {
            println!("   ⚠️  coverage.bin: EXISTS (should NOT exist in this mode)");
        }
    } else if should_exist {
        println!("   ❌ coverage.bin: NOT FOUND (should exist)");
    } else {
        println!("   ✅ coverage.bin: NOT FOUND (as expected)");
    }

    Ok(())
}

fn check_checkpoints_file(should_exist: bool) -> anyhow::Result<()> {
    let path = Path::new("test_artifacts/checkpoints.log");
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let line_count = content.lines().count();

        if should_exist {
            println!("   ✅ checkpoints.log: {} API calls logged", line_count);

            // Show first 3 checkpoints
            for (i, line) in content.lines().take(3).enumerate() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(line)
                    && let Some(checkpoint) = json.get("checkpoint")
                {
                    println!("      {}. {}", i + 1, checkpoint);
                }
            }
        } else {
            println!("   ⚠️  checkpoints.log: EXISTS (should NOT exist in this mode)");
        }
    } else if should_exist {
        println!("   ❌ checkpoints.log: NOT FOUND (should exist)");
    } else {
        println!("   ✅ checkpoints.log: NOT FOUND (as expected)");
    }

    Ok(())
}

fn check_trace_file(should_exist: bool) -> anyhow::Result<()> {
    let path = Path::new("test_artifacts/trace.log");
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let line_count = content.lines().count();

        if should_exist {
            println!("   ✅ trace.log: {} lines traced", line_count);

            // Show first 5 trace lines
            println!("      Sample traces:");
            for (i, line) in content.lines().take(5).enumerate() {
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(line)
                    && let (Some(seq), Some(file), Some(line_num), Some(func)) = (
                        json.get("seq"),
                        json.get("file"),
                        json.get("line"),
                        json.get("func"),
                    )
                {
                    // Decode base64 file and func (if you want to show decoded)
                    println!(
                        "      {}. seq={} line={} file={} func={}",
                        i + 1,
                        seq,
                        line_num,
                        file,
                        func
                    );
                }
            }
        } else {
            println!("   ⚠️  trace.log: EXISTS (should NOT exist in this mode)");
            println!("      {} lines found", line_count);
        }
    } else if should_exist {
        println!("   ❌ trace.log: NOT FOUND (should exist)");
    } else {
        println!("   ✅ trace.log: NOT FOUND (as expected)");
    }

    Ok(())
}
