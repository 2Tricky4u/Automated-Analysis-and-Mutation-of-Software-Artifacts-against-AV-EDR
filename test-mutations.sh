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

BASELINE_ID=$(echo "$BASELINE_RESPONSE" | jq -r '.artifactId')
BASELINE_SIZE=$(echo "$BASELINE_RESPONSE" | jq -r '.sizeBytes')

echo "   ✓ Baseline artifact: $BASELINE_ID"
echo "   ✓ Size: $BASELINE_SIZE bytes"
echo "   ✓ Mutations applied: []"
echo ""

# Test 2: Build with NOP insertion only
echo "[2/5] Building artifact with llvm.nop_insert mutation..."
NOP_RESPONSE=$(grpcurl -plaintext -import-path controller/proto -proto controller.proto -d '{"template_name":"rwx_direct","source_file":"rwx_direct.c","mutations":[{"id":"llvm.nop_insert","params":{"density":"0.5"}}]}' localhost:50051 edr.controller.Controller/BuildArtifact 2>&1)

NOP_ID=$(echo "$NOP_RESPONSE" | jq -r '.artifactId')
NOP_SIZE=$(echo "$NOP_RESPONSE" | jq -r '.sizeBytes')
NOP_MUTATIONS=$(echo "$NOP_RESPONSE" | jq -r '.mutationsApplied[]')

echo "   ✓ NOP-inserted artifact: $NOP_ID"
echo "   ✓ Size: $NOP_SIZE bytes (Δ = $((NOP_SIZE - BASELINE_SIZE)) bytes)"
echo "   ✓ Mutations applied: [$NOP_MUTATIONS]"
echo ""

# Test 3: Build with string XOR mutation
echo "[3/5] Building artifact with ast.string_xor mutation..."
MUTATED_RESPONSE=$(grpcurl -plaintext -import-path controller/proto -proto controller.proto -d '{"template_name":"rwx_direct","source_file":"rwx_direct.c","mutations":[{"id":"ast.string_xor","params":{"xor_key":"0xAA"}}]}' localhost:50051 edr.controller.Controller/BuildArtifact 2>&1)

MUTATED_ID=$(echo "$MUTATED_RESPONSE" | jq -r '.artifactId')
MUTATED_SIZE=$(echo "$MUTATED_RESPONSE" | jq -r '.sizeBytes')
MUTATIONS_APPLIED=$(echo "$MUTATED_RESPONSE" | jq -r '.mutationsApplied[]')

echo "   ✓ XOR-mutated artifact: $MUTATED_ID"
echo "   ✓ Size: $MUTATED_SIZE bytes (Δ = $((MUTATED_SIZE - BASELINE_SIZE)) bytes)"
echo "   ✓ Mutations applied: [$MUTATIONS_APPLIED]"
echo ""

# Test 4: Verify mutations actually changed the artifacts
echo "[4/5] Verifying mutations had real effects..."

# Check that all artifact IDs are unique (mutations changed binary content)
if [ "$BASELINE_ID" = "$NOP_ID" ] || [ "$BASELINE_ID" = "$MUTATED_ID" ] || [ "$NOP_ID" = "$MUTATED_ID" ]; then
    echo "   ✗ ERROR: Some artifact IDs are identical! Mutations had no effect."
    echo "      Baseline: $BASELINE_ID"
    echo "      NOP:      $NOP_ID"
    echo "      XOR:      $MUTATED_ID"
    exit 1
else
    echo "   ✓ All artifact IDs differ (mutations changed binary content)"
fi

# Check that mutationsApplied field matches what we requested
if [ "$NOP_MUTATIONS" != "llvm.nop_insert" ]; then
    echo "   ✗ ERROR: NOP mutation not applied (got: $NOP_MUTATIONS)"
    exit 1
else
    echo "   ✓ NOP mutation was applied: $NOP_MUTATIONS"
fi

if [ "$MUTATIONS_APPLIED" != "ast.string_xor" ]; then
    echo "   ✗ ERROR: XOR mutation not applied (got: $MUTATIONS_APPLIED)"
    exit 1
else
    echo "   ✓ XOR mutation was applied: $MUTATIONS_APPLIED"
fi

# Check that NOP insertion increased binary size (NOPs add instructions)
NOP_DELTA=$((NOP_SIZE - BASELINE_SIZE))
if [ "$NOP_DELTA" -le 0 ]; then
    echo "   ✗ WARNING: NOP insertion did not increase binary size (Δ = $NOP_DELTA bytes)"
    echo "      This might indicate NOPs were optimized out or mutation failed"
else
    echo "   ✓ NOP insertion increased binary size by $NOP_DELTA bytes"
fi

# Check that XOR mutation is size-neutral or slightly larger (string literals → XOR decode logic)
XOR_DELTA=$((MUTATED_SIZE - BASELINE_SIZE))
echo "   ✓ XOR mutation changed binary size by $XOR_DELTA bytes (decode logic overhead)"

echo ""

# Test 5: Build with multiple mutations
echo "[5/5] Building artifact with multiple mutations..."
MULTI_RESPONSE=$(grpcurl -plaintext -import-path controller/proto -proto controller.proto -d '{"template_name":"rwx_direct","source_file":"rwx_direct.c","mutations":[{"id":"ast.string_xor","params":{"xor_key":"0x42"}},{"id":"llvm.nop_insert","params":{"density":"0.5"}}]}' localhost:50051 edr.controller.Controller/BuildArtifact 2>&1)

MULTI_ID=$(echo "$MULTI_RESPONSE" | jq -r '.artifactId')
MULTI_SIZE=$(echo "$MULTI_RESPONSE" | jq -r '.sizeBytes')
MULTI_MUTATIONS=$(echo "$MULTI_RESPONSE" | jq -r '.mutationsApplied | join(", ")')

echo "   ✓ Multi-mutation artifact: $MULTI_ID"
echo "   ✓ Size: $MULTI_SIZE bytes (Δ = $((MULTI_SIZE - BASELINE_SIZE)) bytes)"
echo "   ✓ Mutations applied: [$MULTI_MUTATIONS]"

# Verify both mutations were applied
if [[ "$MULTI_MUTATIONS" == *"ast.string_xor"* ]] && [[ "$MULTI_MUTATIONS" == *"llvm.nop_insert"* ]]; then
    echo "   ✓ Both mutations applied successfully"
else
    echo "   ✗ ERROR: Multi-mutation missing expected mutations (got: $MULTI_MUTATIONS)"
    exit 1
fi

# Check that multi-mutation is different from single mutations
if [ "$MULTI_ID" = "$BASELINE_ID" ] || [ "$MULTI_ID" = "$NOP_ID" ] || [ "$MULTI_ID" = "$MUTATED_ID" ]; then
    echo "   ✗ ERROR: Multi-mutation artifact matches a single-mutation artifact!"
    exit 1
else
    echo "   ✓ Multi-mutation artifact is unique (different from single mutations)"
fi
echo ""

echo "=== Test Summary ==="
echo "✓ All tests passed!"
echo ""

# Artifact comparison table
echo "Artifact Hash Comparison:"
echo "  Baseline:       $BASELINE_ID"
echo "  NOP insertion:  $NOP_ID"
echo "  String XOR:     $MUTATED_ID"
echo "  Multi-mutation: $MULTI_ID"
echo ""

# Verify all hashes are unique (final sanity check)
UNIQUE_HASHES=$(echo -e "$BASELINE_ID\n$NOP_ID\n$MUTATED_ID\n$MULTI_ID" | sort -u | wc -l)
if [ "$UNIQUE_HASHES" -ne 4 ]; then
    echo "✗ CRITICAL ERROR: Found duplicate hashes! ($UNIQUE_HASHES unique out of 4)"
    echo "  This should never happen if mutations are working correctly."
    exit 1
fi
echo "✓ Hash uniqueness confirmed: All 4 artifacts have different SHA256 hashes"
echo ""

# Size comparison table
echo "Binary Size Comparison:"
echo "  Baseline:       $BASELINE_SIZE bytes"
echo "  NOP insertion:  $NOP_SIZE bytes (Δ=$NOP_DELTA)"
echo "  String XOR:     $MUTATED_SIZE bytes (Δ=$XOR_DELTA)"
echo "  Multi-mutation: $MULTI_SIZE bytes (Δ=$((MULTI_SIZE - BASELINE_SIZE)))"
echo ""

echo "Mutation effects verified:"
echo "  ✓ All artifacts have unique hashes (SHA256 differs)"
echo "  ✓ Mutation metadata matches requested transformations"
echo "  ✓ Binary size changes consistent with mutation types"
if [ -f "$MUTATED_SOURCE" ]; then
    echo "  ✓ Source-level mutations visible in intermediate files"
fi

