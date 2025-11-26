# Diagnostic script to check what event types are actually in Elasticsearch
# PowerShell version

$ELASTIC_URL = "http://localhost:9200"
$INDEX_PATTERN = "telemetry-*"

Write-Host "=== Checking Elasticsearch for telemetry event types ===" -ForegroundColor Cyan
Write-Host ""

# 1. Check if indices exist
Write-Host "1. Available telemetry indices:" -ForegroundColor Yellow
try {
    $response = Invoke-RestMethod -Uri "${ELASTIC_URL}/_cat/indices/${INDEX_PATTERN}?v&s=index:desc" -Method Get
    $response -split "`n" | Select-Object -First 10
} catch {
    Write-Host "Failed to get indices: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

# 2. Get sample document
Write-Host "2. Sample telemetry document (first 1):" -ForegroundColor Yellow
try {
    $response = Invoke-RestMethod -Uri "${ELASTIC_URL}/${INDEX_PATTERN}/_search?size=1" -Method Get
    if ($response.hits.hits.Count -gt 0) {
        $response.hits.hits[0]._source | ConvertTo-Json -Depth 10
    } else {
        Write-Host "No documents found in index" -ForegroundColor Red
    }
} catch {
    Write-Host "Failed to get sample: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

# 3. Aggregate event_type field
Write-Host "3. Event type distribution (top-level event_type):" -ForegroundColor Yellow
try {
    $body = @{
        size = 0
        aggs = @{
            event_types = @{
                terms = @{
                    field = "event_type.keyword"
                    size = 50
                }
            }
        }
    } | ConvertTo-Json -Depth 5

    $response = Invoke-RestMethod -Uri "${ELASTIC_URL}/${INDEX_PATTERN}/_search" -Method Post -Body $body -ContentType "application/json"
    $response.aggregations.event_types.buckets | ForEach-Object {
        Write-Host "  $($_.key): $($_.doc_count)" -ForegroundColor Gray
    }
} catch {
    Write-Host "Failed to aggregate: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

# 4. Check payload_type field (RedEDR's type field after flattening)
Write-Host "4. Payload type distribution (payload_type from RedEDR):" -ForegroundColor Yellow
try {
    $body = @{
        size = 0
        aggs = @{
            payload_types = @{
                terms = @{
                    field = "payload_type.keyword"
                    size = 50
                }
            }
        }
    } | ConvertTo-Json -Depth 5

    $response = Invoke-RestMethod -Uri "${ELASTIC_URL}/${INDEX_PATTERN}/_search" -Method Post -Body $body -ContentType "application/json"
    if ($response.aggregations.payload_types.buckets.Count -gt 0) {
        $response.aggregations.payload_types.buckets | ForEach-Object {
            Write-Host "  $($_.key): $($_.doc_count)" -ForegroundColor Gray
        }
    } else {
        Write-Host "  No payload_type field found (might not be flattened correctly)" -ForegroundColor Yellow
    }
} catch {
    Write-Host "Failed to aggregate: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

# 5. Check metadata.event_type
Write-Host "5. Metadata event_type distribution:" -ForegroundColor Yellow
try {
    $body = @{
        size = 0
        aggs = @{
            metadata_event_types = @{
                terms = @{
                    field = "metadata.event_type.keyword"
                    size = 50
                }
            }
        }
    } | ConvertTo-Json -Depth 5

    $response = Invoke-RestMethod -Uri "${ELASTIC_URL}/${INDEX_PATTERN}/_search" -Method Post -Body $body -ContentType "application/json"
    if ($response.aggregations.metadata_event_types.buckets.Count -gt 0) {
        $response.aggregations.metadata_event_types.buckets | ForEach-Object {
            Write-Host "  $($_.key): $($_.doc_count)" -ForegroundColor Gray
        }
    } else {
        Write-Host "  No metadata.event_type field found" -ForegroundColor Yellow
    }
} catch {
    Write-Host "Failed to aggregate: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

# 6. Count total documents
Write-Host "6. Total telemetry documents:" -ForegroundColor Yellow
try {
    $response = Invoke-RestMethod -Uri "${ELASTIC_URL}/${INDEX_PATTERN}/_count" -Method Get
    Write-Host "  Total: $($response.count)" -ForegroundColor Green
} catch {
    Write-Host "Failed to count: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

# 7. Search for specific event types
Write-Host "7. Searching for ETW and DLL events specifically:" -ForegroundColor Yellow
try {
    # Search for events where event_type contains "etw" or "dll"
    $body = @{
        size = 0
        query = @{
            bool = @{
                should = @(
                    @{ term = @{ "event_type.keyword" = "etw" } }
                    @{ term = @{ "event_type.keyword" = "dll" } }
                    @{ term = @{ "payload_type.keyword" = "etw" } }
                    @{ term = @{ "payload_type.keyword" = "dll" } }
                )
            }
        }
    } | ConvertTo-Json -Depth 5

    $response = Invoke-RestMethod -Uri "${ELASTIC_URL}/${INDEX_PATTERN}/_search" -Method Post -Body $body -ContentType "application/json"
    Write-Host "  ETW/DLL events found: $($response.hits.total.value)" -ForegroundColor Green
} catch {
    Write-Host "Failed to search: $($_.Exception.Message)" -ForegroundColor Red
}
Write-Host ""

Write-Host "=== Diagnostic complete ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Yellow
Write-Host "  1. If event_type shows only 'trace' or 'unknown', check worker logs for RedEDR API response"
Write-Host "  2. If payload_type shows 'etw'/'dll', use 'payload_type' field in Kibana searches"
Write-Host "  3. Check sample document structure to see field naming"
