#!/bin/bash
# End-to-End Instrumented Test: Build → Deploy → Execute with ALL telemetry
# Tests: RedEDR syscalls + Line-level traces + API tracing + BB coverage
set -euo pipefail

echo "=== End-to-End Instrumented Telemetry Test ==="
echo ""

# Configuration
CONTROLLER_IP="localhost"
CONTROLLER_PORT="50051"
WORKER_IP="${WORKER_IP:-10.200.200.100}"
WORKER_PORT="50052"
TEMPLATE="rwx_direct"
SOURCE_FILE="rwx_direct.c"
JOB_PREFIX="instrumented-e2e"

echo "[CONFIG]"
echo "  Controller: $CONTROLLER_IP:$CONTROLLER_PORT"
echo "  Worker:     $WORKER_IP:$WORKER_PORT"
echo "  Template:   $TEMPLATE"
echo "  Source:     $SOURCE_FILE"
echo "  Instrumentation: ALL (RedEDR + Line Trace + API + BB Coverage)"
echo ""

# ============================================================================
# Step 1: Build artifact with FULL instrumentation
# ============================================================================
echo "[1/5] Building artifact with full instrumentation..."
echo "   Using trace_mode='all' (RedEDR + Line Trace + API + BB Coverage)"
echo ""

BUILD_RESPONSE=$(grpcurl -plaintext \
  -import-path controller/proto \
  -proto controller.proto \
  -d "{\"template_name\":\"$TEMPLATE\",\"source_file\":\"$SOURCE_FILE\",\"trace_mode\":\"all\"}" \
  $CONTROLLER_IP:$CONTROLLER_PORT \
  edr.controller.Controller/BuildArtifact 2>&1)

# Parse response fields
ARTIFACT_ID=$(echo "$BUILD_RESPONSE" | jq -r '.artifactId // empty' 2>/dev/null)
ARTIFACT_SIZE=$(echo "$BUILD_RESPONSE" | jq -r '.sizeBytes // empty' 2>/dev/null)
BUILD_STATUS=$(echo "$BUILD_RESPONSE" | jq -r '.buildStatus // empty' 2>/dev/null)
TRACE_MODE=$(echo "$BUILD_RESPONSE" | jq -r '.traceMode // .trace_mode // empty' 2>/dev/null)

# Check if we got valid responses
if [ -z "$ARTIFACT_ID" ] || [ -z "$BUILD_STATUS" ]; then
    echo "   ✗ ERROR: Build failed or invalid response"
    echo ""
    echo "   Response:"
    echo "$BUILD_RESPONSE"
    exit 1
fi

if [ "$BUILD_STATUS" != "success" ]; then
    echo "   ✗ ERROR: Build status is '$BUILD_STATUS'"
    exit 1
fi

echo "   ✓ Artifact built successfully"
echo "      Artifact ID: $ARTIFACT_ID"
echo "      Size:        $ARTIFACT_SIZE bytes"
echo "      Trace mode:  ${TRACE_MODE:-not reported}"
echo ""

# ============================================================================
# Step 2: Deploy artifact to Worker
# ============================================================================
echo "[2/5] Deploying artifact to worker VM..."

WORKER_ADDRESS="$WORKER_IP:$WORKER_PORT"

DEPLOY_RESPONSE=$(grpcurl -plaintext \
  -import-path controller/proto \
  -proto controller.proto \
  -d "{\"artifact_id\":\"$ARTIFACT_ID\",\"worker_address\":\"$WORKER_ADDRESS\"}" \
  $CONTROLLER_IP:$CONTROLLER_PORT \
  edr.controller.Controller/DeployArtifact 2>&1)

# Parse response fields
DEPLOY_STATUS=$(echo "$DEPLOY_RESPONSE" | jq -r '.success // empty' 2>/dev/null)
WORKER_PATH=$(echo "$DEPLOY_RESPONSE" | jq -r '.workerStoragePath // .worker_storage_path // empty' 2>/dev/null)

# Check if we got valid responses
if [ -z "$DEPLOY_STATUS" ]; then
    echo "   ✗ ERROR: Deploy failed or invalid response"
    echo ""
    echo "   Response:"
    echo "$DEPLOY_RESPONSE"
    exit 1
fi

if [ "$DEPLOY_STATUS" != "true" ]; then
    echo "   ✗ ERROR: Deployment failed"
    echo "   $(echo "$DEPLOY_RESPONSE" | jq -r '.error // "No error message"')"
    exit 1
fi

echo "   ✓ Artifact deployed to worker"
echo "      Worker:      $WORKER_ADDRESS"
echo "      Worker path: $WORKER_PATH"
echo ""

# ============================================================================
# Step 3: Execute artifact on Worker with telemetry collection
# ============================================================================
echo "[3/5] Executing instrumented artifact on worker VM..."
echo "   Telemetry channels enabled:"
echo "      - RedEDR (syscall tracing)"
echo "      - Line-level traces (named pipe)"
echo "      - API tracing (if instrumented)"
echo "      - BB coverage (if instrumented)"
echo ""

# Generate unique job ID
JOB_ID="${JOB_PREFIX}-$(date +%s)"

# Execute with timeout
EXECUTE_RESPONSE=$(grpcurl -plaintext \
  -import-path controller/proto \
  -proto worker.proto \
  -max-time 30 \
  -d "{\"job_id\":\"$JOB_ID\",\"artifact_id\":\"$ARTIFACT_ID\",\"timeout_seconds\":15,\"enable_etw\":true}" \
  $WORKER_IP:$WORKER_PORT \
  edr.worker.WorkerAgent/RunSample 2>&1)

# Parse response fields
EXEC_SUCCESS=$(echo "$EXECUTE_RESPONSE" | jq -r '.success // empty' 2>/dev/null)
EXIT_CODE=$(echo "$EXECUTE_RESPONSE" | jq -r '.exitCode // .exit_code // empty' 2>/dev/null)
OUTPUT=$(echo "$EXECUTE_RESPONSE" | jq -r '.output // empty' 2>/dev/null)
TELEMETRY_IDS=$(echo "$EXECUTE_RESPONSE" | jq -r '.telemetryIds[]? // empty' 2>/dev/null)

# Check if we got a valid response
if [ -z "$EXEC_SUCCESS" ]; then
    echo "   ⚠ Execution result unclear"
    echo ""
    echo "   Raw response:"
    echo "$EXECUTE_RESPONSE" | head -20
    echo ""
else
    if [ "$EXEC_SUCCESS" = "true" ]; then
        echo "   ✓ Artifact executed successfully"
        echo "      Exit code: $EXIT_CODE"
        echo ""
        echo "   Output (first 500 chars):"
        echo "$OUTPUT" | head -c 500
        echo ""
    else
        echo "   ⚠ Artifact execution failed or blocked"
        echo "      Exit code: $EXIT_CODE"
        echo ""
        if [ -n "$OUTPUT" ]; then
            echo "   Output:"
            echo "$OUTPUT" | head -20
        fi
    fi

    # Show telemetry IDs
    if [ -n "$TELEMETRY_IDS" ]; then
        echo ""
        echo "   Telemetry collected:"
        for run_id in $TELEMETRY_IDS; do
            echo "      Run ID: $run_id"
        done
    fi
fi
echo ""

# ============================================================================
# Step 4: Check worker logs for telemetry collection
# ============================================================================
echo "[4/5] Checking worker logs for telemetry..."
echo "   Look for these messages in worker logs:"
echo "      - 'Collected N RedEDR events'"
echo "      - '✅ Collected N line-level trace events' (if instrumented)"
echo "      - 'Total telemetry events collected: N'"
echo ""
echo "   Expected telemetry types:"
echo "      - event_type='rededr' (syscalls from RedEDR)"
echo "      - event_type='trace' (line-level execution)"
echo "      - event_type='api' (API tracing, if enabled)"
echo "      - event_type='coverage' (BB coverage, if enabled)"
echo ""

# ============================================================================
# Step 5: Query telemetry from Controller (if streaming is enabled)
# ============================================================================
echo "[5/5] Telemetry verification..."

if [ -z "$TELEMETRY_IDS" ]; then
    echo "   ⚠ No telemetry IDs returned (check worker logs)"
    echo ""
else
    RUN_ID=$(echo "$TELEMETRY_IDS" | head -1)
    echo "   Run ID: $RUN_ID"
    echo ""
    echo "   To query telemetry in Elasticsearch:"
    echo "      curl 'http://localhost:9200/rededr-*,trace-*/_search?q=run_id:$RUN_ID'"
    echo ""
    echo "   To check controller logs:"
    echo "      grep '$JOB_ID' controller.log"
    echo ""
fi

# ============================================================================
# Summary
# ============================================================================
echo "=== Test Summary ==="
echo ""
echo "Job ID:         $JOB_ID"
echo "Artifact ID:    $ARTIFACT_ID"
echo "Build status:   $BUILD_STATUS"
echo "Deploy status:  $DEPLOY_STATUS"
echo "Exec success:   ${EXEC_SUCCESS:-unknown}"
echo "Exit code:      ${EXIT_CODE:-unknown}"
echo ""

echo "Telemetry Collection Status:"
echo "  [Expected] RedEDR syscall traces"
echo "  [Expected] Line-level execution traces (if artifact sends to named pipe)"
echo "  [Future]   API tracing (when build/emitter adds --trace=api)"
echo "  [Future]   BB coverage (when build/emitter adds --trace=bb)"
echo ""

echo "Verification Steps:"
echo "  1. Check worker logs for telemetry collection messages"
echo "  2. Verify 'Total telemetry events collected: N' where N > 0"
echo "  3. Check for 'event_type=trace' entries (if instrumented artifact)"
echo "  4. Query Elasticsearch for run_id: $RUN_ID"
echo "  5. Verify telemetry was sent to controller (check controller logs)"
echo ""

echo "Current Implementation Status:"
echo "  ✅ gRPC proto trace_mode parameter (controller.proto BuildRequest/BuildResponse)"
echo "  ✅ Controller scheduler extracts and logs trace_mode"
echo "  ✅ Builder infrastructure passes trace_mode through pipeline"
echo "  ⏸️  build/emitter --trace flag (pending implementation)"
echo "  ✅ Worker trace collector (named pipe \\\\\.\\pipe\\rededr_trace)"
echo "  ✅ Telemetry merging (RedEDR + trace events in single batch)"
echo ""
echo "Next Steps:"
echo "  1. Implement build/emitter --trace flag to perform actual instrumentation:"
echo "     --trace=off    (no instrumentation)"
echo "     --trace=lines  (line-level tracing via named pipe)"
echo "     --trace=api    (API call tracing)"
echo "     --trace=bb     (basic-block coverage bitmap)"
echo "     --trace=all    (everything)"
echo ""
echo "  2. AST transformation to inject instrumentation code:"
echo "     - Lines: printf to named pipe: b64line:<base64('line:file.c:42:main')>\\n"
echo "     - API: Wrap target API calls with logging"
echo "     - BB: Insert coverage bitmap writes at basic block entries"
echo ""

echo "🎉 End-to-end instrumented test complete!"
echo ""
