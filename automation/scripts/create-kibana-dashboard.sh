#!/bin/bash
# Automated Kibana Dashboard Setup Script
# Creates visualizations and dashboard for artifact execution analysis

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

# Function to create index pattern
create_index_pattern() {
    local pattern_name=$1
    local time_field=$2

    echo "[*] Creating index pattern: $pattern_name"

    curl -X POST "$KIBANA_URL/api/saved_objects/index-pattern/$pattern_name" \
        -H 'kbn-xsrf: true' \
        -H 'Content-Type: application/json' \
        -d "{
            \"attributes\": {
                \"title\": \"$pattern_name\",
                \"timeFieldName\": \"$time_field\"
            }
        }" 2>/dev/null | jq -r '.id // "exists"'

    echo "[+] Index pattern $pattern_name created/verified"
}

# Function to create scripted field
create_scripted_field() {
    local pattern_id=$1
    local field_name=$2
    local script=$3
    local field_type=$4

    echo "[*] Creating scripted field: $field_name on $pattern_id"

    # Get current pattern
    pattern_json=$(curl -s "$KIBANA_URL/api/saved_objects/index-pattern/$pattern_id" \
        -H 'kbn-xsrf: true')

    # Add scripted field
    updated_json=$(echo "$pattern_json" | jq \
        --arg name "$field_name" \
        --arg script "$script" \
        --arg type "$field_type" \
        '.attributes.fields = (.attributes.fields // "[]" | fromjson) + [{
            "name": $name,
            "type": $type,
            "scripted": true,
            "script": $script,
            "lang": "painless"
        }] | tostring')

    curl -X PUT "$KIBANA_URL/api/saved_objects/index-pattern/$pattern_id" \
        -H 'kbn-xsrf: true' \
        -H 'Content-Type: application/json' \
        -d "$updated_json" > /dev/null 2>&1

    echo "[+] Scripted field $field_name added"
}

# Step 1: Create Index Patterns
echo ""
echo "=== Step 1: Creating Index Patterns ==="
create_index_pattern "telemetry-*" "@timestamp"
create_index_pattern "rededr-*" "@timestamp"
create_index_pattern "etw-*" "@timestamp"
create_index_pattern "run-results-*" "start_ts"

# Step 2: Add Scripted Fields
echo ""
echo "=== Step 2: Adding Scripted Fields ==="

# Execution duration
create_scripted_field "run-results-*" "execution_duration_ms" \
    "if (doc['end_ts'].size() > 0 && doc['start_ts'].size() > 0) { return doc['end_ts'].value.toInstant().toEpochMilli() - doc['start_ts'].value.toInstant().toEpochMilli(); } return 0;" \
    "number"

# BB coverage percentage
create_scripted_field "telemetry-*" "bb_coverage_percent" \
    "if (doc['bb_total'].size() > 0 && doc['bb_hit'].size() > 0) { return (doc['bb_hit'].value / (double)doc['bb_total'].value) * 100.0; } return 0;" \
    "number"

# Suspicious API detector
create_scripted_field "telemetry-*" "is_suspicious_api" \
    "if (doc['api_name.keyword'].size() == 0) return false; String api = doc['api_name.keyword'].value; return api.contains('VirtualAlloc') || api.contains('WriteProcessMemory') || api.contains('CreateRemoteThread') || api.contains('VirtualProtect');" \
    "boolean"

# Step 3: Create Visualizations
echo ""
echo "=== Step 3: Creating Visualizations ==="

# Helper function to create visualization
create_viz() {
    local viz_id=$1
    local viz_title=$2
    local viz_type=$3
    local viz_state=$4

    echo "[*] Creating visualization: $viz_title"

    curl -X POST "$KIBANA_URL/api/saved_objects/visualization" \
        -H 'kbn-xsrf: true' \
        -H 'Content-Type: application/json' \
        -d "{
            \"attributes\": {
                \"title\": \"$viz_title\",
                \"visState\": $viz_state,
                \"uiStateJSON\": \"{}\",
                \"description\": \"\",
                \"version\": 1,
                \"kibanaSavedObjectMeta\": {
                    \"searchSourceJSON\": \"{\\\"query\\\":{\\\"language\\\":\\\"kuery\\\",\\\"query\\\":\\\"\\\"},\\\"filter\\\":[]}\"
                }
            }
        }" > /dev/null 2>&1

    echo "[+] Visualization created: $viz_title"
}

# 3.1: Run Status Metric
create_viz "run-status-metric" "Run Status" "metric" '{
    "title": "Run Status",
    "type": "metric",
    "params": {
        "addTooltip": true,
        "addLegend": false,
        "type": "metric",
        "metric": {
            "colorSchema": "Green to Red",
            "colorsRange": [
                {"from": 0, "to": 0, "color": "#6DCE9E"},
                {"from": 1, "to": 100, "color": "#E7664C"}
            ],
            "labels": {"show": true},
            "percentageMode": false,
            "style": {"fontSize": 48}
        }
    },
    "aggs": [
        {
            "id": "1",
            "enabled": true,
            "type": "terms",
            "schema": "metric",
            "params": {
                "field": "status.keyword",
                "size": 1,
                "order": "desc",
                "orderBy": "_count"
            }
        }
    ]
}'

# 3.2: Exit Code Metric
create_viz "exit-code-metric" "Exit Code" "metric" '{
    "title": "Exit Code",
    "type": "metric",
    "params": {
        "addTooltip": true,
        "addLegend": false,
        "type": "metric",
        "metric": {
            "colorSchema": "Green to Red",
            "colorsRange": [
                {"from": 0, "to": 0, "color": "#6DCE9E"},
                {"from": 1, "to": 255, "color": "#E7664C"}
            ],
            "labels": {"show": true},
            "percentageMode": false,
            "style": {"fontSize": 60, "fontWeight": "bold"}
        }
    },
    "aggs": [
        {
            "id": "1",
            "enabled": true,
            "type": "max",
            "schema": "metric",
            "params": {"field": "exit_code"}
        }
    ]
}'

# 3.3: BB Coverage Bar Chart
create_viz "bb-coverage-bar" "BB Coverage" "horizontal_bar" '{
    "title": "BB Coverage",
    "type": "horizontal_bar",
    "params": {
        "type": "histogram",
        "grid": {"categoryLines": false},
        "categoryAxes": [{
            "id": "CategoryAxis-1",
            "type": "category",
            "position": "left",
            "show": true,
            "style": {},
            "scale": {"type": "linear"},
            "labels": {"show": true, "truncate": 100},
            "title": {}
        }],
        "valueAxes": [{
            "id": "ValueAxis-1",
            "name": "LeftAxis-1",
            "type": "value",
            "position": "bottom",
            "show": true,
            "style": {},
            "scale": {"type": "linear", "mode": "normal"},
            "labels": {"show": true, "rotate": 0, "filter": false, "truncate": 100},
            "title": {"text": "Basic Blocks"}
        }],
        "seriesParams": [
            {
                "show": true,
                "type": "histogram",
                "mode": "stacked",
                "data": {"label": "Hit", "id": "1"},
                "valueAxis": "ValueAxis-1",
                "drawLinesBetweenPoints": true,
                "lineWidth": 2,
                "showCircles": true
            },
            {
                "show": true,
                "type": "histogram",
                "mode": "stacked",
                "data": {"label": "Total", "id": "2"},
                "valueAxis": "ValueAxis-1",
                "drawLinesBetweenPoints": true,
                "lineWidth": 2,
                "showCircles": true
            }
        ],
        "addTooltip": true,
        "addLegend": true,
        "legendPosition": "right",
        "showCircles": true
    },
    "aggs": [
        {
            "id": "1",
            "enabled": true,
            "type": "max",
            "schema": "metric",
            "params": {"field": "bb_hit", "customLabel": "Hit"}
        },
        {
            "id": "2",
            "enabled": true,
            "type": "max",
            "schema": "metric",
            "params": {"field": "bb_total", "customLabel": "Total"}
        },
        {
            "id": "3",
            "enabled": true,
            "type": "terms",
            "schema": "segment",
            "params": {"field": "artifact_id.keyword", "size": 5, "order": "desc", "orderBy": "1"}
        }
    ]
}'

# 3.4: Function Call Timeline
create_viz "function-timeline" "Function Call Timeline" "line" '{
    "title": "Function Call Timeline",
    "type": "line",
    "params": {
        "type": "line",
        "grid": {"categoryLines": false},
        "categoryAxes": [{
            "id": "CategoryAxis-1",
            "type": "category",
            "position": "bottom",
            "show": true,
            "style": {},
            "scale": {"type": "linear"},
            "labels": {"show": true, "filter": true, "truncate": 100},
            "title": {}
        }],
        "valueAxes": [{
            "id": "ValueAxis-1",
            "name": "LeftAxis-1",
            "type": "value",
            "position": "left",
            "show": true,
            "style": {},
            "scale": {"type": "linear", "mode": "normal"},
            "labels": {"show": true, "rotate": 0, "filter": false, "truncate": 100},
            "title": {"text": "Count"}
        }],
        "seriesParams": [{
            "show": true,
            "type": "line",
            "mode": "normal",
            "data": {"label": "Count", "id": "1"},
            "valueAxis": "ValueAxis-1",
            "drawLinesBetweenPoints": true,
            "lineWidth": 2,
            "showCircles": true
        }],
        "addTooltip": true,
        "addLegend": true,
        "legendPosition": "right"
    },
    "aggs": [
        {
            "id": "1",
            "enabled": true,
            "type": "count",
            "schema": "metric",
            "params": {}
        },
        {
            "id": "2",
            "enabled": true,
            "type": "date_histogram",
            "schema": "segment",
            "params": {
                "field": "@timestamp",
                "timeRange": {"from": "now-15m", "to": "now"},
                "useNormalizedEsInterval": true,
                "scaleMetricValues": false,
                "interval": "auto",
                "drop_partials": false,
                "min_doc_count": 1,
                "extended_bounds": {}
            }
        },
        {
            "id": "3",
            "enabled": true,
            "type": "terms",
            "schema": "group",
            "params": {
                "field": "function_name.keyword",
                "size": 10,
                "order": "desc",
                "orderBy": "1"
            }
        }
    ]
}'

# 3.5: Event Type Pie Chart
create_viz "event-type-pie" "Event Type Distribution" "pie" '{
    "title": "Event Type Distribution",
    "type": "pie",
    "params": {
        "type": "pie",
        "addTooltip": true,
        "addLegend": true,
        "legendPosition": "bottom",
        "isDonut": true,
        "labels": {
            "show": true,
            "values": true,
            "last_level": true,
            "truncate": 100
        }
    },
    "aggs": [
        {
            "id": "1",
            "enabled": true,
            "type": "count",
            "schema": "metric",
            "params": {}
        },
        {
            "id": "2",
            "enabled": true,
            "type": "terms",
            "schema": "segment",
            "params": {
                "field": "event.opcode_name.keyword",
                "size": 10,
                "order": "desc",
                "orderBy": "1"
            }
        }
    ]
}'

# 3.6: API Trace Table
create_viz "api-trace-table" "API Trace Sequence" "table" '{
    "title": "API Trace Sequence",
    "type": "table",
    "params": {
        "perPage": 20,
        "showPartialRows": false,
        "showMetricsAtAllLevels": false,
        "sort": {"columnIndex": null, "direction": null},
        "showTotal": false,
        "totalFunc": "sum",
        "percentageCol": ""
    },
    "aggs": [
        {
            "id": "1",
            "enabled": true,
            "type": "count",
            "schema": "metric",
            "params": {"customLabel": "Call Count"}
        },
        {
            "id": "2",
            "enabled": true,
            "type": "terms",
            "schema": "bucket",
            "params": {
                "field": "api_name.keyword",
                "size": 50,
                "order": "desc",
                "orderBy": "1",
                "customLabel": "API Name"
            }
        },
        {
            "id": "3",
            "enabled": true,
            "type": "terms",
            "schema": "bucket",
            "params": {
                "field": "thread_id",
                "size": 10,
                "order": "desc",
                "orderBy": "1",
                "customLabel": "Thread ID"
            }
        }
    ]
}'

# Step 4: Create Dashboard
echo ""
echo "=== Step 4: Creating Dashboard ==="

dashboard_json='{
    "attributes": {
        "title": "Artifact Execution Analysis",
        "description": "Comprehensive artifact execution with telemetry, coverage, and behavioral analysis",
        "panelsJSON": "[{\"version\":\"8.0.0\",\"gridData\":{\"x\":0,\"y\":0,\"w\":12,\"h\":8,\"i\":\"1\"},\"panelIndex\":\"1\",\"embeddableConfig\":{},\"panelRefName\":\"panel_1\"},{\"version\":\"8.0.0\",\"gridData\":{\"x\":12,\"y\":0,\"w\":12,\"h\":8,\"i\":\"2\"},\"panelIndex\":\"2\",\"embeddableConfig\":{},\"panelRefName\":\"panel_2\"},{\"version\":\"8.0.0\",\"gridData\":{\"x\":24,\"y\":0,\"w\":12,\"h\":8,\"i\":\"3\"},\"panelIndex\":\"3\",\"embeddableConfig\":{},\"panelRefName\":\"panel_3\"},{\"version\":\"8.0.0\",\"gridData\":{\"x\":36,\"y\":0,\"w\":12,\"h\":8,\"i\":\"4\"},\"panelIndex\":\"4\",\"embeddableConfig\":{},\"panelRefName\":\"panel_4\"},{\"version\":\"8.0.0\",\"gridData\":{\"x\":0,\"y\":8,\"w\":24,\"h\":12,\"i\":\"5\"},\"panelIndex\":\"5\",\"embeddableConfig\":{},\"panelRefName\":\"panel_5\"},{\"version\":\"8.0.0\",\"gridData\":{\"x\":24,\"y\":8,\"w\":24,\"h\":12,\"i\":\"6\"},\"panelIndex\":\"6\",\"embeddableConfig\":{},\"panelRefName\":\"panel_6\"}]",
        "optionsJSON": "{\"darkTheme\":false,\"useMargins\":true,\"hidePanelTitles\":false}",
        "version": 1,
        "timeRestore": true,
        "timeTo": "now",
        "timeFrom": "now-24h",
        "refreshInterval": {
            "pause": true,
            "value": 0
        },
        "kibanaSavedObjectMeta": {
            "searchSourceJSON": "{\"query\":{\"language\":\"kuery\",\"query\":\"\"},\"filter\":[]}"
        }
    },
    "references": [
        {"name": "panel_1", "type": "visualization", "id": "run-status-metric"},
        {"name": "panel_2", "type": "visualization", "id": "exit-code-metric"},
        {"name": "panel_3", "type": "visualization", "id": "bb-coverage-bar"},
        {"name": "panel_4", "type": "visualization", "id": "event-type-pie"},
        {"name": "panel_5", "type": "visualization", "id": "function-timeline"},
        {"name": "panel_6", "type": "visualization", "id": "api-trace-table"}
    ]
}'

curl -X POST "$KIBANA_URL/api/saved_objects/dashboard/artifact-execution-dashboard" \
    -H 'kbn-xsrf: true' \
    -H 'Content-Type: application/json' \
    -d "$dashboard_json" > /dev/null 2>&1

echo "[+] Dashboard created: Artifact Execution Analysis"

# Step 5: Summary
echo ""
echo "=== Dashboard Setup Complete! ==="
echo ""
echo "Dashboard URL: $KIBANA_URL/app/dashboards#/view/artifact-execution-dashboard"
echo ""
echo "Index Patterns Created:"
echo "  - telemetry-*"
echo "  - rededr-*"
echo "  - etw-*"
echo "  - run-results-*"
echo ""
echo "Visualizations Created:"
echo "  - Run Status (Metric)"
echo "  - Exit Code (Metric)"
echo "  - BB Coverage (Bar Chart)"
echo "  - Function Timeline (Line Chart)"
echo "  - Event Type Distribution (Pie)"
echo "  - API Trace Sequence (Table)"
echo ""
echo "Scripted Fields Added:"
echo "  - execution_duration_ms"
echo "  - bb_coverage_percent"
echo "  - is_suspicious_api"
echo ""
echo "[+] Ready to analyze artifact executions!"
