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
            vec![]
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
            vec![]
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
            vec![]
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
            vec![]
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
            vec![]
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
            vec![]
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
            vec![]
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

#[test]
fn test_empty_queue() {
    let queue = JobQueue::new();

    assert_eq!(queue.total_count(), 0);
    assert_eq!(queue.queued_count(), 0);
    assert_eq!(queue.running_count(), 0);
    assert_eq!(queue.completed_count(), 0);
    assert!(queue.pop_next().is_none());
}

#[test]
fn test_get_nonexistent_job() {
    let queue = JobQueue::new();

    let result = queue.get_job("job-999999");
    assert!(result.is_none());
}

#[test]
fn test_update_nonexistent_job() {
    let queue = JobQueue::new();

    let fake_job = Job::new(
        "job-999999".to_string(),
        "test".to_string(),
        "test.c".to_string(),
        vec![],
        "api+bb".to_string(),
        0,
        10,
    );

    let result = queue.update_job(&fake_job);
    assert!(result.is_err());
}

#[test]
fn test_list_jobs_no_filter() {
    let queue = JobQueue::new();

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
            vec![]
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
            vec![]
        )
        .unwrap();

    let all_jobs = queue.list_jobs(None);
    assert_eq!(all_jobs.len(), 2);
}

#[test]
fn test_status_counts() {
    let queue = JobQueue::new();

    // Submit 3 jobs
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
            vec![]
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
            vec![]
        )
        .unwrap();
    let job_id3 = queue
        .submit_job(
            "test3".to_string(),
            "test3.c".to_string(),
            vec![],
            "api+bb".to_string(),
            0,
            10,
            false,
            false,
            vec![]
        )
        .unwrap();

    // All queued initially
    assert_eq!(queue.queued_count(), 3);
    assert_eq!(queue.running_count(), 0);
    assert_eq!(queue.completed_count(), 0);

    // Mark one running
    let mut job1 = queue.get_job(&job_id1).unwrap();
    job1.start_running();
    queue.update_job(&job1).unwrap();

    assert_eq!(queue.queued_count(), 2);
    assert_eq!(queue.running_count(), 1);
    assert_eq!(queue.completed_count(), 0);

    // Mark one completed
    let mut job2 = queue.get_job(&job_id2).unwrap();
    job2.mark_completed();
    queue.update_job(&job2).unwrap();

    assert_eq!(queue.queued_count(), 1);
    assert_eq!(queue.running_count(), 1);
    assert_eq!(queue.completed_count(), 1);

    // Mark one failed
    let mut job3 = queue.get_job(&job_id3).unwrap();
    job3.mark_failed("Test error".to_string());
    queue.update_job(&job3).unwrap();

    assert_eq!(queue.queued_count(), 0);
    assert_eq!(queue.running_count(), 1);
    assert_eq!(queue.completed_count(), 2); // completed + failed
}

#[test]
fn test_job_id_generation() {
    let queue = JobQueue::new();

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
            vec![]
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
            vec![]
        )
        .unwrap();

    assert_eq!(job_id1, "job-000001");
    assert_eq!(job_id2, "job-000002");
}

#[test]
fn test_stop_conditions_preserved() {
    let queue = JobQueue::new();

    let job_id = queue
        .submit_job(
            "test".to_string(),
            "test.c".to_string(),
            vec![],
            "api+bb".to_string(),
            0,
            10,
            true,  // stop_on_evasion
            false, // stop_on_detection
            vec![]
        )
        .unwrap();

    let job = queue.get_job(&job_id).unwrap();
    assert!(job.stop_on_evasion);
    assert!(!job.stop_on_detection);
}

#[test]
fn test_pop_next_skips_non_queued() {
    let queue = JobQueue::new();

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
            vec![],
        )
        .unwrap();

    let job_id2 = queue
        .submit_job("test2".to_string(), "test2.c".to_string(), vec![], "api+bb".to_string(), 0, 10, false, false, vec![])
        .unwrap();

    // Mark first job as running
    let mut job1 = queue.get_job(&job_id1).unwrap();
    job1.start_running();
    queue.update_job(&job1).unwrap();

    // pop_next should return second job (first queued)
    let next = queue.pop_next().unwrap();
    assert_eq!(next.id, job_id2);
}