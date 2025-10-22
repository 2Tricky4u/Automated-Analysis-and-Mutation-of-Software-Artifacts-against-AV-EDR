#!/bin/bash
# Start Elasticsearch and Kibana in background
set -e

AUTOMATION_DIR="/mnt/c/Users/xagao/RustroverProjects/Automated-Analysis-and-Mutation-of-Software-Artifacts-against-AV-EDR/automation"

echo "[i] Starting Elasticsearch and Kibana..."

cd "$AUTOMATION_DIR"

# Start Elasticsearch in background
nohup docker compose up elasticsearch > /tmp/elasticsearch.log 2>&1 &
ES_PID=$!
echo "[i] Elasticsearch starting (PID: $ES_PID)"

# Start Kibana in background
nohup docker compose up kibana > /tmp/kibana.log 2>&1 &
KIBANA_PID=$!
echo "[i] Kibana starting (PID: $KIBANA_PID)"

# Wait for Elasticsearch to be ready
echo "[i] Waiting for Elasticsearch to be ready..."
MAX_RETRIES=30
RETRY_COUNT=0

while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
    if curl -s http://localhost:9200 > /dev/null 2>&1; then
        echo "[OK] Elasticsearch is ready"
        break
    fi
    sleep 2
    RETRY_COUNT=$((RETRY_COUNT + 1))
    echo -n "."
done
echo ""

if [ $RETRY_COUNT -eq $MAX_RETRIES ]; then
    echo "[WARN] Elasticsearch did not respond after 60 seconds"
    echo "[INFO] Check logs: tail -f /tmp/elasticsearch.log"
    exit 1
fi

echo ""
echo "=========================================="
echo "Services Started Successfully"
echo "=========================================="
echo ""
echo "  Elasticsearch: http://localhost:9200"
echo "  Kibana:        http://localhost:5601"
echo ""
echo "Background processes:"
echo "  Elasticsearch PID: $ES_PID"
echo "  Kibana PID:        $KIBANA_PID"
echo ""
echo "Logs:"
echo "  tail -f /tmp/elasticsearch.log"
echo "  tail -f /tmp/kibana.log"
echo ""
echo "To stop:"
echo "  kill $ES_PID $KIBANA_PID"
echo ""
