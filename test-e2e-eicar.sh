#!/bin/bash
# End-to-End EICAR Test: Build → Deploy → Execute
# Tests the full pipeline: Controller (build) → Worker (deploy + execute) → Telemetry
set -euo pipefail

echo "=== End-to-End EICAR Test ==="
echo ""

# Configuration
CONTROLLER_IP="localhost"
CONTROLLER_PORT="50051"
WORKER_IP="${WORKER_IP:-10.200.200.100}"  # Default worker IP, can override with env var
WORKER_PORT="50052"
TEMPLATE="rwx_direct"
SOURCE_FILE="rwx_direct.c"
JOB_PREFIX="loader-e2e"

echo "[CONFIG]"
echo "  Controller: $CONTROLLER_IP:$CONTROLLER_PORT"
echo "  Worker:     $WORKER_IP:$WORKER_PORT"
echo "  Template:   $TEMPLATE"
echo "  Source:     $SOURCE_FILE"
echo ""

# ============================================================================
# Step 1: Build artifact on Controller
# ============================================================================
echo "[1/4] Building EICAR artifact..."

BUILD_RESPONSE=$(grpcurl -plaintext \
  -import-path controller/proto \
  -proto controller.proto \
  -d "{\"template_name\":\"$TEMPLATE\",\"source_file\":\"$SOURCE_FILE\"}" \
  $CONTROLLER_IP:$CONTROLLER_PORT \
  edr.controller.Controller/BuildArtifact 2>&1)

# Check for errors
if ! echo "$BUILD_RESPONSE" | jq -e . >/dev/null 2>&1; then
    echo "   ✗ ERROR: Build failed or invalid response"
    echo ""
    echo "   Response:"
    echo "$BUILD_RESPONSE"
    exit 1
fi

ARTIFACT_ID=$(echo "$BUILD_RESPONSE" | jq -r '.artifactId')
ARTIFACT_SIZE=$(echo "$BUILD_RESPONSE" | jq -r '.sizeBytes')
BUILD_STATUS=$(echo "$BUILD_RESPONSE" | jq -r '.buildStatus')

if [ "$BUILD_STATUS" != "success" ]; then
    echo "   ✗ ERROR: Build status is '$BUILD_STATUS'"
    exit 1
fi

echo "   ✓ Artifact built successfully"
echo "      Artifact ID: $ARTIFACT_ID"
echo "      Size:        $ARTIFACT_SIZE bytes"
echo ""

# ============================================================================
# Step 2: Deploy artifact to Worker
# ============================================================================
echo "[2/4] Deploying artifact to worker VM..."

WORKER_ADDRESS="$WORKER_IP:$WORKER_PORT"

DEPLOY_RESPONSE=$(grpcurl -plaintext \
  -import-path controller/proto \
  -proto controller.proto \
  -d "{\"artifact_id\":\"$ARTIFACT_ID\",\"worker_address\":\"$WORKER_ADDRESS\"}" \
  $CONTROLLER_IP:$CONTROLLER_PORT \
  edr.controller.Controller/DeployArtifact 2>&1)

if ! echo "$DEPLOY_RESPONSE" | jq -e . >/dev/null 2>&1; then
    echo "   ✗ ERROR: Deploy failed or invalid response"
    echo ""
    echo "   Response:"
    echo "$DEPLOY_RESPONSE"
    exit 1
fi

DEPLOY_STATUS=$(echo "$DEPLOY_RESPONSE" | jq -r '.success')
WORKER_PATH=$(echo "$DEPLOY_RESPONSE" | jq -r '.workerStoragePath // .worker_storage_path // "unknown"')

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
# Step 3: Execute artifact on Worker
# ============================================================================
echo "[3/4] Executing EICAR artifact on worker VM..."
echo "   [WARNING] This will trigger AV/EDR detection!"
echo ""

# Generate unique job ID
JOB_ID="${JOB_PREFIX}-$(date +%s)"

# Execute with timeout (EICAR should be caught quickly)
EXECUTE_RESPONSE=$(grpcurl -plaintext \
  -import-path controller/proto \
  -proto worker.proto \
  -max-time 30 \
  -d "{\"job_id\":\"$JOB_ID\",\"artifact_id\":\"$ARTIFACT_ID\",\"timeout_seconds\":10,\"enable_etw\":true}" \
  $WORKER_IP:$WORKER_PORT \
  edr.worker.WorkerAgent/RunSample 2>&1)

# Check response (may timeout or be killed by EDR)
if ! echo "$EXECUTE_RESPONSE" | jq -e . >/dev/null 2>&1; then
    echo "   ⚠ Execution result unclear (likely blocked/killed by AV)"
    echo ""
    echo "   Raw response:"
    echo "$EXECUTE_RESPONSE" | head -20
    echo ""
    echo "   This is EXPECTED behavior for EICAR!"
else
    EXEC_SUCCESS=$(echo "$EXECUTE_RESPONSE" | jq -r '.success')
    EXIT_CODE=$(echo "$EXECUTE_RESPONSE" | jq -r '.exit_code')
    OUTPUT=$(echo "$EXECUTE_RESPONSE" | jq -r '.output')

    if [ "$EXEC_SUCCESS" = "true" ]; then
        echo "   ⚠ WARNING: Artifact executed successfully (AV may be disabled!)"
        echo "      Exit code: $EXIT_CODE"
        echo ""
        echo "   Output:"
        echo "$OUTPUT" | head -20
    else
        echo "   ✓ Artifact execution blocked (EXPECTED for EICAR)"
        echo "      This indicates AV/EDR is working correctly"
    fi
fi
echo ""

# ============================================================================
# Step 4: Check telemetry in Elasticsearch
# ============================================================================
echo "[4/4] Querying telemetry from Elasticsearch..."

# Wait for telemetry to be indexed
sleep 3

# Query for events related to this job
TELEMETRY_QUERY=$(cat <<EOF
{
  "query": {
    "bool": {
      "must": [
        {"term": {"job_id": "$JOB_ID"}},
        {"range": {"@timestamp": {"gte": "now-5m"}}}
      ]
    }
  },
  "size": 10,
  "sort": [{"@timestamp": "desc"}]
}
EOF
)

TELEMETRY_RESPONSE=$(curl -s -X POST "http://localhost:9200/etw-*,rededr-*/_search" \
  -H 'Content-Type: application/json' \
  -d "$TELEMETRY_QUERY" 2>&1)

if echo "$TELEMETRY_RESPONSE" | jq -e '.hits.total.value' >/dev/null 2>&1; then
    TELEMETRY_COUNT=$(echo "$TELEMETRY_RESPONSE" | jq -r '.hits.total.value')
    echo "   ✓ Telemetry events collected: $TELEMETRY_COUNT"

    if [ "$TELEMETRY_COUNT" -gt 0 ]; then
        echo ""
        echo "   Sample events:"
        echo "$TELEMETRY_RESPONSE" | jq -r '.hits.hits[0:3][] | "     - [\(.fields["event.id"][0] // "N/A")] \(.fields["event.name"][0] // .fields["provider.name"][0] // "unknown")"' 2>/dev/null || echo "     (Unable to parse event details)"
    else
        echo "   ⚠ No telemetry events found (may still be processing)"
    fi
else
    echo "   ⚠ Unable to query Elasticsearch"
    echo "   $(echo "$TELEMETRY_RESPONSE" | jq -r '.error.reason // "Unknown error"' 2>/dev/null || echo "$TELEMETRY_RESPONSE" | head -5)"
fi
echo ""

# ============================================================================
# Summary
# ============================================================================
echo "=== Test Summary ==="
echo ""
echo "Artifact ID:    $ARTIFACT_ID"
echo "Job ID:         $JOB_ID"
echo "Build status:   $BUILD_STATUS"
echo "Deploy status:  $DEPLOY_STATUS"
echo ""

echo "Expected behavior for EICAR:"
echo "  ✓ Build:   Should succeed (artifact contains test signature)"
echo "  ✓ Deploy:  Should succeed (file transfer works)"
echo "  ✓ Execute: Should FAIL or be KILLED (AV detects EICAR signature)"
echo "  ✓ Telemetry: Should show process start + termination events"
echo ""

echo "Next steps:"
echo "  1. Check Kibana for detailed telemetry: http://localhost:5601"
echo "  2. Query runs index: curl 'http://localhost:9200/runs-*/_search?q=job_id:$JOB_ID'"
echo "  3. Check worker logs on VM for detection details"
echo "  4. Verify Windows Defender quarantine (if enabled)"
echo ""

echo "🎉 End-to-end test complete!"
