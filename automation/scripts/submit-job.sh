#!/bin/bash
# -----------------------------------------------------------------------
# submit-job.sh -- Submit job to scheduler via ScheduleJob gRPC endpoint
#
# Sends a multi-round mutation job to the controller's ScheduleJob RPC.
# The controller queues the job, selects mutations per round, builds
# artifacts, and dispatches them to worker VMs for execution and
# telemetry collection.
#
# Usage:  ./submit-job.sh --payload <path> [--name <name>]
#                         [--max-rounds <n>] [--stop-on-evasion]
#                         [--carrier <mod>] [--decoder <mod>]
#                         [--antiemulation <mod>] [--guardrail <mod>]
#                         [--virtualprotect <mod>] [--decoy <mod>]
#                         [--encoding <type>] [--controller <addr>]
# Prerequisites: grpcurl, jq, proto/ directory with controller.proto
# Called by: standalone
# -----------------------------------------------------------------------

set -e

# Default values
CONTROLLER_ADDRESS="${CONTROLLER_ADDRESS:-10.200.200.1:50051}"
PAYLOAD=""
NAME=""
MAX_ROUNDS=""
STOP_ON_EVASION=""

# Module defaults (empty = use server defaults)
CARRIER=""
DECODER=""
ANTIEMULATION=""
GUARDRAIL=""
VIRTUALPROTECT=""
DECOY=""
ENCODING=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --payload|--source)
            PAYLOAD="$2"
            shift 2
            ;;
        --name)
            NAME="$2"
            shift 2
            ;;
        --max-rounds)
            MAX_ROUNDS="$2"
            shift 2
            ;;
        --stop-on-evasion)
            STOP_ON_EVASION="true"
            shift
            ;;
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
        --controller)
            CONTROLLER_ADDRESS="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 --payload <path> [options]"
            echo ""
            echo "Required:"
            echo "  --payload <path>        Path to payload .bin file"
            echo ""
            echo "Optional - Job settings:"
            echo "  --name <name>           Job name (default: auto-generated)"
            echo "  --max-rounds <n>        Maximum mutation rounds (default: 10)"
            echo "  --stop-on-evasion       Stop when evasion achieved"
            echo "  --controller <addr>     Controller address (default: 10.200.200.1:50051)"
            echo ""
            echo "Optional - Module selection (defaults shown):"
            echo "  --carrier <module>      Carrier module (default: alloc_rw_rx)"
            echo "                          Options: alloc_rw_rx, change_rw_rx, peb_walk"
            echo "  --decoder <module>      Decoder module (default: xor)"
            echo "                          Options: xor, english"
            echo "  --antiemulation <mod>   Anti-emulation module (default: none)"
            echo "                          Options: none, sirallocalot, timeraw"
            echo "  --guardrail <module>    Guardrail module (default: none)"
            echo "                          Options: none, env"
            echo "  --virtualprotect <mod>  VirtualProtect module (default: standard)"
            echo "                          Options: standard, undersized"
            echo "  --decoy <module>        Decoy module (default: none)"
            echo "                          Options: none, calc, winexec"
            echo "  --encoding <type>       Payload encoding (default: xor)"
            echo "                          Options: xor, english"
            echo ""
            echo "Examples:"
            echo "  # Simple job with defaults"
            echo "  $0 --payload calc.bin"
            echo ""
            echo "  # Custom modules"
            echo "  $0 --payload calc.bin --carrier alloc_rw_rx --decoder xor_rolling"
            echo ""
            echo "  # Full customization"
            echo "  $0 --payload calc.bin \\"
            echo "     --carrier change_rw_rx \\"
            echo "     --antiemulation sirallocalot \\"
            echo "     --encoding english \\"
            echo "     --max-rounds 20 \\"
            echo "     --stop-on-evasion"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Validate required arguments
if [ -z "$PAYLOAD" ]; then
    echo "Error: --payload is required"
    echo "Use --help for usage information"
    exit 1
fi

# Default name if not specified
if [ -z "$NAME" ]; then
    NAME="job-$(basename "$PAYLOAD" .bin)"
fi

# Build modules JSON (only include non-empty fields)
MODULES_JSON="{"
FIRST=true
if [ -n "$CARRIER" ]; then
    MODULES_JSON+="\"carrier\": \"$CARRIER\""
    FIRST=false
fi
if [ -n "$DECODER" ]; then
    [ "$FIRST" = false ] && MODULES_JSON+=", "
    MODULES_JSON+="\"decoder\": \"$DECODER\""
    FIRST=false
fi
if [ -n "$ANTIEMULATION" ]; then
    [ "$FIRST" = false ] && MODULES_JSON+=", "
    MODULES_JSON+="\"antiemulation\": \"$ANTIEMULATION\""
    FIRST=false
fi
if [ -n "$GUARDRAIL" ]; then
    [ "$FIRST" = false ] && MODULES_JSON+=", "
    MODULES_JSON+="\"guardrail\": \"$GUARDRAIL\""
    FIRST=false
fi
if [ -n "$VIRTUALPROTECT" ]; then
    [ "$FIRST" = false ] && MODULES_JSON+=", "
    MODULES_JSON+="\"virtualprotect\": \"$VIRTUALPROTECT\""
    FIRST=false
fi
if [ -n "$DECOY" ]; then
    [ "$FIRST" = false ] && MODULES_JSON+=", "
    MODULES_JSON+="\"decoy\": \"$DECOY\""
    FIRST=false
fi
MODULES_JSON+="}"

# Build request JSON
REQUEST_JSON="{\"name\": \"$NAME\", \"source\": \"$PAYLOAD\""
if [ -n "$MAX_ROUNDS" ]; then
    REQUEST_JSON+=", \"maxRounds\": $MAX_ROUNDS"
fi
if [ "$STOP_ON_EVASION" = "true" ]; then
    REQUEST_JSON+=", \"stopOnEvasion\": true"
fi
if [ "$MODULES_JSON" != "{}" ]; then
    REQUEST_JSON+=", \"modules\": $MODULES_JSON"
fi
if [ -n "$ENCODING" ]; then
    REQUEST_JSON+=", \"encoding\": \"$ENCODING\""
fi
REQUEST_JSON+="}"

echo "[*] Submitting job to scheduler"
echo "    Controller: $CONTROLLER_ADDRESS"
echo "    Payload: $PAYLOAD"
echo "    Name: $NAME"
[ -n "$CARRIER" ] && echo "    Carrier: $CARRIER"
[ -n "$DECODER" ] && echo "    Decoder: $DECODER"
[ -n "$ANTIEMULATION" ] && echo "    Antiemulation: $ANTIEMULATION"
[ -n "$GUARDRAIL" ] && echo "    Guardrail: $GUARDRAIL"
[ -n "$VIRTUALPROTECT" ] && echo "    VirtualProtect: $VIRTUALPROTECT"
[ -n "$DECOY" ] && echo "    Decoy: $DECOY"
[ -n "$ENCODING" ] && echo "    Encoding: $ENCODING"
[ -n "$MAX_ROUNDS" ] && echo "    Max Rounds: $MAX_ROUNDS"
[ "$STOP_ON_EVASION" = "true" ] && echo "    Stop on Evasion: yes"
echo ""

# Submit job via grpcurl
RESPONSE=$(grpcurl -plaintext \
    -import-path proto \
    -proto controller.proto \
    -d "$REQUEST_JSON" \
    "$CONTROLLER_ADDRESS" \
    automutate.controller.Controller/ScheduleJob)

# Parse response
JOB_ID=$(echo "$RESPONSE" | jq -r '.jobId.value // empty')
ACCEPTED=$(echo "$RESPONSE" | jq -r '.accepted // false')
MESSAGE=$(echo "$RESPONSE" | jq -r '.message // "No message"')

if [ "$ACCEPTED" == "true" ] && [ -n "$JOB_ID" ]; then
    echo "[+] Job submitted successfully!"
    echo "    Job ID: $JOB_ID"
    echo "    Message: $MESSAGE"
    echo ""
    echo "[*] Track progress with:"
    echo "    automation/scripts/get-job.sh $JOB_ID"
    exit 0
else
    echo "[!] Job submission failed"
    echo "    Message: $MESSAGE"
    exit 1
fi
