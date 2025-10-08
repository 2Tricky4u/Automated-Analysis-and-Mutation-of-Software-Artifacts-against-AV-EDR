#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}======================================${NC}"
echo -e "${GREEN}  EDR Lab Deployment Script${NC}"
echo -e "${GREEN}======================================${NC}"
echo ""

# Check prerequisites
echo -e "${YELLOW}Checking prerequisites...${NC}"

if ! command -v cargo &> /dev/null; then
    echo -e "${RED}Error: Rust/Cargo not found. Please install Rust.${NC}"
    exit 1
fi

if ! command -v protoc &> /dev/null; then
    echo -e "${RED}Error: protoc not found. Please install Protocol Buffers compiler.${NC}"
    exit 1
fi

if ! command -v docker &> /dev/null; then
    echo -e "${RED}Error: Docker not found. Please install Docker.${NC}"
    exit 1
fi

echo -e "${GREEN}✓ All prerequisites found${NC}"
echo ""

# Build Rust services
echo -e "${YELLOW}Building Rust services...${NC}"
cargo build --release
echo -e "${GREEN}✓ Rust services built${NC}"
echo ""

# Build Docker images
echo -e "${YELLOW}Building Docker images...${NC}"
cd build/dockerfiles
docker-compose build
cd ../..
echo -e "${GREEN}✓ Docker images built${NC}"
echo ""

# Start services
echo -e "${YELLOW}Starting Docker services...${NC}"
cd build/dockerfiles
docker-compose up -d
cd ../..
echo -e "${GREEN}✓ Docker services started${NC}"
echo ""

# Wait for Elasticsearch
echo -e "${YELLOW}Waiting for Elasticsearch to be ready...${NC}"
for i in {1..30}; do
    if curl -s http://localhost:9200 &> /dev/null; then
        echo -e "${GREEN}✓ Elasticsearch is ready${NC}"
        break
    fi
    echo -n "."
    sleep 2
done
echo ""

# Wait for Kibana
echo -e "${YELLOW}Waiting for Kibana to be ready...${NC}"
for i in {1..30}; do
    if curl -s http://localhost:5601/api/status &> /dev/null; then
        echo -e "${GREEN}✓ Kibana is ready${NC}"
        break
    fi
    echo -n "."
    sleep 2
done
echo ""

# Import Kibana dashboards (optional)
echo -e "${YELLOW}Importing Kibana dashboards...${NC}"
if curl -X POST "http://localhost:5601/api/saved_objects/_import" \
    -H "kbn-xsrf: true" \
    --form file=@ui/kibana-dashboards/edr-dashboard.ndjson &> /dev/null; then
    echo -e "${GREEN}✓ Dashboards imported${NC}"
else
    echo -e "${YELLOW}⚠ Dashboard import failed (this is optional)${NC}"
fi
echo ""

# Print service endpoints
echo -e "${GREEN}======================================${NC}"
echo -e "${GREEN}  Deployment Complete!${NC}"
echo -e "${GREEN}======================================${NC}"
echo ""
echo -e "Service Endpoints:"
echo -e "  ${GREEN}Controller gRPC:${NC}    localhost:50051"
echo -e "  ${GREEN}Worker gRPC:${NC}        localhost:50052"
echo -e "  ${GREEN}Elasticsearch:${NC}      http://localhost:9200"
echo -e "  ${GREEN}Kibana:${NC}             http://localhost:5601"
echo ""
echo -e "Next steps:"
echo -e "  1. Open Kibana: ${GREEN}http://localhost:5601${NC}"
echo -e "  2. Run controller: ${GREEN}cargo run --release -p scheduler${NC}"
echo -e "  3. Run worker: ${GREEN}cargo run --release -p worker-agent${NC}"
echo -e "  4. Run triage client: ${GREEN}cargo run --release -p triage-client${NC}"
echo ""
echo -e "To stop services: ${GREEN}cd build/dockerfiles && docker-compose down${NC}"
echo ""
