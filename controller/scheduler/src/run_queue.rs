// Run queue for asynchronous worker allocation
// Manages pending runs (baseline/instrumented executions) that are waiting for workers
//
// Architecture:
// - Jobs are high-level (multi-round tasks) tracked by JobQueue
// - Runs are low-level (single execution: baseline OR instrumented) tracked by RunQueue
// - Round processor submits runs to RunQueue, doesn't block on worker availability
// - Workers pull runs from RunQueue when they become available
//
// Benefits:
// - Jobs can execute independently as workers become available
// - No blocking on worker availability during round execution
// - Better worker utilization (workers pull work when ready)

use crate::job::MutationSpec;
use crate::round::RunType;  // Use existing RunType from round.rs
use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

/// Pending run waiting for worker execution
#[derive(Debug)]
pub struct PendingRun {
    /// Unique run ID (UUID)
    pub run_id: String,

    /// Parent job ID
    pub job_id: String,

    /// Round ID within job
    pub round_id: String,

    /// Run type (baseline or instrumented)
    pub run_type: RunType,

    /// Template name for build
    pub template_name: String,

    /// Source file for build
    pub source_file: String,

    /// Mutations to apply
    pub mutations: Vec<MutationSpec>,

    /// Trace mode ("off", "lines", etc.)
    pub trace_mode: String,

    /// Required OS (e.g., "win10", "win11")
    /// Worker MUST match this OS
    pub required_os: String,

    /// Oneshot channel to send result back to submitter
    /// This allows async/await on run completion
    pub result_tx: Option<oneshot::Sender<RunResult>>,
}

/// Result of a completed run
#[derive(Debug, Clone)]
pub struct RunResult {
    pub run_id: String,
    pub success: bool,
    pub detected: bool,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
}

/// Queue managing pending runs with OS-aware worker matching
#[derive(Clone)]
pub struct RunQueue {
    state: Arc<Mutex<RunQueueState>>,
}

struct RunQueueState {
    /// Pending runs indexed by run_id
    pending: HashMap<String, PendingRun>,

    /// Pending runs grouped by OS for efficient worker matching
    /// Map<os, Vec<run_id>>
    by_os: HashMap<String, Vec<String>>,

    /// Counter for generating run IDs
    next_run_id: u64,
}

impl RunQueue {
    /// Create a new empty run queue
    pub fn new() -> Self {
        RunQueue {
            state: Arc::new(Mutex::new(RunQueueState {
                pending: HashMap::new(),
                by_os: HashMap::new(),
                next_run_id: 1,
            })),
        }
    }

    /// Submit a run to the queue
    /// Returns (run_id, result_receiver)
    /// Caller can await result_receiver to get RunResult when execution completes
    pub fn submit_run(
        &self,
        job_id: String,
        round_id: String,
        run_type: RunType,
        template_name: String,
        source_file: String,
        mutations: Vec<MutationSpec>,
        trace_mode: String,
        required_os: String,
    ) -> (String, oneshot::Receiver<RunResult>) {
        let mut state = self.state.lock().unwrap();

        // Generate run ID
        let run_id = format!("run-{:08}", state.next_run_id);
        state.next_run_id += 1;

        // Create oneshot channel for result
        let (tx, rx) = oneshot::channel();

        // Create pending run
        let pending_run = PendingRun {
            run_id: run_id.clone(),
            job_id,
            round_id,
            run_type,
            template_name,
            source_file,
            mutations,
            trace_mode,
            required_os: required_os.clone(),
            result_tx: Some(tx),
        };

        // Add to pending runs
        state.pending.insert(run_id.clone(), pending_run);

        // Add to OS index
        state
            .by_os
            .entry(required_os)
            .or_insert_with(Vec::new)
            .push(run_id.clone());

        (run_id, rx)
    }

    /// Get next pending run for a worker with specific OS
    /// Returns None if no matching runs available
    ///
    /// This is the "worker pull" model - workers call this when they become available
    pub fn pop_for_os(&self, worker_os: &str) -> Option<PendingRun> {
        let mut state = self.state.lock().unwrap();

        // Get run ID for this OS (FIFO)
        let run_id = {
            let os_runs = state.by_os.get_mut(worker_os)?;
            os_runs.pop()?
        };

        // Clean up empty OS bucket
        if let Some(os_runs) = state.by_os.get(worker_os) {
            if os_runs.is_empty() {
                state.by_os.remove(worker_os);
            }
        }

        // Remove from pending map
        state.pending.remove(&run_id)
    }

    /// Complete a run with result
    /// This sends the result back to the submitter via the oneshot channel
    pub fn complete_run(&self, run_id: &str, result: RunResult) -> Result<()> {
        let mut state = self.state.lock().unwrap();

        // Remove from pending (should already be removed by pop_for_os, but double-check)
        if let Some(mut pending) = state.pending.remove(run_id) {
            // Send result to submitter
            if let Some(tx) = pending.result_tx.take() {
                let _ = tx.send(result); // Ignore error if receiver dropped
            }
        }

        Ok(())
    }

    /// Get count of pending runs for specific OS
    pub fn pending_count_for_os(&self, os: &str) -> usize {
        let state = self.state.lock().unwrap();
        state.by_os.get(os).map(|v| v.len()).unwrap_or(0)
    }

    /// Get total pending run count
    pub fn pending_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.pending.len()
    }

    /// List all pending runs (for debugging/monitoring)
    pub fn list_pending(&self) -> Vec<(String, String, String)> {
        let state = self.state.lock().unwrap();
        state
            .pending
            .values()
            .map(|r| (r.run_id.clone(), r.job_id.clone(), r.required_os.clone()))
            .collect()
    }
}

impl Default for RunQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_submit_and_pop() {
        let queue = RunQueue::new();

        // Submit run for win10
        let (run_id, _rx) = queue.submit_run(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test".to_string(),
            "test.c".to_string(),
            vec![],
            "off".to_string(),
            "win10".to_string(),
        );

        assert_eq!(queue.pending_count(), 1);
        assert_eq!(queue.pending_count_for_os("win10"), 1);

        // Pop for win10 should return the run
        let pending = queue.pop_for_os("win10").unwrap();
        assert_eq!(pending.run_id, run_id);
        assert_eq!(pending.required_os, "win10");

        assert_eq!(queue.pending_count(), 0);
    }

    #[test]
    fn test_os_matching() {
        let queue = RunQueue::new();

        // Submit runs for different OSes
        queue.submit_run(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test".to_string(),
            "test.c".to_string(),
            vec![],
            "off".to_string(),
            "win10".to_string(),
        );

        queue.submit_run(
            "job-000002".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test".to_string(),
            "test.c".to_string(),
            vec![],
            "off".to_string(),
            "win11".to_string(),
        );

        assert_eq!(queue.pending_count(), 2);
        assert_eq!(queue.pending_count_for_os("win10"), 1);
        assert_eq!(queue.pending_count_for_os("win11"), 1);

        // Pop for win11 should only return win11 run
        let pending = queue.pop_for_os("win11").unwrap();
        assert_eq!(pending.required_os, "win11");

        assert_eq!(queue.pending_count(), 1);
        assert_eq!(queue.pending_count_for_os("win10"), 1);
        assert_eq!(queue.pending_count_for_os("win11"), 0);
    }

    #[test]
    fn test_fifo_per_os() {
        let queue = RunQueue::new();

        // Submit 3 runs for win10
        let (run_id1, _) = queue.submit_run(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test1".to_string(),
            "test1.c".to_string(),
            vec![],
            "off".to_string(),
            "win10".to_string(),
        );

        let (run_id2, _) = queue.submit_run(
            "job-000002".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test2".to_string(),
            "test2.c".to_string(),
            vec![],
            "off".to_string(),
            "win10".to_string(),
        );

        // Pop should return FIFO order
        let first = queue.pop_for_os("win10").unwrap();
        assert_eq!(first.run_id, run_id1);

        let second = queue.pop_for_os("win10").unwrap();
        assert_eq!(second.run_id, run_id2);
    }

    #[tokio::test]
    async fn test_async_result() {
        let queue = RunQueue::new();

        // Submit run
        let (run_id, rx) = queue.submit_run(
            "job-000001".to_string(),
            "round-1".to_string(),
            RunType::Baseline,
            "test".to_string(),
            "test.c".to_string(),
            vec![],
            "off".to_string(),
            "win10".to_string(),
        );

        // Pop run (simulating worker)
        let _pending = queue.pop_for_os("win10").unwrap();

        // Complete run with result
        let result = RunResult {
            run_id: run_id.clone(),
            success: true,
            detected: false,
            exit_code: Some(0),
            error: None,
        };

        queue.complete_run(&run_id, result.clone()).unwrap();

        // Receiver should get the result
        let received = rx.await.unwrap();
        assert_eq!(received.run_id, run_id);
        assert_eq!(received.success, true);
        assert_eq!(received.detected, false);
    }
}