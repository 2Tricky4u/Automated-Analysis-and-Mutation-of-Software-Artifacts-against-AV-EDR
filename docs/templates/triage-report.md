# Triage Hypothesis Report

**Run ID:** [uuid]
**Artifact ID:** [sha256]
**Worker ID:** [worker-NN]
**Date:** [YYYY-MM-DD HH:MM:SS UTC]

## Run Summary

- **Status:** [detected | not_detected | noisy | crash]
- **Telemetry Seen:** [true/false]
- **Alert Level:** [none | low | med | high]
- **Blocked:** [true/false]
- **Detection Latency:** [NNN ms]

## Performance Metrics

- **CPU:** [N.N%]
- **Memory:** [NNN MB]
- **Event-to-Record (p95):** [NNN ms]

## Hypotheses

| Rank | Hypothesis | Evidence (fields) | Confidence | Avoid/Seek |
|------|-------------|------------------|-------------|-------------|
| 1 | Flagged due to short RWX window + thread start in anonymous region | mem.write_to_execute_ms<15, thread.start.anon=true | 0.82 | Avoid |
| 2 | Provenance risk from unsigned child under trusted parent | proc.parent.signed=true, child.signed=false | 0.66 | Avoid |
| 3 | Direct syscall pattern detected | callstack contains ntdll!Nt* | 0.54 | Avoid |

## Feature-Avoid List

For next mutation round, avoid these features:

- `mem.rwx_short_window` (confidence: 0.82)
- `thread.start.anon` (confidence: 0.82)
- `proc.parent.unsigned` (confidence: 0.66)
- `syscall.direct` (confidence: 0.54)

## Recommended Mutations

Based on analysis, consider:

1. **Increase RWX window timing**
   ```yaml
   - id: beh.timing
     params:
       min_delay_ms: 500  # Increase from 100
   ```

2. **Use named memory sections**
   ```yaml
   - id: ast.memory_backing
     params:
       use_file_backed: true
   ```

3. **Sign child process or use signed parent**
   ```yaml
   - id: beh.provenance
     params:
       parent_process: "C:\\Windows\\explorer.exe"
   ```

## Next Actions

- [ ] Apply recommended mutations
- [ ] Re-run with new seed
- [ ] Verify evasion improvement
- [ ] Update corpus with successful mutations

## Telemetry Summary

- **Total Events:** [NNNN]
- **Process Events:** [NNN]
- **Memory Events:** [NNN]
- **Network Events:** [NNN]
- **Callstack Samples:** [NNN]

## Notes

[Free-form analysis, observations, manual verification results]
