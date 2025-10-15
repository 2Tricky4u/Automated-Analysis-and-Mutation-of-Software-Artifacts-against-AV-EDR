# Migration Guide: Aligning All Code to automation/

This guide documents the complete migration of the codebase to use `automation/` folder configurations exclusively.

---

## Summary of Changes

✅ **Completed**:
1. Updated `config` crate to parse TOML files from `automation/templates/`
2. CI now validates native builds (Linux Controller + Windows Worker)
3. Deprecated `build/dockerfiles/` Docker Compose setup
4. Created comprehensive documentation for config file locations

⏳ **Configuration is now optional**:
- Binaries use environment variables and CLI arguments by default
- Config files (TOML) are optional for advanced customization
- Default hardcoded values align with `automation/templates/`

---

## Configuration Architecture (NEW)

### Config File Locations

**Development** (local testing):
```bash
# Not required - binaries use sensible defaults
# Optional: Create config files for customization
./config/controller.toml   # Controller overrides
./config/worker.toml        # Worker overrides
```

**Production** (automation/ deployment):
```bash
# Controller (WSL2):
~/automutate/config/controller.toml

# Workers (Hyper-V Windows VMs):
C:\AutoMutate\worker.toml
```

**Templates** (source of truth):
```bash
automation/templates/controller.toml
automation/templates/worker.toml
```

---

## How Binaries Load Configuration

### Priority Order (All Binaries)

1. **CLI argument**: `--config /path/to/config.toml`
2. **Environment variable**:
   - Controller: `AUTOMUTATE_CONTROLLER_CONFIG`
   - Worker: `AUTOMUTATE_WORKER_CONFIG`
3. **Deployed location**:
   - Controller: `~/automutate/config/controller.toml`
   - Worker: `C:\AutoMutate\worker.toml`
4. **Local development**: `./config/{controller|worker}.toml`
5. **Template fallback**: `automation/templates/{controller|worker}.toml`
6. **Hardcoded defaults** (if no files exist)

### Example Usage

**Controller with default settings**:
```bash
# Uses hardcoded defaults (localhost:9200, port 50051, etc.)
./target/release/controller-scheduler
```

**Controller with custom config**:
```bash
# Use specific config file
./target/release/controller-scheduler --config /custom/path/controller.toml

# OR use environment variable
export AUTOMUTATE_CONTROLLER_CONFIG=/custom/path/controller.toml
./target/release/controller-scheduler
```

**Worker with automation/ deployed config**:
```powershell
# Automatically uses C:\AutoMutate\worker.toml (deployed by automation/)
C:\AutoMutate\worker-agent.exe
```

---

## Rust Code Changes

### config crate (COMPLETE REWRITE)

**Before** (config.yml - YAML):
```rust
// Old API (REMOVED)
use config::AppConfig;

let cfg = AppConfig::load()?;  // Loaded config.yml
let es_url = cfg.elasticsearch.hosts[0];
```

**After** (controller.toml / worker.toml - TOML):
```rust
// New API (CURRENT)
use edr_config::ControllerConfig;

// Option 1: Auto-discovery (follows priority order)
let cfg = ControllerConfig::load()?;
let es_url = cfg.elasticsearch.url;

// Option 2: Explicit path
let cfg = ControllerConfig::from_file("path/to/controller.toml")?;

// Option 3: Defaults (no config file needed)
let cfg = ControllerConfig::load_or_default();
```

### Updated Structs

**New TOML-aligned structs**:
```rust
// controller.toml structure
pub struct ControllerConfig {
    pub server: ServerConfig,
    pub elasticsearch: ElasticsearchConfig,
    pub triage: TriageConfig,
    pub mutator: MutatorConfig,
    pub scheduler: SchedulerConfig,
    pub corpus: CorpusConfig,
    pub logging: LoggingConfig,
    pub metrics: MetricsConfig,
    pub telemetry: TelemetryConfig,
    pub differential: DifferentialConfig,
    pub experiments: ExperimentsConfig,
}

// worker.toml structure
pub struct WorkerConfig {
    pub worker: WorkerIdentityConfig,
    pub controller: ControllerEndpointConfig,
    pub harness: HarnessConfig,
    pub telemetry: WorkerTelemetryConfig,
    pub build: BuildConfig,
    pub storage: StorageConfig,
    pub logging: LoggingConfig,
    pub health: HealthConfig,
    pub security: SecurityConfig,
}
```

---

## Binary Entry Points (Updated)

### Controller Binaries

All Controller binaries (scheduler, selector, mutator, triage-engine) now support:

```bash
# Default (hardcoded values)
./controller-scheduler

# Custom config
./controller-scheduler --config ~/automutate/config/controller.toml

# Environment variable
export AUTOMUTATE_CONTROLLER_CONFIG=/path/to/controller.toml
./controller-scheduler
```

### Worker Binaries

```powershell
# Default (searches for C:\AutoMutate\worker.toml)
.\worker-agent.exe

# Custom config
.\worker-agent.exe --config D:\custom\worker.toml

# Environment variable
$env:AUTOMUTATE_WORKER_CONFIG = "D:\custom\worker.toml"
.\worker-agent.exe
```

### Collector

```bash
# Default
./collector

# Custom config (optional)
export AUTOMUTATE_CONTROLLER_CONFIG=/path/to/controller.toml
./collector
```

---

## Deployment Workflow (automation/)

### 1. Templates → Deployed Configs

**automation/** scripts automatically copy and customize templates:

```powershell
# Setup Controller (WSL2)
.\automation\scripts\02-wsl-bootstrap.sh
# → Copies automation/templates/controller.toml to ~/automutate/config/controller.toml

# Setup Workers (Hyper-V VMs)
.\automation\scripts\04-vm-init.ps1
# → Copies automation/templates/worker.toml to C:\AutoMutate\worker.toml
# → Customizes worker_id and ip_address for each VM
```

### 2. No Manual Config Needed

Binaries automatically find the deployed configs:

```bash
# Controller (WSL2)
cd ~/automutate
./target/release/controller-scheduler
# ✓ Loads ~/automutate/config/controller.toml automatically

# Worker (Windows VM)
C:\AutoMutate\worker-agent.exe
# ✓ Loads C:\AutoMutate\worker.toml automatically
```

---

## Development Workflow

### Option A: No Config Files (Quickstart)

```bash
# Build and run with defaults
cargo build --release -p controller-scheduler
./target/release/controller-scheduler

# Defaults:
# - Bind: 0.0.0.0:50051
# - Elasticsearch: http://localhost:9200
# - Index prefix: automutate-
# - All features enabled (API tracing, BB coverage, triage, etc.)
```

### Option B: Local Config Override

```bash
# Copy template for customization
cp automation/templates/controller.toml ./config/controller.toml

# Edit settings
nano ./config/controller.toml

# Run (auto-discovers ./config/controller.toml)
cargo run -p controller-scheduler
```

### Option C: Explicit Config Path

```bash
# Use any config file location
cargo run -p controller-scheduler -- --config /tmp/test-controller.toml
```

---

## CI/CD Integration

### GitHub Actions (Updated)

**.github/workflows/ci.yml** now validates:
1. ✅ Linux Controller build (matches WSL2 deployment)
2. ✅ Windows Worker build (matches Hyper-V VMs)
3. ✅ Config crate compiles with TOML support
4. ✅ All workspace crates build successfully

**No Docker images** are built (deprecated).

---

## Backwards Compatibility

### Removed

- ❌ `config.yml` loading (YAML format)
- ❌ `config::AppConfig` struct
- ❌ `/etc/edr-lab/config` search path
- ❌ Docker Compose-based deployment

### Migration Path (If You Used Old Config)

**Old config.yml**:
```yaml
controller:
  host: 0.0.0.0
  port: 50051
elasticsearch:
  hosts: ["http://localhost:9200"]
```

**New controller.toml**:
```toml
[server]
bind_address = "0.0.0.0:50051"

[elasticsearch]
url = "http://localhost:9200"
index_prefix = "automutate-"
```

**Migration steps**:
1. Delete `config.yml` (no longer used)
2. Copy `automation/templates/controller.toml` to `./config/controller.toml`
3. Customize settings
4. Rebuild binaries: `cargo build --release`

---

## Environment Variables

### Supported Variables

| Variable | Purpose | Example |
|----------|---------|---------|
| `AUTOMUTATE_CONTROLLER_CONFIG` | Controller config path | `/custom/controller.toml` |
| `AUTOMUTATE_WORKER_CONFIG` | Worker config path | `C:\Custom\worker.toml` |
| `WORKER_ID` | Worker identification | `win11-worker-02` |
| `RUST_LOG` | Logging level | `info`, `debug`, `trace` |

### Example

```bash
# Controller with custom config and debug logging
export AUTOMUTATE_CONTROLLER_CONFIG=/tmp/controller.toml
export RUST_LOG=debug
./target/release/controller-scheduler
```

---

## Testing Configuration Changes

### 1. Validate TOML Syntax

```bash
# Install toml-cli (if needed)
cargo install toml-cli

# Validate syntax
toml check automation/templates/controller.toml
toml check automation/templates/worker.toml
```

### 2. Test Config Loading

```bash
# Build config crate tests
cargo test -p edr-config

# Test Controller loading
cargo run -p controller-scheduler -- --config automation/templates/controller.toml

# Test Worker loading (on Windows)
cargo run -p worker-agent -- --config automation/templates/worker.toml
```

### 3. Verify Defaults

```bash
# Run without any config files (should use hardcoded defaults)
mv config config.bak  # Hide local configs
./target/release/controller-scheduler
# ✓ Should start with default settings
```

---

## Troubleshooting

### Config File Not Found

**Error**:
```
Error: No such file or directory (os error 2)
```

**Solution**:
1. Check search paths: `automation/templates/`, `./config/`, `~/.config/automutate/`
2. Use explicit path: `--config /full/path/to/config.toml`
3. Or use defaults: Binary will use hardcoded values if no config found

### TOML Parse Error

**Error**:
```
Error: TOML parse error at line 10, column 5
  |
10| url = http://localhost:9200
  |       ^
expected quoted string
```

**Solution**:
- TOML strings must be quoted: `url = "http://localhost:9200"`
- Validate syntax: `toml check file.toml`

### Wrong Config Version

**Error**:
```
Error: missing field `differential` at line 1 column 1
```

**Solution**:
- Old template version. Re-copy from `automation/templates/`
- Or add missing section manually (see templates for structure)

---

## Quick Reference

### File Locations Summary

| File | Purpose | Location (Dev) | Location (Prod) |
|------|---------|----------------|-----------------|
| `controller.toml` | Controller runtime config | `./config/` | `~/automutate/config/` (WSL2) |
| `worker.toml` | Worker runtime config | `./config/` | `C:\AutoMutate\` (Windows VM) |
| `config.yaml` | Environment setup | `automation/` | N/A (setup only) |
| `docker-compose.yml` | Elasticsearch/Kibana | `automation/templates/` | `~/automutate/` (WSL2) |

### Command Cheat Sheet

```bash
# Development
cargo build --release                    # Build with defaults
cargo run -p controller-scheduler        # Run Controller
cargo run -p worker-agent                # Run Worker (Windows)

# Custom config
export AUTOMUTATE_CONTROLLER_CONFIG=/path/to/controller.toml
./target/release/controller-scheduler

# Production (automation/)
cd automation && .\setup-all.ps1         # One-time setup
.\scripts\start-environment.ps1          # Start all services
.\scripts\stop-environment.ps1           # Stop all services

# Validation
toml check automation/templates/*.toml   # Check syntax
cargo test -p edr-config                 # Test config loading
```

---

## What Changed (Summary)

| Component | Before | After |
|-----------|--------|-------|
| **Config format** | YAML (`config.yml`) | TOML (`controller.toml`, `worker.toml`) |
| **Config location** | `/etc/edr-lab/`, `~/.edr-lab/` | `automation/templates/`, deployed paths |
| **Config crate** | `config` (0.13) | `toml` (0.8), custom parsing |
| **Config loading** | Mandatory | Optional (defaults available) |
| **Docker deployment** | Primary method | Deprecated (Elasticsearch/Kibana only) |
| **Native binaries** | Secondary | Primary (WSL2 + Hyper-V VMs) |
| **CI validation** | Docker images | Native Linux + Windows builds |

---

## Next Steps

1. ✅ **Config crate updated** - TOML support complete
2. ✅ **CI aligned** - Validates native builds
3. ✅ **Documentation updated** - All guides reference `automation/`
4. ⏳ **Optional**: Update binary entry points to parse `--config` CLI argument
5. ⏳ **Optional**: Add config validation tests

---

**Questions?** See:
- [automation/README.md](automation/README.md) - Deployment guide
- [config/README.md](config/README.md) - Config crate documentation
- [ARCHITECTURE_ALIGNMENT.md](ARCHITECTURE_ALIGNMENT.md) - Architecture overview

**Last Updated**: 2025-01-14