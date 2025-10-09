# Experiment Plan

**Date:** [YYYY-MM-DD]
**Experiment ID:** [exp-NNNN]
**Author:** [Name]

## Objectives

### Primary Objective
- [Main goal of this experiment]

### Secondary Objectives
- [Additional goals]
- [Metrics to collect]

## Inputs

### Artifact
- **Type:** [exe, dll, shellcode, script]
- **Source:** [path or description]
- **Base Hash (SHA-256):** [hash]

### Mutations
```yaml
mutations:
  - id: ast.import_reshape
    params:
      delay_load: true
  - id: beh.preamble.fs
    params:
      fs_ops: 3
```

### Deterministic Seed
- **Seed:** `[hexstring or integer]`

## Method

1. **Baseline Run**
   - Execute unmutated artifact on baseline VM
   - Collect full telemetry

2. **Mutation Application**
   - Apply mutations with recorded seed
   - Build artifact deterministically

3. **Execution**
   - Run on Defender VM
   - Monitor for detection

4. **Differential Analysis**
   - Compare baseline vs mutated telemetry
   - Identify changed behaviors

## Telemetry

### ETW Providers
- [ ] Microsoft-Windows-Kernel-Process
- [ ] Microsoft-Windows-Kernel-Audit-API-Calls
- [ ] Microsoft-Windows-TCPIP
- [ ] [Additional providers]

### RedEDR Channels
- [ ] ETW events
- [ ] Kernel callbacks
- [ ] Memory protection changes
- [ ] Callstack analysis

### Expected Fields
- `mem.rwx_short_window`: boolean
- `thread.start.anon`: boolean
- `proc.parent.signed`: boolean
- `mem.write_to_execute_ms`: int
- [Additional fields]

## Labels

- **telemetry_seen:** [true/false]
- **alert_level:** [none | low | med | high]
- **blocked:** [true/false]
- **detection_latency_ms:** [integer]

## Risks

### Technical Risks
1. **Semantics breakage:** Mutations may alter intended behavior
   - **Mitigation:** Run validation harness before submission

2. **VM snapshot corruption:** Baseline may be polluted
   - **Mitigation:** Verify snapshot hash before run

### Data Quality Risks
1. **Telemetry loss:** ETW buffer overflow
   - **Mitigation:** Increase buffer size, monitor lost_events metric

## Definition of Done

- [ ] All runs completed without errors
- [ ] Data exported to Elastic indices: `rededr-*`, `etw-*`
- [ ] Hypothesis report generated
- [ ] Artifact IDs and seeds recorded
- [ ] Collector config facts documented

## Notes

[Free-form notes, observations, unexpected results]
