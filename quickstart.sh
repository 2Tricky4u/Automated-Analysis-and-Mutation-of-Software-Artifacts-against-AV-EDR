#!/bin/bash

# EDR Lab Quick Start Script
set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}════════════════════════════════════════${NC}"
echo -e "${GREEN}  EDR Lab Quick Start${NC}"
echo -e "${GREEN}════════════════════════════════════════${NC}"
echo ""

# Check for required tools
echo -e "${YELLOW}Checking prerequisites...${NC}"
MISSING=()

if ! command -v cargo &> /dev/null; then
    MISSING+=("Rust/Cargo")
fi

if ! command -v protoc &> /dev/null; then
    MISSING+=("protoc")
fi

if ! command -v docker &> /dev/null; then
    MISSING+=("Docker")
fi

if [ ${#MISSING[@]} -ne 0 ]; then
    echo -e "${RED}Missing prerequisites:${NC}"
    for item in "${MISSING[@]}"; do
        echo "  - $item"
    done
    echo ""
    echo "Please install missing prerequisites. See docs/SETUP.md for details."
    exit 1
fi

echo -e "${GREEN}✓ All prerequisites found${NC}"
echo ""

# Build Rust services
echo -e "${YELLOW}Building Rust services...${NC}"
cargo build --release
echo -e "${GREEN}✓ Services built${NC}"
echo ""

# Start infrastructure
echo -e "${YELLOW}Starting infrastructure...${NC}"
cd build/dockerfiles
docker-compose up -d
cd ../..
echo -e "${GREEN}✓ Infrastructure started${NC}"
echo ""

# Wait for Elasticsearch
echo -e "${YELLOW}Waiting for Elasticsearch...${NC}"
for i in {1..30}; do
    if curl -s http://localhost:9200 &> /dev/null; then
        echo -e "${GREEN}✓ Elasticsearch ready${NC}"
        break
    fi
    sleep 2
done
echo ""

# Summary
echo -e "${GREEN}════════════════════════════════════════${NC}"
echo -e "${GREEN}  Quick Start Complete!${NC}"
echo -e "${GREEN}════════════════════════════════════════${NC}"
echo ""
echo "Services are running:"
echo "  • Elasticsearch: http://localhost:9200"
echo "  • Kibana: http://localhost:5601"
echo ""
echo "To start the controller:"
echo "  ${GREEN}cargo run --release -p scheduler${NC}"
echo ""
echo "To start a worker:"
echo "  ${GREEN}cargo run --release -p worker-agent${NC}"
echo ""
echo "To stop all services:"
echo "  ${GREEN}cd build/dockerfiles && docker-compose down${NC}"
echo ""
echo "For more information, see:"
echo "  • README.md - Project overview"
echo "  • docs/SETUP.md - Detailed setup guide"
echo "  • docs/EXAMPLES.md - Usage examples"
echo ""
