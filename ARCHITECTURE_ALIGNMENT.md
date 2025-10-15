# Architecture Alignment Issues & Fixes

This document tracks inconsistencies between `automation/` (production deployment) and legacy files.

---

## Status: ⚠️ PARTIALLY ALIGNED

CI has been updated, but several config files still conflict.

---

## Critical Issues

### 1. Conflicting Docker Compose Files

**Problem:**
- `build/dockerfiles/docker-compose.yml` tries to run Controller/Worker as containers (WRONG)
- `automation/templates/docker-compose.yml` only runs Elasticsearch/Kibana (CORRECT)

**Fix:**
```powershell
# Option A: Deprecate the old file
mv build/dockerfiles/docker-compose.yml build/dockerfiles/docker-compose.yml.deprecated

# Option B: Replace it with a link to the correct one
rm build/dockerfiles/docker-compose.yml
# (Then document that users should use automation/templates/)
```

**Decision:** Keep both but add clear warnings. Already documented in `build/dockerfiles/README.md`.

---

### 2. Missing config.yml

**Problem:**
- `build/dockerfiles/docker-compose.yml` references `../../config.yml` (doesn't exist)
- This will cause Docker Compose to fail

**Fix:**
Since we're deprecating Docker-based deployment, we **don't** need to create `config.yml`.

**Action:** Add a note to `build/dockerfiles/README.md` explaining this file is not needed for `automation/`.

---

### 3. Elasticsearch Version Mismatch

**Problem:**
- Old docker-compose: 8.14.2 (2GB RAM)
- automation/: 8.11.0 (4GB RAM)

**Fix:**
Update `automation/templates/docker-compose.yml` to use 8.14.2 for consistency with latest stable.

**Recommendation:** Use automation/ version (8.11.0 is tested and stable for this project).

---

## Minor Issues

### 4. No Runtime Config Files in config/

**Problem:**
The `config/` directory is a Rust crate (for parsing config), but doesn't contain actual `.toml` files.

**Current state:**
```
config/
├── Cargo.toml      # ✅ Rust crate for config parsing
└── src/            # ✅ Rust code
```

**Expected by automation/ scripts:**
```
config/
├── controller.toml  # ❌ Missing (runtime config for Controller)
├── worker.toml      # ❌ Missing (runtime config for Worker)
```

**Fix:**
Runtime configs live in `automation/templates/`. The setup scripts copy them to the deployment locations:
- **Controller:** WSL2 at `~/automutate/config/controller.toml`
- **Workers:** VMs at `C:\AutoMutate\worker.toml`

**Action:** Document this in `config/README.md`.

---

## Summary Table

| File/Directory | Status | Action Needed |
|----------------|--------|---------------|
| `.github/workflows/ci.yml` | ✅ FIXED | None (validates native builds) |
| `build/dockerfiles/docker-compose.yml` | ⚠️ DEPRECATED | Keep but document as legacy |
| `build/dockerfiles/README.md` | ✅ CREATED | None (explains deprecation) |
| `automation/templates/docker-compose.yml` | ✅ CORRECT | None (production config) |
| `automation/config.yaml` | ✅ CORRECT | None (environment config) |
| `automation/templates/*.toml` | ✅ CORRECT | None (runtime configs) |
| `config.yml` (root) | ❌ MISSING | Not needed (don't create) |
| `config/*.toml` (runtime) | ❌ MISSING | Not needed (automation/ handles) |

---

## Decision Matrix: Which Files to Use?

### For Development (Rust coding):
```bash
# Build locally (no special config needed)
cargo build --workspace
cargo test --workspace

# CI handles validation automatically on push
```

### For EDR Experiments (production):
```powershell
# Use automation/ exclusively
cd automation
.\setup-all.ps1  # Uses automation/config.yaml + automation/templates/
```

### For Elasticsearch Only (debugging):
```bash
# Use automation/ docker-compose
cd automation
wsl -e docker-compose -f templates/docker-compose.yml up -d
```

---

## Files That Can Be Safely Ignored

These files are **legacy** and not used by `automation/`:

- ❌ `build/dockerfiles/docker-compose.yml` (use `automation/templates/docker-compose.yml`)
- ❌ `build/dockerfiles/docker-compose.dev.yml` (not referenced by automation/)
- ❌ `build/dockerfiles/Dockerfile.*` (automation/ builds native binaries, not containers)

**Keep them?** Yes, for reference. But don't maintain or update them.

---

## Recommended Actions

### Immediate (Critical):
1. ✅ **DONE:** Update CI to validate native builds (not Docker)
2. ✅ **DONE:** Create deprecation notice in `build/dockerfiles/README.md`
3. ⏳ **TODO:** Add `config/README.md` explaining where runtime configs live

### Optional (Cleanup):
4. **Consider:** Rename `build/dockerfiles/docker-compose.yml` to `docker-compose.yml.deprecated`
5. **Consider:** Move `build/dockerfiles/` to `legacy/dockerfiles/` to make deprecation explicit

---

## Testing Checklist

To verify alignment:

```powershell
# 1. Verify CI passes (native builds)
git push  # Check GitHub Actions

# 2. Verify automation/ setup works
cd automation
.\setup-all.ps1

# 3. Verify no references to missing config.yml
rg "config\.yml" --type yaml --type toml
# (Should only find references in deprecated docker-compose.yml)

# 4. Verify Controller/Worker configs exist
ls automation/templates/*.toml
# (Should list controller.toml, worker.toml)

# 5. Verify no confusion about which docker-compose to use
cat build/dockerfiles/README.md
# (Should clearly state "DEPRECATED")
```

---

## Questions?

- **Q: Can I delete `build/dockerfiles/`?**
  - A: Keep it for reference, but mark as deprecated. Someone might need to see the old architecture.

- **Q: Why have two docker-compose.yml files?**
  - A: Historical artifact. The old one (build/dockerfiles/) tried to containerize everything. The new one (automation/templates/) only containerizes Elasticsearch/Kibana (storage layer), while Controller/Worker run as native binaries.

- **Q: Where do runtime configs live?**
  - A: Templates in `automation/templates/`, deployed copies in WSL2 (`~/automutate/config/`) and VMs (`C:\AutoMutate\`).

- **Q: Do I need to create config.yml?**
  - A: No. That was for the deprecated Docker setup. Use `automation/config.yaml` and `automation/templates/*.toml` instead.

---

**Last Updated:** 2025-01-14
**Related Docs:**
- [build/dockerfiles/README.md](build/dockerfiles/README.md) - Docker deprecation notice
- [automation/README.md](automation/README.md) - Production deployment guide
- [.github/workflows/ci.yml](.github/workflows/ci.yml) - CI configuration

