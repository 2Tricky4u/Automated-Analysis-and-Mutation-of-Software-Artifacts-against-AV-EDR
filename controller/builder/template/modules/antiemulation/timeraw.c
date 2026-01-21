/* MODULE: ANTIEMULATION
 * TYPE: timeraw
 * DESC: Busy sleep using KUSER_SHARED_DATA time register.
 * ADDR: 0x7FFE0004 (TickCountMultiplier)
 */
#include "../header/definitions.h"

// Helper function to read raw time from shared kernel page
// This bypasses hooked time APIs (GetTickCount, etc.)
int get_time_raw() {
    ULONG* PUserSharedData_TickCountMultiplier = (PULONG)0x7ffe0004;
    LONG* PUserSharedData_High1Time = (PLONG)0x7ffe0324;
    ULONG* PUserSharedData_LowPart = (PULONG)0x7ffe0320;
    DWORD kernelTime = (*PUserSharedData_TickCountMultiplier) * (*PUserSharedData_High1Time << 8) +
        ((*PUserSharedData_LowPart) * (unsigned __int64)(*PUserSharedData_TickCountMultiplier) >> 24);
    return kernelTime;
}

// Busy wait loop
void sleep_ms_raw(DWORD sleeptime) {
    DWORD start = get_time_raw();
    while (get_time_raw() - start < sleeptime) {}
}

void antiemulation() {
    // 3 seconds busy wait
    sleep_ms_raw(3000);
}
