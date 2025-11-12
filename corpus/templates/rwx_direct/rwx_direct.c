/**
 * RWX Direct Allocation Template
 *
 * Suspicious Pattern: Allocates RWX memory directly (not RW→RX)
 * EDR Trigger: Single-step RWX allocation with immediate execution
 * Benign Effect: Mimics malware reconnaissance but only reads/prints info
 *
 * Detection likelihood: HIGH (malware API sequence)
 */

#include <windows.h>
#include <tlhelp32.h>
#include <wininet.h>
#include <stdio.h>

#pragma comment(lib, "wininet.lib")

// Shellcode that calls a realistic malware-like payload
unsigned char shellcode[] = {
    // mov rax, 0x1234567812345678  (function address, patched at runtime)
    0x48, 0xB8, 0x78, 0x56, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12,
    // call rax
    0xFF, 0xD0,
    // ret
    0xC3
};

// Realistic malware-like payload (but benign)
DWORD WINAPI MalwareLikePayload(LPVOID param) {
    printf("\n[PAYLOAD] Executing from RWX memory...\n");

    // === 1. Process enumeration (recon) ===
    printf("[MALWARE-LIKE] Enumerating processes...\n");
    HANDLE hSnapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (hSnapshot != INVALID_HANDLE_VALUE) {
        PROCESSENTRY32 pe32;
        pe32.dwSize = sizeof(PROCESSENTRY32);

        int count = 0;
        if (Process32First(hSnapshot, &pe32)) {
            do {
                count++;
                // Real malware would search for AV/EDR processes here
            } while (Process32Next(hSnapshot, &pe32) && count < 10);
        }
        CloseHandle(hSnapshot);
        printf("[MALWARE-LIKE] Found %d processes (malware checks for AV here)\n", count);
    }

    // === 2. Allocate more suspicious memory ===
    printf("[MALWARE-LIKE] Allocating additional RWX memory (stager pattern)...\n");
    LPVOID stage2 = VirtualAlloc(NULL, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
    if (stage2) {
        printf("[MALWARE-LIKE] Stage 2 memory at: %p\n", stage2);
        // Write NOPs (typical shellcode pattern)
        memset(stage2, 0x90, 4096);  // Fill with NOPs
        VirtualFree(stage2, 0, MEM_RELEASE);
    }

    // === 3. Query system info (fingerprinting) ===
    printf("[MALWARE-LIKE] Fingerprinting system...\n");
    SYSTEM_INFO sysInfo;
    GetSystemInfo(&sysInfo);
    printf("[MALWARE-LIKE] Processors: %lu (malware checks for sandboxes)\n",
           sysInfo.dwNumberOfProcessors);

    // === 4. Check for common sandbox/VM artifacts ===
    printf("[MALWARE-LIKE] Anti-VM checks...\n");

    // Check for low memory (sandboxes often have <2GB)
    MEMORYSTATUSEX memStatus;
    memStatus.dwLength = sizeof(memStatus);
    if (GlobalMemoryStatusEx(&memStatus)) {
        DWORD totalGB = (DWORD)(memStatus.ullTotalPhys / (1024 * 1024 * 1024));
        printf("[MALWARE-LIKE] Total RAM: %lu GB ", totalGB);
        if (totalGB < 2) {
            printf("(SANDBOX DETECTED! Malware would exit)\n");
        } else {
            printf("(Real system)\n");
        }
    }

    // === 5. Registry enumeration (typical malware) ===
    printf("[MALWARE-LIKE] Checking registry for product info...\n");
    HKEY hKey;
    if (RegOpenKeyExA(HKEY_LOCAL_MACHINE,
                      "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
                      0, KEY_READ, &hKey) == ERROR_SUCCESS) {
        char productName[256];
        DWORD size = sizeof(productName);
        if (RegQueryValueExA(hKey, "ProductName", NULL, NULL,
                            (LPBYTE)productName, &size) == ERROR_SUCCESS) {
            printf("[MALWARE-LIKE] OS: %s (malware logs this to C2)\n", productName);
        }
        RegCloseKey(hKey);
    }

    // === 6. File system enumeration ===
    printf("[MALWARE-LIKE] Checking for user files...\n");
    char userProfile[MAX_PATH];
    if (GetEnvironmentVariableA("USERPROFILE", userProfile, MAX_PATH) > 0) {
        printf("[MALWARE-LIKE] User profile: %s (ransomware targets here)\n", userProfile);
    }

    // === 7. Network check (C2 preparation) ===
    printf("[MALWARE-LIKE] Checking internet connectivity...\n");
    if (InternetCheckConnectionA("http://www.microsoft.com", FLAG_ICC_FORCE_CONNECTION, 0)) {
        printf("[MALWARE-LIKE] Internet AVAILABLE (malware would beacon C2 now)\n");
    } else {
        printf("[MALWARE-LIKE] No internet (malware would wait or use offline mode)\n");
    }

    printf("\n[PAYLOAD] Malware-like recon complete!\n");
    printf("[INFO] In real malware, this data goes to C2 server\n");
    printf("[INFO] Next step would be: download stage2, inject, persist\n");
    printf("\nPress Enter to continue...\n");
    getchar();

    return 0;
}

int main(void) {
    printf("[RWX_DIRECT] Starting suspicious pattern test\n");
    printf("[RWX_DIRECT] PID: %lu\n", GetCurrentProcessId());
    printf("\n");

    // === ADDITIONAL SUSPICION: Disable error dialogs (malware technique) ===
    SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    printf("[SUSPICIOUS] Disabled crash dialogs (malware anti-analysis)\n");

    // === ADDITIONAL SUSPICION: Check for debugger ===
    if (IsDebuggerPresent()) {
        printf("[SUSPICIOUS] Debugger detected - malware would exit here\n");
    }

    // === SUSPICIOUS PATTERN: Direct RWX allocation ===
    printf("[SUSPICIOUS] Allocating memory with RWX protection directly...\n");

    // Allocate MUCH larger region (malware often allocates big chunks)
    SIZE_T alloc_size = 1024 * 1024;  // 1MB (typical for shellcode loaders)

    LPVOID mem = VirtualAlloc(
        NULL,
        alloc_size,
        MEM_COMMIT | MEM_RESERVE,
        PAGE_EXECUTE_READWRITE  // ← RED FLAG: Direct RWX
    );

    if (!mem) {
        fprintf(stderr, "[ERROR] VirtualAlloc failed: %lu\n", GetLastError());
        return 1;
    }

    printf("[SUSPICIOUS] Allocated RWX memory at: %p (size: %zu bytes)\n", mem, alloc_size);
    printf("[INFO] 1MB RWX allocation is CLASSIC shellcode loader behavior\n");
    printf("\n");

    // === ADDITIONAL SUSPICION: Zero memory (anti-analysis) ===
    printf("[SUSPICIOUS] Zeroing memory (malware does this to avoid traces)...\n");
    memset(mem, 0, alloc_size);

    // Patch shellcode with function address
    *(UINT64*)(shellcode + 2) = (UINT64)MalwareLikePayload;

    // Write shellcode to the START of RWX memory
    printf("[SUSPICIOUS] Writing shellcode to RWX memory...\n");
    memcpy(mem, shellcode, sizeof(shellcode));

    // === ADDITIONAL SUSPICION: Flush instruction cache (x64 shellcode pattern) ===
    FlushInstructionCache(GetCurrentProcess(), mem, sizeof(shellcode));
    printf("[SUSPICIOUS] Flushed instruction cache (shellcode loader pattern)\n");

    // === ADDITIONAL SUSPICION: Very short time window ===
    printf("[SUSPICIOUS] Creating thread IMMEDIATELY in RWX memory...\n");
    printf("[INFO] Time window < 100ms is CLASSIC malware pattern\n");
    printf("\n");

    HANDLE hThread = CreateThread(NULL, 0, (LPTHREAD_START_ROUTINE)mem, NULL, 0, NULL);
    if (!hThread) {
        fprintf(stderr, "[ERROR] CreateThread failed: %lu\n", GetLastError());
        VirtualFree(mem, 0, MEM_RELEASE);
        return 1;
    }

    printf("[SUSPICIOUS] Thread created in anonymous RWX region!\n");
    printf("[INFO] Thread start address NOT in .text section = RED FLAG\n");
    printf("\n");

    // Wait for completion
    WaitForSingleObject(hThread, INFINITE);

    printf("\n[INFO] Thread completed successfully\n");
    printf("[INFO] Cleaning up...\n");

    CloseHandle(hThread);
    VirtualFree(mem, 0, MEM_RELEASE);

    printf("[RWX_DIRECT] Test complete - press Enter to exit\n");
    getchar();

    return 0;
}
