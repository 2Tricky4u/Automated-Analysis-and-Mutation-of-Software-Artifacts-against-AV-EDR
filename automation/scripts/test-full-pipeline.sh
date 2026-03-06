#!/usr/bin/env bash
# -----------------------------------------------------------------------
# test-full-pipeline.sh -- End-to-end integration test — build, deploy, execute, query
#
# Drives the full artifact lifecycle through all four gRPC hops:
# controller BuildArtifact, controller DeployArtifact, worker RunSample,
# then queries Elasticsearch for the resulting telemetry. Intended for
# verifying that the controller-worker-elastic chain is wired correctly.
#
# Usage:  ./test-full-pipeline.sh [template] [source_file]
# Prerequisites: grpcurl, jq, curl, running controller + worker + ES
# Called by: standalone (integration smoke test)
# -----------------------------------------------------------------------
set -euo pipefail

GREEN="\033[0;32m"
BLUE="\033[0;34m"
YELLOW="\033[1;33m"
RED="\033[0;31m"
NC="\033[0m"

echo -e "${BLUE}==================================================${NC}"
echo -e "${BLUE}  Fully Automated Build-Deploy-Execute Pipeline ${NC}"
echo -e "${BLUE}==================================================${NC}"
echo ""

# Configuration
CONTROLLER_ADDR="${CONTROLLER_ADDR:-localhost:50051}"
WORKER_ADDR="${WORKER_ADDR:-10.200.200.11:50052}"
TEMPLATE="${1:-rwx_direct}"
SOURCE="${2:-rwx_direct.c}"

echo -e "${BLUE}Configuration:${NC}"
echo "  Controller: ${CONTROLLER_ADDR}"
echo "  Worker: ${WORKER_ADDR}"
echo "  Template: ${TEMPLATE}/${SOURCE}"
echo ""

# ============================================================================
# Compile the C template into a Windows PE via the controller's build service
# ============================================================================
echo -e "${BLUE}[1/4] Building artifact: ${TEMPLATE}/${SOURCE}${NC}"

BUILD_RESPONSE=$(grpcurl -plaintext \
  -d "{
    \"template_name\": \"${TEMPLATE}\",
    \"source_file\": \"${SOURCE}\"
  }" \
  ${CONTROLLER_ADDR} \
  edr.controller.Controller/BuildArtifact 2>&1)

if [ $? -ne 0 ]; then
  echo -e "${RED}✗ Build failed${NC}"
  echo "$BUILD_RESPONSE"
  exit 1
fi

echo "$BUILD_RESPONSE"

ARTIFACT_ID=$(echo "$BUILD_RESPONSE" | jq -r '.artifact_id')
if [ -z "$ARTIFACT_ID" ] || [ "$ARTIFACT_ID" = "null" ]; then
  echo -e "${RED}✗ Build failed or artifact_id not found${NC}"
  exit 1
fi

echo -e "${GREEN}✓ Built artifact: ${ARTIFACT_ID}${NC}"
echo ""

# ============================================================================
# Stream the built PE to the worker VM so it is ready for execution
# ============================================================================
echo -e "${BLUE}[2/4] Deploying artifact to worker: ${WORKER_ADDR}${NC}"

DEPLOY_RESPONSE=$(grpcurl -plaintext \
  -d "{
    \"artifact_id\": \"${ARTIFACT_ID}\",
    \"worker_address\": \"${WORKER_ADDR}\"
  }" \
  ${CONTROLLER_ADDR} \
  edr.controller.Controller/DeployArtifact 2>&1)

if [ $? -ne 0 ]; then
  echo -e "${RED}✗ Deploy failed${NC}"
  echo "$DEPLOY_RESPONSE"
  exit 1
fi

echo "$DEPLOY_RESPONSE"

DEPLOY_SUCCESS=$(echo "$DEPLOY_RESPONSE" | jq -r '.success')
if [ "$DEPLOY_SUCCESS" != "true" ]; then
  echo -e "${RED}✗ Deploy failed${NC}"
  exit 1
fi

CHUNKS_SENT=$(echo "$DEPLOY_RESPONSE" | jq -r '.chunks_sent')
WORKER_PATH=$(echo "$DEPLOY_RESPONSE" | jq -r '.worker_storage_path')

echo -e "${GREEN}✓ Deployed artifact: ${CHUNKS_SENT} chunks sent${NC}"
echo -e "${GREEN}  Worker path: ${WORKER_PATH}${NC}"
echo ""

# ============================================================================
# Run the artifact on the worker under ETW monitoring to collect telemetry
# ============================================================================
JOB_ID="test-$(date +%s)"
echo -e "${BLUE}[3/4] Executing on worker (job_id=${JOB_ID})...${NC}"

RUN_RESPONSE=$(grpcurl -plaintext \
  -d "{
    \"job_id\": \"${JOB_ID}\",
    \"artifact_id\": \"${ARTIFACT_ID}\",
    \"timeout_seconds\": 30,
    \"enable_etw\": true
  }" \
  ${WORKER_ADDR} \
  edr.worker.WorkerAgent/RunSample 2>&1)

if [ $? -ne 0 ]; then
  echo -e "${YELLOW}⚠ Execution failed or timed out${NC}"
  echo "$RUN_RESPONSE"
else
  echo "$RUN_RESPONSE"

  RUN_SUCCESS=$(echo "$RUN_RESPONSE" | jq -r '.success')
  if [ "$RUN_SUCCESS" = "true" ]; then
    echo -e "${GREEN}✓ Execution complete${NC}"
  else
    echo -e "${YELLOW}⚠ Execution reported failure${NC}"
  fi
fi
echo ""

# ============================================================================
# Fetch run results and RedEDR events from Elasticsearch to verify indexing
# ============================================================================
echo -e "${BLUE}[4/4] Querying telemetry (waiting 3s for indexing)...${NC}"
sleep 3

echo ""
echo -e "${BLUE}=== Run Results ===${NC}"
curl -s -X GET "localhost:9200/run-results-*/_search?pretty" \
  -H 'Content-Type: application/json' \
  -d "{
    \"query\": {
      \"term\": {
        \"job_id.keyword\": \"${JOB_ID}\"
      }
    },
    \"size\": 1
  }" 2>/dev/null | jq -r '.hits.hits[]._source | "  Job: \(.job_id)\n  Status: \(.status)\n  PID: \(.pid)\n  Exit Code: \(.exit_code)\n  Events: \(.telemetry_events_count)\n  CPU: \(.cpu_percent)%\n  Memory: \(.memory_mb)MB"'

echo ""
echo -e "${BLUE}=== RedEDR Events (first 5) ===${NC}"
curl -s -X GET "localhost:9200/rededr-*/_search?pretty" \
  -H 'Content-Type: application/json' \
  -d "{
    \"query\": {
      \"term\": {
        \"job_id.keyword\": \"${JOB_ID}\"
      }
    },
    \"size\": 5,
    \"sort\": [{\"@timestamp\": \"asc\"}]
  }" 2>/dev/null | jq -r '.hits.hits[]._source | "[\(.["@timestamp"])] \(.payload.EventType // "Unknown"): \(.payload | to_entries | map("\(.key)=\(.value)") | join(", "))"'

echo ""
echo -e "${GREEN}==================================================${NC}"
echo -e "${GREEN}  ✓ Full Pipeline Test Complete!                 ${NC}"
echo -e "${GREEN}==================================================${NC}"
echo ""
echo -e "Artifact ID: ${ARTIFACT_ID}"
echo -e "Job ID: ${JOB_ID}"
echo ""
echo -e "View in Kibana:"
echo -e "  http://localhost:5601/app/discover"
echo -e "  Filter: job_id.keyword : \"${JOB_ID}\""
echo ""
