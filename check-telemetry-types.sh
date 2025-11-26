#!/bin/bash
# Diagnostic script to check what event types are actually in Elasticsearch

ELASTIC_URL="http://localhost:9200"
INDEX_PATTERN="telemetry-*"

echo "=== Checking Elasticsearch for telemetry event types ==="
echo ""

# 1. Check if indices exist
echo "1. Available telemetry indices:"
curl -s "${ELASTIC_URL}/_cat/indices/${INDEX_PATTERN}?v&s=index:desc" | head -10
echo ""

# 2. Get sample documents
echo "2. Sample telemetry document (first 1):"
curl -s "${ELASTIC_URL}/${INDEX_PATTERN}/_search?size=1" | jq '.hits.hits[0]._source' 2>/dev/null || echo "No documents found or jq not installed"
echo ""

# 3. Aggregate event_type field
echo "3. Event type distribution:"
curl -s "${ELASTIC_URL}/${INDEX_PATTERN}/_search?size=0" -H 'Content-Type: application/json' -d '{
  "aggs": {
    "event_types": {
      "terms": {
        "field": "event_type.keyword",
        "size": 50
      }
    }
  }
}' | jq '.aggregations.event_types.buckets' 2>/dev/null || echo "Failed to aggregate"
echo ""

# 4. Check payload_type field (RedEDR's actual type field)
echo "4. Payload type distribution (RedEDR type field):"
curl -s "${ELASTIC_URL}/${INDEX_PATTERN}/_search?size=0" -H 'Content-Type: application/json' -d '{
  "aggs": {
    "payload_types": {
      "terms": {
        "field": "payload_type.keyword",
        "size": 50
      }
    }
  }
}' | jq '.aggregations.payload_types.buckets' 2>/dev/null || echo "Failed to aggregate"
echo ""

# 5. Check metadata.event_type
echo "5. Metadata event_type distribution:"
curl -s "${ELASTIC_URL}/${INDEX_PATTERN}/_search?size=0" -H 'Content-Type: application/json' -d '{
  "aggs": {
    "metadata_event_types": {
      "terms": {
        "field": "metadata.event_type.keyword",
        "size": 50
      }
    }
  }
}' | jq '.aggregations.metadata_event_types.buckets' 2>/dev/null || echo "Failed to aggregate"
echo ""

# 6. Count total documents
echo "6. Total telemetry documents:"
curl -s "${ELASTIC_URL}/${INDEX_PATTERN}/_count" | jq '.count' 2>/dev/null || echo "Failed to count"
echo ""

echo "=== Diagnostic complete ==="
