/* MODULE: VIRTUALPROTECT
 * TYPE: standard
 * DESC: Direct wrapper around VirtualProtect.
 */
#include "../header/definitions.h"

FORCE_INLINE BOOL MyVirtualProtect(LPVOID lpAddress, SIZE_T dwSize, DWORD flNewProtect, PDWORD lpflOldProtect) {
    return VirtualProtect(lpAddress, dwSize, flNewProtect, lpflOldProtect);
}
