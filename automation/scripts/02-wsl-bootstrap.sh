#!/usr/bin/env bash
# WSL2 Bootstrap: Install Rust, protoc, Elasticsearch, build Controller
set -euo pipefail

# --- WSL systemd and networking setup ---
# Enable systemd in WSL2
echo "[i] Configuring WSL2 systemd and networking..."
if [ -f /etc/wsl.conf ]; then
  # Update existing file: ensure systemd and networking sections
  sudo sed -i '/^\[boot\]/d; /^systemd\s*=/d' /etc/wsl.conf
  sudo sed -i '/^\[network\]/d; /^generateResolvConf\s*=/d' /etc/wsl.conf

  # Add boot section at the top
  echo -e "[boot]\nsystemd=true\n" | sudo tee /etc/wsl.conf >/dev/null
  echo -e "[network]\ngenerateResolvConf = true" | sudo tee -a /etc/wsl.conf >/dev/null
else
  # Create new wsl.conf
  echo -e "[boot]\nsystemd=true\n\n[network]\ngenerateResolvConf = true" | sudo tee /etc/wsl.conf >/dev/null
fi

echo "[i] /etc/wsl.conf configured for systemd"

# Ensure Windows .wslconfig has proper networking
echo "[i] Checking Windows .wslconfig for localhost forwarding..."
WSLCONFIG_PATH="/mnt/c/Users/$USER/.wslconfig"
# Try to find actual Windows username
WIN_USER=$(powershell.exe -Command '$env:USERNAME' 2>/dev/null | tr -d '\r\n' || echo "$USER")
if [ -n "$WIN_USER" ]; then
  WSLCONFIG_PATH="/mnt/c/Users/${WIN_USER}/.wslconfig"
fi

# Create or update .wslconfig
if [ ! -f "$WSLCONFIG_PATH" ]; then
  echo "[i] Creating .wslconfig at $WSLCONFIG_PATH"
  cat > "$WSLCONFIG_PATH" <<'WSLCFG'
[wsl2]
networkingMode=nat
localhostForwarding=true
WSLCFG
  echo "[OK] .wslconfig created"
else
  # Check if it has the required settings
  if ! grep -q "localhostForwarding" "$WSLCONFIG_PATH" 2>/dev/null; then
    echo "[i] Adding localhostForwarding to existing .wslconfig"
    echo -e "\nlocalhostForwarding=true" >> "$WSLCONFIG_PATH"
  fi
  echo "[OK] .wslconfig configured"
fi

# Check if systemd is running
if command -v systemctl &>/dev/null && systemctl is-system-running &>/dev/null 2>&1; then
  echo "[OK] systemd is running"
else
  echo "[WARN] systemd is NOT running."
  echo ""

  # Try to start systemd manually (only works if systemd is enabled in WSL config)
  if command -v systemctl &>/dev/null; then
    sudo /usr/lib/systemd/systemd --system &>/dev/null & disown
    sleep 2
  fi

  # Re-check
  if systemctl is-system-running &>/dev/null 2>&1; then
    echo "[OK] systemd started successfully."
  else
    echo "=========================================="
  fi
fi


# If someone made resolv.conf immutable, undo it
sudo chattr -i /etc/resolv.conf 2>/dev/null || true
# Regenerate if empty/broken
if ! grep -q '^nameserver ' /etc/resolv.conf 2>/dev/null; then
  sudo rm -f /etc/resolv.conf
  # WSL recreates it on launch
  echo "[warn] /etc/resolv.conf was empty; run 'wsl.exe --shutdown' from Windows then re-run this script."
fi

# fail fast
if ! ping -c1 -W1 1.1.1.1 >/dev/null 2>&1; then
  echo "[error] =========================================="
  echo "[error] No outbound connectivity from WSL2!"
  echo "[error] =========================================="
  echo ""
  exit 2
fi


PROJECT_ROOT="${1:-$HOME/automutate}"
WORKDIR="$PROJECT_ROOT/automation"

echo "[i] Project root: $PROJECT_ROOT"
cd "$WORKDIR"

# Update packages
sudo apt update
sudo apt install -y build-essential curl git unzip jq ca-certificates gnupg lsb-release

# Install Docker Engine
if ! command -v docker &>/dev/null; then
    echo "[i] Installing Docker Engine in WSL2..."

    # Add official GPG key
    sudo install -m 0755 -d /etc/apt/keyrings
    curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
    sudo chmod a+r /etc/apt/keyrings/docker.gpg

    # Add Docker repo
    echo \
      "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu \
      $(lsb_release -cs) stable" | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

    # Install Docker Engine + Compose
    sudo apt update
    sudo apt install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin

    # Add current user to docker group
    sudo usermod -aG docker $USER

    # Enable and start Docker via systemd
    sudo systemctl enable docker
    sudo systemctl start docker
    sleep 3

    echo "[OK] Docker Engine installed and started via systemd"
else
    echo "[i] Docker already installed"

    # Ensure Docker service is running
    if ! sudo systemctl is-active --quiet docker; then
        echo "[i] Starting Docker service..."
        sudo systemctl start docker
        sleep 3
    fi
fi

# Verify Docker is running
echo "[i] Verifying Docker daemon..."
if ! sudo docker info &>/dev/null; then
    echo "[ERROR] Docker daemon is not responding"
    echo ""
    echo "Troubleshooting:"
    echo "  sudo systemctl status docker"
    echo "  sudo journalctl -xeu docker"
    exit 1
fi

# Allow current user to use Docker without sudo requires re-login or newgrp
if ! docker info &>/dev/null 2>&1; then
    echo "[i] Docker group membership requires session refresh"
    echo "[i] Using 'sudo docker' for initial setup"
    DOCKER_CMD="sudo docker"
else
    DOCKER_CMD="docker"
fi

echo "[OK] Docker daemon is running"

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

# Parse config.yaml to get Elasticsearch settings
ES_VERSION=$(grep -A5 '^elasticsearch:' "$WORKDIR/config.yaml" | grep 'version:' | sed 's/.*version: *"\(.*\)".*/\1/')
ES_MEMORY=$(grep -A5 '^elasticsearch:' "$WORKDIR/config.yaml" | grep 'memory_gb:' | sed 's/.*memory_gb: *\([0-9]*\).*/\1/')
ES_PORT=$(grep -A10 '^controller:' "$WORKDIR/config.yaml" | grep 'elasticsearch_port:' | sed 's/.*elasticsearch_port: *\([0-9]*\).*/\1/')
KIBANA_PORT=$(grep -A10 '^controller:' "$WORKDIR/config.yaml" | grep 'kibana_port:' | sed 's/.*kibana_port: *\([0-9]*\).*/\1/')

# Use defaults if parsing failed
ES_VERSION="${ES_VERSION:-8.11.0}"
ES_MEMORY="${ES_MEMORY:-4}"
ES_PORT="${ES_PORT:-9200}"
KIBANA_PORT="${KIBANA_PORT:-5601}"

echo "[i] Using Elasticsearch version: $ES_VERSION, memory: ${ES_MEMORY}g, ports: ES=$ES_PORT, Kibana=$KIBANA_PORT"

# Docker Compose for Elasticsearch + Kibana (modern format)
cat > "$WORKDIR/docker-compose.yml" <<EOF
services:
  elasticsearch:
    image: docker.elastic.co/elasticsearch/elasticsearch:$ES_VERSION
    container_name: automation-elasticsearch
    restart: unless-stopped
    environment:
      - discovery.type=single-node
      - xpack.security.enabled=false
      - "ES_JAVA_OPTS=-Xms${ES_MEMORY}g -Xmx${ES_MEMORY}g"
      - bootstrap.memory_lock=true
    ports:
      - "$ES_PORT:9200"
    volumes:
      - esdata:/usr/share/elasticsearch/data
    ulimits:
      memlock:
        soft: -1
        hard: -1
      nofile:
        soft: 65536
        hard: 65536
    healthcheck:
      test: ["CMD-SHELL", "curl -f http://localhost:9200/_cluster/health || exit 1"]
      interval: 30s
      timeout: 10s
      retries: 5
      start_period: 60s
    networks:
      - elastic

  kibana:
    image: docker.elastic.co/kibana/kibana:$ES_VERSION
    container_name: automation-kibana
    restart: unless-stopped
    ports:
      - "$KIBANA_PORT:5601"
    environment:
      - ELASTICSEARCH_HOSTS=http://elasticsearch:9200
      - SERVER_NAME=kibana
      - SERVER_HOST=0.0.0.0
    depends_on:
      elasticsearch:
        condition: service_healthy
    healthcheck:
      test: ["CMD-SHELL", "curl -f http://localhost:5601/api/status || exit 1"]
      interval: 30s
      timeout: 10s
      retries: 5
      start_period: 60s
    networks:
      - elastic

networks:
  elastic:
    driver: bridge

volumes:
  esdata:
    driver: local
EOF

# Set vm.max_map_count for Elasticsearch (persistent across reboots)
echo "[i] Configuring vm.max_map_count for Elasticsearch..."
if ! grep -q "vm.max_map_count" /etc/sysctl.d/99-elasticsearch.conf 2>/dev/null; then
    echo "vm.max_map_count=262144" | sudo tee /etc/sysctl.d/99-elasticsearch.conf >/dev/null
    sudo sysctl -w vm.max_map_count=262144 >/dev/null
    echo "[OK] vm.max_map_count=262144 configured (persistent)"
else
    echo "[OK] vm.max_map_count already configured"
fi

# Create systemd service for Elastic Stack (auto-start on WSL boot)
echo "[i] Creating systemd service for auto-start..."
sudo tee /etc/systemd/system/elastic-stack.service >/dev/null <<SYSTEMD_EOF
[Unit]
Description=Elastic Stack (Elasticsearch + Kibana) for AutoMutate++
Requires=docker.service
After=docker.service network-online.target
Wants=network-online.target

[Service]
Type=oneshot
RemainAfterExit=yes
WorkingDirectory=$WORKDIR
User=$USER
Group=docker

# Clean up any stale containers from previous runs
ExecStartPre=/usr/bin/docker compose down --remove-orphans

# Start stack
ExecStart=/usr/bin/docker compose up -d --wait

# Graceful shutdown
ExecStop=/usr/bin/docker compose down

# Restart policy
Restart=on-failure
RestartSec=10s

[Install]
WantedBy=multi-user.target
SYSTEMD_EOF

# Enable systemd service (will auto-start containers on WSL boot)
sudo systemctl daemon-reload
sudo systemctl enable elastic-stack.service
echo "[OK] Systemd service created and enabled"

# Start Elasticsearch + Kibana
echo "[i] Starting Elasticsearch + Kibana via systemd..."

# Start the systemd service (which will run docker compose)
sudo systemctl start elastic-stack.service

if [ $? -ne 0 ]; then
    echo "[ERROR] Failed to start elastic-stack service"
    echo "[i] Check logs: sudo journalctl -xeu elastic-stack.service"
    exit 1
fi

echo "[OK] Containers started successfully via systemd"
echo "[i] Container status:"
docker compose ps

# Wait for Elasticsearch to be healthy
echo "[i] Waiting for Elasticsearch to be healthy (max 90 seconds)..."
MAX_RETRIES=45
RETRY_COUNT=0
while [ $RETRY_COUNT -lt $MAX_RETRIES ]; do
    if curl -s http://localhost:"$ES_PORT"/_cluster/health > /dev/null 2>&1; then
        HEALTH=$(curl -s http://localhost:"$ES_PORT"/_cluster/health | grep -o '"status":"[^"]*"' | cut -d'"' -f4)
        if [ "$HEALTH" = "green" ] || [ "$HEALTH" = "yellow" ]; then
            echo "[OK] Elasticsearch is healthy (status: $HEALTH)"
            break
        fi
    fi
    sleep 2
    RETRY_COUNT=$((RETRY_COUNT + 1))
    if [ $((RETRY_COUNT % 5)) -eq 0 ]; then
        echo -n " [${RETRY_COUNT}s]"
    else
        echo -n "."
    fi
done
echo ""

if [ $RETRY_COUNT -eq $MAX_RETRIES ]; then
    echo "[WARN] Elasticsearch did not become healthy after 90 seconds"
    echo "[INFO] Check logs: $COMPOSE_CMD logs elasticsearch"
    echo ""
    echo "Containers may still be starting. You can verify status with: $COMPOSE_CMD ps"
else
    # Test from within WSL
    ES_RESPONSE=$(curl -s http://localhost:"$ES_PORT" 2>/dev/null || echo "failed")
    if [[ "$ES_RESPONSE" == *"cluster_name"* ]]; then
        echo "[OK] Elasticsearch accessible: http://localhost:$ES_PORT"
    fi

    # Wait a bit for Kibana to be ready (it waits for Elasticsearch healthcheck)
    echo "[i] Waiting for Kibana (depends on Elasticsearch health)..."
    sleep 10
    KIBANA_RETRIES=15
    KIBANA_COUNT=0
    while [ $KIBANA_COUNT -lt $KIBANA_RETRIES ]; do
        if curl -s http://localhost:"$KIBANA_PORT"/api/status > /dev/null 2>&1; then
            echo "[OK] Kibana is ready: http://localhost:$KIBANA_PORT"
            break
        fi
        sleep 2
        KIBANA_COUNT=$((KIBANA_COUNT + 1))
        echo -n "."
    done
    echo ""
fi

echo ""
echo "=========================================="
echo "Service Endpoints (accessible from Windows & WSL):"
echo "  Elasticsearch: http://localhost:$ES_PORT"
echo "  Kibana:        http://localhost:$KIBANA_PORT"
echo "=========================================="
echo ""
echo "Container Management (via systemd):"
echo "  Status:   sudo systemctl status elastic-stack"
echo "  Logs:     docker compose -f $WORKDIR/docker-compose.yml logs -f [elasticsearch|kibana]"
echo "  Stop:     sudo systemctl stop elastic-stack"
echo "  Restart:  sudo systemctl restart elastic-stack"
echo "  Disable:  sudo systemctl disable elastic-stack  # prevent auto-start"
echo ""
echo "Containers will automatically start when WSL starts (systemd service enabled)"
echo "WSL will stay alive as long as the systemd service is running"
echo ""

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
url = "http://localhost:$ES_PORT"
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
