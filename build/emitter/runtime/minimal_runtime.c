/**
 * Minimal Runtime - Clean Exit Mechanism
 *
 * Provides __runtime_exit() for clean process termination via direct syscalls.
 * This runtime is ALWAYS linked (even when trace_mode=off) to ensure
 * artifacts can exit cleanly when RedEDR or other hooks are active.
 *
 * Standalone - no dependencies on instrumentation_runtime.c
 */

#include <windows.h>

// Forward declaration
__attribute__((visibility("default"))) __declspec(noreturn) void __runtime_exit(int exit_code);

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
 * - Instrumented artifacts (runtime flushes happen before this is called)
 * - Non-instrumented artifacts (just exits cleanly)
 * - RedEDR observation (bypasses hooked exit functions)
 *
 * Usage:
 *   #include "instrumentation.h"
 *   int main() {
 *       // ... artifact code ...
 *       __runtime_exit(0);  // Instead of: return 0;
 *   }
 */
__attribute__((visibility("default"))) __declspec(noreturn) void __runtime_exit(int exit_code) {
    // Small delay to let any pending I/O flush (if instrumentation was used)
    Sleep(50);

    // Initialize syscalls if needed
    if (!g_syscalls_initialized) {
        InitSyscalls();
    }

    // Terminate via direct syscall (bypasses hooked NtTerminateProcess)
    // Use pseudo-handle -1 for current process (no need to open handle)
    HANDLE current_process = (HANDLE)-1;
    DirectSyscall2(g_syscall_NtTerminateProcess, (ULONG_PTR)current_process, (ULONG_PTR)exit_code);

    // Should never reach here, but loop forever if syscall fails
    while(1) {
        Sleep(1000);
    }
}

#else
// Non-x64 fallback: use normal exit
__attribute__((visibility("default"))) __declspec(noreturn) void __runtime_exit(int exit_code) {
    Sleep(50);
    ExitProcess((UINT)exit_code);
}
#endif
