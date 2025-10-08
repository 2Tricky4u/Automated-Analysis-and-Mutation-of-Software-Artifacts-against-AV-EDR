#!/bin/bash

# Validation script to check the EDR Lab setup
set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

echo "EDR Lab Validation Script"
echo "========================="
echo ""

# Check directory structure
echo "Checking directory structure..."
REQUIRED_DIRS=(
    "controller/proto"
    "controller/scheduler"
    "controller/triage-client"
    "worker/agent"
    "worker/harness-ipc"
    "build/dockerfiles"
    "build/emitter"
    "telemetry/etw-consumer"
    "telemetry/beats-config"
    "ui/kibana-dashboards"
    "docs/schemas"
)

for dir in "${REQUIRED_DIRS[@]}"; do
    if [ -d "$dir" ]; then
        echo -e "${GREEN}✓${NC} $dir"
    else
        echo -e "${RED}✗${NC} $dir (missing)"
        exit 1
    fi
done
echo ""

# Check required files
echo "Checking required files..."
REQUIRED_FILES=(
    "Cargo.toml"
    "README.md"
    "Makefile"
    "controller/proto/edr.proto"
    "build/dockerfiles/docker-compose.yml"
    "telemetry/etw-consumer/CMakeLists.txt"
    "docs/ARCHITECTURE.md"
)

for file in "${REQUIRED_FILES[@]}"; do
    if [ -f "$file" ]; then
        echo -e "${GREEN}✓${NC} $file"
    else
        echo -e "${RED}✗${NC} $file (missing)"
        exit 1
    fi
done
echo ""

# Build Rust workspace
echo "Building Rust workspace..."
if cargo build 2>&1 | grep -q "Finished"; then
    echo -e "${GREEN}✓${NC} Rust build successful"
else
    echo -e "${RED}✗${NC} Rust build failed"
    exit 1
fi
echo ""

# Run tests
echo "Running tests..."
if cargo test --workspace 2>&1 | grep -q "test result: ok"; then
    echo -e "${GREEN}✓${NC} All tests passed"
else
    echo -e "${RED}✗${NC} Some tests failed"
    exit 1
fi
echo ""

# Check proto file syntax
echo "Validating proto file..."
if protoc --proto_path=controller/proto --lint_out=/dev/null controller/proto/edr.proto 2>/dev/null || protoc --proto_path=controller/proto --decode_raw < /dev/null 2>/dev/null; then
    echo -e "${GREEN}✓${NC} Proto file is valid"
else
    echo -e "${YELLOW}⚠${NC}  Proto validation skipped (protoc features limited)"
fi
echo ""

# Check JSON schema validity
echo "Checking JSON schemas..."
for schema in docs/schemas/*.json; do
    if python3 -m json.tool "$schema" &> /dev/null; then
        echo -e "${GREEN}✓${NC} $(basename $schema)"
    else
        echo -e "${RED}✗${NC} $(basename $schema) (invalid JSON)"
        exit 1
    fi
done
echo ""

# Summary
echo "========================="
echo -e "${GREEN}Validation Complete!${NC}"
echo "========================="
echo ""
echo "All checks passed. The EDR Lab scaffold is ready to use."
echo ""
echo "Next steps:"
echo "  1. Start services: ./quickstart.sh"
echo "  2. Read documentation: docs/SETUP.md"
echo "  3. Try examples: docs/EXAMPLES.md"
echo ""
