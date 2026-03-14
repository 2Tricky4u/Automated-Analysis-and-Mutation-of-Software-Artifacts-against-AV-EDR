# Zombie ZIP Decoder

## What it is

A payload encoding that wraps shellcode in a **malformed ZIP container**: the header says `method=STORED` (uncompressed), but the file data is actually raw DEFLATE-compressed bytes. The decoder ignores the method field entirely and inflates the data at compile-time-known offsets.

## Why it is interesting as a decoder

### 1. Confuses static analysis tools that trust ZIP metadata

Most ZIP parsers (including those inside AV/EDR scan engines) read the `compression method` field to decide how to extract file contents. When it says `STORED (0)`, they treat the data as raw uncompressed bytes and scan it directly. Because the actual data is DEFLATE-compressed, the scanner reads garbage — it never sees the real payload.

This is the core of the **Zombie ZIP** technique: the container is structurally valid ZIP, but semantically a lie.

### 2. No cryptographic keys in the binary

Unlike XOR or SubByte encoding, there is no key material embedded in the artifact. The payload is compressed, not encrypted. This means:
- No key bytes for signature scanners to anchor on
- No XOR loop pattern in the decoder code
- The decoder is a generic DEFLATE inflater (`tinfl.h` from miniz), which is legitimate code found in countless benign programs

### 3. Compile-time offsets eliminate runtime ZIP parsing

The encoder (Rust, build-time) computes `ZOMBIEZIP_DATA_OFFSET` and `ZOMBIEZIP_COMPRESSED_LEN` and bakes them into `payload.h` as `#define` constants. The C decoder jumps directly to the compressed data — no local-file-header parsing, no filename scanning, no EOCD search. This:
- Reduces decoder code surface (fewer API calls to hook/trace)
- Eliminates string constants like filenames from the decoder path
- Makes the decoder a single `tinfl_decompress_mem_to_mem()` call

### 4. Entropy profile mimics benign compressed data

The DEFLATE stream has high entropy (~7.5-8.0 bits/byte), but this is expected inside a ZIP container. EDR heuristics that flag "high-entropy blob = likely encrypted shellcode" are less likely to trigger when the surrounding structure is a recognizable archive format.

### 5. The full ZIP container is valid enough to pass format checks

The encoder writes all three required ZIP structures (local file header, central directory, EOCD). Tools like `file(1)`, `zipinfo`, or library-level `IsZipFile()` checks will identify the blob as a ZIP archive. Only tools that actually try to decompress using the declared method will discover the mismatch.

## PoC reference

The technique is based on the **head-flip ZIP** / **Zombie ZIP** concept documented in:

- **"Zombie ZIP" / "Head Flipping"** — A file format confusion technique where the compression method field in a ZIP local file header is set to `STORED` (0) while the actual data uses DEFLATE (8). AV scanners that trust the method field extract garbage; custom decoders that know the truth inflate correctly.
- Related prior art: polyglot file attacks, ZIP-based smuggling (e.g., ZIP with mismatched local vs central directory entries), and archive-based evasion discussed in AV bypass research communities.

## Implementation

| Component | File | Role |
|-----------|------|------|
| Encoder (Rust) | `build/src/template/payload.rs` — `encode_zombiezip()` | Builds the malformed ZIP container at build time |
| Decoder (C) | `build/templates/modules/decoder/zombie_zip.c` | Inflates raw DEFLATE at runtime via `tinfl.h` |
| Decompressor | `build/runtime/tinfl.h` | Standalone DEFLATE from miniz, no libc dependency |
| Header | `payload.h` (generated) | `#define` constants: offsets, lengths, byte array |

## Carrier compatibility

Requires carriers that allocate a **separate destination buffer** (e.g., `alloc_rw_rx`, `peb_walk`). In-place carriers like `change_rw_rx` will corrupt data because decompression cannot operate in-place — same constraint as the `english` decoder.
