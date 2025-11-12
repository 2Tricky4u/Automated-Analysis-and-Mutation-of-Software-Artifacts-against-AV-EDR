/**
 * Simple Loader Template v1
 *
 * Purpose: Minimal test artifact for build/emitter pipeline testing
 * Behaviors: Dynamic API resolution, memory allocation, basic execution
 *
 * Build: make
 * Usage: ./loader.exe
 */

#include <windows.h>
#include <stdio.h>

// Function pointer types for dynamic resolution
typedef LPVOID (WINAPI *VirtualAllocFunc)(LPVOID, SIZE_T, DWORD, DWORD);
typedef BOOL (WINAPI *VirtualFreeFunc)(LPVOID, SIZE_T, DWORD);
typedef HANDLE (WINAPI *CreateThreadFunc)(LPSECURITY_ATTRIBUTES, SIZE_T, LPTHREAD_START_ROUTINE, LPVOID, DWORD, LPDWORD);
typedef DWORD (WINAPI *WaitForSingleObjectFunc)(HANDLE, DWORD);

// Simple position-independent shellcode: prints message via MessageBoxA
// x64 shellcode that calls MessageBoxA (requires kernel32.dll at known location)
// For demo purposes, we'll use a simpler approach: just return success
unsigned char shellcode[] = {
    // mov rax, 0x1234567812345678  ; placeholder for function address
    0x48, 0xB8, 0x78, 0x56, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12,
    // call rax
    0xFF, 0xD0,
    // ret
    0xC3
};

// This function will be called from the shellcode
DWORD WINAPI PayloadFunction(LPVOID param) {
    printf("===========================================\n");
    printf("[PAYLOAD] Hello from injected code!\n");
    printf("[PAYLOAD] This is executing in RX memory\n");
    printf("[PAYLOAD] Address: %p\n", PayloadFunction);
    printf("[PAYLOAD] Press any key to continue...\n");
    printf("===========================================\n");
    getchar();
    return 0;
}

int main(int argc, char *argv[]) {
    printf("[Loader] Starting (PID: %d)\n", GetCurrentProcessId());

    // === Phase 1: Dynamic API Resolution ===
    printf("[Loader] Resolving APIs dynamically...\n");

    HMODULE hKernel32 = GetModuleHandleA("kernel32.dll");
    if (!hKernel32) {
        fprintf(stderr, "[ERROR] Failed to get kernel32.dll handle\n");
        return 1;
    }

    VirtualAllocFunc pVirtualAlloc = (VirtualAllocFunc)GetProcAddress(hKernel32, "VirtualAlloc");
    VirtualFreeFunc pVirtualFree = (VirtualFreeFunc)GetProcAddress(hKernel32, "VirtualFree");
    CreateThreadFunc pCreateThread = (CreateThreadFunc)GetProcAddress(hKernel32, "CreateThread");
    WaitForSingleObjectFunc pWaitForSingleObject = (WaitForSingleObjectFunc)GetProcAddress(hKernel32, "WaitForSingleObject");

    if (!pVirtualAlloc || !pVirtualFree || !pCreateThread || !pWaitForSingleObject) {
        fprintf(stderr, "[ERROR] Failed to resolve APIs\n");
        return 1;
    }

    printf("[Loader] APIs resolved successfully\n");

    // === Phase 2: Patch shellcode with PayloadFunction address ===
    printf("[Loader] Payload function at %p\n", PayloadFunction);

    // Patch the shellcode with the actual function address
    *(UINT64*)(shellcode + 2) = (UINT64)PayloadFunction;
    printf("[Loader] Shellcode patched with function address\n");

    // === Phase 3: Allocate RW Memory ===
    SIZE_T shellcodeSize = sizeof(shellcode);
    printf("[Loader] Allocating RW memory (%zu bytes)...\n", shellcodeSize);

    LPVOID mem = pVirtualAlloc(NULL, shellcodeSize, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE);
    if (!mem) {
        fprintf(stderr, "[ERROR] VirtualAlloc failed (RW)\n");
        return 1;
    }

    printf("[Loader] Allocated at %p\n", mem);

    // === Phase 4: Copy shellcode ===
    printf("[Loader] Copying shellcode...\n");
    memcpy(mem, shellcode, shellcodeSize);
    printf("[Loader] Shellcode copied\n");

    // === Phase 5: Change to RX Protection ===
    printf("[Loader] Changing protection to RX...\n");

    DWORD oldProtect;
    if (!VirtualProtect(mem, shellcodeSize, PAGE_EXECUTE_READ, &oldProtect)) {
        fprintf(stderr, "[ERROR] VirtualProtect failed\n");
        pVirtualFree(mem, 0, MEM_RELEASE);
        return 1;
    }

    printf("[Loader] Protection changed (old: 0x%lx)\n", (unsigned long)oldProtect);

    // === Phase 6: Create Execution Thread ===
    printf("[Loader] Creating execution thread...\n");
    printf("[Loader] Thread will execute shellcode at %p, which calls PayloadFunction\n", mem);

    HANDLE hThread = pCreateThread(NULL, 0, (LPTHREAD_START_ROUTINE)mem, NULL, 0, NULL);
    if (!hThread) {
        fprintf(stderr, "[ERROR] CreateThread failed\n");
        pVirtualFree(mem, 0, MEM_RELEASE);
        return 1;
    }

    printf("[Loader] Thread created (handle: %p)\n", hThread);

    // === Phase 7: Wait for Completion ===
    printf("[Loader] Waiting for thread completion...\n");
    pWaitForSingleObject(hThread, INFINITE);
    printf("[Loader] Thread completed\n");

    // === Phase 8: Cleanup ===
    printf("[Loader] Cleaning up...\n");
    CloseHandle(hThread);
    pVirtualFree(mem, 0, MEM_RELEASE);

    printf("[Loader] Done! Press any key to exit...\n");
    getchar();
    return 0;
}
