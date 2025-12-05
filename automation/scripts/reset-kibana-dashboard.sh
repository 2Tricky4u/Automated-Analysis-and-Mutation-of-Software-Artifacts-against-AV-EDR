#!/bin/bash
# Reset Kibana Dashboard - Clean up and recreate
# Use this if dashboard setup failed or needs to be refreshed

set -e

KIBANA_URL="${KIBANA_URL:-http://localhost:5601}"

echo "[*] Resetting Kibana Dashboard for Artifact Execution Analysis"
echo "[*] Kibana URL: $KIBANA_URL"

# Check Kibana is accessible
if ! curl -s "$KIBANA_URL/api/status" > /dev/null; then
    echo "[!] ERROR: Cannot connect to Kibana at $KIBANA_URL"
    exit 1
fi

echo "[+] Kibana is accessible"
echo ""

# Function to delete saved object (ignore errors if doesn't exist)
delete_object() {
    local obj_type=$1
    local obj_id=$2

    echo "[*] Deleting $obj_type: $obj_id"
    curl -X DELETE "$KIBANA_URL/api/saved_objects/$obj_type/$obj_id" \
        -H 'kbn-xsrf: true' 2>/dev/null | grep -q "200" || echo "  (not found or already deleted)"
}

echo "=== Step 1: Deleting Existing Dashboard and Saved Searches ==="
delete_object "dashboard" "artifact-execution-dashboard"
delete_object "search" "run-results-search"
delete_object "search" "coverage-events-search"
delete_object "search" "all-telemetry-search"

echo ""
echo "[+] Cleanup complete"
echo ""
echo "=== Step 2: Recreating Dashboard ==="
echo ""

# Run the main setup script
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bash "$SCRIPT_DIR/create-kibana-dashboard.sh"
