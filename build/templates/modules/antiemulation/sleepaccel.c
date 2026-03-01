/* MODULE: ANTIEMULATION
 * TYPE: sleepaccel
 * DESC: Detects sleep acceleration — emulators often fast-forward Sleep() to
 *       speed up analysis. We Sleep(2000ms) then verify real wall-clock time
 *       via QueryPerformanceCounter. If < 1500ms actually elapsed, we're in
 *       an accelerated sandbox and stall indefinitely.
 * MUTATIONS: literal_encoding, timing_jitter
 */
#include "../header/definitions.h"

// @MUTATE:literal_encoding
#define SLEEP_DURATION_MS  2000
#define MIN_ELAPSED_MS     1500

void antiemulation() {
    LARGE_INTEGER freq, before, after;

    if (!QueryPerformanceFrequency(&freq) || freq.QuadPart == 0) {
        return;  // Can't measure — skip check
    }

    QueryPerformanceCounter(&before);

    // @MUTATE:timing_jitter
    // @MUTATE:literal_encoding
    Sleep(SLEEP_DURATION_MS);

    QueryPerformanceCounter(&after);

    // Compute elapsed milliseconds
    // @MUTATE:literal_encoding
    LONGLONG elapsed_ms = ((after.QuadPart - before.QuadPart) * 1000) / freq.QuadPart;

    // @MUTATE:opaque_predicate
    // @MUTATE:literal_encoding
    if (elapsed_ms < MIN_ELAPSED_MS) {
        // Sleep was accelerated — we're in a sandbox
        // Stall forever (or until emulator budget is exhausted)
        // @MUTATE:timing_jitter
        while (1) {
            Sleep(60000);
        }
    }
}
