/**
 * Instrumentation Runtime Library
 *
 * Provides runtime support for:
 * - BB coverage (LLVM SanitizerCoverage + AFL-style edge bitmap)
 * - Line-level tracing (Base64-encoded events to named pipe)
 * - Checkpoint markers (API call tracking)
 *
 * BB Coverage uses LLVM SanitizerCoverage (industry standard):
 * - __sanitizer_cov_trace_pc_guard_init() - Called once to initialize guards
 * - __sanitizer_cov_trace_pc_guard() - Called on every BB entry
 * - Produces AFL-compatible edge coverage bitmap (64KB)
 * - Outputs: coverage.bin (bitmap), coverage_bbs.txt (metadata)
 *
 * Linked into instrumented artifacts at build time
 */

#include <windows.h>
#include <stdio.h>
#include <stdint.h>
#include <limits.h>

// Forward declarations (use default C calling convention for compatibility)
__attribute__((visibility("default"))) void __coverage_init(void);
__attribute__((visibility("default"))) void __coverage_bb(uint32_t bb_id);
__attribute__((visibility("default"))) void __coverage_flush(void);

// SanitizerCoverage callbacks (LLVM standard interface)
// Note: Using trace-pc mode (simple callback) instead of trace-pc-guard (section-based)
// trace-pc is more portable and works better on Windows COFF
__attribute__((visibility("default"))) void __sanitizer_cov_trace_pc(void);
__attribute__((visibility("default"))) void __sanitizer_cov_trace_pc_guard_init(uint32_t *start, uint32_t *stop);
__attribute__((visibility("default"))) void __sanitizer_cov_trace_pc_guard(uint32_t *guard);

__attribute__((visibility("default"))) void __trace_init(const char* pipe_name);
__attribute__((visibility("default"))) void __trace_line(uint32_t seq, const char* file, uint32_t line, const char* func);
__attribute__((visibility("default"))) void __trace_line_b64(const char* base64_marker);
__attribute__((visibility("default"))) void __trace_line_binary(const char* file, uint32_t line, const char* func);
__attribute__((visibility("default"))) void __trace_flush(void);
__attribute__((visibility("default"))) void __checkpoint(const char* name);
__attribute__((visibility("default"))) void __checkpoint_flush(void);
__attribute__((visibility("default"))) void __artifact_checkpoint(const char* checkpoint_name);
__attribute__((visibility("default"))) void __artifact_success(const char* message);
__attribute__((visibility("default"))) void __artifact_failure(const char* message, int error_code);

// Internal functions (granular flushing)
static void __coverage_write_internal(void);
static void __coverage_flush_incremental(void);

// ============================================================================
// BB Coverage (AFL-style bitmap)
// ============================================================================

#define COVERAGE_MAP_SIZE (1 << 16)  // 64KB bitmap
#define MAX_BB_IDS 1024               // Track up to 1024 unique BB IDs

static uint8_t __coverage_map[COVERAGE_MAP_SIZE];
static uint32_t __coverage_prev_bb = 0;
static int __coverage_initialized = 0;
static int __coverage_flushed = 0;

// Track which BB IDs actually exist in the binary and their hit counts
static uint32_t __bb_ids[MAX_BB_IDS];
static uint32_t __bb_hit_counts[MAX_BB_IDS];
static uint32_t __bb_count = 0;

// Granular flush control (for EDR early termination)
static uint32_t __bb_since_last_flush = 0;
#define BB_FLUSH_INTERVAL 1  // Flush coverage every 50 BBs

// SanitizerCoverage guard tracking
static uint32_t *__sancov_guards_start = NULL;
static uint32_t *__sancov_guards_end = NULL;
static uint32_t __total_bbs = 0;

// SanitizerCoverage guard section markers
// LLVM SanitizerCoverage expects these symbols to bound the guard array
// On Windows MSVC, we use sorted sections: .SCOV$GA (start) < .SCOV$GM (middle) < .SCOV$GZ (end)
// The guards themselves are placed in .SCOV$GM by LLVM
#pragma section(".SCOV$GA", read, write)
#pragma section(".SCOV$GZ", read, write)

// Start marker (goes at beginning of .SCOV section)
__declspec(allocate(".SCOV$GA"))
uint32_t __start___sancov_guards_dummy = 0;

// End marker (goes at end of .SCOV section)
__declspec(allocate(".SCOV$GZ"))
uint32_t __stop___sancov_guards_dummy = 0;

// Provide the symbols that LLVM expects (pointers to the markers)
// These need to be extern (non-static) so linker can resolve them
// Use weak linkage to allow override if needed
#ifdef _MSC_VER
__declspec(selectany)
#else
__attribute__((weak))
#endif
uint32_t* __start___sancov_guards = &__start___sancov_guards_dummy;

#ifdef _MSC_VER
__declspec(selectany)
#else
__attribute__((weak))
#endif
uint32_t* __stop___sancov_guards = &__stop___sancov_guards_dummy;

/**
 * Initialize coverage tracking (called automatically via constructor)
 */
void __coverage_init(void) {
    if (__coverage_initialized) return;
    __coverage_initialized = 1;

    // Zero out coverage map
    memset(__coverage_map, 0, COVERAGE_MAP_SIZE);
    __coverage_prev_bb = 0;

    // Register atexit (safe - uses flushed flag to prevent double-flush)
    atexit(__coverage_flush);
}

/**
 * Record BB execution (AFL-style edge coverage)
 * Uses prev_bb XOR curr_bb to track transitions
 */
void __coverage_bb(uint32_t bb_id) {
    // Initialize if needed
    if (!__coverage_initialized) {
        __coverage_init();
    }

    // Track this BB ID and increment its hit count
    int found_idx = -1;
    for (uint32_t i = 0; i < __bb_count; i++) {
        if (__bb_ids[i] == bb_id) {
            found_idx = (int)i;
            break;
        }
    }

    if (found_idx >= 0) {
        // BB already registered, increment hit count
        if (__bb_hit_counts[found_idx] < UINT32_MAX) {
            __bb_hit_counts[found_idx]++;
        }
    } else if (__bb_count < MAX_BB_IDS) {
        // New BB, register it with hit count = 1
        __bb_ids[__bb_count] = bb_id;
        __bb_hit_counts[__bb_count] = 1;
        __bb_count++;
    }

    // AFL-style edge coverage: XOR previous BB with current BB
    uint32_t edge = (__coverage_prev_bb << 1) ^ bb_id;
    uint32_t idx = edge % COVERAGE_MAP_SIZE;

    // Increment hit count (saturating at 255)
    if (__coverage_map[idx] < 255) {
        __coverage_map[idx]++;
    }

    __coverage_prev_bb = bb_id;

    // Incremental flush every N BBs (for EDR early termination)
    __bb_since_last_flush++;
    if (__bb_since_last_flush >= BB_FLUSH_INTERVAL) {
        __coverage_flush_incremental();
    }
}

/**
 * Internal function to write coverage bitmap (called by both incremental and final flush)
 */
static void __coverage_write_internal(void) {
    // Skip if no coverage data yet
    if (!__coverage_initialized) {
        return;
    }

    // Write bitmap to current directory (works in Wine, WSL, and native Windows)
    const char* paths[] = {
        "coverage.bin",          // Current directory (FIRST PRIORITY)
        "C:\\temp\\coverage.bin", // Windows fallback
        "/tmp/coverage.bin"      // Unix fallback for WSL/Wine
    };

    HANDLE hFile = INVALID_HANDLE_VALUE;
    for (int i = 0; i < 3 && hFile == INVALID_HANDLE_VALUE; i++) {
        hFile = CreateFileA(
            paths[i],
            GENERIC_WRITE,
            FILE_SHARE_READ,  // Allow worker to read while we write
            NULL,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            NULL
        );
    }

    if (hFile != INVALID_HANDLE_VALUE) {
        DWORD written;
        WriteFile(hFile, __coverage_map, COVERAGE_MAP_SIZE, &written, NULL);
        CloseHandle(hFile);
    }

    // Save BB metadata (which BBs exist and were hit)
    const char* bb_paths[] = {
        "coverage_bbs.txt",           // Current directory
        "C:\\temp\\coverage_bbs.txt",
        "/tmp/coverage_bbs.txt"
    };

    HANDLE hBBFile = INVALID_HANDLE_VALUE;
    for (int i = 0; i < 3 && hBBFile == INVALID_HANDLE_VALUE; i++) {
        hBBFile = CreateFileA(
            bb_paths[i],
            GENERIC_WRITE,
            FILE_SHARE_READ,  // Allow worker to read while we write
            NULL,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            NULL
        );
    }

    if (hBBFile != INVALID_HANDLE_VALUE) {
        char bb_info[256];
        DWORD written;

        // Write header
        int len = snprintf(bb_info, sizeof(bb_info), "# Basic Block Coverage Report\n");
        WriteFile(hBBFile, bb_info, len, &written, NULL);

        // Use __total_bbs if SanitizerCoverage was used, otherwise __bb_count
        uint32_t total_instrumented = (__total_bbs > 0) ? __total_bbs : __bb_count;
        len = snprintf(bb_info, sizeof(bb_info), "# Total BBs instrumented: %u\n", total_instrumented);
        WriteFile(hBBFile, bb_info, len, &written, NULL);

        len = snprintf(bb_info, sizeof(bb_info), "# Total BBs hit: %u\n", __bb_count);
        WriteFile(hBBFile, bb_info, len, &written, NULL);

        len = snprintf(bb_info, sizeof(bb_info), "# BB_ID HIT_COUNT\n");
        WriteFile(hBBFile, bb_info, len, &written, NULL);

        // Write each BB ID and its hit count
        for (uint32_t i = 0; i < __bb_count; i++) {
            uint32_t bb_id = __bb_ids[i];
            uint32_t hit_count = __bb_hit_counts[i];

            len = snprintf(bb_info, sizeof(bb_info), "%u %u\n", bb_id, hit_count);
            WriteFile(hBBFile, bb_info, len, &written, NULL);
        }

        CloseHandle(hBBFile);
    }
}

/**
 * Flush coverage bitmap to file (final flush at exit)
 */
void __coverage_flush(void) {
    // Prevent double-flush
    if (__coverage_flushed) {
        return;
    }
    __coverage_flushed = 1;

    fprintf(stderr, "[RUNTIME] Final coverage flush\n");
    __coverage_write_internal();
}

/**
 * Incremental coverage flush (called periodically during execution)
 * Can be called multiple times - overwrites file with latest snapshot
 */
static void __coverage_flush_incremental(void) {
    static uint32_t flush_count = 0;
    flush_count++;
    fprintf(stderr, "[RUNTIME] Incremental BB flush #%u (bb_count=%u)\n", flush_count, __bb_count);
    __coverage_write_internal();
    __bb_since_last_flush = 0;
}

// ============================================================================
// SanitizerCoverage Callbacks (LLVM standard interface)
// ============================================================================

// BB counter for trace-pc mode (simple incrementing ID)
static uint32_t __sancov_bb_counter = 0;

/**
 * Simple PC trace callback (called on every BB entry)
 * This is used with -sanitizer-coverage-trace-pc mode
 * More portable than guard-based mode, works on all platforms
 */
void __sanitizer_cov_trace_pc(void) {
    // Initialize coverage if needed
    if (!__coverage_initialized) {
        __coverage_init();
    }

    // Get caller's return address to use as BB ID
    // On x86-64, return address is at [rsp]
    void *return_address = __builtin_return_address(0);
    uint32_t bb_id = (uint32_t)((uintptr_t)return_address & 0xFFFFFFFF);

    // Track this BB
    int found_idx = -1;
    for (uint32_t i = 0; i < __bb_count; i++) {
        if (__bb_ids[i] == bb_id) {
            found_idx = (int)i;
            break;
        }
    }

    if (found_idx >= 0) {
        // BB already registered, increment hit count
        if (__bb_hit_counts[found_idx] < UINT32_MAX) {
            __bb_hit_counts[found_idx]++;
        }
    } else if (__bb_count < MAX_BB_IDS) {
        // New BB, register it
        __bb_ids[__bb_count] = bb_id;
        __bb_hit_counts[__bb_count] = 1;
        __bb_count++;
    }

    // AFL-style edge coverage
    uint32_t edge = (__coverage_prev_bb << 1) ^ bb_id;
    uint32_t idx = edge % COVERAGE_MAP_SIZE;
    if (__coverage_map[idx] < 255) {
        __coverage_map[idx]++;
    }
    __coverage_prev_bb = bb_id;

    // Incremental flush every N BBs (for EDR early termination)
    __bb_since_last_flush++;
    if (__bb_since_last_flush >= BB_FLUSH_INTERVAL) {
        __coverage_flush_incremental();
    }
}

/**
 * Called once at program startup by LLVM SanitizerCoverage (guard mode)
 * Initializes guard array and assigns unique IDs to each guard
 *
 * @param start Pointer to first guard in .data section
 * @param stop Pointer to end of guard array
 */
void __sanitizer_cov_trace_pc_guard_init(uint32_t *start, uint32_t *stop) {
    if (start == stop || *start) {
        return;  // Already initialized or empty range
    }

    __sancov_guards_start = start;
    __sancov_guards_end = stop;
    __total_bbs = (uint32_t)(stop - start);

    fprintf(stderr, "[RUNTIME] SanitizerCoverage initialized: %u basic blocks\n", __total_bbs);

    // Initialize coverage tracking
    if (!__coverage_initialized) {
        __coverage_init();
    }

    // Assign unique IDs to each guard (1-based, 0 means "already visited")
    uint32_t id = 1;
    for (uint32_t *guard = start; guard < stop; guard++) {
        *guard = id++;
    }

    fprintf(stderr, "[RUNTIME] Assigned IDs 1 to %u\n", __total_bbs);
}

/**
 * Called on every instrumented basic block entry by LLVM SanitizerCoverage
 * Records BB hit and updates AFL-style edge coverage map
 *
 * @param guard Pointer to this BB's guard value (contains unique BB ID)
 */
void __sanitizer_cov_trace_pc_guard(uint32_t *guard) {
    if (!guard || *guard == 0) {
        return;  // Invalid guard or already visited (if single-visit mode enabled)
    }

    uint32_t bb_id = *guard;

    // Initialize coverage if needed (shouldn't happen, but defensive)
    if (!__coverage_initialized) {
        __coverage_init();
    }

    // Track this BB ID and increment its hit count
    int found_idx = -1;
    for (uint32_t i = 0; i < __bb_count; i++) {
        if (__bb_ids[i] == bb_id) {
            found_idx = (int)i;
            break;
        }
    }

    if (found_idx >= 0) {
        // BB already registered, increment hit count
        if (__bb_hit_counts[found_idx] < UINT32_MAX) {
            __bb_hit_counts[found_idx]++;
        }
    } else if (__bb_count < MAX_BB_IDS) {
        // New BB, register it with hit count = 1
        __bb_ids[__bb_count] = bb_id;
        __bb_hit_counts[__bb_count] = 1;
        __bb_count++;
    }

    // AFL-style edge coverage: XOR previous BB with current BB
    uint32_t edge = (__coverage_prev_bb << 1) ^ bb_id;
    uint32_t idx = edge % COVERAGE_MAP_SIZE;

    // Increment hit count (saturating at 255)
    if (__coverage_map[idx] < 255) {
        __coverage_map[idx]++;
    }

    __coverage_prev_bb = bb_id;

    // Incremental flush every N BBs (for EDR early termination)
    __bb_since_last_flush++;
    if (__bb_since_last_flush >= BB_FLUSH_INTERVAL) {
        __coverage_flush_incremental();
    }

    // Optional: Enable single-visit mode by uncommenting:
    // *guard = 0;  // Mark as visited (won't call again for this BB)
}

// ============================================================================
// Line-level Tracing (Base64 to named pipe)
// ============================================================================

static HANDLE __trace_pipe = INVALID_HANDLE_VALUE;
static char __trace_buffer[65536];  // 64KB buffer (increased from 4KB for loops)
static int __trace_buffer_pos = 0;

/**
 * Initialize line tracing (connect to named pipe)
 * Pipe name: \\.\pipe\rededr_trace
 */
void __trace_init(const char* pipe_name) {
    // Already initialized? Skip
    if (__trace_pipe != INVALID_HANDLE_VALUE) {
        return;
    }

    if (pipe_name == NULL) {
        pipe_name = "\\\\.\\pipe\\rededr_trace";
    }

    __trace_pipe = CreateFileA(
        pipe_name,
        GENERIC_WRITE,
        0,
        NULL,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        NULL
    );

    // If pipe doesn't exist, fallback to file (try current directory first)
    if (__trace_pipe == INVALID_HANDLE_VALUE) {
        const char* paths[] = {
            "trace.log",              // Current directory (FIRST PRIORITY)
            "C:\\temp\\trace.log",    // Windows fallback
            "/tmp/trace.log"          // Unix fallback
        };

        for (int i = 0; i < 3 && __trace_pipe == INVALID_HANDLE_VALUE; i++) {
            __trace_pipe = CreateFileA(
                paths[i],
                GENERIC_WRITE,
                FILE_SHARE_READ,
                NULL,
                CREATE_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                NULL
            );
        }
    }

    __trace_buffer_pos = 0;

    // Register flush at exit
    if (__trace_pipe != INVALID_HANDLE_VALUE) {
        atexit(__trace_flush);
    }
}

/**
 * Record line execution with source-level metadata (from __FILE__, __LINE__, __func__)
 * Format: {"seq":N,"file":"path","line":N,"func":"name"}
 * Note: file and func are plain strings, not Base64 (since we're using preprocessor macros)
 */
void __trace_line(uint32_t seq, const char* file, uint32_t line, const char* func) {
    if (__trace_pipe == INVALID_HANDLE_VALUE) {
        __trace_init(NULL);
    }

    // Format JSON line (plain strings from preprocessor)
    int len = snprintf(
        __trace_buffer + __trace_buffer_pos,
        sizeof(__trace_buffer) - __trace_buffer_pos,
        "{\"seq\":%u,\"file\":\"%s\",\"line\":%u,\"func\":\"%s\"}\n",
        seq, file, line, func
    );

    __trace_buffer_pos += len;

    // AGGRESSIVE FLUSH: Flush immediately for EDR early termination
    __trace_flush();
}

/**
 * Record AST-level line trace with Base64 marker (Lepori format)
 * Format: "YjY0<base64_encoded_metadata>\n"
 * Where metadata = "line:<filepath>:<line_number>:<extra>"
 */
void __trace_line_b64(const char* base64_marker) {
    if (__trace_pipe == INVALID_HANDLE_VALUE) {
        __trace_init(NULL);
    }

    // Write Base64 marker directly to buffer
    int len = snprintf(
        __trace_buffer + __trace_buffer_pos,
        sizeof(__trace_buffer) - __trace_buffer_pos,
        "%s\n",
        base64_marker
    );

    __trace_buffer_pos += len;

    // AGGRESSIVE FLUSH: Flush immediately for EDR early termination
    __trace_flush();
}

// ============================================================================
// Binary Protocol for Line Tracing (Phase 1: coexist with Base64)
// ============================================================================

#pragma pack(push, 1)
typedef struct {
    uint32_t magic;       // 0x49535452 ('ISTR')
    uint16_t version;     // 1
    uint16_t event_type;  // 1=line, 2=func_enter, 3=syscall, 4=bb
    uint32_t thread_id;   // GetCurrentThreadId()
    uint64_t seq_no;      // Monotonic sequence counter
    uint64_t ts_us;       // Microseconds since process start
    uint32_t payload_len; // Size of following payload
} InstRecordHeader;
#pragma pack(pop)

// Global sequence counter (thread-safe increment via InterlockedIncrement64)
static volatile LONG64 __binary_seq_counter = 0;

// Timestamp tracking (initialized on first use)
static LARGE_INTEGER __perf_freq = {0};
static LARGE_INTEGER __perf_start = {0};
static int __timestamp_initialized = 0;

static void __init_timestamp(void) {
    if (!__timestamp_initialized) {
        QueryPerformanceFrequency(&__perf_freq);
        QueryPerformanceCounter(&__perf_start);
        __timestamp_initialized = 1;
    }
}

static uint64_t __get_timestamp_us(void) {
    if (!__timestamp_initialized) {
        __init_timestamp();
    }
    LARGE_INTEGER now;
    QueryPerformanceCounter(&now);
    uint64_t elapsed = now.QuadPart - __perf_start.QuadPart;
    return (elapsed * 1000000ULL) / __perf_freq.QuadPart;
}

/**
 * Binary protocol line trace (Phase 1: inline strings in payload)
 * Format: InstRecordHeader + "file:line:func" as UTF-8 string
 *
 * This is the new default for AST instrumentation.
 * Coexists with Base64 format for backward compatibility.
 */
void __trace_line_binary(const char* file, uint32_t line, const char* func) {
    if (__trace_pipe == INVALID_HANDLE_VALUE) {
        __trace_init(NULL);
    }

    // Build payload: "file:line:func"
    char payload[512];
    int payload_len = snprintf(payload, sizeof(payload), "%s:%u:%s", file, line, func);
    if (payload_len < 0 || payload_len >= (int)sizeof(payload)) {
        payload_len = sizeof(payload) - 1;  // Truncate if too long
    }

    // Build header
    InstRecordHeader hdr;
    hdr.magic = 0x49535452;  // 'ISTR'
    hdr.version = 1;
    hdr.event_type = 1;      // LINE_TRACE
    hdr.thread_id = GetCurrentThreadId();
    hdr.seq_no = (uint64_t)InterlockedIncrement64(&__binary_seq_counter);
    hdr.ts_us = __get_timestamp_us();
    hdr.payload_len = (uint32_t)payload_len;

    // Check if buffer has space for header + payload
    size_t total_size = sizeof(InstRecordHeader) + payload_len;
    if (__trace_buffer_pos + total_size > sizeof(__trace_buffer)) {
        __trace_flush();
    }

    // Write header to buffer
    memcpy(__trace_buffer + __trace_buffer_pos, &hdr, sizeof(hdr));
    __trace_buffer_pos += sizeof(hdr);

    // Write payload to buffer
    memcpy(__trace_buffer + __trace_buffer_pos, payload, payload_len);
    __trace_buffer_pos += payload_len;

    // AGGRESSIVE FLUSH: Flush after EVERY trace event for EDR early termination
    // This ensures we capture the exact line where EDR kills the process
    // Trade-off: Higher overhead, but critical for death-bed telemetry
    __trace_flush();
}

/**
 * Flush trace buffer to pipe/file
 */
void __trace_flush(void) {
    if (__trace_pipe != INVALID_HANDLE_VALUE && __trace_buffer_pos > 0) {
        DWORD written;
        WriteFile(__trace_pipe, __trace_buffer, __trace_buffer_pos, &written, NULL);
        __trace_buffer_pos = 0;

        FlushFileBuffers(__trace_pipe);

    }
}

// ============================================================================
// Checkpoint Markers (API call tracking)
// ============================================================================

static HANDLE __checkpoint_pipe = INVALID_HANDLE_VALUE;
static int __checkpoint_initialized = 0;

/**
 * Flush and close checkpoint stream
 */
void __checkpoint_flush(void) {
    if (__checkpoint_pipe != INVALID_HANDLE_VALUE) {

        FlushFileBuffers(__checkpoint_pipe);

        CloseHandle(__checkpoint_pipe);
        __checkpoint_pipe = INVALID_HANDLE_VALUE;
    }
}

/**
 * Initialize checkpoint tracking
 */
static void __checkpoint_init(void) {
    if (__checkpoint_initialized) return;
    __checkpoint_initialized = 1;

    __checkpoint_pipe = CreateFileA(
        "\\\\.\\pipe\\rededr_checkpoints",
        GENERIC_WRITE,
        0,
        NULL,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        NULL
    );

    // Fallback to file if pipe unavailable (try multiple paths)
    if (__checkpoint_pipe == INVALID_HANDLE_VALUE) {
        const char* paths[] = {
            "checkpoints.log",           // Current directory (FIRST PRIORITY)
            "C:\\temp\\checkpoints.log",
            "/tmp/checkpoints.log"       // Unix fallback
        };

        for (int i = 0; i < 3 && __checkpoint_pipe == INVALID_HANDLE_VALUE; i++) {
            __checkpoint_pipe = CreateFileA(
                paths[i],
                GENERIC_WRITE | FILE_APPEND_DATA,
                FILE_SHARE_READ,
                NULL,
                OPEN_ALWAYS,
                FILE_ATTRIBUTE_NORMAL,
                NULL
            );
        }
    }

    // Register flush at exit
    atexit(__checkpoint_flush);
}

/**
 * Record checkpoint (e.g., API call about to execute)
 * Format: {"ts_us":N,"checkpoint":"name"}
 */
void __checkpoint(const char* name) {
    if (!__checkpoint_initialized) {
        __checkpoint_init();
    }

    if (__checkpoint_pipe != INVALID_HANDLE_VALUE) {
        // Get timestamp (microseconds since process start)
        LARGE_INTEGER freq, counter;
        QueryPerformanceFrequency(&freq);
        QueryPerformanceCounter(&counter);
        uint64_t ts_us = (counter.QuadPart * 1000000) / freq.QuadPart;

        char buffer[512];
        int len = snprintf(buffer, sizeof(buffer), "{\"ts_us\":%llu,\"checkpoint\":\"%s\"}\n", ts_us, name);

        DWORD written;
        WriteFile(__checkpoint_pipe, buffer, len, &written, NULL);
    }
}

// ============================================================================
// Auto-initialization and cleanup (called by CRT)
// ============================================================================

#ifdef _WIN32
// Windows: Use DllMain for auto-init/cleanup
BOOL WINAPI DllMain(HINSTANCE hinstDLL, DWORD fdwReason, LPVOID lpvReserved) {
    (void)hinstDLL;
    (void)lpvReserved;

    switch (fdwReason) {
        case DLL_PROCESS_ATTACH:
            // Don't initialize - let lazy init happen on first call
            break;

        case DLL_PROCESS_DETACH:
            // Only flush if initialized
            if (__coverage_initialized) {
                __coverage_flush();
            }
            __trace_flush();
            if (__trace_pipe != INVALID_HANDLE_VALUE) {
                CloseHandle(__trace_pipe);
            }
            if (__checkpoint_pipe != INVALID_HANDLE_VALUE) {
                CloseHandle(__checkpoint_pipe);
            }
            break;
    }
    return TRUE;
}

// For EXE: Don't use constructor/destructor - they don't work reliably in Wine
// We use atexit() instead which is called from __coverage_init() and __checkpoint_init()

// ============================================================================
// Artifact Status API (user-callable from C code)
// ============================================================================
// Event types: 1=line, 2=checkpoint, 3=success, 4=failure

/**
 * Signal artifact reached a checkpoint
 * Usage: __artifact_checkpoint("setup_complete");
 */
void __artifact_checkpoint(const char* checkpoint_name) {
    if (!checkpoint_name) return;

    __trace_init(NULL);  // Ensure pipe is connected

    // Build payload
    char payload[256];
    int payload_len = snprintf(payload, sizeof(payload), "%s", checkpoint_name);
    if (payload_len < 0 || payload_len >= (int)sizeof(payload)) {
        payload_len = sizeof(payload) - 1;
    }

    // Build header (event_type=2 for CHECKPOINT)
    InstRecordHeader hdr;
    hdr.magic = 0x49535452;  // 'ISTR'
    hdr.version = 1;
    hdr.event_type = 2;      // CHECKPOINT
    hdr.thread_id = GetCurrentThreadId();
    hdr.seq_no = (uint64_t)InterlockedIncrement64(&__binary_seq_counter);
    hdr.ts_us = __get_timestamp_us();
    hdr.payload_len = (uint32_t)payload_len;

    // Check buffer space
    size_t total_size = sizeof(InstRecordHeader) + payload_len;
    if (__trace_buffer_pos + total_size > sizeof(__trace_buffer)) {
        __trace_flush();
    }

    // Write to buffer
    memcpy(__trace_buffer + __trace_buffer_pos, &hdr, sizeof(hdr));
    __trace_buffer_pos += sizeof(hdr);
    memcpy(__trace_buffer + __trace_buffer_pos, payload, payload_len);
    __trace_buffer_pos += payload_len;

    // Immediate flush for important status events
    __trace_flush();
}

/**
 * Signal artifact completed successfully
 * Usage: __artifact_success("All stages completed");
 */
void __artifact_success(const char* message) {
    if (!message) message = "success";

    __trace_init(NULL);

    // Build payload
    char payload[512];
    int payload_len = snprintf(payload, sizeof(payload), "%s", message);
    if (payload_len < 0 || payload_len >= (int)sizeof(payload)) {
        payload_len = sizeof(payload) - 1;
    }

    // Build header (event_type=3 for SUCCESS)
    InstRecordHeader hdr;
    hdr.magic = 0x49535452;
    hdr.version = 1;
    hdr.event_type = 3;      // SUCCESS
    hdr.thread_id = GetCurrentThreadId();
    hdr.seq_no = (uint64_t)InterlockedIncrement64(&__binary_seq_counter);
    hdr.ts_us = __get_timestamp_us();
    hdr.payload_len = (uint32_t)payload_len;

    size_t total_size = sizeof(InstRecordHeader) + payload_len;
    if (__trace_buffer_pos + total_size > sizeof(__trace_buffer)) {
        __trace_flush();
    }

    memcpy(__trace_buffer + __trace_buffer_pos, &hdr, sizeof(hdr));
    __trace_buffer_pos += sizeof(hdr);
    memcpy(__trace_buffer + __trace_buffer_pos, payload, payload_len);
    __trace_buffer_pos += payload_len;

    __trace_flush();
}

/**
 * Signal artifact failed
 * Usage: __artifact_failure("Memory allocation failed", 1);
 */
void __artifact_failure(const char* message, int error_code) {
    if (!message) message = "failure";

    __trace_init(NULL);

    // Build payload with error code
    char payload[512];
    int payload_len = snprintf(payload, sizeof(payload), "%s|%d", message, error_code);
    if (payload_len < 0 || payload_len >= (int)sizeof(payload)) {
        payload_len = sizeof(payload) - 1;
    }

    // Build header (event_type=4 for FAILURE)
    InstRecordHeader hdr;
    hdr.magic = 0x49535452;
    hdr.version = 1;
    hdr.event_type = 4;      // FAILURE
    hdr.thread_id = GetCurrentThreadId();
    hdr.seq_no = (uint64_t)InterlockedIncrement64(&__binary_seq_counter);
    hdr.ts_us = __get_timestamp_us();
    hdr.payload_len = (uint32_t)payload_len;

    size_t total_size = sizeof(InstRecordHeader) + payload_len;
    if (__trace_buffer_pos + total_size > sizeof(__trace_buffer)) {
        __trace_flush();
    }

    memcpy(__trace_buffer + __trace_buffer_pos, &hdr, sizeof(hdr));
    __trace_buffer_pos += sizeof(hdr);
    memcpy(__trace_buffer + __trace_buffer_pos, payload, payload_len);
    __trace_buffer_pos += payload_len;

    __trace_flush();
}

#endif
