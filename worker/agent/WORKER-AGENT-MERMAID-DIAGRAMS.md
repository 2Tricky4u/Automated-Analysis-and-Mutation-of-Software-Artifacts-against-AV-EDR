# Worker Agent — Mermaid Diagrams

Visual diagrams for the `worker/agent/` crate architecture. The **Overview** provides a conceptual understanding of the whole module; subsequent diagrams detail each important subsystem.

---

## 1. Overview — Conceptual Architecture

The big picture: what the worker agent does, how data flows through it, and how it fits in the AutoMutate++ pipeline.

```mermaid
graph TB
    subgraph AutoMutate["AutoMutate++ Pipeline"]
        Selector["Selector<br/><i>picks mutations</i>"]
        Build["Build Crate<br/><i>compiles PE artifact</i>"]
        Controller["Controller<br/><i>orchestration + storage</i>"]
        Triage["Triage Engine<br/><i>tokens + differentials</i>"]
    end

    subgraph WorkerAgent["Worker Agent (this crate)"]
        direction TB

        Receive["📥 Receive Artifact<br/><i>chunked gRPC transfer</i><br/><i>SHA-256 verify</i>"]
        Execute["⚡ Execute Under Monitoring<br/><i>spawn PE as child process</i><br/><i>RedEDR ETW tracing</i><br/><i>line-level instrumentation</i>"]
        Classify["🔍 Classify Outcome<br/><i>7-verdict decision tree</i><br/><i>Evasion / Detected / Ambiguous</i><br/><i>Stalled / InfraError / ...</i>"]
        Collect["📊 Collect Telemetry<br/><i>6 sources: ETW, traces,</i><br/><i>coverage, checkpoints</i>"]
        Report["📤 Report Results<br/><i>TelemetryBatch + RunOutcome</i><br/><i>streamed to controller</i>"]

        Receive --> Execute --> Classify
        Execute --> Collect --> Report
    end

    Selector -->|"feedback<br/>(avoid/seek tokens)"| Selector
    Selector --> Build
    Build -->|"PE binary"| Controller
    Controller -->|"SendArtifact<br/>RunSample"| Receive
    Report -->|"RunOutcome +<br/>TelemetryData[]"| Controller
    Controller -->|"raw telemetry"| Triage
    Triage -->|"token scores"| Selector

    style WorkerAgent fill:#1a1a2e,stroke:#e94560,stroke-width:2px,color:#fff
    style Receive fill:#0f3460,stroke:#e94560,color:#fff
    style Execute fill:#0f3460,stroke:#e94560,color:#fff
    style Classify fill:#0f3460,stroke:#e94560,color:#fff
    style Collect fill:#0f3460,stroke:#e94560,color:#fff
    style Report fill:#0f3460,stroke:#e94560,color:#fff
```

---

## 2. Layered Architecture

The crate's strict 5-layer design: each layer only depends on layers below it.

```mermaid
graph TB
    subgraph L0["Layer 0 — Startup"]
        main["main.rs<br/><i>97 lines</i>"]
        caps["capabilities.rs<br/><i>330 lines</i>"]
        consts["constants.rs<br/><i>17 lines</i>"]
    end

    subgraph L1["Layer 1 — Central Struct"]
        lib["lib.rs — WorkerAgentService<br/><i>85 lines</i><br/>7 fields: worker_id, config, system_info,<br/>execution_lock, stream_handler,<br/>heartbeat_handle, capabilities"]
    end

    subgraph L2["Layer 2 — gRPC Thin Adapters"]
        api_mod["api/mod.rs<br/><i>WorkerAgent trait</i>"]
        api_run["api/run.rs<br/><i>RunSample</i>"]
        api_art["api/artifacts.rs<br/><i>SendArtifact</i>"]
        api_info["api/info.rs<br/><i>Ping, Health, Info, Telemetry</i>"]
        api_stream["api/stream.rs<br/><i>EstablishStream</i>"]
    end

    subgraph L3["Layer 3 — Domain Logic"]
        exec["execution/<br/><i>2272 lines</i><br/>engine, classifier,<br/>guards, monitor,<br/>sink, state, types"]
        sess["session/<br/><i>495 lines</i><br/>stream_handler,<br/>worker_state"]
        telem["telemetry/<br/><i>1935 lines</i><br/>rededr, trace,<br/>pipeline,<br/>trace_compressor"]
    end

    subgraph L4["Layer 4 — OS Boundary"]
        infra["infra/<br/><i>137 lines</i><br/>process, system, time"]
    end

    OS["Windows API · Filesystem · sysinfo · chrono"]

    main --> caps
    main --> lib
    lib --> api_mod
    api_mod --> api_run & api_art & api_info & api_stream
    api_run --> exec
    api_stream --> sess
    api_info --> infra
    sess --> exec
    exec --> telem
    exec --> infra
    telem --> infra
    infra --> OS

    style L0 fill:#2d1b69,stroke:#8b5cf6,color:#fff
    style L1 fill:#1e3a5f,stroke:#3b82f6,color:#fff
    style L2 fill:#1a4731,stroke:#22c55e,color:#fff
    style L3 fill:#4a2c17,stroke:#f59e0b,color:#fff
    style L4 fill:#4a1c1c,stroke:#ef4444,color:#fff
```

---

## 3. Inter-Module Dependency Graph

Detailed module-level dependency map showing which files call which.

```mermaid
graph LR
    main["main.rs"] --> caps["capabilities.rs"]
    main --> lib["lib.rs"]

    lib --> api_mod["api/mod.rs"]

    api_mod --> api_run["api/run.rs"]
    api_mod --> api_art["api/artifacts.rs"]
    api_mod --> api_info["api/info.rs"]
    api_mod --> api_stream["api/stream.rs"]

    api_run --> engine["execution/<br/>engine.rs"]
    api_run --> state["execution/<br/>state.rs"]
    api_run --> sink["execution/<br/>sink.rs"]

    api_stream --> sh["session/<br/>stream_handler.rs"]
    api_stream --> ws["session/<br/>worker_state.rs"]

    api_info --> infra_sys["infra/<br/>system.rs"]
    api_info --> infra_time["infra/<br/>time.rs"]

    sh --> engine
    sh --> state
    sh --> sink

    engine --> classifier["execution/<br/>classifier.rs"]
    engine --> guards["execution/<br/>guards.rs"]
    engine --> monitor["execution/<br/>monitor.rs"]
    engine --> types["execution/<br/>types.rs"]
    engine --> pipeline["telemetry/<br/>pipeline.rs"]
    engine --> rededr["telemetry/<br/>rededr.rs"]
    engine --> trace["telemetry/<br/>trace.rs"]
    engine --> infra_proc["infra/<br/>process.rs"]
    engine --> infra_sys

    guards --> rededr
    monitor --> infra_proc

    style main fill:#6b21a8,color:#fff
    style lib fill:#1d4ed8,color:#fff
    style engine fill:#b45309,color:#fff
    style sh fill:#0d9488,color:#fff
```

---

## 4. RunSample — Detailed Sequence

The complete lifecycle of a RunSample request from arrival through all 10 execution phases to response delivery. Two entry points (unary RPC and stream command) converge on the same execution engine.

```mermaid
sequenceDiagram
    autonumber
    participant C as Controller
    participant SH as StreamHandler
    participant U as api/run.rs
    participant L as ExecutionState lock
    participant S as ControlPlaneSink
    participant E as execute_run engine
    participant RE as RedEDR collector
    participant T as TraceCollector
    participant P as Artifact process
    participant M as ExecutionMonitor
    participant FS as Telemetry pipeline

    %% ─── ENTRY POINTS ───
    alt Bidirectional stream path
        C->>SH: ControllerMessage RunSampleCommand
        SH->>C: send_ack(request_id, success=true)
        SH->>SH: tokio::spawn (non-blocking, stream loop continues)
        SH->>SH: Update WorkerState (current_job_id, current_run_id)
        SH->>L: state.acquire(job_id, artifact_name, run_id)
        alt Already Running
            L-->>SH: error
            SH->>C: WorkerMessage SampleResponse (error)
            SH->>SH: Clear WorkerState
        else Idle → Running
            L-->>SH: ExecutionLockGuard created
            SH->>S: build_sink(Some(tx))
            SH->>E: execute_run(RunRequest, RunContext, sink)
        end
    else Unary RPC path
        C->>U: RunSample(SampleRequest)
        U->>U: resolve_run_id (worker_state.current_run_id or UUID v4)
        U->>L: state.acquire(job_id, artifact_name, run_id)
        alt Already Running
            L-->>U: error
            U-->>C: Status RESOURCE_EXHAUSTED
        else Idle → Running
            L-->>U: ExecutionLockGuard created
            U->>S: build_sink(stream_handler.sender() or None)
            U->>E: execute_run(RunRequest, RunContext, sink)
        end
    end

    %% ─── PHASE 1: VALIDATE ───
    Note over E: Phase 1 — Validate
    E->>E: Check artifact_path.exists()

    %% ─── PHASE 2: REDEDR SETUP ───
    Note over E,RE: Phase 2 — RedEDR Setup
    E->>RE: RedEdrCollector::new(config)
    Note over E,RE: RedEdrGuard created (reset_on_drop=true)
    E->>RE: collect_all("sanity-check")
    RE-->>E: Vec events

    alt >1 stale events + strict_mode
        E-->>SH: RunError FailedPrecondition
    else clean (0-1 events)
        E->>RE: start_trace([artifact.exe])
        RE->>RE: POST /api/trace/start
    end

    %% ─── PHASE 3: ENVIRONMENT ───
    Note over E,FS: Phase 3 — Prepare Environment
    E->>FS: prepare_telemetry_dir() (remove stale + create fresh)
    E->>T: TraceCollector::new(event_tx, capacity=100K)
    E->>T: spawn start_server() (named pipe \\.\pipe\rededr_trace)
    E->>FS: spawn streaming_writer (trace_rx → BufWriter 256KB → trace_events.jsonl)

    %% ─── PHASE 4: SPAWN ───
    Note over E,P: Phase 4 — Spawn Process
    E->>P: spawn_artifact(artifact_path, telemetry_dir)
    Note over E,P: ProcessGuard created (should_kill=true)
    P-->>E: PID

    %% ─── PHASE 5: MONITORING ───
    Note over E,M: Phase 5 — Start Monitoring
    E->>E: spawn capture_stream(stdout)
    E->>E: spawn capture_stream(stderr)
    E->>M: ExecutionMonitor::new(config, sink)
    E->>M: spawn monitor.start(stop_rx, event_tx)
    Note over E,M: MonitorGuard created (stop_tx, handle, event_consumer)
    M->>S: send_status("started", initial ExecutionStatusReport)
    S->>C: WorkerMessage ExecutionStatus (or dropped by NullSink)

    %% ─── CONCURRENT EXECUTION ───
    par Artifact executes
        P->>P: PE artifact running (payload logic)
        P->>T: writes trace data to named pipe
        T->>T: auto-detect binary (ISTR) or Base64 protocol
        T->>FS: TraceEvent → trace_tx channel → trace_events.jsonl
    and Monitor polls every 3 seconds
        loop Every MONITOR_POLL_INTERVAL_SECS (3s)
            M->>P: is_process_alive(pid)
            M->>M: get_process_metrics(pid) via sysinfo
            M->>RE: GET /api/stats (events_count only)
            M->>M: idle detection (cpu<5% + no new events for 3 cycles)
            M->>M: timeout check (within 5s of deadline)
            M->>S: send_status(heartbeat | telemetry_idle | approaching_timeout)
            S->>C: WorkerMessage ExecutionStatus
        end
    end

    %% ─── PHASE 6: WAIT ───
    Note over E,P: Phase 6 — Wait for Completion or Timeout
    E->>E: tokio::time::timeout(duration, child.wait())

    alt Process exits normally
        P-->>E: exit status
        E->>E: exit_code = status.code() or EXIT_NO_CODE (-2)
    else Timeout fires
        E->>E: sleep 100ms (race window)
        E->>P: try_wait()
        alt Exited during race window
            P-->>E: real exit code
        else Still alive
            E->>P: kill_process_tree() (taskkill /F /T + child.kill)
            E->>P: is_process_alive(pid) verify kill
            E->>E: exit_code = EXIT_TIMEOUT (-3)
        end
    else wait() system call error
        E->>E: exit_code = EXIT_WAIT_FAILED (-1)
    end

    E->>E: ProcessGuard.disarm() (takes Child, should_kill=false)

    %% ─── CLEANUP MONITORING ───
    Note over E,M: Cleanup — Drain concurrent tasks
    E->>E: await stdout_handle, stderr_handle
    E->>M: MonitorGuard.stop() (stop_tx → abort consumer → await handle 10s)
    M-->>E: monitor stopped
    E->>E: sleep 500ms (trace pipe flush)
    E->>T: abort trace_handle
    E->>T: drop trace_tx (closes channel)
    E->>FS: await streaming_handle (CLEANUP_TIMEOUT_SECS)
    FS-->>E: trace_events.jsonl finalized

    %% ─── PHASE 7: COLLECT TELEMETRY ───
    Note over E,FS: Phase 7 — Collect Telemetry (5 sources)
    E->>RE: collect_all(job_id)
    RE->>RE: GET /api/logs/rededr
    RE-->>E: Vec TelemetryData (ETW events)
    E->>FS: package_trace_log(trace_events.jsonl)
    FS->>FS: deduplicate by (file,line,func) ~95% reduction
    FS->>FS: if >3.5MB: progressive tail truncation (binary search halving)
    FS-->>E: trace_log event
    E->>FS: collect_trace_log_binary(trace.log)
    FS-->>E: binary trace events
    E->>FS: collect_bb_coverage(coverage_bbs.txt)
    FS-->>E: CoverageEvent
    E->>FS: collect_api_checkpoints(checkpoints.log)
    FS-->>E: Vec CheckpointEvent

    %% ─── PHASE 7B: CLASSIFY ───
    Note over E: Phase 7b — Classify Detection Outcome
    E->>E: extract_evidence (scan checkpoints for has_launched, last_checkpoint)
    E->>E: classify_outcome (11-step decision tree)
    E->>E: detection_verdict + last_checkpoint
    E->>E: add phase_timings as telemetry event

    %% ─── PHASE 8: STREAM TELEMETRY ───
    Note over E,C: Phase 8 — Stream Telemetry to Controller
    E->>S: send_telemetry(TelemetryBatch is_final=true)
    S->>C: WorkerMessage Telemetry (or dropped by NullSink)

    %% ─── PHASE 9: RESET REDEDR ───
    Note over E,RE: Phase 9 — Reset RedEDR
    E->>RE: reset_now()
    RE->>RE: POST /api/trace/reset
    Note over E,RE: RedEdrGuard disarmed (reset_on_drop=false)

    %% ─── PHASE 10: CLEANUP ───
    Note over E,FS: Phase 10 — Cleanup
    E->>FS: cleanup_run_artifacts (remove artifact.exe + telemetry_dir)

    E-->>E: Return RunOutcome

    %% ─── RESPONSE ───
    alt Stream path response
        E-->>SH: RunOutcome
        SH->>SH: format_output() + sample_response_ok()
        SH->>C: WorkerMessage SampleResponse via tx
        SH->>SH: Clear WorkerState (job_id=None, run_id=None)
    else Unary path response
        E-->>U: RunOutcome
        U->>U: format_output() + sample_response_ok()
        U-->>C: Response SampleResponse
    end

    Note over L: ExecutionLockGuard Drop → tokio::spawn state.release() → Idle
```

**Key differences between paths:**

| Aspect | Unary RPC (api/run.rs) | Stream Command (stream_handler.rs) |
|--------|----------------------|-----------------------------------|
| Blocking | Blocks gRPC call until completion | Spawns async task, returns immediately |
| Run ID source | worker_state.current_run_id or UUID | request_id from RunSampleCommand |
| Response delivery | gRPC Response return value | WorkerMessage via tx channel |
| Worker state | Not updated | Sets/clears current_job_id and current_run_id |
| Ack | None | Immediate Ack before execution |
| Error handling | Returns gRPC Status error | Sends error SampleResponse via stream |

---

## 5. Execution Pipeline — 10 Phases

The core of the worker agent: the `execute_run()` function's 10-phase pipeline.

```mermaid
flowchart TD
    Start([execute_run called]) --> P1

    P1["Phase 1: Validate<br/>─────────────<br/>Check artifact_path.exists()"]
    P1 -->|"missing"| E1[/"RunError::ArtifactNotFound"/]
    P1 -->|"exists"| P2

    P2["Phase 2: RedEDR Setup<br/>─────────────<br/>Create RedEdrCollector<br/>Create RedEdrGuard (RAII)<br/>Sanity check: collect_all()"]
    P2 -->|"contaminated +<br/>strict_mode"| E2[/"RunError::FailedPrecondition"/]
    P2 -->|"clean or lenient"| P2b["start_trace([artifact.exe])"]
    P2b --> P3

    P3["Phase 3: Environment<br/>─────────────<br/>prepare_telemetry_dir()<br/>Create TraceCollector (named pipe)<br/>Start streaming writer (BufWriter)"]
    P3 --> P4

    P4["Phase 4: Spawn<br/>─────────────<br/>spawn_artifact(path, telemetry_dir)<br/>Create ProcessGuard (RAII)<br/>Extract PID"]
    P4 -->|"spawn failed"| E4[/"RunError::ProcessSpawnFailed"/]
    P4 -->|"success"| P5

    P5["Phase 5: Monitor<br/>─────────────<br/>capture_stream(stdout)<br/>capture_stream(stderr)<br/>Create ExecutionMonitor<br/>Create MonitorGuard (RAII)<br/>Every 3s: alive? CPU? events?"]
    P5 --> P6

    P6["Phase 6: Wait<br/>─────────────<br/>timeout(duration, process.wait())"]
    P6 -->|"normal exit"| P6a["exit_code = code or -2"]
    P6 -->|"timeout"| P6b["try_wait() race check<br/>├ exited → real code<br/>└ alive → kill_tree, -3"]
    P6 -->|"wait failed"| P6c["exit_code = -1"]
    P6a & P6b & P6c --> P6d["ProcessGuard.disarm()"]
    P6d --> P7

    P7["Phase 7: Collect<br/>─────────────<br/>Stop monitor, drain trace pipe<br/>├ RedEDR collect_all() (HTTP)<br/>├ package_trace_log() (JSONL)<br/>├ collect_trace_log_binary()<br/>├ collect_bb_coverage()<br/>└ collect_api_checkpoints()"]
    P7 --> P7b

    P7b["Phase 7b: Classify<br/>─────────────<br/>classifier::classify_run()<br/>→ DetectionVerdict (7 options)"]
    P7b --> P8

    P8["Phase 8: Stream<br/>─────────────<br/>TelemetryBatch (is_final=true)<br/>sink.send_telemetry()<br/>├ StreamSink → controller<br/>└ NullSink → /dev/null"]
    P8 --> P9

    P9["Phase 9: Reset RedEDR<br/>─────────────<br/>rededr_guard.reset_now()<br/>Disarm guard (no-op on Drop)"]
    P9 --> P10

    P10["Phase 10: Cleanup<br/>─────────────<br/>remove_file(artifact.exe)<br/>remove_dir_all(telemetry_dir)<br/>(non-fatal on failure)"]
    P10 --> Done(["Return RunOutcome"])

    style P1 fill:#1e3a5f,color:#fff
    style P2 fill:#1e3a5f,color:#fff
    style P3 fill:#1e3a5f,color:#fff
    style P4 fill:#1e3a5f,color:#fff
    style P5 fill:#1e3a5f,color:#fff
    style P6 fill:#1e3a5f,color:#fff
    style P7 fill:#1e3a5f,color:#fff
    style P7b fill:#7c3aed,color:#fff
    style P8 fill:#1e3a5f,color:#fff
    style P9 fill:#1e3a5f,color:#fff
    style P10 fill:#1e3a5f,color:#fff
    style E1 fill:#dc2626,color:#fff
    style E2 fill:#dc2626,color:#fff
    style E4 fill:#dc2626,color:#fff
```

---

## 6. Detection Classifier — 11-Step Decision Tree

The `classify_run()` function that produces one of 7 verdicts from local signals.

```mermaid
flowchart TD
    Start(["classify_run(exit_code, timed_out, events)"])
    Start --> Extract["Extract evidence from events:<br/>has_launched? last_checkpoint?"]
    Extract --> S1

    S1{"exit_code == -4<br/>(EXIT_INFRA)?"}
    S1 -->|yes| InfraError1["InfraError ❌"]

    S1 -->|no| S2{"exit_code == -1<br/>(EXIT_WAIT_FAILED)?"}
    S2 -->|yes| InfraError2["InfraError ❌"]

    S2 -->|no| S3{"exit_code in [10,20)<br/>(guardrail codes)?"}
    S3 -->|yes| InfraError3["InfraError ❌"]

    S3 -->|no| S4{"exit_code == 0?"}
    S4 -->|yes| Evasion1["Evasion ✅"]

    S4 -->|no| S5{"timed_out?"}
    S5 -->|yes| S5b{"has_launched?"}
    S5b -->|yes| Evasion2["Evasion ✅"]
    S5b -->|no| Stalled["Stalled ⏳"]

    S5 -->|no| S5c{"exit_code == -3<br/>(EXIT_TIMEOUT)?"}
    S5c -->|yes| S5d{"has_launched?"}
    S5d -->|yes| Evasion3["Evasion ✅"]
    S5d -->|no| Stalled2["Stalled ⏳"]

    S5c -->|no| S7{"exit_code == -2<br/>(EXIT_NO_CODE)?"}
    S7 -->|yes| Detected1["Detected 🚨"]

    S7 -->|no| S8{"NTSTATUS =<br/>0xC0000906 or<br/>0xC0000907?"}
    S8 -->|yes| Detected2["Detected 🚨"]

    S8 -->|no| S9{"Crash NTSTATUS?<br/>0xC0000005, 0xC0000409,<br/>0xC00000FD, 0xC0000374,<br/>0xC0000094"}
    S9 -->|yes| Ambiguous1["Ambiguous ⚠️"]

    S9 -->|no| S10{"exit_code in [30,40)<br/>(carrier codes)?"}
    S10 -->|yes| Ambiguous2["Ambiguous ⚠️"]

    S10 -->|no| Ambiguous3["Ambiguous ⚠️<br/>(other nonzero)"]

    style Evasion1 fill:#15803d,color:#fff
    style Evasion2 fill:#15803d,color:#fff
    style Evasion3 fill:#15803d,color:#fff
    style Detected1 fill:#dc2626,color:#fff
    style Detected2 fill:#dc2626,color:#fff
    style Ambiguous1 fill:#d97706,color:#fff
    style Ambiguous2 fill:#d97706,color:#fff
    style Ambiguous3 fill:#d97706,color:#fff
    style Stalled fill:#6b7280,color:#fff
    style Stalled2 fill:#6b7280,color:#fff
    style InfraError1 fill:#4b5563,color:#fff
    style InfraError2 fill:#4b5563,color:#fff
    style InfraError3 fill:#4b5563,color:#fff
```

---

## 7. Telemetry Architecture — 6 Sources

Data flow from artifact execution through the 6 telemetry collection paths into the final `TelemetryBatch`.

```mermaid
flowchart LR
    subgraph Artifact["Artifact Execution"]
        stdout["stdout"]
        stderr["stderr"]
        cov["coverage_bbs.txt"]
        chk["checkpoints.log"]
        pipe["Named Pipe<br/>\\\\.\pipe\rededr_trace"]
        tracelog["trace.log<br/>(binary ISTR)"]
    end

    subgraph RealTime["Real-Time (during execution)"]
        rededr_poll["RedEDR HTTP poll<br/>GET /api/logs/rededr<br/>every 1000ms"]
        trace_rx["Trace Pipe Server<br/>binary or Base64<br/>auto-detect first 4 bytes"]
    end

    subgraph Batch["Batch (Phase 7, after execution)"]
        pkg_trace["package_trace_log()<br/>dedup by (file,line,func)<br/>95%+ reduction"]
        parse_bin["collect_trace_log_binary()<br/>parse ISTR records"]
        parse_cov["collect_bb_coverage()<br/>parse coverage_bbs.txt"]
        parse_chk["collect_api_checkpoints()<br/>parse JSON lines"]
        rededr_batch["RedEDR collect_all()<br/>(second pass, full batch)"]
    end

    subgraph Pipeline["Telemetry Pipeline"]
        vec["Vec&lt;TelemetryData&gt;"]
        trunc{"size > 3.5MB?"}
        tail["Progressive tail truncation<br/>binary-search halving<br/>keep most recent lines"]
        batch_msg["TelemetryBatch<br/>{job_id, run_id,<br/>events[], is_final: true}"]
    end

    subgraph Sink["Transport"]
        stream_sink["StreamSink<br/>→ gRPC stream<br/>→ controller"]
        null_sink["NullSink<br/>→ /dev/null<br/>(no stream)"]
    end

    pipe --> trace_rx
    trace_rx -->|"mpsc channel<br/>100K capacity"| disk["trace_events.jsonl"]

    rededr_poll -->|"mpsc → gRPC stream"| stream_sink

    disk --> pkg_trace
    tracelog --> parse_bin
    cov --> parse_cov
    chk --> parse_chk

    pkg_trace --> vec
    parse_bin --> vec
    parse_cov --> vec
    parse_chk --> vec
    rededr_batch --> vec

    vec --> trunc
    trunc -->|"yes"| tail --> batch_msg
    trunc -->|"no"| batch_msg

    batch_msg -->|"sink.send_telemetry()"| stream_sink
    batch_msg -->|"sink.send_telemetry()"| null_sink

    style RealTime fill:#1e3a5f,stroke:#3b82f6,color:#fff
    style Batch fill:#4a2c17,stroke:#f59e0b,color:#fff
    style Pipeline fill:#2d1b69,stroke:#8b5cf6,color:#fff
```

---

## 8. Communication Architecture — Unary vs Bidirectional Stream

Two coexisting gRPC communication models sharing the same execution engine.

```mermaid
flowchart TB
    subgraph Unary["Phase 1 — Unary RPCs"]
        direction LR
        C1["Controller"] -->|"RunSample"| W1["Worker"]
        W1 -->|"SampleResponse"| C1
        C2["Controller"] -->|"SendArtifact (stream)"| W2["Worker"]
        W2 -->|"TransferAck"| C2
        C3["Controller"] -->|"HealthCheck"| W3["Worker"]
        W3 -->|"HealthResponse"| C3
        C4["Controller"] -->|"GetTelemetry"| W4["Worker"]
        W4 ==>|"TelemetryData (server stream)"| C4
    end

    subgraph Bidi["Phase 2 — Bidirectional Stream"]
        direction TB
        subgraph Inbound["ControllerMessage (inbound)"]
            cm_run["RunSample"]
            cm_health["HealthCheck"]
            cm_hb["Heartbeat"]
            cm_disc["Disconnect"]
            cm_ack["Ack"]
            cm_art["ArtifactChunks"]
        end
        subgraph Outbound["WorkerMessage (outbound)"]
            wm_reg["Registration"]
            wm_status["Status"]
            wm_ack["Ack"]
            wm_sample["SampleResponse"]
            wm_telem["Telemetry"]
            wm_exec["ExecutionStatus"]
        end
    end

    subgraph Shared["Shared Infrastructure"]
        lock["execution_lock<br/>Arc&lt;Mutex&lt;ExecutionState&gt;&gt;"]
        engine["execute_run()<br/>10-phase pipeline"]
        classifier["classify_run()<br/>7-verdict tree"]
        sink_trait["ControlPlaneSink trait"]
    end

    Unary -->|"api/run.rs"| Shared
    Bidi -->|"stream_handler.rs"| Shared

    sink_trait -.->|"StreamSink"| Bidi
    sink_trait -.->|"NullSink"| Unary

    style Unary fill:#1e3a5f,stroke:#3b82f6,color:#fff
    style Bidi fill:#0f3460,stroke:#e94560,color:#fff
    style Shared fill:#4a2c17,stroke:#f59e0b,color:#fff
```

---

## 9. Stream Handler Lifecycle

How the bidirectional gRPC stream session is established, used, and replaced on reconnection.

```mermaid
flowchart TD
    Start(["Worker starts"]) --> NoStream

    NoStream["NO STREAM<br/>stream_handler = None<br/><i>waiting for controller</i>"]

    NoStream -->|"EstablishStream RPC"| Create

    Create["Create NEW StreamHandler<br/>├ WorkerState - Arc RwLock<br/>├ tx channel - mpsc cap=100<br/>├ Send Registration message<br/>└ Spawn heartbeat_loop"]
    Create --> Active

    subgraph Active["ACTIVE STREAM"]
        direction LR
        MsgLoop["handle_stream() loop<br/>─────────────────<br/>Process inbound<br/>ControllerMessages<br/>RunSample, HealthCheck,<br/>Heartbeat, Disconnect, Ack"]
        HeartbeatLoop["heartbeat_loop()<br/>─────────────────<br/>Every 30s: send<br/>WorkerMessage Heartbeat<br/>via tx channel"]
    end

    Active -->|"stream drops / error"| Cleanup
    Cleanup["StreamHandler dropped<br/>heartbeat_handle aborted<br/>worker_state persists in Arc"]
    Cleanup --> NoStream

    Active -->|"NEW EstablishStream<br/>(reconnection)"| Replace
    Replace["Reconnection replaces old:<br/>1. Old heartbeat_handle.abort()<br/>2. Create NEW StreamHandler<br/>3. Store in stream_handler Arc<br/>4. Old StreamHandler dropped<br/><i>NOT a state transition —<br/>a fresh instance</i>"]
    Replace --> Active

    style NoStream fill:#4b5563,color:#fff
    style Active fill:#1e3a5f,stroke:#3b82f6,color:#fff
    style Create fill:#15803d,color:#fff
    style Cleanup fill:#dc2626,color:#fff
    style Replace fill:#d97706,color:#fff
```

---

## 10. RAII Guard System

Three guard types that guarantee resource cleanup on all exit paths, including panics.

```mermaid
flowchart TB
    subgraph Scope["execute_run() scope"]
        direction TB

        rg["RedEdrGuard<br/>─────────<br/>collector: RedEdrCollector<br/>reset_on_drop: bool"]
        pg["ProcessGuard<br/>─────────<br/>child: Option&lt;Child&gt;<br/>should_kill: bool"]
        mg["MonitorGuard<br/>─────────<br/>stop_tx: Option&lt;Sender&gt;<br/>handle: Option&lt;JoinHandle&gt;<br/>event_consumer: Option&lt;JoinHandle&gt;"]
    end

    subgraph CallerScope["Caller scope (api/run.rs or stream_handler.rs)"]
        elg["ExecutionLockGuard<br/>─────────<br/>state: Arc&lt;Mutex&lt;ExecutionState&gt;&gt;<br/>Drop → tokio::spawn → state.release()"]
    end

    rg -->|"Normal: Phase 9"| rg_normal["reset_now()<br/>POST /api/trace/reset<br/>set reset_on_drop=false"]
    rg -->|"Drop (panic/error)"| rg_drop["fire-and-forget<br/>tokio::spawn reset<br/>(best-effort)"]

    pg -->|"Normal: Phase 6"| pg_normal["disarm()<br/>takes Child ownership<br/>set should_kill=false"]
    pg -->|"Drop (panic/error)"| pg_drop["start_kill()<br/>synchronous signal<br/>(no runtime needed)"]

    mg -->|"Normal: Phase 7"| mg_normal["stop()<br/>send stop_tx → abort consumer<br/>→ await monitor (10s timeout)"]
    mg -->|"Drop (panic/error)"| mg_drop["send stop_tx<br/>abort consumer<br/>(no await — Drop is sync)"]

    elg -->|"Normal: function return"| elg_normal["Drop runs<br/>tokio::spawn → state.release()<br/>ExecutionState → Idle"]

    style rg fill:#7c3aed,color:#fff
    style pg fill:#2563eb,color:#fff
    style mg fill:#059669,color:#fff
    style elg fill:#d97706,color:#fff
    style rg_normal fill:#15803d,color:#fff
    style pg_normal fill:#15803d,color:#fff
    style mg_normal fill:#15803d,color:#fff
    style elg_normal fill:#15803d,color:#fff
    style rg_drop fill:#dc2626,color:#fff
    style pg_drop fill:#dc2626,color:#fff
    style mg_drop fill:#dc2626,color:#fff
```

---

## 11. Shared State & Concurrency Model

How the `WorkerAgentService` struct's shared state is accessed by concurrent tasks.

```mermaid
flowchart TB
    subgraph WAS["WorkerAgentService (#[derive(Clone)])"]
        direction TB
        wid["worker_id: String<br/><i>immutable</i>"]
        cfg["config: WorkerConfig<br/><i>immutable (Clone)</i>"]
        si["system_info<br/>Arc&lt;Mutex&lt;System&gt;&gt;"]
        el["execution_lock<br/>Arc&lt;Mutex&lt;ExecutionState&gt;&gt;"]
        sh["stream_handler<br/>Arc&lt;RwLock&lt;Option&lt;<br/>Arc&lt;StreamHandler&gt;&gt;&gt;&gt;"]
        hh["heartbeat_handle<br/>Arc&lt;RwLock&lt;Option&lt;<br/>JoinHandle&lt;()&gt;&gt;&gt;&gt;"]
        cap["capabilities<br/>Arc&lt;WorkerCapabilities&gt;<br/><i>immutable after startup</i>"]
    end

    api_run["api/run.rs"]
    api_info["api/info.rs"]
    api_stream["api/stream.rs"]
    stream_h["stream_handler.rs"]
    heartbeat["heartbeat_loop()"]

    api_run -->|"lock"| el
    api_run -->|"read"| sh
    api_info -->|"lock"| si
    api_info -->|"read"| cap
    api_stream -->|"write"| sh
    api_stream -->|"write"| hh
    api_stream -->|"clone"| el
    stream_h -->|"lock"| el
    heartbeat -->|"read"| sh

    subgraph CyclePrevention["Arc-Cycle Prevention"]
        direction LR
        no_cycle["StreamHandler clones worker_id, config<br/>shares execution_lock by Arc<br/>does NOT hold Arc&lt;WorkerAgentService&gt;"]
    end

    style WAS fill:#1e3a5f,stroke:#3b82f6,color:#fff
    style el fill:#dc2626,color:#fff
    style sh fill:#d97706,color:#fff
    style CyclePrevention fill:#2d1b69,stroke:#8b5cf6,color:#fff
```

---

## 12. Execution Lock State Machine

The single-execution guarantee enforced by the `ExecutionState` enum.

```mermaid
flowchart LR
    Init(["Worker starts"]) --> Idle

    Idle["IDLE<br/>ExecutionState Idle"]

    Idle -->|"acquire() succeeds<br/>ExecutionLockGuard created"| Running

    Running["RUNNING<br/>ExecutionState Running<br/>─────────────────<br/>job_id: String<br/>artifact: String<br/>run_id: String"]

    Running -->|"ExecutionLockGuard Drop<br/>tokio spawn state.release()"| Idle

    Reject(["Second request while Running<br/>→ RESOURCE_EXHAUSTED"]) ~~~ Running

    Note1["Invariant: at most ONE artifact<br/>executing at any time.<br/>Enforcement: Arc Mutex ExecutionState<br/>Both unary and stream paths<br/>share the same lock."]

    style Idle fill:#15803d,color:#fff
    style Running fill:#dc2626,color:#fff
    style Reject fill:#4b5563,color:#fff
    style Note1 fill:#1e3a5f,stroke:#3b82f6,color:#fff
```

---

## 13. Execution Monitor Poll Loop

The `ExecutionMonitor` that runs every 3 seconds during artifact execution.

```mermaid
flowchart TD
    Start(["monitor.start()"])
    Start --> Loop

    Loop["Sleep 3 seconds<br/>(MONITOR_POLL_INTERVAL_SECS)"]
    Loop --> CheckStop{"stop_rx<br/>received?"}
    CheckStop -->|"yes"| Done(["Exit monitor"])

    CheckStop -->|"no"| CheckAlive{"is_process_alive(pid)?"}
    CheckAlive -->|"dead"| Terminated["Emit TERMINATED status"]
    Terminated --> Done

    CheckAlive -->|"alive"| Collect["Collect status:<br/>├ per-PID CPU/memory (sysinfo)<br/>├ elapsed time<br/>├ RedEDR event count (HTTP /api/stats)<br/>└ delta since last poll"]

    Collect --> IdleCheck{"No new events<br/>AND cpu < 5%?"}
    IdleCheck -->|"yes"| IncIdle["idle_count++"]
    IdleCheck -->|"no"| ResetIdle["idle_count = 0"]

    IncIdle --> IdleThreshold{"idle_count >= 3?"}
    IdleThreshold -->|"yes"| TelIdle["Set telemetry_idle = true"]
    IdleThreshold -->|"no"| TimeoutCheck

    TelIdle --> TimeoutCheck
    ResetIdle --> TimeoutCheck

    TimeoutCheck{"Within 5s of<br/>timeout?"}
    TimeoutCheck -->|"yes"| Approach["Set approaching_timeout = true"]
    TimeoutCheck -->|"no"| SendStatus

    Approach --> SendStatus["Send ExecutionStatusReport<br/>├ sink.send_status() → controller<br/>└ event_tx → local consumer"]

    SendStatus --> Loop

    style Loop fill:#1e3a5f,color:#fff
    style Collect fill:#4a2c17,stroke:#f59e0b,color:#fff
    style Terminated fill:#dc2626,color:#fff
    style TelIdle fill:#6b7280,color:#fff
    style Approach fill:#d97706,color:#fff
```

---

## 14. Artifact Transfer Flow

Chunked PE binary transfer with SHA-256 integrity verification.

```mermaid
sequenceDiagram
    participant C as Controller
    participant W as Worker (api/artifacts.rs)
    participant FS as Filesystem

    C->>W: SendArtifact (client-streaming)
    C->>W: chunk_0 {artifact_id, sha256, data, index=0}
    C->>W: chunk_1 {data, index=1}
    C->>W: chunk_2 {data, index=2}
    C->>W: ... (4MB chunks)
    C--xW: stream closes

    Note over W: Sort chunks by chunk_index
    Note over W: Reassemble bytes in order
    Note over W: Compute SHA-256 of reassembled data

    alt SHA-256 matches
        W->>FS: Write {artifacts_path}/{artifact_id}.exe
        W->>C: TransferAck {received: true, path, chunks: N}
    else SHA-256 mismatch
        W->>C: Error: integrity check failed
    end
```

---

## 15. Capability Detection at Startup

Environment probing that runs once at startup and is cached for the process lifetime.

```mermaid
flowchart TD
    Start(["detect_capabilities()"])

    Start --> R["RedEDR<br/>HTTP GET localhost:8081/api/stats"]
    R -->|"200 OK"| RV["Get version:<br/>GET /api/logs/agent<br/>regex RedEdr (\\d+)"]
    R -->|"failed"| NoR["RedEDR not present"]

    Start --> D["Defender<br/>sc query WinDefend"]
    D -->|"RUNNING"| DV["Get version:<br/>PowerShell<br/>Get-MpComputerStatus"]
    D -->|"not running"| NoD["Defender not active"]

    Start --> M["MDE<br/>Registry HKLM\\...\\<br/>Windows Advanced<br/>Threat Protection\\<br/>OnboardedInfo"]
    M -->|"non-empty"| YesM["MDE onboarded"]
    M -->|"empty/missing"| NoM["No MDE"]

    Start --> X["Cortex XDR<br/>├ Registry CyveraService<br/>├ C:\\ProgramData\\Cyvera<br/>└ C:\\Program Files\\<br/>  Palo Alto\\Traps"]
    X -->|"any found"| YesX["Cortex present"]
    X -->|"none"| NoX["No Cortex"]

    Start --> OS["OS + Hardware<br/>├ Registry CurrentBuildNumber<br/>├ Build ≥ 22000 → Win11<br/>├ available_parallelism() → cores<br/>└ sysinfo total_memory → RAM"]

    RV & NoR & DV & NoD & YesM & NoM & YesX & NoX & OS --> Result

    Result["WorkerCapabilities<br/>├ capabilities: Vec&lt;String&gt;<br/>│ ['rededr', 'mde', ...]<br/>├ tools: HashMap<br/>│ {rededr_version, defender_version}<br/>└ metadata: HashMap<br/>  {hostname, cpu_cores, ram_gb, os_key}"]

    Extra["config.worker.extra_capabilities<br/>e.g. ['dryrun']"] -->|"merge"| Result

    style Start fill:#7c3aed,color:#fff
    style Result fill:#15803d,color:#fff
```

---

## 16. Dryrun Path — Lightweight 6-Phase Execution

Simplified execution for ground-truth behavior on clean VMs (no instrumentation, no monitoring).

```mermaid
flowchart LR
    P1["Phase 1<br/>Validate<br/>─────<br/>artifact<br/>exists?"]
    P2["Phase 2<br/>Spawn<br/>─────<br/>spawn_artifact()<br/>no RedEDR<br/>no trace pipe"]
    P3["Phase 3<br/>Wait<br/>─────<br/>timeout + wait()<br/>no monitor<br/>no capture"]
    P4["Phase 4<br/>Classify<br/>─────<br/>classify_run()<br/>empty telemetry"]
    P5["Phase 5<br/>Cleanup<br/>─────<br/>remove<br/>artifact file"]
    P6["Phase 6<br/>Return<br/>─────<br/>RunOutcome"]

    P1 --> P2 --> P3 --> P4 --> P5 --> P6

    style P1 fill:#1e3a5f,color:#fff
    style P2 fill:#1e3a5f,color:#fff
    style P3 fill:#1e3a5f,color:#fff
    style P4 fill:#7c3aed,color:#fff
    style P5 fill:#4a2c17,color:#fff
    style P6 fill:#15803d,color:#fff
```

---

## 17. ControlPlaneSink — Strategy Pattern

Transport abstraction that decouples the execution engine from gRPC.

```mermaid
classDiagram
    class ControlPlaneSink {
        <<trait>>
        +send_status(ExecutionStatusReport) Result
        +send_telemetry(TelemetryBatch) Result
        +send_ack(request_id, success, error) Result
    }

    class StreamSink {
        -tx: mpsc::Sender~Result~WorkerMessage, Status~~
        +send_status() wraps in WorkerMessage::Status
        +send_telemetry() wraps in WorkerMessage::Telemetry
        +send_ack() wraps in WorkerMessage::Ack
    }

    class NullSink {
        +send_status() Ok(()) silently
        +send_telemetry() Ok(()) silently
        +send_ack() Ok(()) silently
    }

    ControlPlaneSink <|.. StreamSink : implements
    ControlPlaneSink <|.. NullSink : implements

    class build_sink {
        <<factory>>
        +build_sink(Option~Sender~) Arc~dyn ControlPlaneSink~
    }

    build_sink --> StreamSink : "Some(tx)"
    build_sink --> NullSink : "None"

    note for StreamSink "Used when bidirectional\nstream is active"
    note for NullSink "Used for unary RPCs\nwithout active stream"
```

---

## 18. Trace Collector — Protocol Auto-Detection

Named pipe server that accepts line traces from the instrumented artifact, supporting two wire formats.

```mermaid
flowchart TD
    Artifact["Artifact Process<br/>writes to \\\\.\pipe\\rededr_trace"]
    Artifact --> PipeServer["TraceCollector pipe server<br/>tokio named pipe"]

    PipeServer --> Peek["Read first 4 bytes"]

    Peek -->|"0x49535452 (ISTR)"| Binary["Binary Protocol"]
    Peek -->|"other"| Text["Base64 Text Protocol"]

    subgraph Binary["Binary Protocol"]
        direction TB
        header["InstRecordHeader (32 bytes)<br/>├ magic: u32 (0x49535452)<br/>├ version: u16<br/>├ event_type: u16<br/>│ (1=line_trace, 2-4=checkpoint)<br/>├ thread_id: u32<br/>├ seq_no: u64<br/>├ ts_us: u64<br/>└ payload_len: u32"]
        payload["Payload: UTF-8<br/>file:line:func"]
        header --> payload
    end

    subgraph Text["Base64 Text Protocol"]
        direction TB
        fmt1["b64line:&lt;base64&gt;<br/><i>(old IR format)</i>"]
        fmt2["YjY0&lt;base64&gt;<br/><i>(new AST format)</i><br/><i>YjY0 = Base64('b64')</i>"]
        decoded["Decoded: line:file.c:42:main"]
        fmt1 --> decoded
        fmt2 --> decoded
    end

    Binary --> Channel["event_tx: mpsc::Sender&lt;TraceEvent&gt;<br/>capacity: 100,000"]
    Text --> Channel

    Channel --> JSONL["trace_events.jsonl<br/>(via streaming BufWriter 256KB)"]

    style Binary fill:#1e3a5f,color:#fff
    style Text fill:#4a2c17,color:#fff
    style Channel fill:#7c3aed,color:#fff
```

---

## 19. Concurrency — Task Tree for One Execution

All async tasks spawned during a single `execute_run()` invocation.

```mermaid
flowchart TD
    Runtime["Tokio Runtime (main)"]

    Runtime --> GRPC["gRPC Server task (tonic)<br/>WorkerAgent trait handlers"]
    Runtime --> Stream["Stream handler task<br/>handle_stream() message loop<br/><i>spawned per EstablishStream</i>"]
    Runtime --> HB["Heartbeat task<br/>heartbeat_loop() every 30s<br/><i>spawned per EstablishStream</i>"]

    subgraph ExecTasks["Per-execution tasks (spawned in engine)"]
        trace_h["trace_handle<br/>TraceCollector.start_server()<br/>(named pipe)"]
        stream_h["streaming_handle<br/>BufWriter → trace_events.jsonl"]
        stdout_h["stdout_handle<br/>capture stdout → String"]
        stderr_h["stderr_handle<br/>capture stderr → String"]
        monitor_h["monitor_handle<br/>ExecutionMonitor.start()<br/>(3s poll loop)"]
        event_c["event_consumer<br/>Log monitor events"]
    end

    Stream -->|"RunSample command"| ExecTasks
    GRPC -->|"RunSample RPC"| ExecTasks

    style Runtime fill:#2d1b69,stroke:#8b5cf6,color:#fff
    style ExecTasks fill:#1e3a5f,stroke:#3b82f6,color:#fff
```

---

## 20. Two-Run Differential Protocol

How the worker agent supports the two-run differential that distinguishes real detections from instrumentation artifacts.

```mermaid
flowchart LR
    subgraph Round["One Mutation Round"]
        direction TB
        RunA["Run A<br/>--trace=lines<br/><i>execution path +<br/>truncation localization</i>"]
        RunB["Run B<br/>--trace=off<br/><i>ground-truth<br/>EDR behavior</i>"]
    end

    RunA --> Compare
    RunB --> Compare

    Compare{"Compare Outcomes"}

    Compare -->|"A: Detected<br/>B: Detected"| Real["Real Detection<br/><i>used for learning</i>"]
    Compare -->|"A: Detected<br/>B: Not detected"| Artifact["Instrumentation Artifact<br/><i>discarded</i>"]
    Compare -->|"A: Not detected<br/>B: Not detected"| Evasion["Full Evasion<br/><i>mutation succeeded</i>"]

    Real --> Tokens["Token Extraction<br/>api:VirtualProtect<br/>seq3:alloc→write→thread<br/>trunc:loader.c:143"]
    Tokens --> Feedback["Mutation Feedback<br/>avoid / seek tokens"]

    style RunA fill:#1e3a5f,color:#fff
    style RunB fill:#4a2c17,color:#fff
    style Real fill:#dc2626,color:#fff
    style Artifact fill:#6b7280,color:#fff
    style Evasion fill:#15803d,color:#fff
```
