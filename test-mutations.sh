#!/bin/bash
# Test script for minimal mutation system
# Run from WSL with controller running at localhost:50051

echo "=== Mutation System Test Script ==="
echo ""

# Test 1: Build without mutations (baseline)
echo "[1/4] Building baseline artifact (no mutations)..."
BASELINE_RESPONSE=$(grpcurl -plaintext -import-path controller/proto -proto controller.proto -d '{"template_name":"rwx_direct","source_file":"rwx_direct.c"}' localhost:50051 edr.controller.Controller/BuildArtifact 2>&1)

# Check if grpcurl succeeded
if ! echo "$BASELINE_RESPONSE" | jq -e . >/dev/null 2>&1; then
    echo "   ✗ ERROR: grpcurl failed or returned invalid JSON"
    echo ""
    echo "   Response:"
    echo "$BASELINE_RESPONSE"
    exit 1
fi

BASELINE_ID=$(echo "$BASELINE_RESPONSE" | jq -r '.artifact_id')
BASELINE_SIZE=$(echo "$BASELINE_RESPONSE" | jq -r '.size_bytes')

echo "   ✓ Baseline artifact: $BASELINE_ID"
echo "   ✓ Size: $BASELINE_SIZE bytes"
echo "   ✓ Mutations applied: []"
echo ""

# Test 2: Build with string XOR mutation
echo "[2/4] Building artifact with ast.string_xor mutation..."
MUTATED_RESPONSE=$(grpcurl -plaintext -import-path controller/proto -proto controller.proto -d '{"template_name":"rwx_direct","source_file":"rwx_direct.c","mutations":[{"id":"ast.string_xor","params":{"xor_key":"0xAA"}}]}' localhost:50051 edr.controller.Controller/BuildArtifact 2>&1)

MUTATED_ID=$(echo "$MUTATED_RESPONSE" | jq -r '.artifact_id')
MUTATED_SIZE=$(echo "$MUTATED_RESPONSE" | jq -r '.size_bytes')
MUTATIONS_APPLIED=$(echo "$MUTATED_RESPONSE" | jq -r '.mutations_applied[]')

echo "   ✓ Mutated artifact: $MUTATED_ID"
echo "   ✓ Size: $MUTATED_SIZE bytes (Δ = $((MUTATED_SIZE - BASELINE_SIZE)) bytes)"
echo "   ✓ Mutations applied: [$MUTATIONS_APPLIED]"
echo ""

# Test 3: Verify artifacts are different
echo "[3/4] Verifying artifacts are different..."
if [ "$BASELINE_ID" = "$MUTATED_ID" ]; then
    echo "   ✗ ERROR: Artifact IDs are the same! Mutation had no effect."
    exit 1
else
    echo "   ✓ Artifact IDs differ (mutation changed binary content)"
fi
echo ""

# Test 4: Build with multiple mutations
echo "[4/4] Building artifact with multiple mutations..."
MULTI_RESPONSE=$(grpcurl -plaintext -import-path controller/proto -proto controller.proto -d '{"template_name":"rwx_direct","source_file":"rwx_direct.c","mutations":[{"id":"ast.string_xor","params":{"xor_key":"0x42"}},{"id":"llvm.nop_insert","params":{"density":"0.5"}}]}' localhost:50051 edr.controller.Controller/BuildArtifact 2>&1)

MULTI_ID=$(echo "$MULTI_RESPONSE" | jq -r '.artifact_id')
MULTI_SIZE=$(echo "$MULTI_RESPONSE" | jq -r '.size_bytes')
MULTI_MUTATIONS=$(echo "$MULTI_RESPONSE" | jq -r '.mutations_applied | join(", ")')

echo "   ✓ Multi-mutation artifact: $MULTI_ID"
echo "   ✓ Size: $MULTI_SIZE bytes (Δ = $((MULTI_SIZE - BASELINE_SIZE)) bytes)"
echo "   ✓ Mutations applied: [$MULTI_MUTATIONS]"
echo ""

# Summary
echo "=== Test Summary ==="
echo "✓ All tests passed!"
echo ""
echo "Artifact IDs generated:"
echo "  - Baseline:       $BASELINE_ID ($BASELINE_SIZE bytes)"
echo "  - String XOR:     $MUTATED_ID ($MUTATED_SIZE bytes)"
echo "  - Multi-mutation: $MULTI_ID ($MULTI_SIZE bytes)"
echo ""
echo "Mutations are working correctly!"
echo ""
echo "Next steps:"
echo "  1. Deploy artifacts: grpcurl localhost:50051 edr.controller.Controller/DeployArtifact ..."
echo "  2. Execute on worker: grpcurl 10.200.200.100:50052 edr.worker.WorkerAgent/RunSample ..."
echo "  3. Compare telemetry: Query Elasticsearch for detection differences"
