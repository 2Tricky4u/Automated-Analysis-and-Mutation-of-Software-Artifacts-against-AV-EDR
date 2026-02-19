.PHONY: all build test clean docker-build docker-up docker-down help

# Default target
all: build

# Build all Rust services
build:
	@echo "Building Rust services..."
	cargo build --release

# Run tests
test:
	@echo "Running tests..."
	cargo test --workspace

# Clean build artifacts
clean:
	@echo "Cleaning build artifacts..."
	cargo clean
	rm -rf telemetry/etw-consumer/build

# Build Docker images
docker-build:
	@echo "Building Docker images..."
	cd build/dockerfiles && docker-compose build

# Start Docker services
docker-up:
	@echo "Starting Docker services..."
	cd build/dockerfiles && docker-compose up -d
	@echo "Waiting for services to start..."
	@sleep 10
	@echo "Services started. Check status with 'make docker-status'"

# Stop Docker services
docker-down:
	@echo "Stopping Docker services..."
	cd build/dockerfiles && docker-compose down

# Check Docker service status
docker-status:
	@cd build/dockerfiles && docker-compose ps

# View Docker logs
docker-logs:
	@cd build/dockerfiles && docker-compose logs -f

# Build ETW consumer (requires CMake and C++ compiler)
etw-consumer:
	@echo "Building ETW consumer..."
	@mkdir -p telemetry/etw-consumer/build
	@cd telemetry/etw-consumer/build && cmake .. && cmake --build . --config Release

# Run controller
run-controller:
	@echo "Starting controller..."
	cargo run --release -p controller

# Run worker
run-worker:
	@echo "Starting worker..."
	cargo run --release -p worker-agent

# Format code
fmt:
	@echo "Formatting code..."
	cargo fmt --all

# Lint code
lint:
	@echo "Linting code..."
	cargo clippy --workspace -- -D warnings

# Check Elasticsearch
check-elastic:
	@echo "Checking Elasticsearch..."
	@curl -s http://localhost:9200 | jq '.' || echo "Elasticsearch not reachable"

# Check Kibana
check-kibana:
	@echo "Checking Kibana..."
	@curl -s http://localhost:5601/api/status | jq '.' || echo "Kibana not reachable"

# Import Kibana dashboards
import-dashboards:
	@echo "Importing Kibana dashboards..."
	@curl -X POST "localhost:5601/api/saved_objects/_import" \
		-H "kbn-xsrf: true" \
		--form file=@ui/kibana-dashboards/edr-dashboard.ndjson

# Full setup (build everything and start services)
setup: build docker-up
	@echo "Setup complete!"
	@echo "Controller gRPC: localhost:50051"
	@echo "Worker gRPC: localhost:50052"
	@echo "Elasticsearch: http://localhost:9200"
	@echo "Kibana: http://localhost:5601"

# Help
help:
	@echo "EDR Lab Makefile"
	@echo ""
	@echo "Available targets:"
	@echo "  build              - Build all Rust services"
	@echo "  test               - Run all tests"
	@echo "  clean              - Clean build artifacts"
	@echo "  docker-build       - Build Docker images"
	@echo "  docker-up          - Start Docker services"
	@echo "  docker-down        - Stop Docker services"
	@echo "  docker-status      - Check Docker service status"
	@echo "  docker-logs        - View Docker logs"
	@echo "  etw-consumer       - Build ETW consumer (C++)"
	@echo "  run-controller     - Run controller service"
	@echo "  run-worker         - Run worker service"
	@echo "  fmt                - Format code"
	@echo "  lint               - Lint code"
	@echo "  check-elastic      - Check Elasticsearch status"
	@echo "  check-kibana       - Check Kibana status"
	@echo "  import-dashboards  - Import Kibana dashboards"
	@echo "  setup              - Full setup (build + start services)"
	@echo "  help               - Show this help message"
