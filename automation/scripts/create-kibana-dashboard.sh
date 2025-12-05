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

# Step 3: Create Visualizations using Lens API
echo ""
echo "=== Step 3: Creating Visualizations ==="

# Helper function to create Lens visualization
create_lens_viz() {
    local viz_id=$1
    local viz_title=$2
    local state=$3

    echo "[*] Creating visualization: $viz_title"

    curl -X POST "$KIBANA_URL/api/saved_objects/lens/$viz_id" \
        -H 'kbn-xsrf: true' \
        -H 'Content-Type: application/json' \
        -d "$state" 2>/dev/null | jq -r '.id // "exists"' > /dev/null

    echo "[+] Visualization created: $viz_title"
}

# 3.1: Run Status Distribution (Pie Chart)
create_lens_viz "run-status-pie" "Run Status Distribution" '{
    "attributes": {
        "title": "Run Status Distribution",
        "description": "Breakdown of execution outcomes",
        "visualizationType": "lnsPie",
        "state": {
            "datasourceStates": {
                "formBased": {
                    "layers": {
                        "layer1": {
                            "columns": {
                                "col1": {
                                    "label": "Status",
                                    "dataType": "string",
                                    "operationType": "terms",
                                    "sourceField": "status.keyword",
                                    "params": {
                                        "size": 10,
                                        "orderBy": {"type": "column", "columnId": "col2"},
                                        "orderDirection": "desc"
                                    }
                                },
                                "col2": {
                                    "label": "Count",
                                    "dataType": "number",
                                    "operationType": "count",
                                    "sourceField": "___records___"
                                }
                            },
                            "columnOrder": ["col1", "col2"],
                            "indexPatternId": "runs-star"
                        }
                    }
                }
            },
            "visualization": {
                "shape": "donut",
                "layers": [{
                    "layerId": "layer1",
                    "groups": ["col1"],
                    "metric": "col2",
                    "numberDisplay": "percent",
                    "categoryDisplay": "default",
                    "legendDisplay": "show",
                    "nestedLegend": false
                }]
            },
            "query": {"query": "", "language": "kuery"},
            "filters": []
        },
        "references": [
            {"name": "indexpattern-datasource-layer-layer1", "type": "index-pattern", "id": "runs-star"}
        ]
    }
}'

# 3.2: BB Coverage Metric
create_lens_viz "bb-coverage-metric" "BB Coverage" '{
    "attributes": {
        "title": "BB Coverage",
        "description": "Total basic blocks covered",
        "visualizationType": "lnsMetric",
        "state": {
            "datasourceStates": {
                "formBased": {
                    "layers": {
                        "layer1": {
                            "columns": {
                                "col1": {
                                    "label": "Max BB Coverage",
                                    "dataType": "number",
                                    "operationType": "max",
                                    "sourceField": "payload_total_bbs"
                                }
                            },
                            "columnOrder": ["col1"],
                            "indexPatternId": "telemetry-star"
                        }
                    }
                }
            },
            "visualization": {
                "layerId": "layer1",
                "accessor": "col1"
            },
            "query": {"query": "event_type.keyword: coverage", "language": "kuery"},
            "filters": []
        },
        "references": [
            {"name": "indexpattern-datasource-layer-layer1", "type": "index-pattern", "id": "telemetry-star"}
        ]
    }
}'

# 3.3: BB Coverage Timeline (Line Chart)
create_lens_viz "bb-coverage-line" "BB Coverage Timeline" '{
    "attributes": {
        "title": "BB Coverage Timeline",
        "description": "BB coverage over time",
        "visualizationType": "lnsXY",
        "state": {
            "datasourceStates": {
                "formBased": {
                    "layers": {
                        "layer1": {
                            "columns": {
                                "col1": {
                                    "label": "@timestamp",
                                    "dataType": "date",
                                    "operationType": "date_histogram",
                                    "sourceField": "indexed_at",
                                    "params": {
                                        "interval": "auto"
                                    }
                                },
                                "col2": {
                                    "label": "Max BB Coverage",
                                    "dataType": "number",
                                    "operationType": "max",
                                    "sourceField": "payload_total_bbs"
                                }
                            },
                            "columnOrder": ["col1", "col2"],
                            "indexPatternId": "telemetry-star"
                        }
                    }
                }
            },
            "visualization": {
                "legend": {"isVisible": true, "position": "right"},
                "valueLabels": "hide",
                "fittingFunction": "None",
                "axisTitlesVisibilitySettings": {"x": true, "yLeft": true, "yRight": true},
                "tickLabelsVisibilitySettings": {"x": true, "yLeft": true, "yRight": true},
                "layers": [{
                    "layerId": "layer1",
                    "accessors": ["col2"],
                    "position": "top",
                    "seriesType": "line",
                    "showGridlines": false,
                    "xAccessor": "col1"
                }]
            },
            "query": {"query": "event_type.keyword: coverage", "language": "kuery"},
            "filters": []
        },
        "references": [
            {"name": "indexpattern-datasource-layer-layer1", "type": "index-pattern", "id": "telemetry-star"}
        ]
    }
}'

# 3.4: Execution Time Histogram
create_lens_viz "exec-time-histogram" "Execution Time Distribution" '{
    "attributes": {
        "title": "Execution Time Distribution",
        "description": "Histogram of execution times",
        "visualizationType": "lnsXY",
        "state": {
            "datasourceStates": {
                "formBased": {
                    "layers": {
                        "layer1": {
                            "columns": {
                                "col1": {
                                    "label": "Elapsed Seconds",
                                    "dataType": "number",
                                    "operationType": "range",
                                    "sourceField": "elapsed_seconds",
                                    "params": {
                                        "type": "histogram",
                                        "ranges": [
                                            {"from": 0, "to": 5, "label": ""},
                                            {"from": 5, "to": 10, "label": ""},
                                            {"from": 10, "to": 20, "label": ""},
                                            {"from": 20, "to": 30, "label": ""},
                                            {"from": 30, "to": 1000, "label": ""}
                                        ],
                                        "maxBars": "auto"
                                    }
                                },
                                "col2": {
                                    "label": "Count",
                                    "dataType": "number",
                                    "operationType": "count",
                                    "sourceField": "___records___"
                                }
                            },
                            "columnOrder": ["col1", "col2"],
                            "indexPatternId": "runs-star"
                        }
                    }
                }
            },
            "visualization": {
                "legend": {"isVisible": false, "position": "right"},
                "valueLabels": "hide",
                "fittingFunction": "None",
                "axisTitlesVisibilitySettings": {"x": true, "yLeft": true, "yRight": true},
                "tickLabelsVisibilitySettings": {"x": true, "yLeft": true, "yRight": true},
                "layers": [{
                    "layerId": "layer1",
                    "accessors": ["col2"],
                    "position": "top",
                    "seriesType": "bar",
                    "showGridlines": false,
                    "xAccessor": "col1"
                }]
            },
            "query": {"query": "", "language": "kuery"},
            "filters": []
        },
        "references": [
            {"name": "indexpattern-datasource-layer-layer1", "type": "index-pattern", "id": "runs-star"}
        ]
    }
}'

# 3.5: Top Workers (Bar Chart)
create_lens_viz "top-workers-bar" "Top Workers" '{
    "attributes": {
        "title": "Top Workers by Run Count",
        "description": "Most active workers",
        "visualizationType": "lnsXY",
        "state": {
            "datasourceStates": {
                "formBased": {
                    "layers": {
                        "layer1": {
                            "columns": {
                                "col1": {
                                    "label": "Worker ID",
                                    "dataType": "string",
                                    "operationType": "terms",
                                    "sourceField": "worker_id.keyword",
                                    "params": {
                                        "size": 10,
                                        "orderBy": {"type": "column", "columnId": "col2"},
                                        "orderDirection": "desc"
                                    }
                                },
                                "col2": {
                                    "label": "Count",
                                    "dataType": "number",
                                    "operationType": "count",
                                    "sourceField": "___records___"
                                }
                            },
                            "columnOrder": ["col1", "col2"],
                            "indexPatternId": "runs-star"
                        }
                    }
                }
            },
            "visualization": {
                "legend": {"isVisible": false, "position": "right"},
                "valueLabels": "hide",
                "fittingFunction": "None",
                "axisTitlesVisibilitySettings": {"x": true, "yLeft": true, "yRight": true},
                "tickLabelsVisibilitySettings": {"x": true, "yLeft": true, "yRight": true},
                "layers": [{
                    "layerId": "layer1",
                    "accessors": ["col2"],
                    "position": "top",
                    "seriesType": "bar_horizontal",
                    "showGridlines": false,
                    "xAccessor": "col1"
                }]
            },
            "query": {"query": "", "language": "kuery"},
            "filters": []
        },
        "references": [
            {"name": "indexpattern-datasource-layer-layer1", "type": "index-pattern", "id": "runs-star"}
        ]
    }
}'

# 3.6: Event Type Distribution (Pie Chart)
create_lens_viz "event-type-pie" "Event Type Distribution" '{
    "attributes": {
        "title": "Telemetry Event Types",
        "description": "Distribution of telemetry events",
        "visualizationType": "lnsPie",
        "state": {
            "datasourceStates": {
                "formBased": {
                    "layers": {
                        "layer1": {
                            "columns": {
                                "col1": {
                                    "label": "Event Type",
                                    "dataType": "string",
                                    "operationType": "terms",
                                    "sourceField": "event_type.keyword",
                                    "params": {
                                        "size": 10,
                                        "orderBy": {"type": "column", "columnId": "col2"},
                                        "orderDirection": "desc"
                                    }
                                },
                                "col2": {
                                    "label": "Count",
                                    "dataType": "number",
                                    "operationType": "count",
                                    "sourceField": "___records___"
                                }
                            },
                            "columnOrder": ["col1", "col2"],
                            "indexPatternId": "telemetry-star"
                        }
                    }
                }
            },
            "visualization": {
                "shape": "donut",
                "layers": [{
                    "layerId": "layer1",
                    "groups": ["col1"],
                    "metric": "col2",
                    "numberDisplay": "percent",
                    "categoryDisplay": "default",
                    "legendDisplay": "show",
                    "nestedLegend": false
                }]
            },
            "query": {"query": "", "language": "kuery"},
            "filters": []
        },
        "references": [
            {"name": "indexpattern-datasource-layer-layer1", "type": "index-pattern", "id": "telemetry-star"}
        ]
    }
}'

# Step 4: Create Dashboard with Visualizations
echo ""
echo "=== Step 4: Creating Dashboard ==="

# Create dashboard with all visualizations
dashboard_json='{
    "attributes": {
        "title": "Artifact Execution Analysis",
        "description": "Real-time artifact execution telemetry and results",
        "panelsJSON": "[{\"version\":\"8.0.0\",\"type\":\"lens\",\"gridData\":{\"x\":0,\"y\":0,\"w\":12,\"h\":8,\"i\":\"panel-1\"},\"panelIndex\":\"panel-1\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-1\"},{\"version\":\"8.0.0\",\"type\":\"lens\",\"gridData\":{\"x\":12,\"y\":0,\"w\":12,\"h\":8,\"i\":\"panel-2\"},\"panelIndex\":\"panel-2\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-2\"},{\"version\":\"8.0.0\",\"type\":\"lens\",\"gridData\":{\"x\":24,\"y\":0,\"w\":12,\"h\":8,\"i\":\"panel-3\"},\"panelIndex\":\"panel-3\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-3\"},{\"version\":\"8.0.0\",\"type\":\"lens\",\"gridData\":{\"x\":36,\"y\":0,\"w\":12,\"h\":8,\"i\":\"panel-4\"},\"panelIndex\":\"panel-4\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-4\"},{\"version\":\"8.0.0\",\"type\":\"lens\",\"gridData\":{\"x\":0,\"y\":8,\"w\":24,\"h\":12,\"i\":\"panel-5\"},\"panelIndex\":\"panel-5\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-5\"},{\"version\":\"8.0.0\",\"type\":\"lens\",\"gridData\":{\"x\":24,\"y\":8,\"w\":24,\"h\":12,\"i\":\"panel-6\"},\"panelIndex\":\"panel-6\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-6\"},{\"version\":\"8.0.0\",\"type\":\"search\",\"gridData\":{\"x\":0,\"y\":20,\"w\":24,\"h\":15,\"i\":\"panel-7\"},\"panelIndex\":\"panel-7\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-7\"},{\"version\":\"8.0.0\",\"type\":\"search\",\"gridData\":{\"x\":24,\"y\":20,\"w\":24,\"h\":15,\"i\":\"panel-8\"},\"panelIndex\":\"panel-8\",\"embeddableConfig\":{\"enhancements\":{}},\"panelRefName\":\"panel_panel-8\"}]",
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
        {"name": "panel_panel-1", "type": "lens", "id": "run-status-pie"},
        {"name": "panel_panel-2", "type": "lens", "id": "bb-coverage-metric"},
        {"name": "panel_panel-3", "type": "lens", "id": "exec-time-histogram"},
        {"name": "panel_panel-4", "type": "lens", "id": "event-type-pie"},
        {"name": "panel_panel-5", "type": "lens", "id": "bb-coverage-line"},
        {"name": "panel_panel-6", "type": "lens", "id": "top-workers-bar"},
        {"name": "panel_panel-7", "type": "search", "id": "run-results-search"},
        {"name": "panel_panel-8", "type": "search", "id": "coverage-events-search"}
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

# Step 5: Summary
echo ""
echo "=== Dashboard Setup Complete! ==="
echo ""
echo "Dashboard URL: $KIBANA_URL/app/dashboards#/view/artifact-execution-dashboard"
echo ""
echo "Index Patterns Created:"
echo "  - telemetry-* (time field: indexed_at)"
echo "  - runs-* (time field: timestamp)"
echo ""
echo "Visualizations Created:"
echo "  - Run Status Distribution (Pie Chart)"
echo "  - BB Coverage (Metric)"
echo "  - BB Coverage Timeline (Line Chart)"
echo "  - Execution Time Distribution (Histogram)"
echo "  - Top Workers (Bar Chart)"
echo "  - Event Type Distribution (Pie Chart)"
echo ""
echo "Saved Searches Created:"
echo "  - Run Results (table)"
echo "  - Coverage Events (table)"
echo ""
echo "Dashboard Layout:"
echo "  Row 1: Status Pie, BB Metric, Exec Time Histogram, Event Type Pie"
echo "  Row 2: BB Coverage Line Chart, Top Workers Bar Chart"
echo "  Row 3: Run Results Table, Coverage Events Table"
echo ""
echo "=== Quick Filters ==="
echo ""
echo "Add these filters in Kibana to focus on specific data:"
echo "  - job_id.keyword: \"job-000001\"       (specific job)"
echo "  - status.keyword: \"success\"          (only successful runs)"
echo "  - status.keyword: \"error\" OR \"timeout\" (only failures)"
echo "  - worker_id.keyword: \"worker-01\"     (specific worker)"
echo "  - event_type.keyword: \"coverage\"     (coverage events only)"
echo ""
echo "[+] Dashboard ready with 6 visualizations + 2 data tables!"
echo "[+] See automation/ELASTICSEARCH-DATA-SCHEMA.md for field reference"
