/// Integration test for line-level tracing via named pipe
///
/// Tests the complete flow:
/// 1. Artifact writes Base64-encoded traces to named pipe
/// 2. TraceCollector receives and parses them
/// 3. Events are sent via mpsc channel
///
/// This test does NOT require full gRPC/Elasticsearch setup.
use std::time::Duration;
use tokio::sync::mpsc;

#[cfg(windows)]
#[tokio::test]
async fn test_named_pipe_trace_collection() {
    use base64::{Engine as _, engine::general_purpose};
    use worker_agent::telemetry::collectors::trace::TraceCollector;

    println!("🧪 Testing named pipe trace collection...");

    // Create mpsc channel for trace events
    let (trace_tx, mut trace_rx) = mpsc::channel(100);

    // Start async trace collector in background
    let collector = TraceCollector::new(trace_tx);
    let collector_handle = tokio::spawn(async move {
        println!("   Starting async trace collector...");
        if let Err(e) = collector.start_server().await {
            eprintln!("   Collector error: {}", e);
        }
    });

    // Give collector time to create named pipe
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Simulate artifact writing to named pipe
    let client_handle = tokio::task::spawn_blocking(|| {
        println!("   Artifact: Connecting to named pipe...");

        use windows::Win32::Storage::FileSystem::{
            CreateFileA, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, OPEN_EXISTING, WriteFile,
        };
        use windows::core::PCSTR;

        let pipe_name = "\\\\.\\pipe\\rededr_trace\0";

        // Try to connect to pipe (retry a few times)
        let mut pipe_handle = None;
        for attempt in 1..=5 {
            let handle = unsafe {
                CreateFileA(
                    PCSTR(pipe_name.as_ptr()),
                    FILE_GENERIC_WRITE.0,
                    Default::default(),
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            };

            if let Ok(h) = handle {
                pipe_handle = Some(h);
                println!("   Artifact: Connected on attempt {}", attempt);
                break;
            }

            std::thread::sleep(Duration::from_millis(200));
        }

        let pipe_handle = pipe_handle.expect("Failed to connect to named pipe");

        // Write test trace lines (Base64-encoded)
        let test_traces = vec![
            "line:test.c:42:main",
            "line:test.c:43:foo",
            "line:test.c:44:bar",
        ];

        for trace in test_traces {
            // Encode to Base64
            let encoded = general_purpose::STANDARD.encode(trace);
            let line = format!("b64line:{}\n", encoded);

            println!("   Artifact: Writing trace: {}", trace);

            // Write to pipe
            let mut bytes_written = 0u32;
            unsafe {
                WriteFile(
                    pipe_handle,
                    Some(line.as_bytes()),
                    Some(&mut bytes_written),
                    None,
                )
                .expect("Failed to write to pipe");
            }
        }

        println!("   Artifact: All traces written, closing pipe...");

        // Close pipe
        unsafe {
            use windows::Win32::Foundation::CloseHandle;
            let _ = CloseHandle(pipe_handle);
        }
    });

    // Wait for client to write traces
    let _ = tokio::time::timeout(Duration::from_secs(5), client_handle).await;

    // Give collector time to process
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Collect trace events from channel
    let mut events = Vec::new();
    while let Ok(event) = trace_rx.try_recv() {
        events.push(event);
    }

    // Verify results
    println!("\n[STATUS] Results:");
    println!("   Received {} trace events", events.len());

    assert_eq!(events.len(), 3, "Should receive 3 trace events");

    // Verify event contents
    assert_eq!(events[0].file, "test.c");
    assert_eq!(events[0].line, 42);
    assert_eq!(events[0].func, "main");

    assert_eq!(events[1].file, "test.c");
    assert_eq!(events[1].line, 43);
    assert_eq!(events[1].func, "foo");

    assert_eq!(events[2].file, "test.c");
    assert_eq!(events[2].line, 44);
    assert_eq!(events[2].func, "bar");

    // Verify sequence numbers
    assert_eq!(events[0].seq, 0);
    assert_eq!(events[1].seq, 1);
    assert_eq!(events[2].seq, 2);

    println!("   [OK] All events parsed correctly!");
    println!(
        "   [OK] Sequence numbers: {} {} {}",
        events[0].seq, events[1].seq, events[2].seq
    );
    println!(
        "   [OK] Timestamps present: {} {} {}",
        events[0].ts_us, events[1].ts_us, events[2].ts_us
    );

    // Abort collector (it's in infinite loop)
    collector_handle.abort();

    println!("\n[OK] Named pipe trace collection test passed!");
}

#[cfg(not(windows))]
#[tokio::test]
async fn test_named_pipe_trace_collection() {
    println!("[SKIP]  Skipping named pipe test (Windows only)");
}
