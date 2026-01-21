#!/bin/bash
# Submit a job to the scheduler via gRPC
# Usage: ./submit-job.sh --template <template> --source <source> [--mutations <mutations>] [--priority <priority>]

set -e

# Default values
CONTROLLER_ADDRESS="${CONTROLLER_ADDRESS:-10.200.200.1:50051}"
# CONTROLLER_ADDRESS="${CONTROLLER_ADDRESS:-localhost:50051}"
TEMPLATE=""
SOURCE=""
MUTATIONS=""
PRIORITY=0
NAME=""

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --template)
            TEMPLATE="$2"
            shift 2
            ;;
        --source)
            SOURCE="$2"
            shift 2
            ;;
        --mutations)
            MUTATIONS="$2"
            shift 2
            ;;
        --priority)
            PRIORITY="$2"
            shift 2
            ;;
        --name)
            NAME="$2"
            shift 2
            ;;
        --controller)
            CONTROLLER_ADDRESS="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Validate required arguments
if [ -z "$TEMPLATE" ]; then
    echo "Error: --template is required"
    echo ""
    echo "Usage: $0 --template <template> --source <source> [options]"
    echo ""
    echo "Required:"
    echo "  --template <name>     Template name (e.g., 'rwx_direct', 'eicar')"
    echo "  --source <file>       Source file path (e.g., 'rwx_direct.c')"
    echo ""
    echo "Optional:"
    echo "  --mutations <list>    Comma-separated mutations (e.g., 'ast.import_reshape,beh.preamble.fs')"
    echo "  --priority <n>        Job priority (default: 0, higher = earlier)"
    echo "  --name <name>         Job name (default: template name)"
    echo "  --controller <addr>   Controller address (default: 10.200.200.1:50051)"
    echo ""
    echo "Examples:"
    echo "  # Simple job (no mutations)"
    echo "  $0 --template rwx_direct --source rwx_direct.c"
    echo ""
    echo "  # With mutations"
    echo "  $0 --template rwx_direct --source rwx_direct.c \\"
    echo "     --mutations 'ast.import_reshape,beh.preamble.fs' \\"
    echo "     --priority 10"
    exit 1
fi

if [ -z "$SOURCE" ]; then
    echo "Error: --source is required"
    exit 1
fi

# Default name to template if not specified
if [ -z "$NAME" ]; then
    NAME="$TEMPLATE"
fi

# Build mutation array for JSON
if [ -z "$MUTATIONS" ]; then
    MUTATION_ARRAY="[]"
else
    # Convert comma-separated list to JSON array
    # e.g., "ast.import_reshape,beh.preamble.fs" -> ["ast.import_reshape", "beh.preamble.fs"]
    MUTATION_ARRAY=$(echo "$MUTATIONS" | jq -R 'split(",") | map(select(length > 0))')
fi

echo "[*] Submitting job to scheduler"
echo "    Controller: $CONTROLLER_ADDRESS"
echo "    Template: $TEMPLATE"
echo "    Source: $SOURCE"
echo "    Mutations: $MUTATIONS"
echo "    Priority: $PRIORITY"
echo ""

# Submit job via grpcurl
# Note: Requires grpcurl to be installed (go install github.com/fullstorydev/grpcurl/cmd/grpcurl@latest)
RESPONSE=$(grpcurl -plaintext \
    -import-path proto \
    -proto controller.proto \
    -d "{
        \"name\": \"$NAME\",
        \"artifact_type\": \"$TEMPLATE\",
        \"source\": \"$SOURCE\",
        \"priority\": $PRIORITY
    }" \
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
