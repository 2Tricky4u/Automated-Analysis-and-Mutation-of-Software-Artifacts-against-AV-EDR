/// RedEDR File Watcher Collector (v2 - Zero Polling)
///
/// Architecture:
/// 1. Watches C:\RedEdr\Data directory for file changes using Windows ReadDirectoryChangesW
/// 2. When file modified, reads only new lines incrementally (tail -f style)
/// 3. Parses JSON events and streams to gRPC channel
/// 4. Zero CPU overhead when idle, instant event detection (<1ms latency)
///
/// Advantages over polling:
/// - No wasted HTTP requests when idle
/// - Instant event detection (vs. 1000ms polling delay)
/// - No unbounded memory growth (no need to track seen_trace_ids)
/// - Better backpressure handling with async send

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// RedEDR event structure (from JSON log files)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedEdrEvent {
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub trace_id: Option<u64>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub func: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub tid: Option<u32>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub event_id: Option<u32>,
    #[serde(default)]
    pub callstack: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// File reader state (tracks current position for tail -f behavior)
struct FileReaderState {
    path: PathBuf,
    position: u64,
}

/// RedEDR file watcher collector
pub struct RedEdrFileCollector {
    data_path: String,
    job_id: String,
    file_states: Arc<Mutex<HashMap<PathBuf, FileReaderState>>>,
}

impl RedEdrFileCollector {
    pub fn new(data_path: String, job_id: String) -> Self {
        Self {
            data_path,
            job_id,
            file_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Start file watcher and stream events to channel
    pub async fn start(
        self,
        tx: Sender<crate::edr::common::TelemetryData>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!(
            "Starting RedEDR file watcher: {} (zero polling overhead)",
            self.data_path
        );

        let data_path = Path::new(&self.data_path);
        if !data_path.exists() {
            error!("RedEDR data path does not exist: {}", self.data_path);
            return Err(format!("Data path not found: {}", self.data_path).into());
        }

        // Create channel for file system events
        let (watch_tx, mut watch_rx) = tokio::sync::mpsc::channel::<notify::Result<Event>>(100);

        // Spawn blocking watcher thread (notify uses synchronous API)
        let data_path_clone = self.data_path.clone();
        std::thread::spawn(move || {
            let mut watcher: RecommendedWatcher =
                Watcher::new(move |res| {
                    watch_tx.blocking_send(res).ok();
                }, notify::Config::default())
                .expect("Failed to create file watcher");

            watcher
                .watch(Path::new(&data_path_clone), RecursiveMode::Recursive)
                .expect("Failed to watch directory");

            info!("File watcher started on {}", data_path_clone);

            // Keep watcher alive
            loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
            }
        });

        // Read existing files on startup
        self.scan_existing_files(&tx).await?;

        // Process file system events
        while let Some(event_result) = watch_rx.recv().await {
            match event_result {
                Ok(event) => {
                    self.handle_fs_event(event, &tx).await;
                }
                Err(e) => {
                    warn!("File watcher error: {}", e);
                }
            }
        }

        Ok(())
    }

    /// Scan existing files on startup and read from end
    async fn scan_existing_files(
        &self,
        tx: &Sender<crate::edr::common::TelemetryData>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        info!("Scanning existing RedEDR log files in {}", self.data_path);

        let entries = std::fs::read_dir(&self.data_path)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && self.is_log_file(&path) {
                // Initialize file state at end of file (only read new events going forward)
                if let Ok(metadata) = std::fs::metadata(&path) {
                    let mut states = self.file_states.lock().await;
                    states.insert(
                        path.clone(),
                        FileReaderState {
                            path: path.clone(),
                            position: metadata.len(),
                        },
                    );
                    debug!("Tracking log file: {:?} (starting at EOF)", path);
                }
            }
        }

        Ok(())
    }

    /// Handle file system event (file created, modified, deleted)
    async fn handle_fs_event(&self, event: Event, tx: &Sender<crate::edr::common::TelemetryData>) {
        match event.kind {
            EventKind::Create(_) | EventKind::Modify(_) => {
                for path in &event.paths {
                    if self.is_log_file(path) {
                        if let Err(e) = self.read_new_lines(path, tx).await {
                            error!("Failed to read log file {:?}: {}", path, e);
                        }
                    }
                }
            }
            EventKind::Remove(_) => {
                // File deleted, remove from tracking
                let mut states = self.file_states.lock().await;
                for path in &event.paths {
                    states.remove(path);
                    debug!("Stopped tracking removed file: {:?}", path);
                }
            }
            _ => {}
        }
    }

    /// Check if file is a RedEDR log file
    fn is_log_file(&self, path: &Path) -> bool {
        if let Some(ext) = path.extension() {
            // RedEDR typically logs to .json or .log files
            ext == "json" || ext == "log" || ext == "jsonl"
        } else {
            false
        }
    }

    /// Read new lines from file (tail -f style)
    async fn read_new_lines(
        &self,
        path: &Path,
        tx: &Sender<crate::edr::common::TelemetryData>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut states = self.file_states.lock().await;

        // Get or create file state
        let state = states.entry(path.to_path_buf()).or_insert_with(|| {
            debug!("New log file detected: {:?}", path);
            FileReaderState {
                path: path.to_path_buf(),
                position: 0,
            }
        });

        // Open file and seek to last position
        let mut file = File::open(path)?;
        file.seek(SeekFrom::Start(state.position))?;

        let reader = BufReader::new(file);
        let mut lines_read = 0;

        for line in reader.lines() {
            let line = line?;
            state.position += line.len() as u64 + 1; // +1 for newline

            // Skip empty lines
            if line.trim().is_empty() {
                continue;
            }

            // Parse JSON event
            match serde_json::from_str::<RedEdrEvent>(&line) {
                Ok(event) => {
                    let telemetry = self.transform_event(&event);

                    // Send to channel (async, with backpressure)
                    if let Err(e) = tx.send(telemetry).await {
                        error!("Failed to send telemetry event: {}", e);
                        return Err(e.into());
                    }

                    lines_read += 1;
                }
                Err(e) => {
                    warn!("Failed to parse RedEDR event from {:?}: {}", path, e);
                    debug!("Invalid JSON: {}", line);
                }
            }
        }

        if lines_read > 0 {
            debug!("Read {} new events from {:?}", lines_read, path);
        }

        Ok(())
    }

    /// Transform RedEDR event to protobuf TelemetryData
    fn transform_event(&self, event: &RedEdrEvent) -> crate::edr::common::TelemetryData {
        let payload = serde_json::to_vec(event).unwrap_or_default();

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("source".to_string(), "rededr".to_string());

        if let Some(ref event_type) = event.r#type {
            metadata.insert("event_type".to_string(), event_type.clone());
        }
        if let Some(pid) = event.pid {
            metadata.insert("pid".to_string(), pid.to_string());
        }
        if let Some(tid) = event.tid {
            metadata.insert("tid".to_string(), tid.to_string());
        }
        if let Some(ref provider) = event.provider {
            metadata.insert("provider".to_string(), provider.clone());
        }
        if let Some(trace_id) = event.trace_id {
            metadata.insert("trace_id".to_string(), trace_id.to_string());
        }

        crate::edr::common::TelemetryData {
            job_id: self.job_id.clone(),
            event_type: event.r#type.clone().unwrap_or_else(|| "unknown".to_string()),
            timestamp: chrono::Utc::now().timestamp_millis(),
            payload,
            metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_log_file() {
        let collector = RedEdrFileCollector::new("C:\\RedEdr\\Data".to_string(), "job-001".to_string());

        assert!(collector.is_log_file(Path::new("events.json")));
        assert!(collector.is_log_file(Path::new("events.log")));
        assert!(collector.is_log_file(Path::new("events.jsonl")));
        assert!(!collector.is_log_file(Path::new("events.txt")));
        assert!(!collector.is_log_file(Path::new("events")));
    }
}
