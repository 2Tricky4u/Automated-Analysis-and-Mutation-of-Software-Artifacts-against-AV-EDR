#!/usr/bin/env bash
set -euo pipefail

# Usage:
#   ./telemetry_dump.sh
#   ./telemetry_dump.sh --run-id "abc123"
#   ./telemetry_dump.sh --out telemetry_inspect.txt --es http://localhost:9200
#
# Notes:
# - Writes BOTH the command and its output into the same file.
# - Requires: curl

ES_URL="http://localhost:9200"
INDEX_PATTERN="telemetry-*"
OUT_FILE="telemetry_inspect.out"
RUN_ID="YOUR_RUN_ID_HERE"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --es)     ES_URL="${2:?missing value}"; shift 2 ;;
    --index)  INDEX_PATTERN="${2:?missing value}"; shift 2 ;;
    --out)    OUT_FILE="${2:?missing value}"; shift 2 ;;
    --run-id) RUN_ID="${2:?missing value}"; shift 2 ;;
    -h|--help)
      cat <<EOF
Usage: $0 [--run-id ID] [--out FILE] [--es URL] [--index PATTERN]

Defaults:
  --es    ${ES_URL}
  --index ${INDEX_PATTERN}
  --out   ${OUT_FILE}
  --run-id ${RUN_ID}
EOF
      exit 0
      ;;
    *)
      echo "Unknown arg: $1" >&2
      exit 2
      ;;
  esac
done

log() { printf "%s\n" "$*" >> "$OUT_FILE"; }

run_curl_json() {
  local title="$1"
  local url="$2"
  local body="$3"

  log "================================================================================"
  log "# ${title}"
  log "# $(date -Is)"
  log "# CMD:"
  log "curl -s \"${url}\" -H 'Content-Type: application/json' -d '$body'"
  log "# OUTPUT:"
  # print output into same file, preserve non-zero curl exit
  curl -s "${url}" -H 'Content-Type: application/json' -d "$body" >> "$OUT_FILE"
  log ""
  log ""
}

run_curl_plain() {
  local title="$1"
  local url="$2"

  log "================================================================================"
  log "# ${title}"
  log "# $(date -Is)"
  log "# CMD:"
  log "curl -s \"${url}\""
  log "# OUTPUT:"
  curl -s "${url}" >> "$OUT_FILE"
  log ""
  log ""
}

# Start fresh each run (comment this out if you want append-only behavior)
: > "$OUT_FILE"

log "# Telemetry inspection dump"
log "# ES_URL=${ES_URL}"
log "# INDEX_PATTERN=${INDEX_PATTERN}"
log "# OUT_FILE=${OUT_FILE}"
log "# RUN_ID=${RUN_ID}"
log ""

# 1. Sample 5 telemetry docs (see all field names)
run_curl_json \
  "1) Sample 5 telemetry docs (latest by timestamp)" \
  "${ES_URL}/${INDEX_PATTERN}/_search?pretty" \
'{
  "size": 5,
  "sort": [{"timestamp": "desc"}]
}'

# 2. All distinct event_type values (see what event categories exist)
run_curl_json \
  "2) Distinct event_type values (terms agg)" \
  "${ES_URL}/${INDEX_PATTERN}/_search?pretty" \
'{
  "size": 0,
  "aggs": { "event_types": { "terms": { "field": "event_type", "size": 50 } } }
}'

# 3. All distinct payload_func values (the API calls we extract tokens from)
run_curl_json \
  "3) Distinct payload_func values (terms agg)" \
  "${ES_URL}/${INDEX_PATTERN}/_search?pretty" \
'{
  "size": 0,
  "aggs": { "funcs": { "terms": { "field": "payload_func", "size": 100 } } }
}'

# 4. Sample telemetry for a specific run (sorted by sequence)
run_curl_json \
  "4) Sample telemetry for run_id='${RUN_ID}' (sorted by payload_seq asc)" \
  "${ES_URL}/${INDEX_PATTERN}/_search?pretty" \
"{
  \"size\": 50,
  \"query\": { \"match_phrase\": { \"run_id\": \"${RUN_ID}\" } },
  \"sort\": [{\"payload_seq\": \"asc\"}],
  \"_source\": [\"event_type\", \"payload_func\", \"payload_seq\", \"payload_ts_us\", \"payload_*\"]

}"

# 5. All distinct field names in telemetry index (mapping)
run_curl_plain \
  "5) Telemetry index mapping (field names/types live here)" \
  "${ES_URL}/${INDEX_PATTERN}/_mapping?pretty"

# 6. Non-trace telemetry with payload_func (what the extractor queries)
run_curl_json \
  "6) Non-trace telemetry that has payload_func (latest by timestamp)" \
  "${ES_URL}/${INDEX_PATTERN}/_search?pretty" \
'{
  "size": 20,
  "query": {
    "bool": {
      "filter": [{ "exists": { "field": "payload_func" } }],
      "must_not": [{ "match_phrase": { "event_type": "trace_log" } }]
    }
  },
  "sort": [{"timestamp": "desc"}]
}'

echo "Wrote output to: ${OUT_FILE}"