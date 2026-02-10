# Controller/Scheduler Architecture

## 1. Component Hierarchy & Ownership

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              MAIN.RS (Entry Point)                              │
│  Creates: Channels, RunPool (shared), TargetManager (shared), Orchestrator      │
│  Spawns: Orchestrator task, gRPC server                                         │
└────────────────────────────────────────────┬────────────────────────────────────┘
                                             │
                     ┌───────────────────────┼───────────────────────┐
                     │                       │                       │
                     ▼                       ▼                       ▼
          ┌─────────────────────┐ ┌─────────────────────┐ ┌─────────────────────┐
          │    TargetManager    │ │       RunPool       │ │    Orchestrator     │
          │    (Arc shared)     │ │    (Arc shared)     │ │   (event loop)      │
          │                     │ │                     │ │                     │
          │ Owns:               │ │ Owns:               │ │ Owns:               │
          │ • targets: DashMap  │ │ • pending: DashMap  │ │ • job_workers: Map  │
          │ • events_tx         │ │ • by_os: DashMap    │ │ • vms: Map          │
          │                     │ │ • result_routers    │ │ • channels (4)      │
          │ Spawns:             │ │ • job_registry      │ │                     │
          │ • VMExecutor        │ │ • shutdown_token    │ │ Spawns:             │
          │ • StreamHandler     │ │                     │ │ • JobWorker         │
          │ • Heartbeat         │ │ No spawns           │ │ • ES indexing tasks │
          └──────────┬──────────┘ └──────────┬──────────┘ └──────────┬──────────┘
                     │                       │                       │
                     │    ┌──────────────────┴──────────────────┐    │
                     │    │                                     │    │
                     ▼    ▼                                     ▼    ▼
          ┌─────────────────────┐                     ┌─────────────────────┐
          │     VMExecutor      │                     │      JobWorker      │
          │   (per VM task)     │                     │   (per Job task)    │
          │                     │                     │                     │
          │ Owns:               │                     │ Owns:               │
          │ • vm_info           │                     │ • job: JobSession   │
          │ • in_flight: Option │                     │ • round_aggs: Map   │
          │ • remote_tx/rx      │                     │ • result_rx         │
          │ • artifact_sender   │                     │ • event_tx          │
          │                     │                     │ • shutdown_token    │
          │ Uses:               │                     │                     │
          │ • run_pool.take_run │                     │ Uses:               │
          │ • run_pool.route    │                     │ • run_pool.add_runs │
          │ • targets.reserve   │                     │ • run_pool.register │
          └─────────────────────┘                     └─────────────────────┘
```

---

## 2. Channel Architecture

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              CHANNEL TOPOLOGY                                   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌─────────────┐                                                                │
│  │  gRPC API   │                                                                │
│  │  (Server)   │                                                                │
│  └──────┬──────┘                                                                │
│         │                                                                       │
│         │ ①job_tx (128)              ②job_control_tx (64)                       │
│         │ JobSession                  JobControlCommand::Stop                   │
│         ▼                             ▼                                         │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │                            Orchestrator                                  │   │
│  │                                                                          │   │
│  │  Receives on 4 channels (select! loop):                                  │   │
│  │    ①job_submit_rx ─────────── New job submissions                        │   │
│  │    ②job_control_rx ────────── Stop commands                              │   │
│  │    ③events_rx ─────────────── VM lifecycle + telemetry                   │   │
│  │    ④job_event_rx ──────────── JobWorker completions                      │   │
│  │                                                                          │   │
│  │  Sends:                                                                  │   │
│  │    ④job_event_tx ──────────── To spawned JobWorkers                      │   │
│  │                                                                          │   │
│  └──────────────────────────────────┬──────────────────────────────────────┘   │
│                                     │                                           │
│         ┌───────────────────────────┼───────────────────────────┐               │
│         │                           │                           │               │
│         │④job_event_tx              │③events_tx                │               │
│         ▼                           │                           ▼               │
│  ┌─────────────────┐                │                ┌─────────────────┐        │
│  │   JobWorker-1   │                │                │  TargetManager  │        │
│  │   JobWorker-2   │                │                │                 │        │
│  │   JobWorker-N   │                │                │  Sends:         │        │
│  │                 │                │                │  ③events_tx ────┼────────┤
│  │  Each receives: │                │                │    TargetEvent  │        │
│  │  ⑤result_rx ────┼────────────────┼────────────────┤                 │        │
│  │    JobRunResult │                │                └────────┬────────┘        │
│  │                 │                │                         │                 │
│  └────────┬────────┘                │                         │                 │
│           │                         │                         │                 │
│           │ Registers result_tx     │                         │ Spawns          │
│           │ with RunPool            │                         ▼                 │
│           ▼                         │          ┌──────────────────────────┐     │
│  ┌────────────────────────────────────────────►│       VMExecutor-1       │     │
│  │                RunPool            │          │       VMExecutor-2       │     │
│  │                                   │          │       VMExecutor-N       │     │
│  │  result_routers: {                │          │                          │     │
│  │    job-1 → Sender(JobWorker-1)    │          │ Each has:                │     │
│  │    job-2 → Sender(JobWorker-2)    │          │ ⑥remote_tx ──────────────┼──┐  │
│  │  }                                │          │   ControllerMessage      │  │  │
│  │                                   │          │ ⑦result_rx ◄─────────────┼──┤  │
│  │  runs_available: Notify           │          │   RemoteRunResult        │  │  │
│  │    (wakes VMExecutors)            │          │                          │  │  │
│  │                                   │          │ route_result() ──────────┼──┤  │
│  └───────────────────────────────────┘          └──────────────────────────┘  │  │
│                                                                                │  │
│                                                              ┌─────────────────┘  │
│                                                              │                    │
│                                                              ▼                    │
│                                              ┌──────────────────────────┐         │
│                                              │    StreamHandler         │         │
│                                              │    (per VM, spawned)     │         │
│                                              │                          │         │
│                                              │  incoming: WorkerMessage │         │
│                                              │  outgoing: ⑥remote_tx    │         │
│                                              │  sends: ⑦result_tx       │         │
│                                              │  sends: ③events_tx       │         │
│                                              └──────────────────────────┘         │
│                                                                                   │
└───────────────────────────────────────────────────────────────────────────────────┘
```

### Channel Summary Table

| # | Channel | Capacity | Type | From | To |
|---|---------|----------|------|------|-----|
| ① | job_tx/rx | 128 | `JobSession` | gRPC API | Orchestrator |
| ② | job_control_tx/rx | 64 | `JobControlCommand` | gRPC API | Orchestrator |
| ③ | events_tx/rx | 4096 | `TargetEvent` | TargetManager, StreamHandler | Orchestrator |
| ④ | job_event_tx/rx | 256 | `JobWorkerEvent` | JobWorkers | Orchestrator |
| ⑤ | result_tx/rx | 64/job | `JobRunResult` | RunPool.route | JobWorker |
| ⑥ | stream_tx (remote_tx) | 128 | `ControllerMessage` | VMExecutor, Heartbeat | StreamHandler→VM |
| ⑦ | result_tx (remote) | 128 | `RemoteRunResult` | StreamHandler | VMExecutor |

---

## 3. Data Flow: Job Lifecycle

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 1: JOB SUBMISSION                                                         │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  Client ─────► gRPC ScheduleJob ─────► schedule_job()                           │
│                                              │                                  │
│                                              ▼                                  │
│                                   ┌─────────────────────┐                       │
│                                   │ Create JobSession   │                       │
│                                   │ • id: UUID          │                       │
│                                   │ • build_spec        │                       │
│                                   │ • max_rounds        │                       │
│                                   │ • target_os         │                       │
│                                   │ • required_caps     │                       │
│                                   └──────────┬──────────┘                       │
│                                              │                                  │
│                                              │ job_tx.send(job)                 │
│                                              ▼                                  │
│                                   ┌─────────────────────┐                       │
│                                   │    Orchestrator     │                       │
│                                   │   job_submit_rx     │                       │
│                                   └──────────┬──────────┘                       │
│                                              │                                  │
│                                              │ resolve constraints              │
│                                              │ (find compatible VMs)            │
│                                              ▼                                  │
│                                   ┌─────────────────────┐                       │
│                                   │  spawn_job_worker() │                       │
│                                   │  • JobWorker::new() │                       │
│                                   │  • tokio::spawn()   │                       │
│                                   │  • store token      │                       │
│                                   └─────────────────────┘                       │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 2: ROUND PRODUCTION (JobWorker)                                           │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  JobWorker.run() loop (every 100ms):                                            │
│                                                                                 │
│  ┌──────────────────────────────────────────────────────────────┐               │
│  │ can_produce_round()?                                          │               │
│  │   • current_round < max_rounds                                │               │
│  │   • in_flight_rounds < MAX_IN_FLIGHT (5)                      │               │
│  │   • pending_runs < MAX_PENDING (10)                           │               │
│  └──────────────────────────────────────────────────────────────┘               │
│                          │                                                      │
│                          ▼ YES                                                  │
│  ┌──────────────────────────────────────────────────────────────┐               │
│  │ produce_round()                                               │               │
│  │                                                               │               │
│  │   1. job.start_round() → (round_number, round_id)             │               │
│  │                                                               │               │
│  │   2. Build artifacts:                                         │               │
│  │      ┌────────────────────────────────────────────────┐       │               │
│  │      │ BASELINE                   │ INSTRUMENTED      │       │               │
│  │      │ trace_mode = "off"         │ trace_mode = "lines"│     │               │
│  │      │ run_type = Baseline        │ run_type = Instrumented │ │               │
│  │      │ run_id = {round_id}-base   │ run_id = {round_id}-inst │ │              │
│  │      └────────────────────────────────────────────────┘       │               │
│  │                                                               │               │
│  │   3. Create RoundAgg (join state):                            │               │
│  │      • spec: RoundSpec                                        │               │
│  │      • baseline_run_id, instrumented_run_id                   │               │
│  │      • baseline: None, instrumented: None                     │               │
│  │                                                               │               │
│  │   4. run_pool.add_runs(vec![baseline_env, instrumented_env])  │               │
│  │      • Stored in pending: DashMap<RunId, RunEnvelope>         │               │
│  │      • Sharded into by_os[required_os].queue                  │               │
│  │      • runs_available.notify_waiters()                        │               │
│  └──────────────────────────────────────────────────────────────┘               │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 3: RUN EXECUTION (VMExecutor)                                             │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  VMExecutor.run() main loop:                                                    │
│                                                                                 │
│  ┌──────────────────────────────────────────────────────────────┐               │
│  │  select! {                                                    │               │
│  │      _ = shutdown.cancelled() => break,                       │               │
│  │                                                               │               │
│  │      Some(result) = result_rx.recv() => {                     │               │
│  │          // Result from remote VM                             │               │
│  │          on_result_received(result)                           │               │
│  │      }                                                        │               │
│  │                                                               │               │
│  │      _ = run_pool.wait_for_runs() => {                        │               │
│  │          // Work available                                    │               │
│  │          try_take_and_dispatch()                              │               │
│  │      }                                                        │               │
│  │  }                                                            │               │
│  └──────────────────────────────────────────────────────────────┘               │
│                                                                                 │
│  try_take_and_dispatch():                                                       │
│                                                                                 │
│  ┌──────────────────────────────────────────────────────────────┐               │
│  │  if let Some(envelope) = run_pool.take_run(vm_os, vm_caps)    │               │
│  │      │                                                        │               │
│  │      │  RunPool internals:                                    │               │
│  │      │  • Lock only by_os[vm_os] queue (not entire pool)      │               │
│  │      │  • Pop run_id from queue                               │               │
│  │      │  • Check capabilities match                            │               │
│  │      │  • Remove from pending map                             │               │
│  │      │  • Return RunEnvelope                                  │               │
│  │      ▼                                                        │               │
│  │  dispatch(envelope):                                          │               │
│  │      1. targets.reserve(vm_id) → mark Busy                    │               │
│  │      2. Upload artifact via gRPC chunks                       │               │
│  │      3. Build ControllerMessage::RunSampleCommand             │               │
│  │      4. in_flight = Some(InFlightRun { run_id, job_id, ... }) │               │
│  │      5. remote_tx.send(command) → to StreamHandler → VM       │               │
│  └──────────────────────────────────────────────────────────────┘               │
│                                                                                 │
│  on_result_received(RemoteRunResult):                                           │
│                                                                                 │
│  ┌──────────────────────────────────────────────────────────────┐               │
│  │  1. Verify run_id matches in_flight                           │               │
│  │  2. targets.release(vm_id) → mark Available                   │               │
│  │  3. Clear in_flight                                           │               │
│  │  4. Build JobRunResult:                                       │               │
│  │     • run_id, job_id, round_id                                │               │
│  │     • RunOutcome { detected, exit_code, error }               │               │
│  │     • vm_id                                                   │               │
│  │  5. run_pool.route_result(job_result)                         │               │
│  │     → Looks up result_routers[job_id]                         │               │
│  │     → Sends to JobWorker's result_rx                          │               │
│  └──────────────────────────────────────────────────────────────┘               │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 4: RESULT AGGREGATION (JobWorker)                                         │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  JobWorker.run() receives on result_rx:                                         │
│                                                                                 │
│  ┌──────────────────────────────────────────────────────────────┐               │
│  │  on_result(JobRunResult):                                     │               │
│  │                                                               │               │
│  │  1. Find RoundAgg for this run:                               │               │
│  │     let agg = round_aggs.get_mut(&result.round_id)            │               │
│  │                                                               │               │
│  │  2. Update join state:                                        │               │
│  │     if result.run_id == agg.baseline_run_id {                 │               │
│  │         agg.baseline = Some(result.outcome)                   │               │
│  │     } else {                                                  │               │
│  │         agg.instrumented = Some(result.outcome)               │               │
│  │     }                                                         │               │
│  │                                                               │               │
│  │  3. Check if round complete:                                  │               │
│  │     if agg.is_complete() {                                    │               │
│  │         finalize_round(round_id)                              │               │
│  │     }                                                         │               │
│  └──────────────────────────────────────────────────────────────┘               │
│                                                                                 │
│  finalize_round(round_id):                                                      │
│                                                                                 │
│  ┌──────────────────────────────────────────────────────────────┐               │
│  │  1. Compute RoundSummary from RoundAgg:                       │               │
│  │     • detected = baseline.detected || instrumented.detected   │               │
│  │     • behavior_match = baseline.exit == instrumented.exit     │               │
│  │     • evasion_score = if !detected { 1.0 } else { 0.0 }       │               │
│  │                                                               │               │
│  │  2. Record in job:                                            │               │
│  │     job.rounds[round_number] = summary                        │               │
│  │     job.last_round = Some(summary)                            │               │
│  │                                                               │               │
│  │  3. Update RunPool:                                           │               │
│  │     run_pool.update_job_progress(&job)                        │               │
│  │                                                               │               │
│  │  4. Emit event:                                               │               │
│  │     event_tx.send(JobWorkerEvent::RoundCompleted { ... })     │               │
│  │     → Orchestrator receives, indexes to ES                    │               │
│  │                                                               │               │
│  │  5. Remove from round_aggs                                    │               │
│  └──────────────────────────────────────────────────────────────┘               │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 5: JOB COMPLETION                                                         │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  JobWorker.run() checks after each round:                                       │
│                                                                                 │
│  ┌──────────────────────────────────────────────────────────────┐               │
│  │  is_job_complete()?                                           │               │
│  │    • !job.should_continue()  (no more rounds)                 │               │
│  │    • round_aggs.is_empty()   (all results received)           │               │
│  │                                                               │               │
│  │  If complete:                                                 │               │
│  │                                                               │               │
│  │  1. Determine outcome:                                        │               │
│  │     • Completed { rounds_completed }                          │               │
│  │     • Stopped { reason }                                      │               │
│  │     • Failed { error }                                        │               │
│  │                                                               │               │
│  │  2. Cleanup:                                                  │               │
│  │     run_pool.complete_job(&job_id, &outcome)                  │               │
│  │     run_pool.unregister_job(&job_id)                          │               │
│  │       → Removes from result_routers                           │               │
│  │       → Removes pending runs for this job                     │               │
│  │       → Updates job_registry status                           │               │
│  │                                                               │               │
│  │  3. Emit completion:                                          │               │
│  │     event_tx.send(JobWorkerEvent::JobCompleted { ... })       │               │
│  │       → Orchestrator removes from job_workers map             │               │
│  │       → Indexes final status to ES                            │               │
│  │                                                               │               │
│  │  4. Task exits                                                │               │
│  └──────────────────────────────────────────────────────────────┘               │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 4. State Management

### RunPool State (Shared)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                              RunPool                                            │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ pending: DashMap<RunId, RunEnvelope>                                    │   │
│  │   • Lock-free concurrent access                                         │   │
│  │   • Primary storage for all pending runs                                │   │
│  │                                                                         │   │
│  │   RunEnvelope {                                                         │   │
│  │     run_id, job_id, round_id, round_number,                             │   │
│  │     run_type: Baseline | Instrumented,                                  │   │
│  │     artifact: { path, sha256 },                                         │   │
│  │     mutations: Vec<String>,                                             │   │
│  │     timeout_seconds,                                                    │   │
│  │     required_os, required_capabilities                                  │   │
│  │   }                                                                     │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ by_os: DashMap<String, Mutex<VecDeque<RunId>>>                          │   │
│  │                                                                         │   │
│  │   Sharding: Each OS has its own queue                                   │   │
│  │                                                                         │   │
│  │   "windows" ──► Mutex ──► [run-1, run-4, run-7, ...]                    │   │
│  │   "linux"   ──► Mutex ──► [run-2, run-5, run-8, ...]                    │   │
│  │   "macos"   ──► Mutex ──► [run-3, run-6, run-9, ...]                    │   │
│  │                                                                         │   │
│  │   Benefits:                                                             │   │
│  │   • VM1 taking from "windows" only locks windows queue                  │   │
│  │   • VM2 can take from "linux" concurrently                              │   │
│  │   • No global lock contention                                           │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ result_routers: RwLock<HashMap<JobId, Sender<JobRunResult>>>            │   │
│  │                                                                         │   │
│  │   job-1 ──► Sender (JobWorker-1's receiver)                             │   │
│  │   job-2 ──► Sender (JobWorker-2's receiver)                             │   │
│  │   job-3 ──► Sender (JobWorker-3's receiver)                             │   │
│  │                                                                         │   │
│  │   • Registered on job start                                             │   │
│  │   • Removed on job completion                                           │   │
│  │   • route_result() uses this to find destination                        │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ job_registry: DashMap<JobId, JobInfo>                                   │   │
│  │                                                                         │   │
│  │   • Persists even after job completes (for API queries)                 │   │
│  │   • Updated on: register, progress update, completion                   │   │
│  │                                                                         │   │
│  │   JobInfo {                                                             │   │
│  │     id, status: Running | Completed | Stopped | Failed,                 │   │
│  │     current_round, max_rounds, target_os, started_at                    │   │
│  │   }                                                                     │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ runs_available: Notify                                                  │   │
│  │                                                                         │   │
│  │   • Called by: add_runs() → notify_waiters()                            │   │
│  │   • Awaited by: VMExecutor.wait_for_runs() → notified()                 │   │
│  │   • Wakes ALL waiting VMExecutors (they compete for work)               │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ shutdown_token: CancellationToken                                       │   │
│  │                                                                         │   │
│  │   • Broadcast shutdown to all VMExecutors                               │   │
│  │   • Each VMExecutor checks: shutdown_token.is_cancelled()               │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### TargetManager State (Shared)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                            TargetManager                                        │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ targets: DashMap<TargetId, Target>                                      │   │
│  │                                                                         │   │
│  │   Target {                                                              │   │
│  │     id: TargetId,                                                       │   │
│  │     address: String,           // e.g., "10.200.200.10:50052"           │   │
│  │     os_version: String,        // e.g., "windows"                       │   │
│  │     capabilities: Vec<String>, // e.g., ["defender", "rededr"]          │   │
│  │                                                                         │   │
│  │     status: TargetStatus,      // Available | Busy | Offline            │   │
│  │     channel: Option<Channel>,  // Cached gRPC channel                   │   │
│  │     stream_tx: Option<Sender>, // To send commands to VM                │   │
│  │                                                                         │   │
│  │     current_job: Option<JobId>,// Which job currently assigned          │   │
│  │     last_seen: SystemTime,     // For health tracking                   │   │
│  │   }                                                                     │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  Status Transitions:                                                            │
│                                                                                 │
│    Offline ──► Available ──► Busy ──► Available                                 │
│       ▲           │           │          │                                      │
│       │           │           │          │                                      │
│       └───────────┴───────────┴──────────┘                                      │
│              (disconnect or error)                                              │
│                                                                                 │
│  Key Operations:                                                                │
│    • reserve(vm_id) → mark Busy, set current_job                               │
│    • release(vm_id) → mark Available, clear current_job                        │
│    • get_available() → list Available targets                                  │
│    • get_available_by_os_and_capabilities(os, caps) → filtered list            │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

### JobWorker State (Per-Job)

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                         JobWorker (per job instance)                            │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ job: JobSession                                                         │   │
│  │                                                                         │   │
│  │   JobSession {                                                          │   │
│  │     id: JobId,                                                          │   │
│  │     target_os: Option<String>,                                          │   │
│  │     required_capabilities: Vec<String>,                                 │   │
│  │     build_spec: ModularBuildSpec,                                       │   │
│  │                                                                         │   │
│  │     current_round: u32,          // Progress counter                    │   │
│  │     max_rounds: u32,             // Max iterations                      │   │
│  │     stop_on_evasion: bool,       // Early stop on success               │   │
│  │                                                                         │   │
│  │     rounds: BTreeMap<u32, RoundSummary>,  // Completed rounds           │   │
│  │     last_round: Option<RoundSummary>,     // Most recent                │   │
│  │                                                                         │   │
│  │     created_at: SystemTime,                                             │   │
│  │     started_at: Option<SystemTime>,                                     │   │
│  │   }                                                                     │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ round_aggs: HashMap<RoundId, RoundAgg>                                  │   │
│  │                                                                         │   │
│  │   Max: MAX_IN_FLIGHT_ROUNDS (5)                                         │   │
│  │                                                                         │   │
│  │   RoundAgg {                                                            │   │
│  │     spec: RoundSpec { id, job_id, round_number, mutations },            │   │
│  │     baseline_run_id: RunId,                                             │   │
│  │     instrumented_run_id: RunId,                                         │   │
│  │     baseline: Option<RunOutcome>,      // Set when baseline completes   │   │
│  │     instrumented: Option<RunOutcome>,  // Set when instrumented completes│   │
│  │   }                                                                     │   │
│  │                                                                         │   │
│  │   is_complete() = baseline.is_some() && instrumented.is_some()          │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  Channels:                                                                      │
│    result_rx ← Receives JobRunResult from RunPool                              │
│    event_tx  → Sends JobWorkerEvent to Orchestrator                            │
│                                                                                 │
│  Shutdown:                                                                      │
│    shutdown_token: CancellationToken (stored in Orchestrator.job_workers)      │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 5. VM Execution Flow

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                            VM Connection Lifecycle                              │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  VM Agent connects via gRPC bidirectional stream                                │
│                              │                                                  │
│                              ▼                                                  │
│  ┌────────────────────────────────────────────────────────────────────────┐    │
│  │ TargetManager.establish_stream(target_id)                              │    │
│  │                                                                        │    │
│  │   1. Create channels:                                                  │    │
│  │      • stream_tx (128) → commands to VM                                │    │
│  │      • result_tx (128) → results from VM                               │    │
│  │                                                                        │    │
│  │   2. Store stream_tx in Target                                         │    │
│  │                                                                        │    │
│  │   3. Spawn StreamHandler task:                                         │    │
│  │      • Receives incoming WorkerMessages                                │    │
│  │      • Routes SampleResponse → result_tx                               │    │
│  │      • Routes Telemetry → events_tx → Orchestrator                     │    │
│  │      • Routes Status → events_tx → Orchestrator                        │    │
│  │                                                                        │    │
│  │   4. Spawn VMExecutor task:                                            │    │
│  │      • remote_tx = stream_tx.clone()                                   │    │
│  │      • result_rx = result_rx (receives from StreamHandler)             │    │
│  │      • Enters main run loop                                            │    │
│  │                                                                        │    │
│  │   5. Spawn Heartbeat task:                                             │    │
│  │      • Every 30 seconds, send Heartbeat via stream_tx                  │    │
│  │      • Keeps gRPC stream alive                                         │    │
│  └────────────────────────────────────────────────────────────────────────┘    │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────┐
│                          VMExecutor Main Loop                                   │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│                     ┌───────────────────┐                                       │
│                     │    VMExecutor     │                                       │
│                     │                   │                                       │
│                     │  State:           │                                       │
│                     │  • vm_id          │                                       │
│                     │  • vm_os          │                                       │
│                     │  • vm_caps        │                                       │
│                     │  • in_flight: Opt │                                       │
│                     │                   │                                       │
│                     └─────────┬─────────┘                                       │
│                               │                                                 │
│         ┌─────────────────────┼─────────────────────┐                           │
│         │                     │                     │                           │
│         ▼                     ▼                     ▼                           │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────┐                       │
│  │   Shutdown   │    │   Result     │    │    Idle      │                       │
│  │   signal?    │    │   arrived?   │    │   (no work)  │                       │
│  └──────┬───────┘    └──────┬───────┘    └──────┬───────┘                       │
│         │                   │                   │                               │
│         ▼                   ▼                   ▼                               │
│      BREAK             on_result_         wait_for_runs()                       │
│      (exit)            received()         then try_take                         │
│                             │                   │                               │
│                             │                   ▼                               │
│                             │         ┌──────────────────┐                      │
│                             │         │ take_run(os,caps)│                      │
│                             │         │                  │                      │
│                             │         │ if Some(env):    │                      │
│                             │         │   dispatch(env)  │                      │
│                             │         │                  │                      │
│                             │         │ else:            │                      │
│                             │         │   (wait again)   │                      │
│                             │         └──────────────────┘                      │
│                             │                                                   │
│                             ▼                                                   │
│                   ┌──────────────────────────────────────────────┐              │
│                   │ on_result_received(result):                  │              │
│                   │                                              │              │
│                   │   1. Verify in_flight matches                │              │
│                   │   2. targets.release(vm_id)                  │              │
│                   │   3. in_flight = None                        │              │
│                   │   4. Build JobRunResult                      │              │
│                   │   5. run_pool.route_result(result)           │              │
│                   │      → Delivered to JobWorker                │              │
│                   │   6. Try to grab more work immediately       │              │
│                   └──────────────────────────────────────────────┘              │
│                                                                                 │
│                   ┌──────────────────────────────────────────────┐              │
│                   │ dispatch(envelope):                          │              │
│                   │                                              │              │
│                   │   1. targets.reserve(vm_id)                  │              │
│                   │   2. Upload artifact chunks via gRPC         │              │
│                   │   3. Build RunSampleCommand:                 │              │
│                   │      { run_id, artifact_name, timeout, ... } │              │
│                   │   4. in_flight = Some(InFlightRun)           │              │
│                   │   5. remote_tx.send(command)                 │              │
│                   │      → StreamHandler → VM Agent              │              │
│                   └──────────────────────────────────────────────┘              │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 6. Telemetry Flow

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                            Telemetry Data Flow                                  │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │                         Worker VM (Remote)                              │   │
│  │                                                                         │   │
│  │   Artifact executes → ETW events, traces, coverage                      │   │
│  │          │                                                              │   │
│  │          ▼                                                              │   │
│  │   Agent collects → TelemetryData { job_id, event_type, payload, ... }   │   │
│  │          │                                                              │   │
│  │          ▼                                                              │   │
│  │   Batched into WorkerMessage::Telemetry { events: Vec }                 │   │
│  │          │                                                              │   │
│  │          │ (gRPC bidirectional stream)                                  │   │
│  └──────────┼──────────────────────────────────────────────────────────────┘   │
│             │                                                                   │
│             ▼                                                                   │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ StreamHandler (spawned per VM)                                          │   │
│  │                                                                         │   │
│  │   Receives WorkerMessage::Telemetry                                     │   │
│  │          │                                                              │   │
│  │          ▼                                                              │   │
│  │   events_tx.send(TargetEvent::Message { telemetry_batch })              │   │
│  └──────────┼──────────────────────────────────────────────────────────────┘   │
│             │                                                                   │
│             ▼                                                                   │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ Orchestrator.on_target_event()                                          │   │
│  │                                                                         │   │
│  │   TargetEvent::Message { Telemetry(batch) }                             │   │
│  │          │                                                              │   │
│  │          ▼                                                              │   │
│  │   Spawns async task for each batch:                                     │   │
│  │   tokio::spawn(async { index_telemetry(&es, &events).await })           │   │
│  │                                                                         │   │
│  │   Fire-and-forget (non-blocking)                                        │   │
│  └──────────┼──────────────────────────────────────────────────────────────┘   │
│             │                                                                   │
│             ▼                                                                   │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ Elasticsearch                                                           │   │
│  │                                                                         │   │
│  │   Index: telemetry-YYYY.MM.DD                                           │   │
│  │                                                                         │   │
│  │   Document:                                                             │   │
│  │   {                                                                     │   │
│  │     "job_id": "...",                                                    │   │
│  │     "event_type": "trace",                                              │   │
│  │     "timestamp": "...",                                                 │   │
│  │     "indexed_at": "...",                                                │   │
│  │     "payload_seq": 42,                                                  │   │
│  │     "payload_file": "loader.c",                                         │   │
│  │     "payload_line": 156,                                                │   │
│  │     "payload_func": "main",                                             │   │
│  │     ...                                                                 │   │
│  │   }                                                                     │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                                                                                 │
│  MISSING (per ES schema TODOs):                                                 │
│    • round_id  ← Need active_runs[worker_id] → run_id → RunEnvelope            │
│    • run_id    ← Need active_runs[worker_id]                                   │
│    • vm_id     ← Known from StreamHandler context                              │
│    • source    ← Constant "worker"                                             │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 7. Concurrency Model

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                          Task Ownership & Concurrency                           │
├─────────────────────────────────────────────────────────────────────────────────┤
│                                                                                 │
│  SINGLE-THREADED COORDINATION (Orchestrator select! loop):                      │
│                                                                                 │
│    ┌────────────────────────────────────────────────────────────────────────┐  │
│    │ Orchestrator.run()                                                     │  │
│    │                                                                        │  │
│    │   loop {                                                               │  │
│    │     select! {                                                          │  │
│    │       job = job_submit_rx.recv() => spawn_job_worker(job),             │  │
│    │       cmd = job_control_rx.recv() => handle_control(cmd),              │  │
│    │       event = events_rx.recv() => handle_target_event(event),          │  │
│    │       event = job_event_rx.recv() => handle_job_event(event),          │  │
│    │     }                                                                  │  │
│    │   }                                                                    │  │
│    │                                                                        │  │
│    │   Benefits:                                                            │  │
│    │   • No mutex needed for job_workers, vms maps                          │  │
│    │   • All state transitions serialized                                   │  │
│    │   • Easy to reason about                                               │  │
│    └────────────────────────────────────────────────────────────────────────┘  │
│                                                                                 │
│  MULTI-THREADED EXECUTION (via tokio::spawn):                                   │
│                                                                                 │
│    ┌────────────────────────────────────────────────────────────────────────┐  │
│    │ Spawned Tasks (run concurrently):                                      │  │
│    │                                                                        │  │
│    │   JobWorker-1 ──┐                                                      │  │
│    │   JobWorker-2 ──┼── Each has own result_rx, no shared mutable state    │  │
│    │   JobWorker-N ──┘                                                      │  │
│    │                                                                        │  │
│    │   VMExecutor-1 ──┐                                                     │  │
│    │   VMExecutor-2 ──┼── Compete for runs via RunPool (lock-free + sharded)│  │
│    │   VMExecutor-N ──┘                                                     │  │
│    │                                                                        │  │
│    │   StreamHandler-1 ──┐                                                  │  │
│    │   StreamHandler-2 ──┼── Independent, route to channels                 │  │
│    │   StreamHandler-N ──┘                                                  │  │
│    │                                                                        │  │
│    │   ES indexing tasks ── Fire-and-forget, no blocking                    │  │
│    └────────────────────────────────────────────────────────────────────────┘  │
│                                                                                 │
│  SYNCHRONIZATION PRIMITIVES:                                                    │
│                                                                                 │
│    ┌────────────────────────────────────────────────────────────────────────┐  │
│    │ DashMap (lock-free concurrent HashMap):                                │  │
│    │   • RunPool.pending                                                    │  │
│    │   • RunPool.by_os                                                      │  │
│    │   • RunPool.job_registry                                               │  │
│    │   • TargetManager.targets                                              │  │
│    │                                                                        │  │
│    │ RwLock (rarely write, often read):                                     │  │
│    │   • RunPool.result_routers                                             │  │
│    │                                                                        │  │
│    │ Mutex (short critical sections):                                       │  │
│    │   • RunPool.by_os[os].queue                                            │  │
│    │   • RunPool.metrics                                                    │  │
│    │                                                                        │  │
│    │ Notify (broadcast wake):                                               │  │
│    │   • RunPool.runs_available                                             │  │
│    │                                                                        │  │
│    │ CancellationToken (graceful shutdown):                                 │  │
│    │   • RunPool.shutdown_token (broadcast to all VMExecutors)              │  │
│    │   • JobWorker.shutdown_token (per-job, stored in Orchestrator)         │  │
│    └────────────────────────────────────────────────────────────────────────┘  │
│                                                                                 │
└─────────────────────────────────────────────────────────────────────────────────┘
```

---

## 8. Shared Pool: Run Distribution

```
┌─────────────────────────────────────────────────────────────────────────────────────────────┐
│                                    PRODUCERS (JobWorkers)                                   │
├─────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                             │
│  ┌─────────────────────┐    ┌─────────────────────┐    ┌─────────────────────┐              │
│  │     JobWorker-1     │    │     JobWorker-2     │    │     JobWorker-3     │              │
│  │     (Job: mut-01)   │    │     (Job: mut-02)   │    │     (Job: mut-03)   │              │
│  │                     │    │                     │    │                     │              │
│  │ Round 3 in progress │    │ Round 1 in progress │    │ Round 7 in progress │              │
│  │ target_os: windows  │    │ target_os: windows  │    │ target_os: linux    │              │
│  │ caps: [defender]    │    │ caps: [rededr]      │    │ caps: []            │              │
│  └──────────┬──────────┘    └──────────┬──────────┘    └──────────┬──────────┘              │
│             │                          │                          │                        │
│             │ add_runs([               │ add_runs([               │ add_runs([             │
│             │   baseline,              │   baseline,              │   baseline,            │
│             │   instrumented           │   instrumented           │   instrumented         │
│             │ ])                       │ ])                       │ ])                     │
│             │                          │                          │                        │
│             ▼                          ▼                          ▼                        │
│  ┌──────────────────────────────────────────────────────────────────────────────────────┐  │
│  │                                                                                      │  │
│  │                              RUN POOL (SharedPool)                                   │  │
│  │                                                                                      │  │
│  │  ┌────────────────────────────────────────────────────────────────────────────────┐ │  │
│  │  │                        pending: DashMap<RunId, RunEnvelope>                    │ │  │
│  │  │                                                                                │ │  │
│  │  │  ┌─────────────────┐ ┌─────────────────┐ ┌─────────────────┐ ┌──────────────┐  │ │  │
│  │  │  │ run-01-base     │ │ run-01-inst     │ │ run-02-base     │ │ run-02-inst  │  │ │  │
│  │  │  │ job: mut-01     │ │ job: mut-01     │ │ job: mut-02     │ │ job: mut-02  │  │ │  │
│  │  │  │ os: windows     │ │ os: windows     │ │ os: windows     │ │ os: windows  │  │ │  │
│  │  │  │ caps: [defender]│ │ caps: [defender]│ │ caps: [rededr]  │ │ caps: [rededr│  │ │  │
│  │  │  └─────────────────┘ └─────────────────┘ └─────────────────┘ └──────────────┘  │ │  │
│  │  │                                                                                │ │  │
│  │  │  ┌─────────────────┐ ┌─────────────────┐                                       │ │  │
│  │  │  │ run-03-base     │ │ run-03-inst     │  ... more runs ...                    │ │  │
│  │  │  │ job: mut-03     │ │ job: mut-03     │                                       │ │  │
│  │  │  │ os: linux       │ │ os: linux       │                                       │ │  │
│  │  │  │ caps: []        │ │ caps: []        │                                       │ │  │
│  │  │  └─────────────────┘ └─────────────────┘                                       │ │  │
│  │  └────────────────────────────────────────────────────────────────────────────────┘ │  │
│  │                                                                                      │  │
│  │  ┌────────────────────────────────────────────────────────────────────────────────┐ │  │
│  │  │                   by_os: DashMap<OS, Mutex<VecDeque<RunId>>>                   │ │  │
│  │  │                                                                                │ │  │
│  │  │   "windows" ─────► Mutex ─────► Queue                                          │ │  │
│  │  │                                 ┌─────┬─────┬─────┬─────┬─────┬─────┐           │ │  │
│  │  │                                 │run01│run01│run02│run02│ ... │     │           │ │  │
│  │  │                                 │base │inst │base │inst │     │     │           │ │  │
│  │  │                                 └─────┴─────┴─────┴─────┴─────┴─────┘           │ │  │
│  │  │                                   ▲                             │               │ │  │
│  │  │                                   │ push_back              pop_front            │ │  │
│  │  │                                                                 ▼               │ │  │
│  │  │                                                                                │ │  │
│  │  │   "linux" ───────► Mutex ─────► Queue                                          │ │  │
│  │  │                                 ┌─────┬─────┬─────┐                             │ │  │
│  │  │                                 │run03│run03│     │                             │ │  │
│  │  │                                 │base │inst │     │                             │ │  │
│  │  │                                 └─────┴─────┴─────┘                             │ │  │
│  │  │                                   ▲           │                                 │ │  │
│  │  │                                   │      pop_front                              │ │  │
│  │  │                                                                                │ │  │
│  │  └────────────────────────────────────────────────────────────────────────────────┘ │  │
│  │                                                                                      │  │
│  │  runs_available: Notify  ◄─────── notify_waiters() on add_runs()                    │  │
│  │                          ───────► VMExecutors wake up and compete                   │  │
│  │                                                                                      │  │
│  └──────────────────────────────────────────────────────────────────────────────────────┘  │
│                                                                                             │
└─────────────────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────────────────┐
│                                   CONSUMERS (VMExecutors)                                   │
├─────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                             │
│                    take_run(os, caps) - Only locks the matching OS queue                    │
│                                                                                             │
│  ┌─────────────────────┐    ┌─────────────────────┐    ┌─────────────────────┐              │
│  │   VMExecutor-1      │    │   VMExecutor-2      │    │   VMExecutor-3      │              │
│  │   (VM: win-vm-01)   │    │   (VM: win-vm-02)   │    │   (VM: linux-vm-01) │              │
│  │                     │    │                     │    │                     │              │
│  │ os: windows         │    │ os: windows         │    │ os: linux           │              │
│  │ caps: [defender,    │    │ caps: [rededr]      │    │ caps: []            │              │
│  │        rededr]      │    │                     │    │                     │              │
│  │                     │    │                     │    │                     │              │
│  │ ┌─────────────────┐ │    │ ┌─────────────────┐ │    │ ┌─────────────────┐ │              │
│  │ │ in_flight:      │ │    │ │ in_flight:      │ │    │ │ in_flight:      │ │              │
│  │ │ run-01-base     │ │    │ │ run-02-base     │ │    │ │ None (idle)     │ │              │
│  │ │ job: mut-01     │ │    │ │ job: mut-02     │ │    │ │                 │ │              │
│  │ └─────────────────┘ │    │ └─────────────────┘ │    │ └─────────────────┘ │              │
│  │                     │    │                     │    │         │           │              │
│  │         │           │    │         │           │    │         │           │              │
│  │         │ executing │    │         │ executing │    │         │ waiting   │              │
│  │         ▼           │    │         ▼           │    │         ▼           │              │
│  │   ┌───────────┐     │    │   ┌───────────┐     │    │   runs_available    │              │
│  │   │  Remote   │     │    │   │  Remote   │     │    │   .notified()       │              │
│  │   │  VM Agent │     │    │   │  VM Agent │     │    │   (waiting for      │              │
│  │   │           │     │    │   │           │     │    │    linux runs)      │              │
│  │   └───────────┘     │    │   └───────────┘     │    │                     │              │
│  └─────────────────────┘    └─────────────────────┘    └─────────────────────┘              │
│                                                                                             │
└─────────────────────────────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────────────────────────────┐
│                                 CAPABILITY MATCHING EXAMPLE                                 │
├─────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                             │
│  Run requires: caps = [defender]                                                            │
│                                                                                             │
│  VMExecutor-1 (caps: [defender, rededr]) ─────► ✅ MATCH (has defender)                     │
│  VMExecutor-2 (caps: [rededr])           ─────► ❌ NO MATCH (missing defender)              │
│  VMExecutor-3 (caps: [])                 ─────► ❌ NO MATCH (missing defender)              │
│                                                                                             │
│  Run requires: caps = []  (no requirements)                                                 │
│                                                                                             │
│  VMExecutor-1 (caps: [defender, rededr]) ─────► ✅ MATCH (empty requirement = any)          │
│  VMExecutor-2 (caps: [rededr])           ─────► ✅ MATCH                                    │
│  VMExecutor-3 (caps: [])                 ─────► ✅ MATCH                                    │
│                                                                                             │
│  Run requires: caps = [defender, rededr]                                                    │
│                                                                                             │
│  VMExecutor-1 (caps: [defender, rededr]) ─────► ✅ MATCH (has both)                         │
│  VMExecutor-2 (caps: [rededr])           ─────► ❌ NO MATCH (missing defender)              │
│  VMExecutor-3 (caps: [])                 ─────► ❌ NO MATCH (missing both)                  │
│                                                                                             │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Run Distribution Sequence

```
TIME ──────────────────────────────────────────────────────────────────────────────────────►

JobWorker-1                    JobWorker-2                    JobWorker-3
(Job: mut-01, os: windows)     (Job: mut-02, os: windows)     (Job: mut-03, os: linux)
     │                              │                              │
     │                              │                              │
t=0  │ add_runs([r1-base, r1-inst]) │                              │
     │ ─────────────────────────────┼──────────────────────────────┼─────────►  Pool
     │                              │                              │            (windows queue grows)
     │                              │                              │
     │                              │                              │            notify_waiters()
     │                              │                              │                 │
     │                              │                              │                 ▼
     │                              │                              │         VMExecutor-1 wakes
     │                              │                              │         VMExecutor-2 wakes
     │                              │                              │
t=1  │                              │                              │         VMExecutor-1:
     │                              │                              │           take_run("windows", [def,red])
     │                              │                              │           → gets r1-base (caps match)
     │                              │                              │           → dispatches to VM
     │                              │                              │
     │                              │ add_runs([r2-base, r2-inst]) │
     │                              │ ─────────────────────────────┼─────────►  Pool
     │                              │                              │            notify_waiters()
     │                              │                              │
t=2  │                              │                              │         VMExecutor-2:
     │                              │                              │           take_run("windows", [rededr])
     │                              │                              │           → skips r1-inst (needs defender)
     │                              │                              │           → gets r2-base (caps match)
     │                              │                              │           → dispatches to VM
     │                              │                              │
     │                              │                              │ add_runs([r3-base, r3-inst])
     │                              │                              │ ─────────────────────────────►  Pool
     │                              │                              │                (linux queue grows)
     │                              │                              │                notify_waiters()
     │                              │                              │
t=3  │                              │                              │         VMExecutor-3:
     │                              │                              │           take_run("linux", [])
     │                              │                              │           → gets r3-base
     │                              │                              │           → dispatches to VM
     │                              │                              │
t=4  │                              │                              │         VMExecutor-1:
     │                              │                              │           result received for r1-base
     │◄──────────────────────────────────────────────────────────────────────── route_result()
     │ on_result(r1-base)           │                              │
     │                              │                              │         VMExecutor-1:
     │                              │                              │           take_run("windows", [def,red])
     │                              │                              │           → gets r1-inst
     │                              │                              │           → dispatches to VM
     │                              │                              │
t=5  │                              │                              │         VMExecutor-2:
     │                              │                              │           result received for r2-base
     │                              │◄─────────────────────────────────────── route_result()
     │                              │ on_result(r2-base)           │
     │                              │                              │
     ...                           ...                            ...
```

### Pool State Over Time

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                              POOL STATE SNAPSHOTS                                      │
├────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                        │
│  t=0 (after JobWorker-1 adds runs):                                                    │
│                                                                                        │
│    windows: [r1-base, r1-inst]                                                         │
│    linux:   []                                                                         │
│    pending: { r1-base: {...}, r1-inst: {...} }                                         │
│                                                                                        │
│  t=1 (after VMExecutor-1 takes r1-base):                                               │
│                                                                                        │
│    windows: [r1-inst]                    ◄── r1-base removed                           │
│    linux:   []                                                                         │
│    pending: { r1-inst: {...} }           ◄── r1-base removed                           │
│                                                                                        │
│  t=1.5 (after JobWorker-2 adds runs):                                                  │
│                                                                                        │
│    windows: [r1-inst, r2-base, r2-inst]  ◄── new runs appended                         │
│    linux:   []                                                                         │
│    pending: { r1-inst, r2-base, r2-inst }                                              │
│                                                                                        │
│  t=2 (after VMExecutor-2 takes r2-base, skipping r1-inst):                             │
│                                                                                        │
│    windows: [r1-inst, r2-inst]           ◄── r2-base removed, r1-inst stays            │
│    linux:   []                               (VMExecutor-2 lacks defender cap)         │
│    pending: { r1-inst, r2-inst }                                                       │
│                                                                                        │
│  t=2.5 (after JobWorker-3 adds linux runs):                                            │
│                                                                                        │
│    windows: [r1-inst, r2-inst]                                                         │
│    linux:   [r3-base, r3-inst]           ◄── new queue created for linux               │
│    pending: { r1-inst, r2-inst, r3-base, r3-inst }                                     │
│                                                                                        │
│  t=3 (after VMExecutor-3 takes r3-base):                                               │
│                                                                                        │
│    windows: [r1-inst, r2-inst]           ◄── unchanged (different OS)                  │
│    linux:   [r3-inst]                    ◄── r3-base removed                           │
│    pending: { r1-inst, r2-inst, r3-inst }                                              │
│                                                                                        │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

### Result Routing

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│                              RESULT ROUTING MAP                                        │
├────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                        │
│  result_routers: RwLock<HashMap<JobId, Sender<JobRunResult>>>                          │
│                                                                                        │
│    ┌─────────────┬───────────────────────────────────────────────────────────┐         │
│    │   JobId     │   Sender                                                  │         │
│    ├─────────────┼───────────────────────────────────────────────────────────┤         │
│    │  "mut-01"   │   Sender → JobWorker-1.result_rx                          │         │
│    │  "mut-02"   │   Sender → JobWorker-2.result_rx                          │         │
│    │  "mut-03"   │   Sender → JobWorker-3.result_rx                          │         │
│    └─────────────┴───────────────────────────────────────────────────────────┘         │
│                                                                                        │
│  When VMExecutor receives result:                                                      │
│                                                                                        │
│    JobRunResult {                                                                      │
│      run_id: "r1-base",                                                                │
│      job_id: "mut-01",     ◄── Used for routing lookup                                 │
│      round_id: "mut-01-round-3",                                                       │
│      outcome: { detected: false, exit_code: 0 },                                       │
│      vm_id: "win-vm-01"                                                                │
│    }                                                                                   │
│         │                                                                              │
│         │ run_pool.route_result(result)                                                │
│         ▼                                                                              │
│    result_routers.get("mut-01")                                                        │
│         │                                                                              │
│         │ sender.send(result)                                                          │
│         ▼                                                                              │
│    JobWorker-1.result_rx.recv()  ───► on_result() ───► update RoundAgg                 │
│                                                                                        │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 9. Summary: Key Design Decisions

| Decision | Implementation | Benefit |
|----------|---------------|---------|
| **Sharded by OS** | `by_os: DashMap<OS, Mutex<Queue>>` | No global lock; VMs only lock their OS queue |
| **Result routing** | `result_routers: HashMap<JobId, Sender>` | Jobs decoupled from VMs; dynamic routing |
| **Round aggregation** | `RoundAgg { baseline, instrumented }` | Wait for both runs; handle out-of-order |
| **Single coordinator** | Orchestrator `select!` loop | Serialize state changes; no mutex needed |
| **Fire-and-forget indexing** | `tokio::spawn(index_telemetry)` | Non-blocking telemetry ingestion |
| **Capability filtering** | `take_run(os, caps)` checks capabilities | Only compatible VMs get matching runs |
| **Job registry persistence** | `job_registry` survives job completion | API can query completed jobs |
| **Bidirectional gRPC** | Single stream for commands + responses | Full duplex; no polling |
