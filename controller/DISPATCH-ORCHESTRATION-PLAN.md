# Worker-VM-Bound Dispatch + Orchestration Architecture

## Overview

This document proposes a clean architecture for per-VM Worker loops that own job execution,
round generation, and run dispatch. The key principle is **single-writer ownership**: each
Worker owns exactly one JobSession at a time, and all mutations to that session happen
within that Worker's task.

---

## Architecture Proposal: Embedded Job Loop with Dual-Lane Worker

### Core Insight

The Worker task is structured as a **dual-lane** executor:

```
┌─────────────────────────────────────────────────────────────────┐
│                         Worker Task                              │
│  ┌─────────────────────┐          ┌─────────────────────────┐   │
│  │   Producer Lane     │          │    Dispatch Lane        │   │
│  │                     │          │                         │   │
│  │  JobSession loop    │  ───►    │  RunPool → Remote VM    │   │
│  │  creates RoundSpecs │ enqueue  │                         │   │
│  │  builds artifacts   │          │  triggered by:          │   │
│  │  submits RunEnvelope│          │  - WorkerAvailable      │   │
│  │                     │          │  - NewRunSubmitted      │   │
│  └─────────────────────┘          └─────────────────────────┘   │
│                                                                  │
│  State owned by Worker:                                          │
│  - active_job: Option<JobSession>                               │
│  - run_pool: VecDeque<RunEnvelope>                              │
│  - round_aggs: HashMap<RoundId, RoundAgg>                       │
│  - available: bool (can dispatch next run)                       │
└─────────────────────────────────────────────────────────────────┘
```

---

## A) Components & Task Structure

### Task Hierarchy

```
┌────────────────────────────────────────────────────────────────────────┐
│                           Orchestrator                                  │
│  - Receives job submissions                                            │
│  - Routes jobs to compatible Workers                                   │
│  - Holds pending_jobs queue (unassigned jobs)                          │
│  - Listens to Worker events for job completion / availability          │
└──────────────────────────────────┬─────────────────────────────────────┘
                                   │
                    ┌──────────────┼──────────────┐
                    │              │              │
                    ▼              ▼              ▼
            ┌───────────┐  ┌───────────┐  ┌───────────┐
            │ Worker A  │  │ Worker B  │  │ Worker C  │
            │ (win10)   │  │ (win11)   │  │ (win10)   │
            │ [mde]     │  │ [cortex]  │  │ [mde,gpu] │
            └───────────┘  └───────────┘  └───────────┘
```

### Tokio Tasks

| Task | Lifetime | Responsibility |
|------|----------|----------------|
| **Orchestrator** | Application lifetime | Job routing, pending queue, global events |
| **Worker** (per VM) | While VM connected | Own connection, job execution, run dispatch |
| **RoundProducer** (spawned) | Per active round | Optional: parallel artifact builds for one round |

### Channel Topology

```rust
// Global channels
type JobSubmitTx = mpsc::Sender<JobSession>;
type OrchestratorEventRx = mpsc::Receiver<OrchestratorEvent>;

// Per-worker channels
type WorkerCmdTx = mpsc::Sender<WorkerCommand>;
type WorkerEventTx = mpsc::Sender<WorkerEvent>;

enum WorkerCommand {
    AssignJob(JobSession),
    Shutdown,
}

enum WorkerEvent {
    Available { worker_id: WorkerId },
    JobCompleted { worker_id: WorkerId, job_id: JobId, outcome: JobOutcome },
    RunCompleted { worker_id: WorkerId, run_id: RunId, outcome: RunOutcomeLite },
}

enum OrchestratorEvent {
    SubmitJob(JobSession),
    WorkerConnected { worker_id: WorkerId, os: String, capabilities: Vec<String> },
    WorkerDisconnected { worker_id: WorkerId },
}
```

### Authoritative State Locations

| State | Owner | Type |
|-------|-------|------|
| Pending jobs (unassigned) | Orchestrator | `VecDeque<JobSession>` |
| Worker registry | Orchestrator | `HashMap<WorkerId, WorkerHandle>` |
| Active JobSession | Worker (exclusive) | `Option<JobSession>` |
| Run pool | Worker (exclusive) | `VecDeque<RunEnvelope>` |
| Round aggregators | Worker (exclusive) | `HashMap<RoundId, RoundAgg>` |
| Dispatch availability | Worker (exclusive) | `bool` |

---

## B) Job Assignment to Workers

### Matching Algorithm

```rust
fn is_compatible(worker: &WorkerInfo, job: &JobSession) -> bool {
    // OS match (if job specifies target_os)
    let os_ok = match &job.target_os {
        None => true,  // any OS acceptable
        Some(required_os) => worker.os == *required_os,
    };

    // Capabilities: worker must have ALL required capabilities
    let caps_ok = job.required_capabilities.iter()
        .all(|req| worker.capabilities.iter().any(|w| w.eq_ignore_ascii_case(req)));

    os_ok && caps_ok
}
```

### Assignment Flow

```
                    ┌─────────────────┐
                    │  Job Submitted  │
                    └────────┬────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │ Find compatible │
                    │ idle workers    │
                    └────────┬────────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
              ▼              ▼              ▼
         No match      One match      Multiple
              │              │         matches
              │              │              │
              ▼              ▼              ▼
        ┌──────────┐  ┌──────────┐  ┌──────────────┐
        │ Queue in │  │ Assign   │  │ Select by:   │
        │ pending  │  │ to worker│  │ round-robin  │
        │ jobs     │  │          │  │ or least-busy│
        └──────────┘  └──────────┘  └──────────────┘
```

### Pending Job Queue

```rust
struct Orchestrator {
    pending_jobs: VecDeque<JobSession>,
    workers: HashMap<WorkerId, WorkerHandle>,
}

impl Orchestrator {
    fn on_worker_available(&mut self, worker_id: &WorkerId) {
        let worker = match self.workers.get(worker_id) {
            Some(w) if w.active_job.is_none() => w,
            _ => return,  // worker busy or not found
        };

        // Find first compatible pending job
        let job_idx = self.pending_jobs.iter()
            .position(|job| is_compatible(&worker.info, job));

        if let Some(idx) = job_idx {
            let job = self.pending_jobs.remove(idx).unwrap();
            self.assign_job_to_worker(worker_id, job);
        }
    }
}
```

---

## C) Run Production vs Run Dispatch (Critical)

### Dual-Lane Execution Model

The Worker runs a single `select!` loop that handles both production and dispatch:

```rust
impl Worker {
    async fn run_loop(&mut self) {
        loop {
            tokio::select! {
                // Lane 1: Commands from orchestrator
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        WorkerCommand::AssignJob(job) => self.start_job(job),
                        WorkerCommand::Shutdown => break,
                    }
                }

                // Lane 2: Remote execution completed
                Some(result) = self.remote_result_rx.recv() => {
                    self.on_run_completed(result);
                    // Triggers: try_dispatch() below
                }

                // Lane 3: Round production tick (if job active and not backpressured)
                _ = self.production_tick(), if self.can_produce_rounds() => {
                    self.produce_next_round().await;
                }
            }

            // After any event, attempt dispatch if available
            self.try_dispatch();
        }
    }
}
```

### Production Flow (within Worker)

```rust
impl Worker {
    async fn produce_next_round(&mut self) {
        let job = match &mut self.active_job {
            Some(j) if j.should_continue() => j,
            _ => return,
        };

        // 1. Start round in job session
        let (round_num, round_id) = job.start_round();

        // 2. Create round spec (mutations from selector or sequential)
        let spec = RoundSpec {
            id: round_id.clone(),
            job_id: job.id.clone(),
            round_number: round_num,
            mutations: self.select_mutations(job),
        };

        // 3. Build artifacts (assume build returns ArtifactRef)
        let baseline_artifact = self.build_artifact(&spec, RunType::Baseline).await;
        let instrumented_artifact = self.build_artifact(&spec, RunType::Instrumented).await;

        // 4. Create run envelopes
        let baseline_run = RunEnvelope {
            run_id: RunId(format!("{}-baseline", round_id.0)),
            job_id: job.id.clone(),
            round_id: round_id.clone(),
            round_number: round_num,
            run_type: RunType::Baseline,
            trace_mode: "off".to_string(),
            artifact: baseline_artifact,
            mutations: spec.mutations.iter().map(|m| m.id.clone()).collect(),
            target_os: job.target_os.clone(),
            required_capabilities: job.required_capabilities.clone(),
        };

        let instrumented_run = RunEnvelope {
            run_id: RunId(format!("{}-instrumented", round_id.0)),
            run_type: RunType::Instrumented,
            trace_mode: "lines".to_string(),
            artifact: instrumented_artifact,
            ..baseline_run.clone()
        };

        // 5. Create round aggregator
        let agg = RoundAgg {
            spec,
            baseline_run_id: baseline_run.run_id.clone(),
            instrumented_run_id: instrumented_run.run_id.clone(),
            baseline: None,
            instrumented: None,
        };
        self.round_aggs.insert(round_id, agg);

        // 6. Submit to local pool
        self.run_pool.push_back(baseline_run);
        self.run_pool.push_back(instrumented_run);
    }
}
```

### Dispatch Flow (within Worker)

```rust
impl Worker {
    fn try_dispatch(&mut self) {
        // Only dispatch if available (not waiting on remote)
        if !self.available {
            return;
        }

        // Pop from pool (FIFO)
        if let Some(envelope) = self.run_pool.pop_front() {
            self.dispatch_to_remote(envelope);
            self.available = false;  // Now busy
        }
    }

    fn dispatch_to_remote(&mut self, envelope: RunEnvelope) {
        // Send to remote VM via connection (owned by this Worker)
        // The actual send is async but we don't await here;
        // completion comes back via remote_result_rx
        self.remote_tx.send(envelope).unwrap();
    }

    fn on_run_completed(&mut self, result: RemoteRunResult) {
        // 1. Mark available
        self.available = true;

        // 2. Index to ES (fire and forget)
        self.index_run_record(result.clone());

        // 3. Update round aggregator
        if let Some(agg) = self.round_aggs.get_mut(&result.round_id) {
            let outcome = RunOutcomeLite {
                detected: result.detected,
                exit_code: result.exit_code,
            };

            match result.run_type {
                RunType::Baseline => agg.baseline = Some(outcome),
                RunType::Instrumented => agg.instrumented = Some(outcome),
            }

            // 4. If round complete, finalize
            if agg.is_complete() {
                self.finalize_round(&result.round_id);
            }
        }

        // 5. Emit event to orchestrator
        self.event_tx.send(WorkerEvent::RunCompleted {
            worker_id: self.id.clone(),
            run_id: result.run_id,
            outcome: result.into(),
        }).await;
    }
}
```

### Backpressure Control

```rust
const MAX_POOL_SIZE: usize = 10;  // Max runs queued per worker
const MAX_IN_FLIGHT_ROUNDS: usize = 3;  // Max rounds with incomplete runs

impl Worker {
    fn can_produce_rounds(&self) -> bool {
        // Backpressure conditions
        if self.run_pool.len() >= MAX_POOL_SIZE {
            return false;  // Pool full, pause production
        }

        if self.round_aggs.len() >= MAX_IN_FLIGHT_ROUNDS {
            return false;  // Too many in-flight rounds
        }

        // Must have an active job that should continue
        match &self.active_job {
            Some(job) => job.should_continue(),
            None => false,
        }
    }
}
```

---

## D) WorkerAvailable Handling

### Event-Driven Dispatch

Two triggers for dispatch:

1. **Run completed → WorkerAvailable** (implicit)
2. **New run submitted while idle** (immediate dispatch)

```rust
impl Worker {
    // Called after any state change that might enable dispatch
    fn try_dispatch(&mut self) {
        if !self.available {
            return;
        }

        if let Some(envelope) = self.run_pool.pop_front() {
            self.available = false;
            self.dispatch_to_remote(envelope);
        } else {
            // Pool empty, emit explicit WorkerAvailable to orchestrator
            // (so orchestrator can assign a new job if pending)
            self.event_tx.try_send(WorkerEvent::Available {
                worker_id: self.id.clone(),
            }).ok();
        }
    }

    // Called when production adds to pool
    fn submit_run(&mut self, envelope: RunEnvelope) {
        self.run_pool.push_back(envelope);

        // If idle, dispatch immediately (no dead time)
        if self.available && self.run_pool.len() == 1 {
            self.try_dispatch();
        }
    }
}
```

### Avoiding Dead Time

The key insight is that `try_dispatch()` is called:
- After `on_run_completed()` sets `available = true`
- After `submit_run()` adds to pool

This ensures:
- If pool was non-empty when run completes → immediate next dispatch
- If worker is idle and production adds a run → immediate dispatch
- No waiting for explicit WorkerAvailable signal

---

## E) Complete Worker State Machine

```
                              ┌─────────────┐
                              │   Idle      │
                              │ (no job)    │
                              └──────┬──────┘
                                     │ AssignJob
                                     ▼
                              ┌─────────────┐
                 ┌───────────►│   Active    │◄───────────┐
                 │            │ (has job)   │            │
                 │            └──────┬──────┘            │
                 │                   │                   │
                 │    ┌──────────────┴──────────────┐    │
                 │    │                             │    │
                 │    ▼                             ▼    │
          ┌─────────────────┐             ┌─────────────────┐
          │   Producing     │             │   Dispatching   │
          │ (creating runs) │────────────►│ (waiting remote)│
          └─────────────────┘   submit    └────────┬────────┘
                 ▲                                 │
                 │                                 │ complete
                 │                                 ▼
                 │                         ┌─────────────────┐
                 │                         │   Available     │
                 └─────────────────────────│ (can dispatch)  │
                           backpressure ok └─────────────────┘
                                                   │
                                                   │ job.should_continue() == false
                                                   ▼
                                           ┌─────────────┐
                                           │ Job Done    │
                                           │ → emit event│
                                           │ → go Idle   │
                                           └─────────────┘
```

---

## F) Data Flow Summary

```
┌──────────────────────────────────────────────────────────────────────────┐
│                            ORCHESTRATOR                                   │
│  pending_jobs: VecDeque<JobSession>                                      │
│  workers: HashMap<WorkerId, WorkerHandle>                                │
│                                                                          │
│  on SubmitJob(job):                                                      │
│    find compatible idle worker → assign OR queue in pending_jobs         │
│                                                                          │
│  on WorkerAvailable(worker_id):                                          │
│    if worker has no active job:                                          │
│      find compatible pending job → assign                                │
│                                                                          │
│  on JobCompleted(worker_id, job_id, outcome):                            │
│    index JobRecord to ES                                                 │
│    try assign next pending job to worker                                 │
└───────────────────────────────────────┬──────────────────────────────────┘
                                        │
              ┌─────────────────────────┼─────────────────────────┐
              │                         │                         │
              ▼                         ▼                         ▼
┌─────────────────────────┐ ┌─────────────────────────┐ ┌─────────────────────────┐
│       WORKER A          │ │       WORKER B          │ │       WORKER C          │
│                         │ │                         │ │                         │
│ State:                  │ │ State:                  │ │ State:                  │
│  - active_job: Option   │ │  - active_job: Option   │ │  - active_job: Option   │
│  - run_pool: VecDeque   │ │  - run_pool: VecDeque   │ │  - run_pool: VecDeque   │
│  - round_aggs: HashMap  │ │  - round_aggs: HashMap  │ │  - round_aggs: HashMap  │
│  - available: bool      │ │  - available: bool      │ │  - available: bool      │
│                         │ │                         │ │                         │
│ Loop:                   │ │ Loop:                   │ │ Loop:                   │
│  1. recv commands       │ │  1. recv commands       │ │  1. recv commands       │
│  2. recv remote results │ │  2. recv remote results │ │  2. recv remote results │
│  3. produce rounds      │ │  3. produce rounds      │ │  3. produce rounds      │
│  4. try_dispatch()      │ │  4. try_dispatch()      │ │  4. try_dispatch()      │
│                         │ │                         │ │                         │
│         │               │ │         │               │ │         │               │
│         ▼               │ │         ▼               │ │         ▼               │
│   Remote VM (win10)     │ │   Remote VM (win11)     │ │   Remote VM (win10)     │
└─────────────────────────┘ └─────────────────────────┘ └─────────────────────────┘
              │                         │                         │
              └─────────────────────────┼─────────────────────────┘
                                        │
                                        ▼
                              ┌───────────────────┐
                              │   Elasticsearch   │
                              │                   │
                              │ - RunRecord       │
                              │ - RoundRecord     │
                              │ - JobRecord       │
                              └───────────────────┘
```

---

## G) Integration with Existing target_manager

### Current target_manager.rs Analysis

The existing `target_manager.rs` is **compatible** with this architecture and requires
only moderate refactoring. Here's the detailed compatibility assessment:

#### What Already Exists (Keep As-Is)

| Feature | Status | Location |
|---------|--------|----------|
| `TargetEvent` enum | ✅ Complete | Lines 59-75 |
| Event bus (`events_tx`) | ✅ Complete | `mpsc::Sender<TargetEvent>` |
| `TargetStatus` enum | ✅ Complete | `Available`, `Busy`, `Offline` |
| gRPC channel management | ✅ Complete | `get_channel()` |
| Stream establishment | ✅ Complete | `establish_stream()` |
| State management | ✅ Complete | `reserve()`, `release()`, `mark_connected()`, `mark_offline()` |
| Capability matching | ✅ Complete | `get_available_by_os_and_capabilities()` |
| Artifact upload | ✅ Complete | `send_artifact()` with chunking |
| Low-level send | ✅ Complete | `send_command()` |

#### What Needs to Change

| Current | New | Change Required |
|---------|-----|-----------------|
| Spawns message handler + heartbeat as separate tasks | Spawn single Worker task | Modify `establish_stream()` |
| `execute_artifact()` owns execution logic | Worker owns execution | Move to Worker |
| `pending_responses` for oneshot tracking | Worker tracks pending | Move to Worker |
| Only emits `TargetEvent` | Also notify Orchestrator | Add `orchestrator_tx` channel |

#### Responsibility Boundary Shift

```
┌─────────────────────────────────────────────────────────────────┐
│                    STAYS in target_manager                       │
├─────────────────────────────────────────────────────────────────┤
│ • Target registration (register, register_with_metadata)        │
│ • gRPC channel management (get_channel)                         │
│ • Stream establishment (establish_stream - modified)            │
│ • State queries (get, list_all, get_available_by_os_and_caps)  │
│ • Low-level send (send_command, send_artifact)                  │
│ • Connection state (mark_connected, mark_offline)               │
│ • TargetEvent emission to event bus                             │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                     MOVES to Worker task                         │
├─────────────────────────────────────────────────────────────────┤
│ • execute_artifact() logic → Worker.dispatch_to_remote()        │
│ • pending_responses tracking → Worker.pending_runs              │
│ • Message handling (inline spawn) → Worker.on_remote_message()  │
│ • reserve/release calls → Worker calls target_manager methods   │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                        NEW in Worker                             │
├─────────────────────────────────────────────────────────────────┤
│ • JobSession ownership                                          │
│ • RoundSpec creation                                            │
│ • RunEnvelope pool (VecDeque)                                   │
│ • RoundAgg tracking                                             │
│ • Dispatch logic (try_dispatch)                                 │
│ • Backpressure control                                          │
└─────────────────────────────────────────────────────────────────┘
```

### Required Changes to target_manager.rs

#### 1. Add Orchestrator channel

```rust
pub struct TargetManager {
    targets: DashMap<String, Target>,
    events_tx: mpsc::Sender<TargetEvent>,

    // NEW: Channel to notify Orchestrator of worker connections
    orchestrator_tx: mpsc::Sender<OrchestratorEvent>,

    // REMOVE: pending_responses moves to Worker
    // pending_responses: Arc<Mutex<HashMap<String, oneshot::Sender<SampleResponse>>>>,

    rpc_timeout: Duration,
}
```

#### 2. Modify establish_stream() to spawn Worker

```rust
pub async fn establish_stream(&self, id: &str) -> Result<()> {
    let channel = self.get_channel(id).await?;
    let mut client = WorkerAgentClient::new(channel);

    // Create bidirectional stream
    let (tx, rx) = mpsc::channel::<ControllerMessage>(100);
    let outgoing = tokio_stream::wrappers::ReceiverStream::new(rx);
    let response = client.establish_stream(Request::new(outgoing)).await?;
    let incoming = response.into_inner();

    // Store stream sender
    if let Some(mut target) = self.targets.get_mut(id) {
        target.stream_tx = Some(tx.clone());
        if target.status == TargetStatus::Offline {
            target.status = TargetStatus::Available;
        }
    }

    // Get worker info for Worker task
    let worker_info = self.get_worker_info_local(id);

    // Create Worker channels
    let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCommand>(16);
    let (event_tx, event_rx) = mpsc::channel::<WorkerEvent>(64);

    // Create channel for remote results (from stream handler to Worker)
    let (remote_result_tx, remote_result_rx) = mpsc::channel::<RemoteRunResult>(64);

    // Spawn stream message handler (forwards to remote_result_tx)
    let id_clone = id.to_string();
    let events_tx = self.events_tx.clone();
    let targets = self.targets.clone();

    tokio::spawn(async move {
        Self::stream_handler(id_clone, incoming, events_tx, targets, remote_result_tx).await;
    });

    // Spawn Worker task
    let worker = Worker::new(
        WorkerId(id.to_string()),
        worker_info,
        cmd_rx,
        event_tx,
        tx.clone(),           // For sending commands to remote
        remote_result_rx,     // For receiving results from remote
    );
    tokio::spawn(worker.run_loop());

    // Notify Orchestrator
    self.orchestrator_tx.send(OrchestratorEvent::WorkerConnected {
        worker_id: WorkerId(id.to_string()),
        cmd_tx,
        event_rx,
        os: worker_info.os.clone(),
        capabilities: worker_info.capabilities.clone(),
    }).await?;

    // Spawn heartbeat (unchanged)
    self.spawn_heartbeat(id, tx);

    Ok(())
}
```

#### 3. Extract stream handler (refactored from inline spawn)

```rust
async fn stream_handler(
    id: String,
    mut incoming: tonic::Streaming<WorkerMessage>,
    events_tx: mpsc::Sender<TargetEvent>,
    targets: DashMap<String, Target>,
    remote_result_tx: mpsc::Sender<RemoteRunResult>,
) {
    while let Some(result) = incoming.next().await {
        match result {
            Ok(msg) => {
                // Update last_seen
                if let Some(mut t) = targets.get_mut(&id) {
                    t.touch();
                }

                // Handle SampleResponse -> forward to Worker
                if let Some(worker_message::Payload::SampleResponse(response)) = &msg.payload {
                    let result = RemoteRunResult::from(response.clone());
                    let _ = remote_result_tx.send(result).await;
                }

                // Forward all messages to event bus
                let _ = events_tx.send(TargetEvent::Message {
                    target_id: id.clone(),
                    msg,
                }).await;
            }
            Err(e) => {
                error!("Stream error from target {}: {}", id, e);
                break;
            }
        }
    }

    // Cleanup on disconnect
    if let Some(mut target) = targets.get_mut(&id) {
        target.stream_tx = None;
        target.status = TargetStatus::Offline;
    }

    let _ = events_tx.send(TargetEvent::Disconnected {
        target_id: id.clone(),
        reason: "Stream closed".to_string(),
    }).await;
}
```

#### 4. Remove execute_artifact() (moves to Worker)

```rust
// DELETE: This entire method moves to Worker task
// pub async fn execute_artifact(&self, id: &str, request: SampleRequest) -> Result<SampleResponse>

// DELETE: These move to Worker
// fn register_pending_execution(&self, run_id: String) -> oneshot::Receiver<SampleResponse>
// pub fn complete_pending_execution(&self, run_id: &str, response: SampleResponse) -> Result<()>
```

### Worker's Use of target_manager

Worker holds a reference to TargetManager for:

```rust
pub struct Worker {
    id: WorkerId,
    info: WorkerInfo,

    // Reference to target_manager for state updates
    target_manager: Arc<TargetManager>,

    // Channels
    cmd_rx: mpsc::Receiver<WorkerCommand>,
    event_tx: mpsc::Sender<WorkerEvent>,
    remote_tx: mpsc::Sender<ControllerMessage>,  // To send commands
    remote_rx: mpsc::Receiver<RemoteRunResult>,  // To receive results

    // ... rest of state
}

impl Worker {
    fn dispatch_to_remote(&mut self, envelope: RunEnvelope) {
        // Reserve target before dispatch
        self.target_manager.reserve(&self.id.0).ok();

        // Build command from envelope
        let command = ControllerMessage {
            payload: Some(controller_message::Payload::RunSample(RunSampleCommand {
                request_id: envelope.run_id.0.clone(),
                request: Some(SampleRequest {
                    job_id: envelope.job_id.0.clone(),
                    artifact_id: envelope.artifact.locator.clone(),
                    // ... fill from envelope
                }),
            })),
        };

        // Track pending run
        self.pending_runs.insert(envelope.run_id.clone(), envelope);

        // Send via stream
        let _ = self.remote_tx.try_send(command);
    }

    fn on_run_completed(&mut self, result: RemoteRunResult) {
        self.in_flight -= 1;

        // Release target if no more in-flight
        if self.in_flight == 0 {
            self.target_manager.release(&self.id.0).ok();
        }

        // ... rest of completion logic
    }
}
```

---

## H) Trade-offs Analysis

| Aspect | This Design | Alternative | Trade-off |
|--------|-------------|-------------|-----------|
| **Ownership** | Single-writer per job | Shared state + locks | Simpler reasoning, no deadlocks |
| **Concurrency** | Per-worker parallelism | Global thread pool | Bounded, predictable load per VM |
| **Fairness** | FIFO within worker | Priority queue | Simple, no starvation |
| **Throughput** | Pipelined (produce while dispatching) | Serial | Higher utilization |
| **Complexity** | Moderate (one task/worker) | Lower with global queue | Worth it for clear boundaries |
| **Extensibility** | Add workers = add tasks | Shared pool resize | Linear scaling |

### Complexity vs Correctness

- **Correctness guaranteed by**: single-writer ownership, local-only mutation
- **Complexity cost**: one task per worker, two channel types
- **Benefit**: no race conditions, no global locks, clear data flow

### Latency Considerations

- **Best case**: Run completes → immediate next dispatch (no idle time)
- **Worst case**: Backpressure → production pauses until pool drains
- **Mitigation**: Tune `MAX_POOL_SIZE` and `MAX_IN_FLIGHT_ROUNDS`

---

## I) Module Structure (Proposed)

```
controller/scheduler/src/
├── dispatch/
│   ├── mod.rs              # Re-exports
│   ├── worker.rs           # Worker task implementation
│   ├── orchestrator.rs     # Job routing, pending queue
│   ├── channels.rs         # WorkerCommand, WorkerEvent, OrchestratorEvent
│   ├── types.rs            # JobSession, RoundSpec, RoundAgg, RunEnvelope
│   └── es_records.rs       # RunRecord, RoundRecord, JobRecord (ES docs)
├── target_manager.rs       # Connection management (MODIFIED)
│   │                       #   - Spawns Worker tasks
│   │                       #   - Notifies Orchestrator
│   │                       #   - REMOVES execute_artifact()
├── service/
│   ├── mod.rs
│   └── job_handlers.rs     # gRPC handlers (submit via channel)
└── main.rs                 # Wire up orchestrator + target_manager

# REMOVED after migration:
# - scheduler_core.rs       (merged into Worker/Orchestrator)
# - round_processor.rs      (merged into Worker)
# - run_queue.rs            (not used)
# - job.rs                  (replaced by dispatch/types.rs)
# - round.rs                (replaced by dispatch/types.rs)
```

### File Responsibility Matrix

| File | Creates | Owns State | Calls |
|------|---------|------------|-------|
| `main.rs` | Orchestrator, TargetManager | - | spawn tasks |
| `target_manager.rs` | Worker, stream handlers | Target registry | gRPC, spawn Worker |
| `dispatch/orchestrator.rs` | - | pending_jobs, worker_handles | assign jobs |
| `dispatch/worker.rs` | RoundSpec, RunEnvelope | JobSession, run_pool, RoundAgg | target_manager.* |
| `service/job_handlers.rs` | JobSession | - | orchestrator.submit() |

---

## J) Key Code Skeletons

### Worker Task

```rust
pub struct Worker {
    id: WorkerId,
    info: WorkerInfo,  // os, capabilities

    // Channels
    cmd_rx: mpsc::Receiver<WorkerCommand>,
    event_tx: mpsc::Sender<WorkerEvent>,
    remote_tx: mpsc::Sender<RunEnvelope>,      // To remote VM
    remote_rx: mpsc::Receiver<RemoteRunResult>, // From remote VM

    // Local state (single-writer)
    active_job: Option<JobSession>,
    run_pool: VecDeque<RunEnvelope>,
    round_aggs: HashMap<RoundId, RoundAgg>,
    available: bool,
}

impl Worker {
    pub async fn run_loop(mut self) {
        self.available = true;

        loop {
            tokio::select! {
                biased;  // Prioritize commands over production

                Some(cmd) = self.cmd_rx.recv() => {
                    if !self.handle_command(cmd).await {
                        break;  // Shutdown
                    }
                }

                Some(result) = self.remote_rx.recv() => {
                    self.on_run_completed(result).await;
                }

                _ = self.production_interval.tick(), if self.can_produce() => {
                    self.produce_round().await;
                }
            }

            self.try_dispatch();
        }

        // Cleanup: finalize any in-progress job
        if let Some(job) = self.active_job.take() {
            self.finalize_job(job, JobOutcome::Stopped).await;
        }
    }
}
```

### Orchestrator Task

```rust
pub struct Orchestrator {
    pending_jobs: VecDeque<JobSession>,
    workers: HashMap<WorkerId, WorkerHandle>,
    event_rx: mpsc::Receiver<OrchestratorEvent>,
    job_submit_rx: mpsc::Receiver<JobSession>,
}

impl Orchestrator {
    pub async fn run_loop(mut self) {
        loop {
            tokio::select! {
                Some(job) = self.job_submit_rx.recv() => {
                    self.on_job_submitted(job).await;
                }

                Some(event) = self.event_rx.recv() => {
                    match event {
                        OrchestratorEvent::WorkerConnected { worker_id, cmd_tx, event_rx } => {
                            self.register_worker(worker_id, cmd_tx, event_rx);
                        }
                        OrchestratorEvent::WorkerDisconnected { worker_id } => {
                            self.unregister_worker(&worker_id);
                        }
                    }
                }

                // Merge all worker event receivers
                result = self.recv_any_worker_event() => {
                    if let Some(event) = result {
                        self.on_worker_event(event).await;
                    }
                }
            }
        }
    }

    async fn on_job_submitted(&mut self, job: JobSession) {
        // Find compatible idle worker
        let compatible = self.workers.iter()
            .filter(|(_, h)| h.is_idle() && is_compatible(&h.info, &job))
            .map(|(id, _)| id.clone())
            .next();

        match compatible {
            Some(worker_id) => self.assign_job(&worker_id, job).await,
            None => self.pending_jobs.push_back(job),
        }
    }

    async fn on_worker_event(&mut self, event: WorkerEvent) {
        match event {
            WorkerEvent::Available { worker_id } => {
                self.try_assign_pending(&worker_id).await;
            }
            WorkerEvent::JobCompleted { worker_id, job_id, outcome } => {
                self.index_job_record(&worker_id, &job_id, outcome).await;
                self.try_assign_pending(&worker_id).await;
            }
            WorkerEvent::RunCompleted { .. } => {
                // Primarily for observability; dispatch handled within worker
            }
        }
    }
}
```

---

## K) Migration Path from Current Code

### Phase 1: Introduce data types
- Add `dispatch/` module with new data types
- Create `dispatch/types.rs` with `JobSession`, `RoundSpec`, `RunEnvelope`, `RoundAgg`
- Create `dispatch/channels.rs` with `WorkerCommand`, `WorkerEvent`, `OrchestratorEvent`
- Keep existing `job.rs`, `round.rs` temporarily (for backwards compat during migration)

### Phase 2: Refactor target_manager.rs

**Step 2a: Add orchestrator channel**
```rust
// In TargetManager::new()
pub fn new(
    rpc_timeout_secs: u64,
    events_tx: mpsc::Sender<TargetEvent>,
    orchestrator_tx: mpsc::Sender<OrchestratorEvent>,  // NEW
) -> Self
```

**Step 2b: Extract stream_handler() as standalone function**
- Move inline spawn logic to `async fn stream_handler(...)`
- Add `remote_result_tx` parameter for forwarding SampleResponse to Worker

**Step 2c: Modify establish_stream()**
- Create Worker channels (cmd_tx/cmd_rx, event_tx/event_rx)
- Create remote result channel (for stream → Worker communication)
- Spawn stream_handler task
- Spawn Worker task
- Notify Orchestrator via orchestrator_tx
- Keep heartbeat spawn unchanged

**Step 2d: Remove execution methods**
- Delete `execute_artifact()`
- Delete `register_pending_execution()`
- Delete `complete_pending_execution()`
- Delete `pending_responses` field

**Step 2e: Keep these methods unchanged**
- `register()`, `register_with_metadata()`
- `get()`, `list_all()`, `get_available()`, `get_available_by_os_and_capabilities()`
- `reserve()`, `release()`, `mark_connected()`, `mark_offline()`
- `get_channel()`, `send_command()`, `send_artifact()`
- `broadcast()`, `disconnect_all()`

### Phase 3: Implement Worker task
- Create `dispatch/worker.rs`
- Implement dual-lane select! loop
- Implement `dispatch_to_remote()` using target_manager.send_command()
- Implement `on_run_completed()` with RoundAgg updates
- Implement `produce_round()` with build integration
- Add backpressure control
- Test with single worker + mock remote

### Phase 4: Implement Orchestrator
- Create `dispatch/orchestrator.rs`
- Implement job assignment with OS + capability matching
- Implement pending_jobs queue
- Implement worker registry with WorkerHandle
- Merge worker event receivers using `futures::stream::select_all`
- Wire up to gRPC SubmitJob endpoint

### Phase 5: Update main.rs
- Remove old event loop (current lines 260-560)
- Create orchestrator channels
- Pass orchestrator_tx to TargetManager
- Spawn Orchestrator task
- Update SchedulerService to submit jobs via channel
- Keep gRPC server setup

### Phase 6: Cleanup
- Remove old `scheduler_core.rs` (job processing merged into Worker)
- Remove old `round_processor.rs` (round logic merged into Worker)
- Remove old `job.rs`, `round.rs` if fully replaced
- Remove `run_queue.rs` (not used in new architecture)
- Update imports throughout
- Run full test suite

### Phase 7: Concurrent Enhancement (Post-Stabilization)
- Replace `available: bool` with `max_concurrent: usize` + `in_flight: usize`
- Update `try_dispatch()` to loop
- Update `on_run_completed()` to decrement
- Test with `max_concurrent = 2`

### Migration Verification Checklist

After each phase, verify:

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes
- [ ] Single worker can connect and receive registration
- [ ] Job can be submitted via gRPC
- [ ] Job is assigned to compatible worker
- [ ] Rounds are produced and runs dispatched
- [ ] Run results are received and RoundAgg updated
- [ ] Round summaries indexed to ES
- [ ] Worker emits JobCompleted when done
- [ ] Orchestrator assigns next pending job

---

## L) Summary

**Key architectural decisions:**

1. **Worker-bound execution**: Each Worker owns exactly one JobSession at a time
2. **Dual-lane model**: Production and dispatch happen concurrently within Worker
3. **Local run pool**: No global run queue; each Worker has its own VecDeque
4. **Event-driven dispatch**: `try_dispatch()` called after any state change
5. **Backpressure via pool limits**: Pause production when pool full
6. **Single-writer ownership**: No locks, no races, clear data flow
7. **ES for durability only**: Runtime correctness doesn't depend on ES

**What this enables:**

- Clean separation of concerns
- Predictable per-VM load
- Easy reasoning about concurrency
- Straightforward extension to more workers
- No global bottlenecks

---

## M) Enhancement: Concurrent Run Execution (Post-Refactor)

After the core refactor is complete and stable, add support for multiple concurrent runs
per Worker. This is a **low-effort enhancement** that significantly improves throughput
by executing baseline + instrumented runs in parallel on the same VM.

### Motivation

In the base design, runs execute serially:
```
Time →
Slot:  ──B1──────────┬──I1──────────┬──B2──────────┬──I2──────────
                     │              │              │
                     wait           wait           wait
```

With 2 concurrent slots, a round's baseline + instrumented execute in parallel:
```
Time →
Slot 1:  ──B1────────────┐     ──B2────────────┐
Slot 2:  ──I1────────┐   │     ──I2────────┐   │
                     │   │                 │   │
                     ▼   ▼                 ▼   ▼
              RoundAgg complete     RoundAgg complete
```

This roughly **halves round completion time** with no additional complexity.

### Changes Required

#### 1. Replace `available: bool` with slot counter

```rust
// Before (base design)
pub struct Worker {
    // ...
    available: bool,
}

// After (concurrent enhancement)
pub struct Worker {
    // ...
    max_concurrent: usize,  // Configured per worker (e.g., 2)
    in_flight: usize,       // Currently executing on remote VM
}
```

#### 2. Update dispatch logic

```rust
impl Worker {
    fn has_capacity(&self) -> bool {
        self.in_flight < self.max_concurrent
    }

    fn try_dispatch(&mut self) {
        // Dispatch up to available slots (not just one)
        while self.in_flight < self.max_concurrent {
            match self.run_pool.pop_front() {
                Some(envelope) => {
                    self.dispatch_to_remote(envelope);
                    self.in_flight += 1;
                }
                None => break,  // Pool empty
            }
        }

        // Emit availability only when fully idle and pool empty
        // (orchestrator cares about job assignment, not run-level availability)
        if self.in_flight == 0 && self.run_pool.is_empty() && self.active_job.is_none() {
            self.event_tx.try_send(WorkerEvent::Available {
                worker_id: self.id.clone(),
            }).ok();
        }
    }

    fn on_run_completed(&mut self, result: RemoteRunResult) {
        // Decrement counter (not set to true)
        self.in_flight -= 1;

        // ... rest unchanged (update RoundAgg, index to ES, etc.)
    }
}
```

#### 3. Updated state diagram

```
                              ┌─────────────┐
                              │   Idle      │
                              │ (no job)    │
                              │ in_flight=0 │
                              └──────┬──────┘
                                     │ AssignJob
                                     ▼
                              ┌─────────────┐
                 ┌───────────►│   Active    │◄───────────┐
                 │            │ (has job)   │            │
                 │            └──────┬──────┘            │
                 │                   │                   │
                 │    ┌──────────────┴──────────────┐    │
                 │    │                             │    │
                 │    ▼                             ▼    │
          ┌─────────────────┐             ┌─────────────────┐
          │   Producing     │             │   Executing     │
          │ (creating runs) │────────────►│ in_flight: 0..N │
          └─────────────────┘   submit    └────────┬────────┘
                 ▲                                 │
                 │                                 │ in_flight < max
                 │                                 ▼
                 │                         ┌─────────────────┐
                 │                         │  Has Capacity   │
                 └─────────────────────────│ (can dispatch)  │
                           backpressure ok └─────────────────┘
```

#### 4. Backpressure adjustment

```rust
const MAX_POOL_SIZE: usize = 10;
const MAX_IN_FLIGHT_ROUNDS: usize = 3;

impl Worker {
    fn can_produce_rounds(&self) -> bool {
        // Pool limit unchanged
        if self.run_pool.len() >= MAX_POOL_SIZE {
            return false;
        }

        // Round limit unchanged (in_flight runs still tracked per round via RoundAgg)
        if self.round_aggs.len() >= MAX_IN_FLIGHT_ROUNDS {
            return false;
        }

        match &self.active_job {
            Some(job) => job.should_continue(),
            None => false,
        }
    }
}
```

### Signal Model: Unchanged

The `WorkerAvailable` signal semantics remain the same:
- **Meaning**: "Worker can accept work" (now: has at least one free slot)
- **Emission**: When transitioning from busy to has-capacity
- **Orchestrator handling**: Unchanged (assigns jobs, not individual runs)

The orchestrator doesn't need to know how many slots exist—it only cares whether
the worker is idle (no active job) for job assignment purposes.

### Configuration

```rust
impl Worker {
    pub fn new(
        id: WorkerId,
        info: WorkerInfo,
        max_concurrent: usize,  // New parameter
        // ... other params
    ) -> Self {
        Self {
            id,
            info,
            max_concurrent,
            in_flight: 0,
            // ...
        }
    }
}

// Recommended defaults:
// - max_concurrent = 2 (baseline + instrumented in parallel)
// - Can be tuned per-VM based on VM resources
```

### Why This Is Low-Effort

| Aspect | Base Design | Concurrent Enhancement | Diff |
|--------|-------------|------------------------|------|
| State | `available: bool` | `in_flight: usize` | 1 field |
| Dispatch | `if available` | `while in_flight < max` | Loop |
| Completion | `available = true` | `in_flight -= 1` | Operator |
| Signal | WorkerAvailable | Same | None |
| RoundAgg | Already async | No change | None |
| Backpressure | Pool + round limits | Same | None |

**Total changes**: ~10 lines of code after base refactor is complete.

### Implementation Order

1. Complete base refactor (Phases 1-5 from Section K)
2. Validate single-run dispatch works correctly
3. Add `max_concurrent` / `in_flight` fields
4. Update `try_dispatch()` to loop
5. Update `on_run_completed()` to decrement
6. Test with `max_concurrent = 2`
7. Tune based on VM performance

### Trade-offs

| Benefit | Cost |
|---------|------|
| ~2x throughput per round | Slightly higher VM load |
| Natural fit (baseline + instrumented) | Must handle partial round completion |
| No additional complexity | Need to tune max_concurrent per VM |

### Edge Cases

**Partial round completion on disconnect:**
- If worker disconnects mid-round, some runs may complete and some may not
- RoundAgg handles this: incomplete rounds are discarded (no ES record)
- JobSession can restart the round on reconnection (or mark job failed)

**Unbalanced run types:**
- If pool has [B1, B2, I1, I2] and max_concurrent=2, dispatch order is FIFO
- Both slots might get baselines, then both get instrumented
- This is fine—RoundAgg doesn't care about execution order

---

## N) Final Architecture Summary

After implementing the concurrent enhancement:

```
┌─────────────────────────────────────────────────────────────────┐
│                         Worker Task                              │
│  ┌─────────────────────┐          ┌─────────────────────────┐   │
│  │   Producer Lane     │          │    Dispatch Lane        │   │
│  │                     │          │                         │   │
│  │  JobSession loop    │  ───►    │  RunPool → Remote VM    │   │
│  │  creates RoundSpecs │ enqueue  │                         │   │
│  │  builds artifacts   │          │  max_concurrent slots   │   │
│  │  submits RunEnvelope│          │  (e.g., 2 for B+I)      │   │
│  │                     │          │                         │   │
│  └─────────────────────┘          └─────────────────────────┘   │
│                                                                  │
│  State owned by Worker:                                          │
│  - active_job: Option<JobSession>                               │
│  - run_pool: VecDeque<RunEnvelope>                              │
│  - round_aggs: HashMap<RoundId, RoundAgg>                       │
│  - max_concurrent: usize (configured)                            │
│  - in_flight: usize (0..max_concurrent)                          │
└─────────────────────────────────────────────────────────────────┘
```

**Key properties preserved:**
- Single-writer ownership (no locks)
- Local run pool (no global queue)
- Event-driven dispatch
- Backpressure via pool limits
- ES for durability only

**Key property added:**
- Concurrent run execution (configurable per worker)

---

## O) Consistency Verification

This section verifies that all parts of the plan are internally consistent.

### Channel Flow Verification

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              CHANNEL TOPOLOGY                                │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│  main.rs creates:                                                           │
│    • events_tx/rx (TargetEvent) ────────────────────────┐                   │
│    • orchestrator_tx/rx (OrchestratorEvent) ────────────│──┐                │
│    • job_submit_tx/rx (JobSession) ─────────────────────│──│──┐             │
│                                                         │  │  │             │
│                                                         ▼  ▼  ▼             │
│  ┌─────────────────┐                          ┌─────────────────────┐       │
│  │  TargetManager  │                          │    Orchestrator     │       │
│  │                 │                          │                     │       │
│  │  events_tx ─────│──────────────────────────│→ (for TargetEvent)  │       │
│  │  orchestrator_tx│──────────────────────────│→ orchestrator_rx    │       │
│  │                 │                          │  job_submit_rx ←────│───────│
│  └────────┬────────┘                          └──────────┬──────────┘       │
│           │                                              │                  │
│           │ establish_stream() creates per worker:       │                  │
│           │  • cmd_tx/rx ────────────────────────────────│──┐               │
│           │  • event_tx/rx ──────────────────────────────│──│──┐            │
│           │  • remote_result_tx/rx ──────────┐           │  │  │            │
│           │                                  │           │  │  │            │
│           ▼                                  ▼           ▼  ▼  ▼            │
│  ┌─────────────────┐                ┌─────────────────────────────────┐     │
│  │ Stream Handler  │                │           Worker                │     │
│  │                 │                │                                 │     │
│  │ remote_result_tx│───────────────▶│ remote_rx                       │     │
│  │                 │                │ cmd_rx ◄────── cmd_tx (from Orch)│     │
│  └─────────────────┘                │ event_tx ─────▶ event_rx (Orch) │     │
│                                     │ remote_tx ─────▶ gRPC stream    │     │
│                                     └─────────────────────────────────┘     │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### State Ownership Verification

| State | Owner | Mutated By | Verified In |
|-------|-------|------------|-------------|
| `Target` registry | TargetManager | register, mark_*, reserve, release | Section G |
| `pending_jobs` | Orchestrator | on_job_submitted, try_assign_pending | Section B, J |
| `worker_handles` | Orchestrator | register_worker, unregister_worker | Section A, J |
| `active_job` | Worker (exclusive) | start_job, finalize_job | Section C, E |
| `run_pool` | Worker (exclusive) | submit_run, try_dispatch | Section C, D |
| `round_aggs` | Worker (exclusive) | produce_round, on_run_completed | Section C |
| `in_flight` | Worker (exclusive) | try_dispatch, on_run_completed | Section M |

### Event Flow Verification

| Event | Producer | Consumer | Action |
|-------|----------|----------|--------|
| `TargetEvent::Connected` | stream_handler | (observability) | Log connection |
| `TargetEvent::Disconnected` | stream_handler | Orchestrator | Unregister worker |
| `TargetEvent::Message` | stream_handler | (observability) | Log/debug |
| `OrchestratorEvent::WorkerConnected` | TargetManager | Orchestrator | Register worker handle |
| `OrchestratorEvent::WorkerDisconnected` | TargetManager | Orchestrator | Unregister, fail jobs |
| `WorkerCommand::AssignJob` | Orchestrator | Worker | Start job session |
| `WorkerCommand::Shutdown` | Orchestrator | Worker | Graceful cleanup |
| `WorkerEvent::Available` | Worker | Orchestrator | Try assign pending job |
| `WorkerEvent::JobCompleted` | Worker | Orchestrator | Index to ES, assign next |
| `WorkerEvent::RunCompleted` | Worker | (observability) | Log/metrics |

### Method Call Verification

| Caller | Calls | Method | Purpose |
|--------|-------|--------|---------|
| Worker | TargetManager | `reserve(id)` | Mark target busy before dispatch |
| Worker | TargetManager | `release(id)` | Mark target available after last run |
| Worker | TargetManager | `send_command(id, msg)` | Send RunSampleCommand to remote |
| Worker | TargetManager | `send_artifact(id, artifact_id, path)` | Upload artifact before run |
| Orchestrator | WorkerHandle | `cmd_tx.send(AssignJob)` | Assign job to worker |
| TargetManager | Worker | spawns task | On stream establishment |
| TargetManager | Orchestrator | `orchestrator_tx.send(WorkerConnected)` | Notify of new worker |

### Compatibility with Current target_manager.rs

| Current Method | Disposition | Notes |
|----------------|-------------|-------|
| `new()` | MODIFY | Add `orchestrator_tx` parameter |
| `register()` | KEEP | Unchanged |
| `register_with_metadata()` | KEEP | Unchanged |
| `get()`, `list_all()`, etc. | KEEP | Unchanged |
| `reserve()`, `release()` | KEEP | Called by Worker |
| `mark_connected()`, `mark_offline()` | KEEP | Unchanged |
| `get_channel()` | KEEP | Unchanged |
| `establish_stream()` | MODIFY | Spawn Worker, notify Orchestrator |
| `send_command()` | KEEP | Called by Worker |
| `send_artifact()` | KEEP | Called by Worker |
| `broadcast()`, `disconnect_all()` | KEEP | Unchanged |
| `execute_artifact()` | DELETE | Moves to Worker |
| `register_pending_execution()` | DELETE | Moves to Worker |
| `complete_pending_execution()` | DELETE | Moves to Worker |
| `pending_responses` field | DELETE | Moves to Worker |
| `get_worker_info()` | KEEP | For initial query |
| `query_all_info()` | KEEP | For diagnostics |

### Verified: Plan is Internally Consistent

All sections reference the same:
- Channel types and directions
- State ownership boundaries
- Event semantics
- Method responsibilities
- Migration steps align with architectural changes
