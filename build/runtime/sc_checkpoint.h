/**
 * Shellcode checkpoint VEH — header with extern decls.
 *
 * When ENABLE_SC_CHECKPOINTS is defined, declares the VEH install/remove
 * functions and the base-address global. Otherwise expands to no-ops so
 * carrier code compiles cleanly regardless of feature state.
 */
#ifndef SC_CHECKPOINT_H
#define SC_CHECKPOINT_H

#ifdef ENABLE_SC_CHECKPOINTS

#include <windows.h>

/** Set by carrier to the base address of decoded shellcode in memory. */
extern volatile void* __sc_base_addr;

/** Macro to set the base address (mirrors no-op version when disabled). */
#define __sc_set_base_addr(addr) (__sc_base_addr = (volatile void*)(addr))

/** Register the VEH handler (call before shellcode execution). */
extern void __sc_veh_install(void);

/** Remove the VEH handler (call after shellcode returns or on cleanup). */
extern void __sc_veh_remove(void);

#else /* !ENABLE_SC_CHECKPOINTS */

/** No-op: swallows the address expression without side effects. */
#define __sc_set_base_addr(addr) ((void)(addr))
#define __sc_veh_install() ((void)0)
#define __sc_veh_remove()  ((void)0)

#endif /* ENABLE_SC_CHECKPOINTS */

/* ---- Unified shellcode execution macro --------------------------------- */
/* Encapsulates VEH install/remove + instrumented vs baseline call pattern.
 * Carriers just call EXECUTE_SHELLCODE(addr) instead of duplicating the
 * #ifdef ENABLE_INSTRUMENTATION ... #else ... #endif block.             */

#ifdef ENABLE_INSTRUMENTATION
  #define EXECUTE_SHELLCODE(addr) do { \
      __sc_set_base_addr(addr); \
      __sc_veh_install(); \
      ((void(*)(void(*)(const char*)))(addr))(__artifact_checkpoint); \
      __sc_veh_remove(); \
  } while(0)
#else
  #define EXECUTE_SHELLCODE(addr) (*(void(*)())(addr))()
#endif

#endif /* SC_CHECKPOINT_H */
