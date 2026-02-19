#!/bin/bash
# Automated Kibana Dashboard Setup Script
# Creates index patterns, saved searches, and dashboard
# Based on ELASTICSEARCH-DATA-SCHEMA.md (verified against controller/src/main.rs)

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

KIBANA_VERSION=$(curl -s "$KIBANA_URL/api/status" | jq -r '.version.number')
echo "[+] Kibana accessible (version: $KIBANA_VERSION)"

# Step 1: Create Index Patterns with CORRECT time fields per schema
echo ""
echo "=== Step 1: Creating Index Patterns ==="

# Telemetry index pattern - time field: indexed_at (per ELASTICSEARCH-DATA-SCHEMA.md line 102)
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

echo "[+] Index pattern telemetry-* created (time field: indexed_at)"

# Runs index pattern - time field: timestamp (per ELASTICSEARCH-DATA-SCHEMA.md line 190)
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

echo "[+] Index pattern runs-* created (time field: timestamp)"

# Step 2: Create Saved Searches with CORRECT field names per schema
echo ""
echo "=== Step 2: Creating Saved Searches ==="

# Coverage events - uses payload_total_bbs (per schema line 155)
echo "[*] Creating saved search: Coverage Events"
curl -X POST "$KIBANA_URL/api/saved_objects/search/coverage-events-search" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d '{
        "attributes": {
            "title": "Coverage Events",
            "description": "BB coverage telemetry (event_type=coverage, fields: payload_total_bbs, payload_bb_ids, payload_hit_counts)",
            "columns": ["job_id", "event_type", "payload_total_bbs", "indexed_at"],
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

# Run results - uses actual controller fields (from main.rs:565-578)
echo "[*] Creating saved search: Run Results"
curl -X POST "$KIBANA_URL/api/saved_objects/search/run-results-search" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d '{
        "attributes": {
            "title": "Run Results",
            "description": "Final execution outcomes (index: runs-YYYY.MM, fields: run_id, job_id, status, elapsed_seconds, artifact_name, worker_id, exit_code, telemetry_events_count, timestamp)",
            "columns": ["timestamp", "run_id", "job_id", "status", "exit_code", "elapsed_seconds", "artifact_name", "worker_id", "telemetry_events_count"],
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

# All telemetry events
echo "[*] Creating saved search: All Telemetry"
curl -X POST "$KIBANA_URL/api/saved_objects/search/all-telemetry-search" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d '{
        "attributes": {
            "title": "All Telemetry Events",
            "description": "All telemetry data (coverage, trace, checkpoint, etw, etc.)",
            "columns": ["job_id", "event_type", "timestamp", "indexed_at"],
            "sort": [["indexed_at", "desc"]],
            "kibanaSavedObjectMeta": {
                "searchSourceJSON": "{\"index\":\"telemetry-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        "references": [
            {"name": "kibanaSavedObjectMeta.searchSourceJSON.index", "type": "index-pattern", "id": "telemetry-star"}
        ]
    }' 2>/dev/null > /dev/null

echo "[+] Saved search created: All Telemetry"

# Step 3: Create Dashboard with Data Tables
echo ""
echo "=== Step 3: Creating Dashboard ==="

# Dashboard with 3 panels matching schema documentation
dashboard_json='{
    "attributes": {
        "title": "Artifact Execution Analysis",
        "description": "Telemetry: telemetry-YYYY.MM.DD (indexed_at), Runs: runs-YYYY.MM (timestamp). Filter by: job_id.keyword, status.keyword, event_type.keyword",
        "panelsJSON": "[{\"version\":\"8.11.0\",\"type\":\"search\",\"gridData\":{\"x\":0,\"y\":0,\"w\":24,\"h\":15,\"i\":\"1\"},\"panelIndex\":\"1\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_1\"},{\"version\":\"8.11.0\",\"type\":\"search\",\"gridData\":{\"x\":24,\"y\":0,\"w\":24,\"h\":15,\"i\":\"2\"},\"panelIndex\":\"2\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_2\"},{\"version\":\"8.11.0\",\"type\":\"search\",\"gridData\":{\"x\":0,\"y\":15,\"w\":48,\"h\":15,\"i\":\"3\"},\"panelIndex\":\"3\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_3\"}]",
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
        {"name": "panel_1", "type": "search", "id": "run-results-search"},
        {"name": "panel_2", "type": "search", "id": "coverage-events-search"},
        {"name": "panel_3", "type": "search", "id": "all-telemetry-search"}
    ]
}'

curl -X POST "$KIBANA_URL/api/saved_objects/dashboard/artifact-execution-dashboard?overwrite=true" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d "$dashboard_json" 2>/dev/null | jq -r '.id // "created"' > /dev/null

echo "[+] Dashboard created: Artifact Execution Analysis"

# Step 4: Summary
echo ""
echo "=== Dashboard Setup Complete! ==="
echo ""
echo "Dashboard URL: $KIBANA_URL/app/dashboards#/view/artifact-execution-dashboard"
echo ""
echo "=== Index Patterns (per ELASTICSEARCH-DATA-SCHEMA.md) ==="
echo "  ✓ telemetry-* (time: indexed_at) → telemetry-YYYY.MM.DD indices"
echo "  ✓ runs-* (time: timestamp) → runs-YYYY.MM indices"
echo ""
echo "=== Data Tables ==="
echo "  ✓ Run Results: timestamp, run_id, job_id, status, exit_code, elapsed_seconds, artifact_name, worker_id, telemetry_events_count"
echo "  ✓ Coverage Events: job_id, event_type, payload_total_bbs, indexed_at"
echo "  ✓ All Telemetry: job_id, event_type, timestamp, indexed_at"
echo ""
echo "=== Dashboard Layout ==="
echo "  Row 1: [Run Results (left 50%)] [Coverage Events (right 50%)]"
echo "  Row 2: [All Telemetry Events (full width 100%)]"
echo ""
echo "=== Verified Field Names (from schema) ==="
echo "  Telemetry fields:"
echo "    - job_id (keyword)"
echo "    - event_type (keyword): 'coverage', 'trace', 'checkpoint', 'etw', etc."
echo "    - timestamp (long): Unix ms"
echo "    - indexed_at (date): RFC3339"
echo "    - payload_total_bbs (integer): For event_type=coverage"
echo "    - payload_bb_ids (array): For event_type=coverage"
echo "    - payload_hit_counts (array): For event_type=coverage"
echo "    - payload_seq, payload_file, payload_line (for event_type=trace)"
echo ""
echo "  Runs fields (controller/src/main.rs:565-578):"
echo "    - run_id (keyword): Unique execution identifier"
echo "    - job_id (keyword): Job identifier (e.g., job-000001/round-1/baseline)"
echo "    - status (keyword): 'success', 'error', 'timeout'"
echo "    - elapsed_seconds (integer): Execution duration"
echo "    - artifact_name (keyword): 'baseline' or 'instrumented'"
echo "    - worker_id (keyword): Worker that executed the run"
echo "    - worker_ip (ip): Worker IP address"
echo "    - exit_code (integer): Process exit code"
echo "    - telemetry_events_count (integer): Number of telemetry events collected"
echo "    - output (text): Execution output/error message"
echo "    - telemetry_ids (array): References to telemetry data"
echo "    - timestamp (date): When indexed (RFC3339)"
echo ""
echo "=== Quick Filters (use in dashboard) ==="
echo "  job_id.keyword: \"job-000001\"          # Filter to specific job"
echo "  status.keyword: \"success\"             # Only successful runs"
echo "  status.keyword: (\"error\" OR \"timeout\") # Only failures"
echo "  event_type.keyword: \"coverage\"        # Only coverage events"
echo "  worker_id.keyword: \"worker-01\"        # Filter to worker"
echo ""
echo "=== Create Visualizations Manually (in Kibana UI) ==="
echo "  1. Run Status Pie: runs-* → Pie → slice by status.keyword"
echo "  2. BB Coverage Metric: telemetry-* → Metric → filter event_type=coverage → max(payload_total_bbs)"
echo "  3. Event Type Pie: telemetry-* → Pie → slice by event_type.keyword"
echo "  4. Coverage Timeline: telemetry-* → Line → filter event_type=coverage → X: date_histogram(indexed_at), Y: max(payload_total_bbs)"
echo "  5. Exec Time Histogram: runs-* → Vertical Bar → X: histogram(elapsed_seconds, interval=5), Y: count"
echo ""
echo "=== Verify Data ==="
echo "  curl http://localhost:9200/telemetry-*/_count  # Should return doc count"
echo "  curl http://localhost:9200/runs-*/_count       # Should return doc count"
echo ""
echo "[+] Dashboard ready! Data will appear once artifact executions complete."
echo "[+] See automation/ELASTICSEARCH-DATA-SCHEMA.md for complete reference"
