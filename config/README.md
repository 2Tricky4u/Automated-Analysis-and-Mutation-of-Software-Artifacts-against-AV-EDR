# config/ - Configuration Parsing Crate

This directory contains a **Rust library crate** for parsing TOML configuration files, not the actual runtime configuration files themselves.

---

## Purpose

This crate provides:
- ✅ Rust structs for configuration (e.g., `ControllerConfig`, `WorkerConfig`)
- ✅ TOML deserialization logic
- ✅ Validation and defaults

**This crate does NOT contain:**
- ❌ Actual `.toml` files (those live in `automation/templates/`)
- ❌ Environment setup configs (see `automation/config.yaml`)

---

## Where Are the Actual Config Files?

### 1. Configuration Templates

**Location:** `automation/templates/`

```
automation/templates/
├── controller.toml     # Template for Controller runtime config
├── worker.toml         # Template for Worker runtime config
└── docker-compose.yml  # Elasticsearch/Kibana config
```

### 2. Deployed Configurations

After running `automation/setup-all.ps1`, configs are deployed to:

**Controller (WSL2):**
```bash
~/automutate/config/controller.toml
```

**Workers (Hyper-V VMs):**
```powershell
C:\AutoMutate\worker.toml
```

---

## How This Crate Is Used

```rust
// In controller-scheduler/src/main.rs
use config::ControllerConfig;

fn main() {
    // Load deployed config
    let cfg = ControllerConfig::from_file("config/controller.toml")?;

    // Access settings
    let es_url = cfg.elasticsearch.url;
    let port = cfg.server.bind_address;

    // Start server with config
    start_grpc_server(cfg)?;
}
```

---

## Development Workflow

### 1. Modify Configuration Schema

Edit Rust structs in `config/src/lib.rs`:

```rust
// Example: Add new field to ControllerConfig
#[derive(Deserialize)]
pub struct ElasticsearchConfig {
    pub url: String,
    pub index_prefix: String,
    pub bulk_size: usize,
    // Add new field:
    pub new_setting: bool,  // ← New field
}
```

### 2. Update Templates

Update `automation/templates/controller.toml`:

```toml
[elasticsearch]
url = "http://localhost:9200"
index_prefix = "automutate-"
bulk_size = 100
new_setting = true  # ← Add corresponding TOML field
```

### 3. Rebuild and Redeploy

```powershell
# Rebuild with new config schema
cargo build --release -p controller-scheduler

# Redeploy to WSL2 (automation scripts handle this)
cd automation
.\scripts\start-environment.ps1
```

---

## File Structure

```
config/
├── Cargo.toml          # Rust crate manifest
├── src/
│   ├── lib.rs          # Configuration structs
│   ├── controller.rs   # ControllerConfig definition
│   ├── worker.rs       # WorkerConfig definition
│   └── validation.rs   # Config validation logic
└── README.md           # This file
```

---

## Example Configuration (NOT stored here)

This is what the actual config files look like (stored in `automation/templates/`):

### controller.toml
```toml
[server]
bind_address = "0.0.0.0:50051"
max_connections = 100

[elasticsearch]
url = "http://localhost:9200"
index_prefix = "automutate-"

[triage]
confidence_threshold = 0.7
```

### worker.toml
```toml
[worker]
worker_id = "win11-worker-01"
ip_address = "192.168.200.100"

[controller]
controller_address = "192.168.200.1:50051"

[telemetry.etw]
enabled = true
buffer_size_kb = 1024
```

---

## Common Mistakes

### ❌ Mistake 1: Looking for configs in config/
```bash
# Wrong: Config files are NOT here
cat config/controller.toml  # ❌ File doesn't exist
```

```bash
# Correct: Templates are in automation/
cat automation/templates/controller.toml  # ✅
```

### ❌ Mistake 2: Manually editing deployed configs
```bash
# Wrong: Manual edits will be overwritten
nano ~/automutate/config/controller.toml  # ❌ Will be lost on redeploy
```

```bash
# Correct: Edit template, then redeploy
nano automation/templates/controller.toml  # ✅
cd automation && .\scripts\start-environment.ps1
```

### ❌ Mistake 3: Expecting config/ to contain .toml files
```
config/
├── controller.toml  # ❌ This should NOT exist here
└── worker.toml      # ❌ This should NOT exist here
```

**Reason:** This is a library crate for **parsing** configs, not a storage location for config files.

---

## Testing Config Changes

```bash
# 1. Modify config struct
nano config/src/controller.rs

# 2. Write test
cargo test -p config

# 3. Update template
nano automation/templates/controller.toml

# 4. Build binaries
cargo build --release

# 5. Validate deployment
cd automation
.\scripts\validate-environment.ps1
```

---

## API Documentation

```bash
# Generate Rust docs for this crate
cargo doc -p config --open
```

---

## Dependencies

This crate depends on:
- `serde` - Serialization framework
- `toml` - TOML parser
- `anyhow` - Error handling

See `Cargo.toml` for full dependency list.

---

## See Also

- [automation/templates/controller.toml](../automation/templates/controller.toml) - Controller config template
- [automation/templates/worker.toml](../automation/templates/worker.toml) - Worker config template
- [automation/config.yaml](../automation/config.yaml) - Environment setup config
- [ARCHITECTURE_ALIGNMENT.md](../ARCHITECTURE_ALIGNMENT.md) - Config file locations explained

---

**Last Updated:** 2025-01-14
