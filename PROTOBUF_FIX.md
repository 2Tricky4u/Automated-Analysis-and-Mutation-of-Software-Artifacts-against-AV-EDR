# Protocol Buffers Build Fix

## Problem

The `selector` and `triage-engine` binaries were failing to compile with the error:
```
error: couldn't read `.../out/edr.rs`: No such file or directory
  --> controller/selector/src/main.rs:14:5
   |
14 |     tonic::include_proto!("edr");
```

## Root Cause

The issue had two parts:

1. **Proto package naming mismatch**: The code was using `tonic::include_proto!("edr")` which expects a file named `edr.rs`, but the actual proto files define packages `edr.controller`, `edr.common`, and `edr.worker`, which generate files named `edr.controller.rs`, `edr.common.rs`, and `edr.worker.rs`.

2. **Missing protoc on Windows**: The `protoc` (Protocol Buffers compiler) was not available in the PATH on Windows, preventing the build.rs scripts from compiling the proto files.

## Solution

### 1. Fixed Proto Module Inclusion

Updated both `controller/selector/src/main.rs` and `controller/triage-engine/src/main.rs`:

**Before:**
```rust
pub mod edr {
    tonic::include_proto!("edr");
}

use edr::controller::{...};
```

**After:**
```rust
pub mod edr {
    pub mod controller {
        tonic::include_proto!("edr.controller");
    }
    pub mod common {
        tonic::include_proto!("edr.common");
    }
}

use edr::controller::{...};
```

This matches the package structure defined in the proto files:
- `controller/proto/common.proto` → package `edr.common`
- `controller/proto/controller.proto` → package `edr.controller`
- `controller/proto/worker.proto` → package `edr.worker`

### 2. Ensured protoc Availability

**For Local Development (Windows):**
- Protoc should be installed via Anaconda: `C:\Users\xagao\anaconda3\Library\bin\protoc.exe`
- Set environment variable: `PROTOC=C:\Users\xagao\anaconda3\Library\bin\protoc.exe`
- Or add to PATH for permanent solution

**For CI (Linux):**
- Already configured in `.github/workflows/ci.yml`
- Installs `protobuf-compiler` via apt-get
- No changes needed

### 3. Updated CI Workflow Order

Modified `.github/workflows/ci.yml` to run Build before Clippy:

```yaml
- name: Check formatting
  run: cargo fmt --all -- --check

- name: Build
  run: cargo build --verbose --workspace

- name: Clippy
  run: cargo clippy --workspace -- -D warnings
```

This ensures that build.rs scripts run first to generate proto files before Clippy checks the code.

### 4. Updated Docker Images to Rust 1.90

Changed all Dockerfiles from `rust:1.75-slim` to `rust:1.90-slim` to match the local Cargo.lock version 4:

- `build/dockerfiles/Dockerfile.controller`
- `build/dockerfiles/Dockerfile.worker`
- `build/dockerfiles/Dockerfile.collector`

## Verification

After these changes:
- ✅ `cargo build --workspace` succeeds
- ✅ All proto files are generated correctly
- ✅ selector and triage-engine binaries compile
- ✅ CI should pass (Build → Clippy → Tests)
- ✅ Docker builds should succeed with Rust 1.90

## Proto File Structure

The project uses a modular proto file structure:

```
controller/proto/
├── edr.proto         (legacy, used by scheduler)
├── common.proto      (shared types: JobId, RunResult, etc.)
├── controller.proto  (Controller, Selector, Triage services)
└── worker.proto      (WorkerAgent service)
```

**Scheduler** uses the old monolithic `edr.proto`:
```rust
// scheduler/build.rs
tonic_build::compile_protos("../proto/edr.proto")?;

// scheduler/src/main.rs
pub mod edr {
    tonic::include_proto!("edr");
}
```

**Selector and Triage** use the modular split files:
```rust
// selector/build.rs
tonic_build::configure()
    .compile(&[
        "../proto/common.proto",
        "../proto/controller.proto",
        "../proto/worker.proto",
    ], &["../proto"])?;

// selector/src/main.rs
pub mod edr {
    pub mod controller {
        tonic::include_proto!("edr.controller");
    }
    pub mod common {
        tonic::include_proto!("edr.common");
    }
}
```

## Local Build Commands

```bash
# Windows (Git Bash/MSYS2)
export PROTOC="C:\Users\xagao\anaconda3\Library\bin\protoc.exe"
cargo build --workspace

# Linux/WSL2/CI
# protoc already in PATH from apt install
cargo build --workspace
```
