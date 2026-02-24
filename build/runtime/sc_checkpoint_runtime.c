/**
 * Shellcode Checkpoint Runtime — VEH-based INT3 handler
 *
 * Catches EXCEPTION_BREAKPOINT exceptions at addresses inside the shellcode
 * region, looks up the hit offset in the generated checkpoint table, reports
 * the checkpoint via __artifact_checkpoint(), restores the original byte,
 * and resumes execution.
 *
 * Only compiled when ENABLE_SC_CHECKPOINTS is defined (via the sc-checkpoints
 * Cargo feature + instrumented build).
 */

#ifdef ENABLE_SC_CHECKPOINTS

#include <windows.h>
#include <stdint.h>

/* Generated per-build by sc_checkpoints.rs — passed via -I<output_dir> */
#include "sc_checkpoints_table.h"

/* From instrumentation_runtime.c (always linked in instrumented builds) */
extern void __artifact_checkpoint(const char* checkpoint_name);

/* ---- globals ---------------------------------------------------------- */

/** Base address of the decoded shellcode buffer.  Set by the carrier module
 *  right before jumping into shellcode. */
volatile void* __sc_base_addr = NULL;

/** Handle returned by AddVectoredExceptionHandler, used for removal. */
static PVOID __veh_handle = NULL;

/* ---- VEH handler ------------------------------------------------------ */

static LONG CALLBACK __sc_veh_handler(PEXCEPTION_POINTERS ep) {
    if (ep->ExceptionRecord->ExceptionCode != EXCEPTION_BREAKPOINT)
        return EXCEPTION_CONTINUE_SEARCH;

    if (!__sc_base_addr)
        return EXCEPTION_CONTINUE_SEARCH;

    /*
     * On x64 Windows, EXCEPTION_BREAKPOINT sets ExceptionAddress to the
     * address of the INT3 byte itself (unlike x86-32 where RIP advances
     * past it).  So exc_addr == address of 0xCC.
     */
    uintptr_t exc_addr = (uintptr_t)ep->ExceptionRecord->ExceptionAddress;
    uintptr_t base = (uintptr_t)__sc_base_addr;

    if (exc_addr < base)
        return EXCEPTION_CONTINUE_SEARCH;

    uint32_t offset = (uint32_t)(exc_addr - base);

    for (int i = 0; i < SC_CHECKPOINT_COUNT; i++) {
        if (SC_CHECKPOINTS[i].offset == offset) {
            /* Report this checkpoint through the instrumentation pipe. */
            __artifact_checkpoint(SC_CHECKPOINTS[i].name);

            /* Restore the original byte so the instruction can execute.
             * The shellcode page is already at least RX; we temporarily
             * flip to RWX to write, then restore. */
            DWORD old_protect;
            VirtualProtect((LPVOID)exc_addr, 1,
                           PAGE_EXECUTE_READWRITE, &old_protect);
            *(unsigned char*)exc_addr = SC_CHECKPOINTS[i].orig_byte;
            VirtualProtect((LPVOID)exc_addr, 1,
                           old_protect, &old_protect);

            /* Re-execute the restored instruction.  RIP already points at
             * the INT3 address, so just setting Rip = exc_addr is correct
             * (no -1 adjustment needed on x64). */
            ep->ContextRecord->Rip = exc_addr;
            return EXCEPTION_CONTINUE_EXECUTION;
        }
    }

    /* Not one of our checkpoints — pass to next handler. */
    return EXCEPTION_CONTINUE_SEARCH;
}

/* ---- public API ------------------------------------------------------- */

void __sc_veh_install(void) {
    if (!__veh_handle)
        __veh_handle = AddVectoredExceptionHandler(1, __sc_veh_handler);
}

void __sc_veh_remove(void) {
    if (__veh_handle) {
        RemoveVectoredExceptionHandler(__veh_handle);
        __veh_handle = NULL;
    }
    __sc_base_addr = NULL;
}

#endif /* ENABLE_SC_CHECKPOINTS */
