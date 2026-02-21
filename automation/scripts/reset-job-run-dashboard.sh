#!/bin/bash
# Reset Job & Run Drill-Down Dashboard
# Deletes all 16 saved objects and optionally re-creates them.
#
# Usage:
#   ./reset-job-run-dashboard.sh          # delete + recreate
#   ./reset-job-run-dashboard.sh --delete  # delete only (no recreate)

set -e

KIBANA_URL="${KIBANA_URL:-http://localhost:5601}"
DELETE_ONLY=false

if [[ "$1" == "--delete" ]]; then
    DELETE_ONLY=true
fi

echo "[*] Resetting Job & Run Drill-Down Dashboard"
echo "[*] Kibana URL: $KIBANA_URL"

# Check Kibana is accessible
if ! curl -s "$KIBANA_URL/api/status" > /dev/null; then
    echo "[!] ERROR: Cannot connect to Kibana at $KIBANA_URL"
    exit 1
fi

echo "[+] Kibana is accessible"
echo ""

# Delete a saved object (ignore errors if doesn't exist)
delete_object() {
    local obj_type=$1
    local obj_id=$2

    echo "  [-] $obj_type/$obj_id"
    local status
    status=$(curl -s -o /dev/null -w "%{http_code}" \
        -X DELETE "$KIBANA_URL/api/saved_objects/$obj_type/$obj_id" \
        -H 'kbn-xsrf: true' 2>/dev/null)
    if [[ "$status" == "200" ]]; then
        echo "      deleted"
    else
        echo "      not found (${status})"
    fi
}

echo "=== Step 1: Deleting Dashboard ==="
delete_object "dashboard" "job-run-drilldown-dashboard"

echo ""
echo "=== Step 2: Deleting Visualizations (6) ==="
delete_object "visualization" "viz-job-status-pie"
delete_object "visualization" "viz-detection-outcome-pie"
delete_object "visualization" "viz-differential-category-pie"
delete_object "visualization" "viz-evasion-score-histogram"
delete_object "visualization" "viz-run-type-breakdown"
delete_object "visualization" "viz-elapsed-time-histogram"

echo ""
echo "=== Step 3: Deleting Saved Searches (7) ==="
delete_object "search" "job-overview-search"
delete_object "search" "job-modules-search"
delete_object "search" "rounds-by-job-search"
delete_object "search" "rounds-modules-search"
delete_object "search" "runs-drilldown-search"
delete_object "search" "runs-errors-search"
delete_object "search" "telemetry-per-run-search"

echo ""
echo "=== Step 4: Deleting Index Patterns (2) ==="
delete_object "index-pattern" "jobs-star"
delete_object "index-pattern" "rounds-star"

echo ""
echo "[+] Cleanup complete (16 objects)"

if [[ "$DELETE_ONLY" == "true" ]]; then
    echo "[*] --delete flag set, skipping recreation"
    exit 0
fi

echo ""
echo "=== Step 5: Recreating Dashboard ==="
echo ""

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bash "$SCRIPT_DIR/create-job-run-dashboard.sh"
