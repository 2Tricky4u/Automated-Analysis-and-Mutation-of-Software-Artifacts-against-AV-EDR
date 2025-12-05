#!/bin/bash
# Automated Kibana Dashboard Setup Script
# Creates visualizations and dashboard for artifact execution analysis
# Based on ELASTICSEARCH-DATA-SCHEMA.md

set -e

KIBANA_URL="${KIBANA_URL:-http://localhost:5601}"
ES_URL="${ES_URL:-http://localhost:9200}"

echo "[*] Kibana Dashboard Setup for Artifact Execution Analysis"
echo "[*] Kibana URL: $KIBANA_URL"
echo "[*] Elasticsearch URL: $ES_URL"

# Check Kibana is accessible
if ! curl -s "$KIBANA_URL/api/status" > /dev/null; then
    echo "[!] ERROR: Cannot connect to Kibana at $KIBANA_URL"
    exit 1
fi

echo "[+] Kibana is accessible"

# Step 1: Create Index Patterns
echo ""
echo "=== Step 1: Creating Index Patterns ==="

# Create telemetry-* index pattern
echo "[*] Creating index pattern: telemetry-*"
curl -X POST "$KIBANA_URL/api/saved_objects/index-pattern/telemetry-star" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d '{
        "attributes": {
            "title": "telemetry-*",
            "timeFieldName": "indexed_at"
        }
    }' 2>/dev/null | jq -r '.id // "exists"' > /dev/null

echo "[+] Index pattern telemetry-* created/verified"

# Create runs-* index pattern
echo "[*] Creating index pattern: runs-*"
curl -X POST "$KIBANA_URL/api/saved_objects/index-pattern/runs-star" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d '{
        "attributes": {
            "title": "runs-*",
            "timeFieldName": "timestamp"
        }
    }' 2>/dev/null | jq -r '.id // "exists"' > /dev/null

echo "[+] Index pattern runs-* created/verified"

# Step 2: Create Saved Searches (basis for visualizations)
echo ""
echo "=== Step 2: Creating Saved Searches ==="

# Saved search: Coverage events
echo "[*] Creating saved search: coverage-events"
curl -X POST "$KIBANA_URL/api/saved_objects/search/coverage-events-search" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d '{
        "attributes": {
            "title": "Coverage Events",
            "description": "Basic block coverage telemetry",
            "columns": ["job_id", "payload_total_bbs", "payload_bb_ids", "indexed_at"],
            "sort": [["indexed_at", "desc"]],
            "kibanaSavedObjectMeta": {
                "searchSourceJSON": "{\"index\":\"telemetry-star\",\"query\":{\"query\":\"event_type.keyword: coverage\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        "references": [
            {"name": "kibanaSavedObjectMeta.searchSourceJSON.index", "type": "index-pattern", "id": "telemetry-star"}
        ]
    }' 2>/dev/null > /dev/null

echo "[+] Saved search created: Coverage Events"

# Saved search: Run results
echo "[*] Creating saved search: run-results"
curl -X POST "$KIBANA_URL/api/saved_objects/search/run-results-search" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d '{
        "attributes": {
            "title": "Run Results",
            "description": "Final execution outcomes",
            "columns": ["run_id", "job_id", "status", "elapsed_seconds", "artifact_name", "worker_id"],
            "sort": [["timestamp", "desc"]],
            "kibanaSavedObjectMeta": {
                "searchSourceJSON": "{\"index\":\"runs-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        "references": [
            {"name": "kibanaSavedObjectMeta.searchSourceJSON.index", "type": "index-pattern", "id": "runs-star"}
        ]
    }' 2>/dev/null > /dev/null

echo "[+] Saved search created: Run Results"

# Step 3: Create Dashboard with Discover Panels
echo ""
echo "=== Step 3: Creating Dashboard ==="

# Create dashboard with embedded searches
dashboard_json='{
    "attributes": {
        "title": "Artifact Execution Analysis",
        "description": "Real-time artifact execution telemetry and results",
        "panelsJSON": "[{\"version\":\"8.0.0\",\"type\":\"search\",\"gridData\":{\"x\":0,\"y\":0,\"w\":48,\"h\":12,\"i\":\"panel-1\"},\"panelIndex\":\"panel-1\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-1\"},{\"version\":\"8.0.0\",\"type\":\"search\",\"gridData\":{\"x\":0,\"y\":12,\"w\":48,\"h\":15,\"i\":\"panel-2\"},\"panelIndex\":\"panel-2\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-2\"}]",
        "optionsJSON": "{\"useMargins\":true,\"hidePanelTitles\":false}",
        "version": 1,
        "timeRestore": true,
        "timeTo": "now",
        "timeFrom": "now-24h",
        "refreshInterval": {
            "pause": false,
            "value": 30000
        },
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"query\":{\"language\":\"kuery\",\"query\":\"\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name": "panel_panel-1", "type": "search", "id": "run-results-search"},
        {"name": "panel_panel-2", "type": "search", "id": "coverage-events-search"}
    ]
}'

result=$(curl -X POST "$KIBANA_URL/api/saved_objects/dashboard/artifact-execution-dashboard?overwrite=true" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d "$dashboard_json" 2>/dev/null)

if echo "$result" | grep -q "artifact-execution-dashboard"; then
    echo "[+] Dashboard created: Artifact Execution Analysis"
else
    echo "[!] Dashboard creation may have issues, check Kibana logs"
fi

# Step 4: Summary
echo ""
echo "=== Dashboard Setup Complete! ==="
echo ""
echo "Dashboard URL: $KIBANA_URL/app/dashboards#/view/artifact-execution-dashboard"
echo ""
echo "Index Patterns Created:"
echo "  - telemetry-* (time field: indexed_at)"
echo "  - runs-* (time field: timestamp)"
echo ""
echo "Saved Searches Created:"
echo "  - Coverage Events (job_id, payload_total_bbs, payload_bb_ids)"
echo "  - Run Results (run_id, status, elapsed_seconds, artifact_name)"
echo ""
echo "=== Next Steps: Customize Visualizations ==="
echo ""
echo "The dashboard now shows raw data. To add custom visualizations:"
echo ""
echo "1. Open dashboard: $KIBANA_URL/app/dashboards#/view/artifact-execution-dashboard"
echo "2. Click 'Edit'"
echo "3. Click 'Create visualization' or 'Add from library'"
echo "4. Choose from recommended visualizations below:"
echo ""
echo "   A. Run Status Distribution (Pie Chart)"
echo "      - Index: runs-*"
echo "      - Slice by: status.keyword"
echo "      - Metric: Count"
echo ""
echo "   B. BB Coverage Timeline (Line Chart)"
echo "      - Index: telemetry-*"
echo "      - Filter: event_type.keyword = 'coverage'"
echo "      - X-axis: Date histogram on indexed_at"
echo "      - Y-axis: Max of payload_total_bbs"
echo ""
echo "   C. Execution Time Distribution (Histogram)"
echo "      - Index: runs-*"
echo "      - X-axis: Histogram on elapsed_seconds (interval: 5)"
echo "      - Y-axis: Count"
echo ""
echo "   D. Top Workers by Run Count (Bar Chart)"
echo "      - Index: runs-*"
echo "      - X-axis: Terms aggregation on worker_id.keyword"
echo "      - Y-axis: Count"
echo ""
echo "   E. Telemetry Events by Type (Pie Chart)"
echo "      - Index: telemetry-*"
echo "      - Slice by: event_type.keyword"
echo "      - Metric: Count"
echo ""
echo "   F. Recent Runs Table (Data Table)"
echo "      - Index: runs-*"
echo "      - Columns: run_id, job_id, status, elapsed_seconds, artifact_name"
echo "      - Sort: timestamp DESC"
echo ""
echo "5. Save each visualization and arrange on dashboard"
echo "6. Click 'Save' to persist dashboard layout"
echo ""
echo "=== Quick Filters ==="
echo ""
echo "Add these filters in Kibana to focus on specific runs:"
echo "  - job_id.keyword: \"job-000001\"       (specific job)"
echo "  - status.keyword: \"success\"          (only successful runs)"
echo "  - status.keyword: \"error\" OR \"timeout\" (only failures)"
echo "  - worker_id.keyword: \"worker-01\"     (specific worker)"
echo ""
echo "[+] Dashboard ready! See automation/ELASTICSEARCH-DATA-SCHEMA.md for field reference"
