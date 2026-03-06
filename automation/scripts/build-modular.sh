#!/bin/bash
# -----------------------------------------------------------------------
# build-modular.sh -- Build single artifact via BuildArtifact gRPC endpoint
#
# Assembles a modular loader from gene modules (carrier, decoder,
# anti-emulation, guardrail, decoy) and submits the build request to the
# controller's BuildArtifact RPC. Returns the artifact ID on success.
#
# Usage:  ./build-modular.sh [--carrier <name>] [--decoder <name>]
#                            [--antiemulation <name>] [--guardrail <name>]
#                            [--virtualprotect <name>] [--decoy <name>]
#                            [--encoding <type>] [--trace <mode>]
#                            [--payload <string> | --payload-file <path>]
#                            [--controller <addr>]
# Prerequisites: grpcurl, jq, base64, proto/ directory with controller.proto
# Called by: standalone or test-full-pipeline.sh
# -----------------------------------------------------------------------

set -e

# Default values
CONTROLLER_ADDRESS="${CONTROLLER_ADDRESS:-10.200.200.1:50051}"
CARRIER="alloc_rw_rx"
DECODER="xor"
ANTIEMULATION="none"
GUARDRAIL="none"
VIRTUALPROTECT="standard"
DECOY="none"
ENCODING="xor"
TRACE_MODE="off"
PAYLOAD=""
PAYLOAD_FILE=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --carrier)
            CARRIER="$2"
            shift 2
            ;;
        --decoder)
            DECODER="$2"
            shift 2
            ;;
        --antiemulation)
            ANTIEMULATION="$2"
            shift 2
            ;;
        --guardrail)
            GUARDRAIL="$2"
            shift 2
            ;;
        --virtualprotect)
            VIRTUALPROTECT="$2"
            shift 2
            ;;
        --decoy)
            DECOY="$2"
            shift 2
            ;;
        --encoding)
            ENCODING="$2"
            shift 2
            ;;
        --trace)
            TRACE_MODE="$2"
            shift 2
            ;;
        --payload)
            PAYLOAD="$2"
            shift 2
            ;;
        --payload-file)
            PAYLOAD_FILE="$2"
            shift 2
            ;;
        --controller)
            CONTROLLER_ADDRESS="$2"
            shift 2
            ;;
        --help|-h)
            echo "Build an artifact using the modular template system"
            echo ""
            echo "Usage: $0 [options]"
            echo ""
            echo "Module Selection:"
            echo "  --carrier <name>        Carrier module (default: alloc_rw_rx)"
            echo "                          Options: alloc_rw_rx, change_rw_rx, peb_walk"
            echo "  --decoder <name>        Decoder module (default: xor)"
            echo "                          Options: xor, english"
            echo "  --antiemulation <name>  Anti-emulation module (default: none)"
            echo "                          Options: none, sirallocalot, timeraw"
            echo "  --guardrail <name>      Guardrail module (default: none)"
            echo "                          Options: none, env"
            echo "  --virtualprotect <name> VirtualProtect module (default: standard)"
            echo "                          Options: standard, undersized"
            echo "  --decoy <name>          Decoy module (default: none)"
            echo "                          Options: none, winexec"
            echo ""
            echo "Payload:"
            echo "  --payload <string>      Raw payload string (will be base64 encoded)"
            echo "  --payload-file <path>   Read payload from file (binary safe)"
            echo ""
            echo "Build Options:"
            echo "  --encoding <type>       Payload encoding (default: xor)"
            echo "                          Options: xor, english"
            echo "  --trace <mode>          Trace mode (default: off)"
            echo "                          Options: off, lines, api, bb, api+bb, all"
            echo ""
            echo "Connection:"
            echo "  --controller <addr>     Controller address (default: 10.200.200.1:50051)"
            echo ""
            echo "Examples:"
            echo "  # Basic build with default modules and test payload"
            echo "  $0 --payload 'EICAR-STANDARD-ANTIVIRUS-TEST-FILE'"
            echo ""
            echo "  # Custom modules with file payload"
            echo "  $0 --carrier peb_walk --decoder xor --antiemulation timeraw \\"
            echo "     --payload-file shellcode.bin"
            echo ""
            echo "  # Full customization"
            echo "  $0 --carrier change_rw_rx --decoder english --antiemulation sirallocalot \\"
            echo "     --guardrail env --virtualprotect undersized --decoy winexec \\"
            echo "     --encoding english --trace lines --payload-file payload.bin"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Handle payload
if [ -n "$PAYLOAD_FILE" ]; then
    if [ ! -f "$PAYLOAD_FILE" ]; then
        echo "Error: Payload file not found: $PAYLOAD_FILE"
        exit 1
    fi
    PAYLOAD_B64=$(base64 -w0 "$PAYLOAD_FILE")
elif [ -n "$PAYLOAD" ]; then
    PAYLOAD_B64=$(echo -n "$PAYLOAD" | base64 -w0)
else
    # Default test payload (EICAR string)
    echo "[*] No payload specified, using EICAR test string"
    PAYLOAD="X5O!P%@AP[4\\PZX54(P^)7CC)7}\$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!\$H+H*"
    PAYLOAD_B64=$(echo -n "$PAYLOAD" | base64 -w0)
fi

echo "[*] Building modular artifact"
echo "    Controller: $CONTROLLER_ADDRESS"
echo ""
echo "    Modules:"
echo "      Carrier:        $CARRIER"
echo "      Decoder:        $DECODER"
echo "      Antiemulation:  $ANTIEMULATION"
echo "      Guardrail:      $GUARDRAIL"
echo "      VirtualProtect: $VIRTUALPROTECT"
echo "      Decoy:          $DECOY"
echo ""
echo "    Encoding:    $ENCODING"
echo "    Trace Mode:  $TRACE_MODE"
echo "    Payload:     ${#PAYLOAD_B64} bytes (base64)"
echo ""

# Build request JSON
REQUEST_JSON=$(cat <<EOF
{
    "modular_build": {
        "modules": {
            "carrier": "$CARRIER",
            "decoder": "$DECODER",
            "antiemulation": "$ANTIEMULATION",
            "guardrail": "$GUARDRAIL",
            "virtualprotect": "$VIRTUALPROTECT",
            "decoy": "$DECOY"
        },
        "payload": "$PAYLOAD_B64",
        "encoding": "$ENCODING"
    },
    "trace_mode": "$TRACE_MODE"
}
EOF
)

# Submit build via grpcurl
RESPONSE=$(grpcurl -plaintext \
    -import-path proto \
    -proto controller.proto \
    -d "$REQUEST_JSON" \
    "$CONTROLLER_ADDRESS" \
    automutate.controller.Controller/BuildArtifact 2>&1)

# Check for connection errors
if echo "$RESPONSE" | grep -q "Failed to dial"; then
    echo "[!] Failed to connect to controller at $CONTROLLER_ADDRESS"
    echo "    Error: $RESPONSE"
    exit 1
fi

# Parse response
ARTIFACT_ID=$(echo "$RESPONSE" | jq -r '.artifactId // empty')
SIZE_BYTES=$(echo "$RESPONSE" | jq -r '.sizeBytes // 0')
BUILD_STATUS=$(echo "$RESPONSE" | jq -r '.buildStatus // "unknown"')
ERROR=$(echo "$RESPONSE" | jq -r '.error // empty')
STORAGE_PATH=$(echo "$RESPONSE" | jq -r '.storagePath // empty')

if [ "$BUILD_STATUS" == "success" ] && [ -n "$ARTIFACT_ID" ]; then
    echo "[+] Build successful!"
    echo "    Artifact ID:   $ARTIFACT_ID"
    echo "    Size:          $SIZE_BYTES bytes"
    echo "    Storage Path:  $STORAGE_PATH"
    echo ""
    echo "[*] Next steps:"
    echo "    # Deploy to worker:"
    echo "    grpcurl -plaintext -import-path proto -proto controller.proto \\"
    echo "        -d '{\"artifact_id\": \"$ARTIFACT_ID\", \"worker_address\": \"<worker>:50052\"}' \\"
    echo "        $CONTROLLER_ADDRESS automutate.controller.Controller/DeployArtifact"
    exit 0
else
    echo "[!] Build failed"
    echo "    Status: $BUILD_STATUS"
    if [ -n "$ERROR" ]; then
        echo "    Error: $ERROR"
    fi
    echo ""
    echo "    Full response:"
    echo "$RESPONSE" | jq . 2>/dev/null || echo "$RESPONSE"
    exit 1
fi
