# Config Crate Name Collision Fix

## Problem

During CI testing, the following error occurred:

```
error[E0464]: multiple candidates for `rlib` dependency `config` found
  --> config/src/lib.rs:53:24
   |
53 |         let settings = config::Config::builder()
   |                        ^^^^^^
   |
   = note: candidate #1: .../libconfig-37906903efc1f180.rlib
   = note: candidate #2: .../libconfig-4f8a4155276e82e5.rlib
```

## Root Cause

Our workspace crate was named `config`, which conflicts with the `config` dependency from crates.io (version 0.13) that we use for configuration file loading. This created a name collision where Rust couldn't distinguish between:
1. Our crate: `config` (workspace member)
2. The dependency: `config` (from crates.io)

This is a known issue when a workspace crate has the same name as one of its dependencies.

## Solution

Renamed the workspace crate from `config` to `edr-config` to eliminate the naming conflict.

### Changes Made

**File: `config/Cargo.toml`**
```toml
# Before:
[package]
name = "config"

# After:
[package]
name = "edr-config"
```

### Why This Fix Works

- The workspace member is still in the `config/` directory (no directory rename needed)
- The crate is now named `edr-config`, distinct from the `config` dependency
- The `config` crate from crates.io can be used without ambiguity
- Docker builds continue to work (they reference the directory path, not the crate name)

## Alternative Solutions Considered

### 1. Rename the dependency with an alias ❌
```toml
# This was attempted but didn't solve the ambiguity issue
config-rs = { package = "config", version = "0.13" }
```
**Why rejected:** Cargo still showed ambiguity errors when using `-p config`

### 2. Rename to a different name entirely ❌
Could have used names like `edr-app-config`, `shared-config`, etc.
**Why rejected:** `edr-config` is clear, concise, and follows the project's naming convention

## Verification

After the fix:
- ✅ `cargo build --workspace` succeeds
- ✅ `cargo test --workspace` passes (all 0 tests pass, no errors)
- ✅ `cargo clippy --workspace -- -D warnings` passes
- ✅ No name collision errors

## Impact

### Files Changed
- `config/Cargo.toml` - Changed package name from `config` to `edr-config`

### Files NOT Changed
- `config/src/lib.rs` - No changes needed (still uses `config::Config` from crates.io)
- `Cargo.toml` (workspace root) - No changes needed (members use directory paths)
- `build/dockerfiles/*.Dockerfile` - No changes needed (reference directory, not crate name)

### Usage in Other Crates
If any other crates in the workspace depend on our config library, they would need to update their dependencies:

```toml
# Before:
[dependencies]
config.workspace = true  # or config = { path = "../config" }

# After:
[dependencies]
edr-config = { path = "../config" }
```

**Note:** Currently, no other workspace members depend on the config crate, so no downstream changes were needed.

## Best Practices

To avoid this issue in the future:
1. Use unique, project-specific names for workspace crates (e.g., `edr-*`)
2. Check crates.io for existing crate names before creating new workspace members
3. Use descriptive prefixes that match the project namespace

## Related Issues

- **PROTOBUF_FIX.md**: Proto compilation and module inclusion fixes
- **DOCKER_FIX.md**: Docker workspace member requirements
- **SKELETON_100_PERCENT.md**: Achieving 100% CLAUDE.md compliance
