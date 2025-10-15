#!/usr/bin/env bash
# WSL2 Bootstrap: Install Rust, protoc, Elasticsearch, build Controller
set -euo pipefail

PROJECT_ROOT="${1:-$HOME/automutate}"
WORKDIR="$PROJECT_ROOT/automation"

echo "[i] Project root: $PROJECT_ROOT"
cd "$WORKDIR"

# Update packages
sudo apt update
sudo apt install -y build-essential curl git unzip jq ca-certificates gnupg lsb-release docker-compose

# Rust
if ! command -v rustc &>/dev/null; then
    echo "[i] Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
fi

rustup toolchain install stable nightly
rustup default stable
rustup component add llvm-tools-preview rust-src --toolchain nightly

# protoc
if ! command -v protoc &>/dev/null; then
    echo "[i] Installing protoc 25.1..."
    PROTOC_VER="25.1"
    TMPDIR=$(mktemp -d)
    cd "$TMPDIR"
    curl -LO "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VER}/protoc-${PROTOC_VER}-linux-x86_64.zip"
    unzip -q "protoc-${PROTOC_VER}-linux-x86_64.zip" -d protoc3
    sudo mv protoc3/bin/protoc /usr/local/bin/
    sudo mv protoc3/include/* /usr/local/include/
    cd -
    rm -rf "$TMPDIR"
fi

# Docker Compose for Elasticsearch + Kibana
cat > "$WORKDIR/docker-compose.yml" <<'EOF'
version: '3.8'
services:
  elasticsearch:
    image: docker.elastic.co/elasticsearch/elasticsearch:8.11.0
    environment:
      - discovery.type=single-node
      - xpack.security.enabled=false
      - "ES_JAVA_OPTS=-Xms4g -Xmx4g"
    ports:
      - "9200:9200"
    volumes:
      - esdata:/usr/share/elasticsearch/data
    ulimits:
      memlock:
        soft: -1
        hard: -1

  kibana:
    image: docker.elastic.co/kibana/kibana:8.11.0
    ports:
      - "5601:5601"
    environment:
      - ELASTICSEARCH_HOSTS=http://elasticsearch:9200
    depends_on:
      - elasticsearch

volumes:
  esdata:
EOF

# Start Elasticsearch + Kibana
if command -v docker &>/dev/null; then
    echo "[i] Starting Elasticsearch + Kibana..."
    docker-compose up -d
    echo "[OK] Elasticsearch: http://localhost:9200"
    echo "[OK] Kibana: http://localhost:5601"
else
    echo "[WARN] Docker not found. Install Docker Desktop with WSL integration."
    exit 1
fi

# Build Controller binaries
cd "$PROJECT_ROOT"
echo "[i] Building Controller binaries..."
cargo build --release -p scheduler -p selector -p queue -p triage-engine \
    --target-dir "$PROJECT_ROOT/target"

if [ $? -eq 0 ]; then
    echo "[OK] Controller binaries built: $PROJECT_ROOT/target/release/"
else
    echo "[WARN] Build failed. Check Cargo.toml and dependencies."
    exit 1
fi

# Create config files
mkdir -p "$PROJECT_ROOT/config"
cat > "$PROJECT_ROOT/config/controller.toml" <<'EOF'
[server]
bind_address = "0.0.0.0:50051"
max_connections = 100

[elasticsearch]
url = "http://localhost:9200"
index_prefix = "etw-"
bulk_size = 100
bulk_timeout_ms = 5000

[triage]
confidence_threshold = 0.7
EOF

echo "[OK] WSL2 bootstrap complete"
echo ""
echo "Start controller:"
echo "  cd $PROJECT_ROOT"
echo "  ./target/release/scheduler --config config/controller.toml"
