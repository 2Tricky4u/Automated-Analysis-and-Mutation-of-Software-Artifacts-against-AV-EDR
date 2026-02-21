#!/bin/bash
# Job & Run Drill-Down Kibana Dashboard Setup
# Creates index patterns (jobs-*, rounds-*), saved searches, visualizations,
# and a unified dashboard with per-job filtering across all 4 indices.
#
# Prerequisites:
#   - Kibana 8.x running
#   - create-kibana-dashboard.sh already run (provides telemetry-star, runs-star)
#   - jq installed
#
# Field names verified against:
#   controller/src/storage/jobs.rs
#   controller/src/storage/rounds.rs
#   controller/src/storage/runs.rs
#   controller/src/storage/telemetry.rs

set -e

KIBANA_URL="${KIBANA_URL:-http://localhost:5601}"
ES_URL="${ES_URL:-http://localhost:9200}"

echo "[*] Job & Run Drill-Down Dashboard Setup"
echo "[*] Kibana URL: $KIBANA_URL"
echo "[*] Elasticsearch URL: $ES_URL"

# ── Prerequisites ──────────────────────────────────────────────────────
if ! command -v jq &>/dev/null; then
    echo "[!] ERROR: jq is required but not installed"
    exit 1
fi

if ! curl -s "$KIBANA_URL/api/status" > /dev/null; then
    echo "[!] ERROR: Cannot connect to Kibana at $KIBANA_URL"
    exit 1
fi

KIBANA_VERSION=$(curl -s "$KIBANA_URL/api/status" | jq -r '.version.number')
echo "[+] Kibana accessible (version: $KIBANA_VERSION)"

# Check prerequisite index patterns exist
for pattern in telemetry-star runs-star; do
    if ! curl -sf "$KIBANA_URL/api/saved_objects/index-pattern/$pattern" \
         -H 'kbn-xsrf: true' > /dev/null 2>&1; then
        echo "[!] ERROR: Index pattern '$pattern' not found."
        echo "    Run create-kibana-dashboard.sh first."
        exit 1
    fi
done
echo "[+] Prerequisite index patterns exist (telemetry-star, runs-star)"

# Helper: create a saved object with overwrite
create_object() {
    local obj_type=$1
    local obj_id=$2
    local json_body=$3

    curl -s -X POST "$KIBANA_URL/api/saved_objects/$obj_type/$obj_id?overwrite=true" \
        -H 'kbn-xsrf: true' \
        -H 'Content-Type: application/json' \
        -d "$json_body" 2>/dev/null | jq -r '.id // "error"' > /dev/null
}

# ── Step 1: Index Patterns ─────────────────────────────────────────────
echo ""
echo "=== Step 1: Creating Index Patterns ==="

echo "[*] Creating index pattern: jobs-*"
create_object "index-pattern" "jobs-star" '{
    "attributes": {
        "title": "jobs-*",
        "timeFieldName": "created_at"
    }
}'
echo "[+] Index pattern jobs-* created (time field: created_at)"

echo "[*] Creating index pattern: rounds-*"
create_object "index-pattern" "rounds-star" '{
    "attributes": {
        "title": "rounds-*",
        "timeFieldName": "completed_at"
    }
}'
echo "[+] Index pattern rounds-* created (time field: completed_at)"

# ── Step 2: Saved Searches ─────────────────────────────────────────────
echo ""
echo "=== Step 2: Creating Saved Searches (7 data tables) ==="

# 2a) Job Overview
echo "[*] Creating saved search: Job Overview"
create_object "search" "job-overview-search" '{
    "attributes": {
        "title": "Job Overview",
        "description": "All jobs with status, template, encoding, round progress, and module config",
        "columns": ["job_id","status","template_name","encoding","current_round","max_rounds","modules.carrier","modules.decoder","created_at"],
        "sort": [["created_at","desc"]],
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"index\":\"jobs-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name":"kibanaSavedObjectMeta.searchSourceJSON.index","type":"index-pattern","id":"jobs-star"}
    ]
}'
echo "[+] Saved search created: Job Overview"

# 2b) Job Modules
echo "[*] Creating saved search: Job Modules"
create_object "search" "job-modules-search" '{
    "attributes": {
        "title": "Job Modules",
        "description": "Per-job module selection and outcome",
        "columns": ["job_id","modules.carrier","modules.decoder","modules.antiemulation","modules.deconditioner","modules.guardrail","modules.virtualprotect","modules.decoy","outcome.detection_outcome"],
        "sort": [["created_at","desc"]],
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"index\":\"jobs-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name":"kibanaSavedObjectMeta.searchSourceJSON.index","type":"index-pattern","id":"jobs-star"}
    ]
}'
echo "[+] Saved search created: Job Modules"

# 2c) Rounds by Job
echo "[*] Creating saved search: Rounds by Job"
create_object "search" "rounds-by-job-search" '{
    "attributes": {
        "title": "Rounds by Job",
        "description": "Round results with detection, differential category, evasion score",
        "columns": ["round_id","job_id","round_number","detected","differential_category","evasion_score","mutations","completed_at"],
        "sort": [["completed_at","desc"]],
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"index\":\"rounds-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name":"kibanaSavedObjectMeta.searchSourceJSON.index","type":"index-pattern","id":"rounds-star"}
    ]
}'
echo "[+] Saved search created: Rounds by Job"

# 2d) Round Modules
echo "[*] Creating saved search: Round Modules"
create_object "search" "rounds-modules-search" '{
    "attributes": {
        "title": "Round Modules",
        "description": "Per-round module selection with detection outcome",
        "columns": ["round_id","job_id","modules.carrier","modules.decoder","modules.antiemulation","detected","evasion_score"],
        "sort": [["completed_at","desc"]],
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"index\":\"rounds-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name":"kibanaSavedObjectMeta.searchSourceJSON.index","type":"index-pattern","id":"rounds-star"}
    ]
}'
echo "[+] Saved search created: Round Modules"

# 2e) Run Drill-down
echo "[*] Creating saved search: Run Drill-down"
create_object "search" "runs-drilldown-search" '{
    "attributes": {
        "title": "Run Drill-down",
        "description": "Per-run details: detection outcome, checkpoint, exit code, timing, trace mode",
        "columns": ["run_id","job_id","round_id","run_type","detected","detection_outcome","last_checkpoint","exit_code","elapsed_ms","trace_mode"],
        "sort": [["timestamp","desc"]],
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"index\":\"runs-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name":"kibanaSavedObjectMeta.searchSourceJSON.index","type":"index-pattern","id":"runs-star"}
    ]
}'
echo "[+] Saved search created: Run Drill-down"

# 2f) Run Errors
echo "[*] Creating saved search: Run Errors"
create_object "search" "runs-errors-search" '{
    "attributes": {
        "title": "Run Errors",
        "description": "Runs with errors (filtered to error.class exists)",
        "columns": ["run_id","job_id","status","error.class","error.message","exit_code"],
        "sort": [["timestamp","desc"]],
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"index\":\"runs-star\",\"query\":{\"query\":\"error.class: *\",\"language\":\"kuery\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name":"kibanaSavedObjectMeta.searchSourceJSON.index","type":"index-pattern","id":"runs-star"}
    ]
}'
echo "[+] Saved search created: Run Errors"

# 2g) Telemetry per Run
echo "[*] Creating saved search: Telemetry per Run"
create_object "search" "telemetry-per-run-search" '{
    "attributes": {
        "title": "Telemetry per Run",
        "description": "Telemetry events correlated by run_id, round_id, job_id",
        "columns": ["run_id","job_id","round_id","event_type","payload_checkpoint_name","payload_file","payload_line","indexed_at"],
        "sort": [["indexed_at","desc"]],
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"index\":\"telemetry-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name":"kibanaSavedObjectMeta.searchSourceJSON.index","type":"index-pattern","id":"telemetry-star"}
    ]
}'
echo "[+] Saved search created: Telemetry per Run"

# ── Step 3: Visualizations (agg-based, jq for safe JSON) ──────────────
echo ""
echo "=== Step 3: Creating Visualizations (6 agg-based charts) ==="

# 3a) Job Status Pie
echo "[*] Creating visualization: Job Status Pie"
VIS_STATE=$(jq -n '{
    title: "Job Status",
    type: "pie",
    aggs: [
        { id: "1", enabled: true, type: "count", params: {}, schema: "metric" },
        { id: "2", enabled: true, type: "terms", params: { field: "status", orderBy: "1", order: "desc", size: 10 }, schema: "segment" }
    ],
    params: {
        type: "pie",
        addTooltip: true,
        addLegend: true,
        legendPosition: "right",
        isDonut: true,
        labels: { show: true, values: true, last_level: true, truncate: 100 }
    }
}')
create_object "visualization" "viz-job-status-pie" "$(jq -n \
    --arg visState "$VIS_STATE" \
    '{
        attributes: {
            title: "Job Status",
            visState: $visState,
            uiStateJSON: "{}",
            description: "Donut chart of job statuses (queued, running, completed, failed)",
            kibanaSavedObjectMeta: {
                searchSourceJSON: "{\"index\":\"jobs-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        references: [
            { name: "kibanaSavedObjectMeta.searchSourceJSON.index", type: "index-pattern", id: "jobs-star" }
        ]
    }')"
echo "[+] Visualization created: Job Status Pie"

# 3b) Detection Outcome Pie
echo "[*] Creating visualization: Detection Outcome Pie"
VIS_STATE=$(jq -n '{
    title: "Detection Outcome",
    type: "pie",
    aggs: [
        { id: "1", enabled: true, type: "count", params: {}, schema: "metric" },
        { id: "2", enabled: true, type: "terms", params: { field: "detection_outcome", orderBy: "1", order: "desc", size: 10 }, schema: "segment" }
    ],
    params: {
        type: "pie",
        addTooltip: true,
        addLegend: true,
        legendPosition: "right",
        isDonut: true,
        labels: { show: true, values: true, last_level: true, truncate: 100 }
    }
}')
create_object "visualization" "viz-detection-outcome-pie" "$(jq -n \
    --arg visState "$VIS_STATE" \
    '{
        attributes: {
            title: "Detection Outcome",
            visState: $visState,
            uiStateJSON: "{}",
            description: "Donut chart of detection outcomes (FULL_EVASION, KILLED_PRE_PAYLOAD, etc.)",
            kibanaSavedObjectMeta: {
                searchSourceJSON: "{\"index\":\"runs-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        references: [
            { name: "kibanaSavedObjectMeta.searchSourceJSON.index", type: "index-pattern", id: "runs-star" }
        ]
    }')"
echo "[+] Visualization created: Detection Outcome Pie"

# 3c) Differential Category Pie
echo "[*] Creating visualization: Differential Category Pie"
VIS_STATE=$(jq -n '{
    title: "Differential Category",
    type: "pie",
    aggs: [
        { id: "1", enabled: true, type: "count", params: {}, schema: "metric" },
        { id: "2", enabled: true, type: "terms", params: { field: "differential_category.keyword", orderBy: "1", order: "desc", size: 10 }, schema: "segment" }
    ],
    params: {
        type: "pie",
        addTooltip: true,
        addLegend: true,
        legendPosition: "right",
        isDonut: true,
        labels: { show: true, values: true, last_level: true, truncate: 100 }
    }
}')
create_object "visualization" "viz-differential-category-pie" "$(jq -n \
    --arg visState "$VIS_STATE" \
    '{
        attributes: {
            title: "Differential Category",
            visState: $visState,
            uiStateJSON: "{}",
            description: "Donut chart of differential categories (real_detection, instrumentation_artifact, full_evasion)",
            kibanaSavedObjectMeta: {
                searchSourceJSON: "{\"index\":\"rounds-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        references: [
            { name: "kibanaSavedObjectMeta.searchSourceJSON.index", type: "index-pattern", id: "rounds-star" }
        ]
    }')"
echo "[+] Visualization created: Differential Category Pie"

# 3d) Evasion Score Histogram
echo "[*] Creating visualization: Evasion Score Histogram"
VIS_STATE=$(jq -n '{
    title: "Evasion Score Distribution",
    type: "histogram",
    aggs: [
        { id: "1", enabled: true, type: "count", params: {}, schema: "metric" },
        { id: "2", enabled: true, type: "histogram", params: { field: "evasion_score", interval: 0.1, min_doc_count: 1, extended_bounds: {} }, schema: "segment" }
    ],
    params: {
        type: "histogram",
        grid: { categoryLines: false, valueAxis: "ValueAxis-1" },
        categoryAxes: [{ id: "CategoryAxis-1", type: "category", position: "bottom", show: true, style: {}, scale: { type: "linear" }, labels: { show: true, filter: true, truncate: 100 }, title: {} }],
        valueAxes: [{ id: "ValueAxis-1", name: "LeftAxis-1", type: "value", position: "left", show: true, style: {}, scale: { type: "linear", mode: "normal", defaultYExtents: false }, labels: { show: true, rotate: 0, filter: false, truncate: 100 }, title: { text: "Count" } }],
        seriesParams: [{ show: true, type: "histogram", mode: "stacked", data: { label: "Count", id: "1" }, valueAxis: "ValueAxis-1", drawLinesBetweenPoints: true, lineWidth: 2, showCircles: true }],
        addTooltip: true,
        addLegend: true,
        legendPosition: "right",
        times: [],
        addTimeMarker: false
    }
}')
create_object "visualization" "viz-evasion-score-histogram" "$(jq -n \
    --arg visState "$VIS_STATE" \
    '{
        attributes: {
            title: "Evasion Score Distribution",
            visState: $visState,
            uiStateJSON: "{}",
            description: "Histogram of evasion scores (0.0 to 1.0, interval 0.1)",
            kibanaSavedObjectMeta: {
                searchSourceJSON: "{\"index\":\"rounds-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        references: [
            { name: "kibanaSavedObjectMeta.searchSourceJSON.index", type: "index-pattern", id: "rounds-star" }
        ]
    }')"
echo "[+] Visualization created: Evasion Score Histogram"

# 3e) Run Type Breakdown Pie
echo "[*] Creating visualization: Run Type Breakdown"
VIS_STATE=$(jq -n '{
    title: "Run Type Breakdown",
    type: "pie",
    aggs: [
        { id: "1", enabled: true, type: "count", params: {}, schema: "metric" },
        { id: "2", enabled: true, type: "terms", params: { field: "run_type", orderBy: "1", order: "desc", size: 10 }, schema: "segment" }
    ],
    params: {
        type: "pie",
        addTooltip: true,
        addLegend: true,
        legendPosition: "right",
        isDonut: true,
        labels: { show: true, values: true, last_level: true, truncate: 100 }
    }
}')
create_object "visualization" "viz-run-type-breakdown" "$(jq -n \
    --arg visState "$VIS_STATE" \
    '{
        attributes: {
            title: "Run Type Breakdown",
            visState: $visState,
            uiStateJSON: "{}",
            description: "Donut chart of run types (baseline vs instrumented)",
            kibanaSavedObjectMeta: {
                searchSourceJSON: "{\"index\":\"runs-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        references: [
            { name: "kibanaSavedObjectMeta.searchSourceJSON.index", type: "index-pattern", id: "runs-star" }
        ]
    }')"
echo "[+] Visualization created: Run Type Breakdown"

# 3f) Elapsed Time Histogram
echo "[*] Creating visualization: Elapsed Time Histogram"
VIS_STATE=$(jq -n '{
    title: "Elapsed Time Distribution",
    type: "histogram",
    aggs: [
        { id: "1", enabled: true, type: "count", params: {}, schema: "metric" },
        { id: "2", enabled: true, type: "histogram", params: { field: "elapsed_ms", interval: 5000, min_doc_count: 1, extended_bounds: {} }, schema: "segment" }
    ],
    params: {
        type: "histogram",
        grid: { categoryLines: false, valueAxis: "ValueAxis-1" },
        categoryAxes: [{ id: "CategoryAxis-1", type: "category", position: "bottom", show: true, style: {}, scale: { type: "linear" }, labels: { show: true, filter: true, truncate: 100 }, title: {} }],
        valueAxes: [{ id: "ValueAxis-1", name: "LeftAxis-1", type: "value", position: "left", show: true, style: {}, scale: { type: "linear", mode: "normal", defaultYExtents: false }, labels: { show: true, rotate: 0, filter: false, truncate: 100 }, title: { text: "Count" } }],
        seriesParams: [{ show: true, type: "histogram", mode: "stacked", data: { label: "Count", id: "1" }, valueAxis: "ValueAxis-1", drawLinesBetweenPoints: true, lineWidth: 2, showCircles: true }],
        addTooltip: true,
        addLegend: true,
        legendPosition: "right",
        times: [],
        addTimeMarker: false
    }
}')
create_object "visualization" "viz-elapsed-time-histogram" "$(jq -n \
    --arg visState "$VIS_STATE" \
    '{
        attributes: {
            title: "Elapsed Time Distribution",
            visState: $visState,
            uiStateJSON: "{}",
            description: "Histogram of run elapsed time in ms (interval 5000ms)",
            kibanaSavedObjectMeta: {
                searchSourceJSON: "{\"index\":\"runs-star\",\"query\":{\"query\":\"\",\"language\":\"kuery\"},\"filter\":[]}"
            }
        },
        references: [
            { name: "kibanaSavedObjectMeta.searchSourceJSON.index", type: "index-pattern", id: "runs-star" }
        ]
    }')"
echo "[+] Visualization created: Elapsed Time Histogram"

# ── Step 4: Dashboard ──────────────────────────────────────────────────
echo ""
echo "=== Step 4: Creating Dashboard ==="

# Build panelsJSON with 48-col grid layout
# Row 0 (y=0,  h=8):  3 pie charts
# Row 1 (y=8,  h=8):  evasion hist + run type pie + elapsed hist
# Row 2 (y=16, h=15): job overview + rounds by job
# Row 3 (y=31, h=12): job modules + round modules
# Row 4 (y=43, h=15): run drill-down (full width)
# Row 5 (y=58, h=12): run errors + telemetry per run

PANELS_JSON=$(jq -n '[
    { version: "8.11.0", type: "visualization", gridData: { x: 0,  y: 0,  w: 16, h: 8,  i: "p1"  }, panelIndex: "p1",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p1" },
    { version: "8.11.0", type: "visualization", gridData: { x: 16, y: 0,  w: 16, h: 8,  i: "p2"  }, panelIndex: "p2",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p2" },
    { version: "8.11.0", type: "visualization", gridData: { x: 32, y: 0,  w: 16, h: 8,  i: "p3"  }, panelIndex: "p3",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p3" },
    { version: "8.11.0", type: "visualization", gridData: { x: 0,  y: 8,  w: 16, h: 8,  i: "p4"  }, panelIndex: "p4",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p4" },
    { version: "8.11.0", type: "visualization", gridData: { x: 16, y: 8,  w: 16, h: 8,  i: "p5"  }, panelIndex: "p5",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p5" },
    { version: "8.11.0", type: "visualization", gridData: { x: 32, y: 8,  w: 16, h: 8,  i: "p6"  }, panelIndex: "p6",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p6" },
    { version: "8.11.0", type: "search",        gridData: { x: 0,  y: 16, w: 24, h: 15, i: "p7"  }, panelIndex: "p7",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p7" },
    { version: "8.11.0", type: "search",        gridData: { x: 24, y: 16, w: 24, h: 15, i: "p8"  }, panelIndex: "p8",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p8" },
    { version: "8.11.0", type: "search",        gridData: { x: 0,  y: 31, w: 24, h: 12, i: "p9"  }, panelIndex: "p9",  embeddableConfig: { enhancements: {} }, panelRefName: "panel_p9" },
    { version: "8.11.0", type: "search",        gridData: { x: 24, y: 31, w: 24, h: 12, i: "p10" }, panelIndex: "p10", embeddableConfig: { enhancements: {} }, panelRefName: "panel_p10" },
    { version: "8.11.0", type: "search",        gridData: { x: 0,  y: 43, w: 48, h: 15, i: "p11" }, panelIndex: "p11", embeddableConfig: { enhancements: {} }, panelRefName: "panel_p11" },
    { version: "8.11.0", type: "search",        gridData: { x: 0,  y: 58, w: 24, h: 12, i: "p12" }, panelIndex: "p12", embeddableConfig: { enhancements: {} }, panelRefName: "panel_p12" },
    { version: "8.11.0", type: "search",        gridData: { x: 24, y: 58, w: 24, h: 12, i: "p13" }, panelIndex: "p13", embeddableConfig: { enhancements: {} }, panelRefName: "panel_p13" }
]' | jq -c .)

DASHBOARD_JSON=$(jq -n \
    --arg panels "$PANELS_JSON" \
    '{
        attributes: {
            title: "Job & Run Drill-Down",
            description: "Per-job and per-run drill-down across jobs, rounds, runs, and telemetry. Filter by job_id to scope all panels.",
            panelsJSON: $panels,
            optionsJSON: "{\"useMargins\":true,\"hidePanelTitles\":false}",
            version: 1,
            timeRestore: true,
            timeTo: "now",
            timeFrom: "now-7d",
            refreshInterval: { pause: false, value: 30000 },
            kibanaSavedObjectMeta: {
                searchSourceJSON: "{\"query\":{\"language\":\"kuery\",\"query\":\"\"},\"filter\":[]}"
            }
        },
        references: [
            { name: "panel_p1",  type: "visualization", id: "viz-job-status-pie" },
            { name: "panel_p2",  type: "visualization", id: "viz-detection-outcome-pie" },
            { name: "panel_p3",  type: "visualization", id: "viz-differential-category-pie" },
            { name: "panel_p4",  type: "visualization", id: "viz-evasion-score-histogram" },
            { name: "panel_p5",  type: "visualization", id: "viz-run-type-breakdown" },
            { name: "panel_p6",  type: "visualization", id: "viz-elapsed-time-histogram" },
            { name: "panel_p7",  type: "search",        id: "job-overview-search" },
            { name: "panel_p8",  type: "search",        id: "rounds-by-job-search" },
            { name: "panel_p9",  type: "search",        id: "job-modules-search" },
            { name: "panel_p10", type: "search",        id: "rounds-modules-search" },
            { name: "panel_p11", type: "search",        id: "runs-drilldown-search" },
            { name: "panel_p12", type: "search",        id: "runs-errors-search" },
            { name: "panel_p13", type: "search",        id: "telemetry-per-run-search" }
        ]
    }')

create_object "dashboard" "job-run-drilldown-dashboard" "$DASHBOARD_JSON"
echo "[+] Dashboard created: Job & Run Drill-Down"

# ── Summary ────────────────────────────────────────────────────────────
echo ""
echo "=== Dashboard Setup Complete! ==="
echo ""
echo "Dashboard URL: $KIBANA_URL/app/dashboards#/view/job-run-drilldown-dashboard"
echo ""
echo "=== Created Objects (16) ==="
echo "  Index Patterns (2):"
echo "    jobs-*   (time: created_at)"
echo "    rounds-* (time: completed_at)"
echo ""
echo "  Saved Searches (7):"
echo "    Job Overview          — jobs-*:      job_id, status, template, encoding, rounds, modules"
echo "    Job Modules           — jobs-*:      all 7 module slots + outcome"
echo "    Rounds by Job         — rounds-*:    round_id, detected, differential, evasion_score"
echo "    Round Modules         — rounds-*:    modules vs detection/evasion"
echo "    Run Drill-down        — runs-*:      full run details with detection outcome"
echo "    Run Errors            — runs-*:      filtered to error.class exists"
echo "    Telemetry per Run     — telemetry-*: events by run_id/round_id/job_id"
echo ""
echo "  Visualizations (6):"
echo "    Job Status Pie            — jobs-*:   status breakdown"
echo "    Detection Outcome Pie     — runs-*:   FULL_EVASION vs KILLED_*"
echo "    Differential Category Pie — rounds-*: real_detection vs artifact vs evasion"
echo "    Evasion Score Histogram   — rounds-*: score distribution (0.0-1.0)"
echo "    Run Type Breakdown        — runs-*:   baseline vs instrumented"
echo "    Elapsed Time Histogram    — runs-*:   execution time distribution"
echo ""
echo "  Dashboard (1):"
echo "    Job & Run Drill-Down — 13 panels, 48-col grid"
echo ""
echo "=== Layout ==="
echo "  Row 0: [Job Status Pie] [Detection Outcome Pie] [Differential Category Pie]"
echo "  Row 1: [Evasion Score Hist] [Run Type Pie] [Elapsed Time Hist]"
echo "  Row 2: [Job Overview] [Rounds by Job]"
echo "  Row 3: [Job Modules] [Round Modules]"
echo "  Row 4: [Run Drill-down (full width)]"
echo "  Row 5: [Run Errors] [Telemetry per Run]"
echo ""
echo "=== Filtering ==="
echo "  All indices share job_id — add 'job_id: \"job-xxx\"' in Kibana filter bar"
echo "  to scope every panel to a single job."
echo ""
echo "[+] Dashboard ready!"
