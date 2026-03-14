# PE Injection Instrumentation — VEH Shellcode Checkpoints

## Overview

PE-injected artifacts can optionally include a VEH (Vectored Exception Handler)
checkpoint stub that reports shellcode execution progress via named pipe, matching
the telemetry format used by template-built artifacts.

## Architecture

When `sc_checkpoint_count > 0`, the injected section layout changes from:

```
[decode_stub | key | encoded_payload]
```

to:

```
[veh_stub_code | checkpoint_data | decode_stub | key | encoded_payload]
```

### VEH Stub (`veh_checkpoint_stub.c`)

Fully PIC (position-independent code), compiled from C with no CRT dependencies.
All Windows API calls are resolved at runtime via PEB walk.

**Resolved APIs (from kernel32.dll):**
- `CreateFileA` — open checkpoint pipe
- `WriteFile` — write checkpoint JSON
- `VirtualProtect` — restore INT3 bytes
- `AddVectoredExceptionHandler` — install handler
- `RemoveVectoredExceptionHandler` — cleanup (resolved but not called in stub)

**VEH Handler:**
1. Checks `EXCEPTION_BREAKPOINT` with address inside shellcode region
2. Looks up offset in packed checkpoint table
3. Writes JSON to `\\.\pipe\rededr_checkpoints`
4. Restores original byte via VirtualProtect RWX flip
5. Sets RIP to exception address, returns `EXCEPTION_CONTINUE_EXECUTION`

### Checkpoint Data Trailer

```
[u32 checkpoint_count]
[count × {u32 offset, u8 orig_byte}]    5 bytes per entry, packed
[pipe_name\0]                            "\\.\pipe\rededr_checkpoints"
[u32 shellcode_base_rel]                 offset from data_start to decoded payload
```

### Sentinel Patching

The C source contains two sentinel values in inline assembly:
- `0xDEADBEEF` — LEA displacement for locating checkpoint data
- `0xCAFEBABE` — JMP displacement for transferring to decode stub

The Rust assembler (`assemble_instrumented_section()`) scans for these 4-byte
patterns in the compiled `.text` section and patches them with correct
RIP-relative offsets based on the assembled section layout.

## Build Pipeline

```
1. patch_shellcode(raw_payload, count, stub_size=0)
   → PatchedShellcode { bytes_with_int3, table }

2. PayloadEncoder::encode(patched_bytes, encoding)
   → encoded_data + key_bytes

3. compile_veh_stub(xwin_dir, source, cache_dir)
   → (veh_code, VehStubLayout)                     [cached]

4. Build decode stub + patch payload_len + OEP
   → decode_stub (existing XOR/SubByte/None)

5. assemble_instrumented_section(
     veh_code, layout, checkpoint_table,
     decode_stub, key_bytes, encoded_data)
   → complete section data with patched sentinels
```

**Critical ordering:** INT3 patching happens on raw payload BEFORE encoding.
The decode stub decodes in-place, restoring INT3-patched shellcode at the
encoded_payload location.

## Compilation Flags

```
clang -c -O2 -nostdlib -fno-stack-protector -fno-exceptions
      --target=x86_64-pc-windows-msvc -fms-compatibility -fms-extensions
      -fno-builtin -mno-red-zone
```

`-mno-red-zone` is critical — the VEH handler is called from the OS exception
dispatcher and must not use the red zone.

## Graceful Degradation

| Failure | Behavior |
|---------|----------|
| PEB walk fails | Skip VEH install, fall through to decode stub |
| Pipe not available | VEH handler skips WriteFile, execution continues |
| Clang not in PATH | Error: "Failed to invoke clang" — non-instrumented path unaffected |
| Code cave too small | Falls back to NewSection (same as today) |

## JSON Format

```json
{"ts_us":0,"checkpoint":"sc_checkpoint_0","type":"artifact_checkpoint"}
```

Compatible with worker agent checkpoint parsing (`ts_us.as_u64().unwrap_or(0)`).

## Usage

### Rust API

```rust
let input = PeInjectInput {
    payload: shellcode_bytes,
    encoding: EncodingType::Xor,
    sc_checkpoint_count: Some(10),
    xwin_dir: Some(PathBuf::from("/path/to/xwin")),
    // ... other fields
};
let artifact = injector.inject(&input)?;
assert!(artifact.checkpoint_count > 0);
```

### CLI

```
cargo run -p build --example pe_inject -- \
  -p shellcode.bin -t target.exe \
  --checkpoints 10 --xwin-dir /path/to/xwin
```

## Space Overhead

For N checkpoints:

```
overhead = veh_code_size + 4 + (N × 5) + 35 + 4
         ≈ 500-800 bytes for compiled stub
         + 5N bytes for table
         + 39 bytes for pipe name + base offset
```

Typical: ~700 bytes for 10 checkpoints.
