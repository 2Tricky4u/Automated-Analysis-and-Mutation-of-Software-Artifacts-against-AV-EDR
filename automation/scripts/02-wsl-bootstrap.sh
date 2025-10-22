#!/usr/bin/env bash
# WSL2 Bootstrap: Install Rust, protoc, Elasticsearch, build Controller
set -euo pipefail

# --- WSL systemd and networking setup ---
# Enable systemd in WSL2 (required for Docker to run properly)
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

# Also ensure Windows .wslconfig has proper networking (for localhost forwarding)
echo "[i] Checking Windows .wslconfig for localhost forwarding..."
WSLCONFIG_PATH="/mnt/c/Users/$USER/.wslconfig"
# Try to find actual Windows username (might differ from WSL username)
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

# Check if systemd is running (if not, user needs to restart WSL)
SYSTEMD_RUNNING=false
if command -v systemctl &>/dev/null && systemctl is-system-running &>/dev/null 2>&1; then
  SYSTEMD_RUNNING=true
  echo "[OK] systemd is running"
else
  echo "[WARN] systemd is NOT running yet. WSL restart required."
  echo ""
  echo "=========================================="
  echo "ACTION REQUIRED: Restart WSL2"
  echo "=========================================="
  echo ""
  echo "From Windows PowerShell, run:"
  echo "  wsl --shutdown"
  echo ""
  echo "Then re-run this script to continue setup."
  echo ""
  echo "=========================================="
  exit 3
fi

# If someone made resolv.conf immutable, undo it
sudo chattr -i /etc/resolv.conf 2>/dev/null || true
# Regenerate if empty/broken
if ! grep -q '^nameserver ' /etc/resolv.conf 2>/dev/null; then
  sudo rm -f /etc/resolv.conf
  # WSL recreates it on launch; advise user if we cannot refresh now
  echo "[warn] /etc/resolv.conf was empty; run 'wsl.exe --shutdown' from Windows then re-run this script."
fi

# Quick connectivity gate: fail fast with a helpful hint
if ! ping -c1 -W1 1.1.1.1 >/dev/null 2>&1; then
  echo "[error] =========================================="
  echo "[error] No outbound connectivity from WSL2!"
  echo "[error] =========================================="
  echo ""
  echo "Troubleshooting steps:"
  echo "  1. From Windows PowerShell (Admin), run:"
  echo "     wsl --shutdown"
  echo "     .\automation\scripts\01-host-setup.ps1 -ConfigPath .\automation\config.yaml"
  echo ""
  echo "  2. Verify .wslconfig exists at: %USERPROFILE%\.wslconfig"
  echo "     Should contain: networkingMode=nat, dnsTunneling=true"
  echo ""
  echo "  3. Check Windows Firewall allows WSL vEthernet adapter outbound"
  echo ""
  echo "  4. Test from Windows: ping 1.1.1.1"
  echo "     If Windows can't reach internet, fix host connectivity first"
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

# Install Docker Engine (not Docker Desktop - WSL2 native)
if ! command -v docker &>/dev/null; then
    echo "[i] Installing Docker Engine in WSL2..."

    # Add Docker's official GPG key
    sudo install -m 0755 -d /etc/apt/keyrings
    curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
    sudo chmod a+r /etc/apt/keyrings/docker.gpg

    # Add Docker repository
    echo \
      "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu \
      $(lsb_release -cs) stable" | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

    # Install Docker Engine + Compose plugin
    sudo apt update
    sudo apt install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin

    # Add current user to docker group (avoid sudo docker)
    sudo usermod -aG docker $USER

    # Enable and start Docker via systemd (systemd is required, verified earlier)
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

# Allow current user to use Docker without sudo (requires re-login or newgrp)
if ! docker info &>/dev/null 2>&1; then
    echo "[i] Docker group membership requires session refresh"
    echo "[i] Using 'sudo docker' for initial setup (user will have access after logout/login)"
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
echo "[i] Starting Elasticsearch + Kibana in background..."
echo ""
echo "NOTE: Docker containers run in foreground mode via nohup"
echo "      This is required due to systemd compatibility issues with 'docker compose up -d'"
echo ""

# Determine Docker Compose command
COMPOSE_CMD=""
if docker compose version &>/dev/null 2>&1; then
    COMPOSE_CMD="docker compose"
elif command -v docker-compose &>/dev/null; then
    COMPOSE_CMD="docker-compose"
else
    echo "[ERROR] Docker Compose not found (neither 'docker compose' nor 'docker-compose')"
    exit 1
fi

# Start Elasticsearch in background (foreground mode via nohup)
echo "[i] Starting Elasticsearch..."
nohup $COMPOSE_CMD up elasticsearch > /tmp/elasticsearch.log 2>&1 &
ES_PID=$!
echo "[i] Elasticsearch process started (PID: $ES_PID)"

# Start Kibana in background (foreground mode via nohup)
echo "[i] Starting Kibana..."
nohup $COMPOSE_CMD up kibana > /tmp/kibana.log 2>&1 &
KIBANA_PID=$!
echo "[i] Kibana process started (PID: $KIBANA_PID)"

# Wait for Elasticsearch to be ready
echo "[i] Waiting for Elasticsearch to be ready (max 60 seconds)..."
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
    echo ""
    echo "Containers may still be starting. You can verify access later"
else
    # Test from within WSL
    ES_RESPONSE=$(curl -s http://localhost:9200 2>/dev/null || echo "failed")
    if [[ "$ES_RESPONSE" == *"cluster_name"* ]]; then
        echo "[OK] Elasticsearch accessible: http://localhost:9200"
    fi
fi

echo ""
echo "=========================================="
echo "Service Endpoints (accessible from Windows & WSL):"
echo "  Elasticsearch: http://localhost:9200"
echo "  Kibana:        http://localhost:5601"
echo "=========================================="
echo ""
echo "Background Processes:"
echo "  Elasticsearch PID: $ES_PID"
echo "  Kibana PID:        $KIBANA_PID"
echo ""
echo "Logs:"
echo "  tail -f /tmp/elasticsearch.log"
echo "  tail -f /tmp/kibana.log"
echo ""
echo "To stop services:"
echo "  kill $ES_PID $KIBANA_PID"
echo "  OR: $COMPOSE_CMD down"
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
