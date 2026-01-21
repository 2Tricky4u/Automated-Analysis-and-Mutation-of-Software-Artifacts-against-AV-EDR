/**
 * Minimal Runtime - Clean Exit Mechanism
 *
 * Provides __runtime_exit() for clean process termination via direct syscalls.
 * This runtime is ALWAYS linked (even when trace_mode=off) to ensure
 * artifacts can exit cleanly when RedEDR or other hooks are active.
 *
 * When instrumentation is enabled (ENABLE_INSTRUMENTATION defined):
 * - Flushes all telemetry (coverage, trace, checkpoints) before exit
 * - Calls flush functions from instrumentation_runtime.c
 *
 * When instrumentation is disabled (trace=off):
 * - Just exits cleanly via direct syscall (no telemetry to flush)
 */

#include <windows.h>

// Forward declaration
__attribute__((visibility("default"))) __declspec(noreturn) void __runtime_exit(int exit_code);

// ============================================================================
// Conditional Instrumentation Support
// ============================================================================

// Declare flush functions as WEAK symbols
// When instrumentation_runtime.o is linked (trace != off):
//   - Real implementations are used from instrumentation_runtime.c
// When instrumentation_runtime.o is NOT linked (trace = off):
//   - Weak symbols resolve to NULL, calls are skipped (no-op)
//
// This allows minimal_runtime.o to be ALWAYS linked regardless of trace mode
__attribute__((weak)) extern void __coverage_flush(void);
__attribute__((weak)) extern void __trace_flush(void);
__attribute__((weak)) extern void __checkpoint_flush(void);

// ============================================================================
// Direct Syscall Exit (bypasses all hooks including RedEDR)
// ============================================================================

#if defined(_M_X64) || defined(__x86_64__)

// Dynamically resolved syscall numbers (initialized at runtime)
static DWORD g_syscall_NtTerminateProcess = 0;
static DWORD g_syscall_NtClose = 0;
static int g_syscalls_initialized = 0;

/**
 * Dynamically resolve syscall number from ntdll.dll
 * Parses the syscall stub to extract the syscall number (portable across Windows versions)
 *
 * Syscall stub format on x64 Windows:
 *   4C 8B D1             mov r10, rcx
 *   B8 XX XX XX XX       mov eax, <syscall_number>
 *   0F 05 / F6 04 25...  syscall / test instruction
 *   C3                   ret
 */
static DWORD GetSyscallNumber(const char* function_name) {
    HMODULE ntdll = GetModuleHandleA("ntdll.dll");
    if (!ntdll) return 0;

    FARPROC func = GetProcAddress(ntdll, function_name);
    if (!func) return 0;

    BYTE* code = (BYTE*)func;

    // Check for standard syscall stub pattern
    if (code[0] == 0x4C && code[1] == 0x8B && code[2] == 0xD1 &&  // mov r10, rcx
        code[3] == 0xB8) {                                         // mov eax, imm32
        // Extract syscall number from bytes [4..7] (little-endian)
        return *(DWORD*)(code + 4);
    }

    // Alternative pattern (some Windows versions)
    if (code[0] == 0xB8) {  // mov eax, imm32 (no r10 setup)
        return *(DWORD*)(code + 1);
    }

    return 0;  // Failed to parse
}

/**
 * Initialize syscall numbers (call once at startup)
 */
static void InitSyscalls(void) {
    if (g_syscalls_initialized) return;
    g_syscalls_initialized = 1;

    g_syscall_NtTerminateProcess = GetSyscallNumber("NtTerminateProcess");
    g_syscall_NtClose = GetSyscallNumber("NtClose");

    // Fallback to known values for Windows 10/11 if parsing fails
    if (g_syscall_NtTerminateProcess == 0) g_syscall_NtTerminateProcess = 0x2C;
    if (g_syscall_NtClose == 0) g_syscall_NtClose = 0x0F;
}

/**
 * Generic direct syscall dispatcher (x64 only)
 * Bypasses ntdll.dll hooks by invoking syscall instruction directly
 *
 * @param syscall_number The syscall number (from GetSyscallNumber)
 * @param arg1 First argument (RCX register)
 * @param arg2 Second argument (RDX register)
 * @return NTSTATUS code
 */
__attribute__((naked))
static NTSTATUS DirectSyscall2(DWORD syscall_number, ULONG_PTR arg1, ULONG_PTR arg2) {
    // GCC inline assembly for x64 syscall
    __asm__ volatile (
        "mov %rcx, %r10\n"      // Save RCX to R10 (Windows syscall convention)
        "mov %edx, %eax\n"      // Move syscall number to EAX
        "mov %r8, %rcx\n"       // arg1 -> RCX
        "mov %r9, %rdx\n"       // arg2 -> RDX
        "syscall\n"             // Invoke syscall
        "ret\n"
    );
}

/**
 * Clean exit using direct syscalls (bypasses all hooks)
 * USE THIS INSTEAD OF: return 0, exit(), ExitProcess()
 *
 * Safe for:
 * - Instrumented artifacts (flushes coverage, trace, checkpoints before exit)
 * - Non-instrumented artifacts (just exits cleanly)
 * - RedEDR observation (bypasses hooked exit functions)
 *
 * Flush order (when instrumentation enabled):
 * 1. Coverage bitmap (BB execution map)
 * 2. Trace events (line-level execution)
 * 3. Checkpoints (API call markers)
 *
 * Usage:
 *   #include "instrumentation.h"
 *   int main() {
 *       // ... artifact code ...
 *       __runtime_exit(0);  // Instead of: return 0;
 *   }
 */
__attribute__((visibility("default"))) __declspec(noreturn) void __runtime_exit(int exit_code) {
    // Flush all telemetry before exit (if instrumentation functions are available)
    // Check weak symbols - they are NULL if instrumentation_runtime.o wasn't linked
    if (__coverage_flush != NULL) {
        __coverage_flush();    // Flush BB coverage bitmap
    }
    if (__trace_flush != NULL) {
        __trace_flush();       // Flush line-level trace events
    }
    if (__checkpoint_flush != NULL) {
        __checkpoint_flush();  // Flush checkpoint markers
    }

    // Small delay to ensure file writes complete
    Sleep(50);

    // Initialize syscalls if needed
    if (!g_syscalls_initialized) {
        InitSyscalls();
    }

    // Terminate via direct syscall (bypasses hooked NtTerminateProcess)
    // Use pseudo-handle -1 for current process (no need to open handle)
    HANDLE current_process = (HANDLE)-1;
    DirectSyscall2(g_syscall_NtTerminateProcess, (ULONG_PTR)current_process, (ULONG_PTR)exit_code);

    // Should never reach here - syscall termination failed
    const char* error_msg = "[FATAL] __runtime_exit: Direct syscall termination failed!\r\n";
    DWORD written;

    // Write to stderr (visible in console/logs)
    HANDLE stderr_handle = GetStdHandle(STD_ERROR_HANDLE);
    if (stderr_handle != INVALID_HANDLE_VALUE) {
        WriteFile(stderr_handle, error_msg, 59, &written, NULL);
    }

    // Write to debugger output (visible in debuggers like WinDbg)
    OutputDebugStringA(error_msg);

    // Fallback: use normal exit as last resort
    ExitProcess((UINT)exit_code);
}

#else
// Non-x64 fallback: use normal exit
__attribute__((visibility("default"))) __declspec(noreturn) void __runtime_exit(int exit_code) {
    // Flush all telemetry before exit (if instrumentation functions are available)
    if (__coverage_flush != NULL) {
        __coverage_flush();
    }
    if (__trace_flush != NULL) {
        __trace_flush();
    }
    if (__checkpoint_flush != NULL) {
        __checkpoint_flush();
    }

    Sleep(50);
    ExitProcess((UINT)exit_code);
}
#endif
