// Job queue implementation for scheduler
// Phase 1: In-memory FIFO queue (simple Vec-based)
// Phase 2: Will upgrade to BinaryHeap for priority scheduling

use crate::job::{Job, JobStatus};
use anyhow::{Result, anyhow};
use std::sync::{Arc, Mutex};

/// Job queue that manages pending and active jobs
/// Phase 1: Simple FIFO queue using Vec
#[derive(Clone)]
pub struct JobQueue {
    /// Shared state containing all jobs
    state: Arc<Mutex<QueueState>>,
}

struct QueueState {
    /// All jobs (queued, running, completed)
    jobs: Vec<Job>,
    /// Next job ID counter
    next_id: u64,
}

impl JobQueue {
    /// Create a new empty job queue
    pub fn new() -> Self {
        JobQueue {
            state: Arc::new(Mutex::new(QueueState {
                jobs: Vec::new(),
                next_id: 1,
            })),
        }
    }

    /// Submit a new job to the queue
    /// Returns the job ID
    pub fn submit_job(
        &self,
        template_name: String,
        source_file: String,
        mutations: Vec<crate::job::MutationSpec>,
        trace_mode: String,
        priority: i32,
        max_rounds: u32,
        stop_on_evasion: bool,
        stop_on_detection: bool,
    ) -> Result<String> {
        let mut state = self.state.lock().unwrap();

        // Generate job ID
        let job_id = format!("job-{:06}", state.next_id);
        state.next_id += 1;

        // Create job
        let mut job = Job::new(
            job_id.clone(),
            template_name,
            source_file,
            mutations,
            trace_mode,
            priority,
            max_rounds,
        );

        // Set stopping conditions
        job.stop_on_evasion = stop_on_evasion;
        job.stop_on_detection = stop_on_detection;

        // Add to queue
        state.jobs.push(job);

        Ok(job_id)
    }

    /// Get next queued job (FIFO ordering in Phase 1)
    /// Returns None if no jobs are queued
    pub fn pop_next(&self) -> Option<Job> {
        let mut state = self.state.lock().unwrap();

        // Find first queued job (FIFO)
        // Phase 2: Will use BinaryHeap for priority ordering
        let position = state
            .jobs
            .iter()
            .position(|j| j.status == JobStatus::Queued)?;

        Some(state.jobs[position].clone())
    }

    /// Update job status in queue
    pub fn update_job(&self, job: &Job) -> Result<()> {
        let mut state = self.state.lock().unwrap();

        let position = state
            .jobs
            .iter()
            .position(|j| j.id == job.id)
            .ok_or_else(|| anyhow!("Job not found: {}", job.id))?;

        state.jobs[position] = job.clone();

        Ok(())
    }

    /// Get job by ID
    pub fn get_job(&self, job_id: &str) -> Option<Job> {
        let state = self.state.lock().unwrap();
        state.jobs.iter().find(|j| j.id == job_id).cloned()
    }

    /// List all jobs (optionally filter by status)
    pub fn list_jobs(&self, status_filter: Option<JobStatus>) -> Vec<Job> {
        let state = self.state.lock().unwrap();

        match status_filter {
            Some(status) => state
                .jobs
                .iter()
                .filter(|j| j.status == status)
                .cloned()
                .collect(),
            None => state.jobs.clone(),
        }
    }

    /// Get count of queued jobs
    pub fn queued_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Queued)
            .count()
    }

    /// Get count of running jobs
    pub fn running_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state
            .jobs
            .iter()
            .filter(|j| j.status == JobStatus::Running)
            .count()
    }

    /// Get count of completed jobs (completed + failed + timeout)
    pub fn completed_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.jobs.iter().filter(|j| j.is_terminal()).count()
    }

    /// Get total job count
    pub fn total_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.jobs.len()
    }
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_submit() {
        let queue = JobQueue::new();

        let job_id = queue
            .submit_job(
                "test".to_string(),
                "test.c".to_string(),
                vec![],
                "api+bb".to_string(),
                0,
                10,    // max_rounds
                false, // stop_on_evasion
                false, // stop_on_detection
            )
            .unwrap();

        assert_eq!(job_id, "job-000001");
        assert_eq!(queue.total_count(), 1);
        assert_eq!(queue.queued_count(), 1);
    }

    #[test]
    fn test_queue_pop_fifo() {
        let queue = JobQueue::new();

        // Submit 3 jobs
        queue
            .submit_job(
                "test1".to_string(),
                "test1.c".to_string(),
                vec![],
                "api+bb".to_string(),
                0,
                10,
                false,
                false,
            )
            .unwrap();
        queue
            .submit_job(
                "test2".to_string(),
                "test2.c".to_string(),
                vec![],
                "api+bb".to_string(),
                0,
                10,
                false,
                false,
            )
            .unwrap();
        queue
            .submit_job(
                "test3".to_string(),
                "test3.c".to_string(),
                vec![],
                "api+bb".to_string(),
                0,
                10,
                false,
                false,
            )
            .unwrap();

        // Pop should return first job (FIFO)
        let job = queue.pop_next().unwrap();
        assert_eq!(job.id, "job-000001");
        assert_eq!(job.template_name, "test1");
    }

    #[test]
    fn test_queue_update() {
        let queue = JobQueue::new();

        let job_id = queue
            .submit_job(
                "test".to_string(),
                "test.c".to_string(),
                vec![],
                "api+bb".to_string(),
                0,
                10,
                false,
                false,
            )
            .unwrap();

        let mut job = queue.get_job(&job_id).unwrap();
        job.start_running();

        queue.update_job(&job).unwrap();

        let updated_job = queue.get_job(&job_id).unwrap();
        assert_eq!(updated_job.status, JobStatus::Running);
    }

    #[test]
    fn test_queue_filter() {
        let queue = JobQueue::new();

        // Submit jobs
        let job_id1 = queue
            .submit_job(
                "test1".to_string(),
                "test1.c".to_string(),
                vec![],
                "api+bb".to_string(),
                0,
                10,
                false,
                false,
            )
            .unwrap();
        let job_id2 = queue
            .submit_job(
                "test2".to_string(),
                "test2.c".to_string(),
                vec![],
                "api+bb".to_string(),
                0,
                10,
                false,
                false,
            )
            .unwrap();

        // Mark one as running
        let mut job1 = queue.get_job(&job_id1).unwrap();
        job1.start_running();
        queue.update_job(&job1).unwrap();

        // Filter queued
        let queued = queue.list_jobs(Some(JobStatus::Queued));
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, job_id2);

        // Filter running
        let running = queue.list_jobs(Some(JobStatus::Running));
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].id, job_id1);
    }
}
