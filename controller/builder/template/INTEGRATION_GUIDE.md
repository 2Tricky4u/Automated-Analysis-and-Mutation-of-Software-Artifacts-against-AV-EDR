# AutoMutate++ Integration Guide: SuperMega Loader

This package provides a **Modular Template System** for the SuperMega shellcode loader. It is designed to be assembled by the AutoMutate++ engine (Rust) or the provided build script.

## 📁 Package Structure

| Path | Purpose |
| :--- | :--- |
| `loader_template.c` | **Harness**. The main entry point that includes other modules in sequence. |
| `modules/` | **Components**. Contains the individual C source files for each technique. |
| `modules/carrier/` | **Execution**. Allocates memory and executes payload. |
| `modules/decoder/` | **Obfuscation**. Decodes the payload before execution. |
| `modules/antiemulation/` | **Evasion**. Burns resources or detects emulators. |
| `modules/guardrails/` | **Targeting**. Checks environment (User/Domain) before running. |
| `modules/virtualprotect/` | **Hook Evasion**. Wrappers for memory protection APIs. |
| `modules/decoy/` | **Distraction**. Launches benign activity. |
| `encoder.py` | **Utility**. Generates standard `payload.h` matching the chosen decoder. |

## 🧩 Available Modules

### 1. Carrier (Memory & Execution)
*   **`modules/carrier/alloc_rw_rx.c`**: Allocates new memory (RW), Decodes, Protects (RX), Executes. Returns `2` if protection fails.
*   **`modules/carrier/change_rw_rx.c`**: Uses *existing* payload memory (e.g. `.data`), sets RW, Decodes, sets RX. Avoids `VirtualAlloc`. Returns `16` if protection fails.
*   **`modules/carrier/peb_walk.c`**: "Import-Free" execution. Dynamically resolves APIs by walking PEB. *Independent of VirtualProtect module*. Returns `2`/`3`/`4` on API resolution failure.

### 2. Decoder (Obfuscation)
*   **`modules/decoder/xor.c`**: Rolling 2-byte XOR key.
*   **`modules/decoder/english.c`**: Dictionary-based "English text" encoding to bypass entropy checks.

### 3. VirtualProtect (Hook Evasion)
*   **`modules/virtualprotect/standard.c`**: Normal `VirtualProtect` wrapper.
*   **`modules/virtualprotect/undersized.c`**: Loops through payload in 4KB pages, asking only for 16-byte changes. Bypasses "Large Block RWX" hooks.

### 4. Decoy (Distraction)
*   **`modules/decoy/none.c`**: Do nothing.
*   **`modules/decoy/winexec.c`**: Launches `notepad.exe` using `WinExec`.

### 5. Anti-Emulation & Guardrails
*   **`modules/antiemulation/sirallocalot.c`**: Triple Loop strategy (Alloc->Touch->Protect->Free). Hard allocations to exhaust legacy emulators.
*   **`modules/antiemulation/timeraw.c`**: Reads `KUSER_SHARED_DATA` (0x7FFE0000) for time, bypassing hooked APIs. Busy waits for 3 seconds.
*   **`modules/guardrails/env.c`**: Checks environment variables (Case-Insensitive Substring). 
    *   **Usage**: Returns failure (Exit 6) if the `ENV_NEEDLE` is **NOT** found in `ENV_KEY`.
    *   **Config**: `-DENV_KEY="USERNAME"` and `-DENV_NEEDLE="Admin"`.

---

## 🚀 Build Instructions

### Manual Build
Use `clang` to compile `loader_template.c` while defining which modules to include.

```bash
# Example: Stealth Build (PEB Walk + English Decoder + Undersized VP)
clang -target x86_64-pc-windows-msvc -o loader.exe loader_template.c \
    -DSELECTED_CARRIER=\"modules/carrier/peb_walk.c\" \
    -DSELECTED_DECODER=\"modules/decoder/english.c\" \
    -DSELECTED_VIRTUALPROTECT=\"modules/virtualprotect/undersized.c\" \
    -DSELECTED_DECOY=\"modules/decoy/none.c\" \
    -DSELECTED_ANTIEMULATION=\"modules/antiemulation/timeraw.c\"
```

### AutoMutate++ Workflow

The engine can generate the final loader in two ways:

#### 1. Template Assembly (Recommended)
The engine simply selects one file from each `modules/` subdirectory and compiles `loader_template.c` with the appropriate `-DSELECTED_*` definitions.

#### 2. Source Injection (AST Mutation)
To fuzz specific modules (e.g., mutate the C code structure):

1.  **Parse**: The fuzzer reads a module file (e.g., `modules/carrier/alloc_rw_rx.c`).
    *   *Note*: Ensure the parser can find `../header/definitions.h` (used for types).
2.  **Mutate**: Apply AST transformations (reorder, insert junk, rename).
3.  **Save**: Write the mutated C code to a temporary file (e.g., `temp/mutated_carrier.c`).
    *   *Tip*: Strip the `#include "../header/definitions.h"` line from the mutated file, as `loader_template.c` already includes it.
4.  **Assemble**: Compile the template pointing to your mutated file.
    ```bash
    clang ... loader_template.c -DSELECTED_CARRIER=\"temp/mutated_carrier.c\"
    ```

## 🧩 Module Interfaces

If you create new custom modules, they must adhere to the signatures in `modules/header/definitions.h`:

*   `int carrier(void)`
*   `void decode_payload(char *dest, int len)`
*   `void antiemulation(void)`
*   `void decoy(void)`
*   `int guardrail(void)`
*   `BOOL MyVirtualProtect(...)`
