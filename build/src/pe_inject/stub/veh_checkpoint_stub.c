/**
 * VEH Checkpoint Stub — PIC x64 code for PE injection instrumentation
 *
 * Fully position-independent: no CRT, no imports, all APIs resolved via PEB walk.
 * Compiled with clang to .o, .text extracted by Rust, patched and assembled into
 * the injected section alongside the decode stub and encoded payload.
 *
 * Layout when assembled into PE section:
 *   [veh_stub_code]      This compiled code
 *   [checkpoint_data]    { u32 count, {u32 offset, u8 orig_byte}[count], pipe_name\0,
 *                          u32 shellcode_base_rel, u64 pipe_handle_slot }
 *   [decode_stub]        XOR/SubByte/None carrier stub
 *   [key_bytes]          Encoding key (0-256 bytes)
 *   [encoded_payload]    Encoded shellcode
 *
 * The stub:
 *   1. Resolves kernel32 APIs via PEB walk
 *   2. Opens checkpoint named pipe
 *   3. Writes pipe handle into the data trailer for the VEH handler
 *   4. Installs VEH handler for INT3 breakpoints
 *   5. Falls through to decode stub (patched RIP-relative JMP)
 *
 * After decode runs and CALLs the decoded payload, INT3 breakpoints fire.
 * The VEH handler catches them, reports via pipe, restores original bytes.
 *
 * DESIGN: No global variables. All state is communicated through the data
 * trailer (in the injected section). stub_entry writes runtime values
 * (pipe handle) into the trailer. veh_handler locates the trailer via its
 * own sentinel (0xBAADF00D) and re-resolves API function pointers via PEB walk.
 * All helper functions are __attribute__((always_inline)) to guarantee they
 * are inlined — ensuring stub_entry is at offset 0 of .text and there are
 * no standalone helper functions with broken relocations after extraction.
 *
 * Build: clang -c -O2 -nostdlib -fno-stack-protector -fno-exceptions
 *        --target=x86_64-pc-windows-msvc -fms-compatibility -fms-extensions
 *        -fno-builtin -mno-red-zone veh_checkpoint_stub.c
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

/* Exception / VEH structures */
typedef struct _EXCEPTION_RECORD {
    DWORD ExceptionCode;
    DWORD ExceptionFlags;
    struct _EXCEPTION_RECORD* ExceptionRecord;
    PVOID ExceptionAddress;
    DWORD NumberParameters;
    ULONG_PTR ExceptionInformation[15];
} EXCEPTION_RECORD;

typedef struct _CONTEXT {
    DWORD64 filler[32];
    DWORD64 Rip;
} CONTEXT;

typedef struct _EXCEPTION_POINTERS {
    EXCEPTION_RECORD* ExceptionRecord;
    CONTEXT*          ContextRecord;
} EXCEPTION_POINTERS;

/* Constants */
#define EXCEPTION_BREAKPOINT        0x80000003
#define EXCEPTION_CONTINUE_EXECUTION (-1)
#define EXCEPTION_CONTINUE_SEARCH    0
#define GENERIC_WRITE               0x40000000
#define OPEN_EXISTING               3
#define FILE_ATTRIBUTE_NORMAL       0x80
#define INVALID_HANDLE_VALUE        ((HANDLE)(ULONG_PTR)-1)
#define PAGE_EXECUTE_READWRITE      0x40
#define IMAGE_DOS_SIGNATURE         0x5A4D

/* API function pointer types */
typedef HANDLE (*CreateFileA_t)(LPCSTR, DWORD, DWORD, PVOID, DWORD, DWORD, HANDLE);
typedef BOOL   (*WriteFile_t)(HANDLE, const void*, DWORD, DWORD*, PVOID);
typedef BOOL   (*VirtualProtect_t)(LPVOID, SIZE_T, DWORD, DWORD*);
typedef PVOID  (*AddVectoredExceptionHandler_t)(ULONG, void*);
typedef ULONG  (*RemoveVectoredExceptionHandler_t)(PVOID);

/* Checkpoint table entry (packed, matches Rust assembly) */
#pragma pack(push, 1)
typedef struct _CheckpointEntry {
    DWORD offset;
    BYTE  orig_byte;
} CheckpointEntry;
#pragma pack(pop)

/* ======================================================================
 * PEB Walk helpers — ALL always_inline to avoid standalone functions
 * ====================================================================== */

/* PEB access via GS segment — AT&T syntax, no <intrin.h> dependency. */
static __inline __attribute__((always_inline)) void* __pic_read_peb(void) {
    void* val;
    __asm__ volatile ("movq %%gs:0x60, %0" : "=r"(val));
    return val;
}

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

static __inline __attribute__((always_inline)) LPVOID get_func_by_name(LPVOID module, const char* func_name) {
    IMAGE_DOS_HEADER* dos = (IMAGE_DOS_HEADER*)module;
    if (dos->e_magic != IMAGE_DOS_SIGNATURE) return 0;

    IMAGE_NT_HEADERS64* nt = (IMAGE_NT_HEADERS64*)((BYTE*)module + dos->e_lfanew);
    IMAGE_DATA_DIRECTORY* exp_dir = &nt->OptionalHeader.DataDirectory[0]; /* EXPORT */
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

/* Manual integer-to-string (no snprintf in PIC) */
static __inline __attribute__((always_inline)) int uint_to_str(char* buf, unsigned int val) {
    char tmp[12];
    int len = 0;
    if (val == 0) { buf[0] = '0'; return 1; }
    while (val > 0) {
        tmp[len++] = '0' + (char)(val % 10);
        val /= 10;
    }
    for (int i = 0; i < len; i++)
        buf[i] = tmp[len - 1 - i];
    return len;
}

static __inline __attribute__((always_inline)) int str_copy(char* dst, const char* src) {
    int i = 0;
    while (src[i]) { dst[i] = src[i]; i++; }
    return i;
}

/* ======================================================================
 * Helper: locate the data trailer end fields from a data_ptr
 *
 * Data trailer layout:
 *   [u32 count]
 *   [count × {u32 offset, u8 orig_byte}]   (5 bytes each)
 *   [pipe_name\0]
 *   [u32 shellcode_base_rel]
 *   [u64 pipe_handle_slot]                  ← NEW: written by stub_entry
 * ====================================================================== */

/* Walk from data_ptr to find pipe_handle_slot pointer */
static __inline __attribute__((always_inline)) void parse_trailer(
    BYTE* data_ptr,
    DWORD* out_count,
    CheckpointEntry** out_table,
    BYTE** out_shellcode_base,
    HANDLE* out_pipe_handle,
    BYTE** out_pipe_handle_slot
) {
    DWORD count = *(DWORD*)data_ptr;
    CheckpointEntry* table = (CheckpointEntry*)(data_ptr + 4);
    char* pipe_name = (char*)(data_ptr + 4 + count * sizeof(CheckpointEntry));

    /* Walk past pipe_name null terminator */
    char* p = pipe_name;
    while (*p) p++;
    p++; /* skip null */

    /* shellcode_base_rel */
    DWORD shellcode_base_rel = *(DWORD*)p;
    p += 4;

    /* pipe_handle_slot (u64) */
    BYTE* handle_slot = (BYTE*)p;
    HANDLE pipe_handle = *(HANDLE*)p;

    if (out_count) *out_count = count;
    if (out_table) *out_table = table;
    if (out_shellcode_base) *out_shellcode_base = data_ptr + shellcode_base_rel;
    if (out_pipe_handle) *out_pipe_handle = pipe_handle;
    if (out_pipe_handle_slot) *out_pipe_handle_slot = handle_slot;
}

/* ======================================================================
 * Forward-declare veh_handler (defined after stub_entry)
 * ====================================================================== */

static LONG __stdcall veh_handler(EXCEPTION_POINTERS* ep);

/* ======================================================================
 * Stub entry point — MUST be the first function in .text (offset 0)
 *
 * After setup, falls through to the decode stub via a JMP whose
 * displacement is patched by Rust at assembly time.
 *
 * Sentinel values for Rust to find and patch:
 *   0xDEADBEEF — LEA displacement pointing to checkpoint data trailer
 *   0xCAFEBABE — JMP displacement to decode stub
 * ====================================================================== */

__attribute__((section(".text")))
void stub_entry(void) {
    /* --- PEB walk: resolve kernel32.dll --- */
    WCHAR k32_name[] = { L'k',L'e',L'r',L'n',L'e',L'l',L'3',L'2',L'.',L'd',L'l',L'l', 0 };
    LPVOID k32 = get_module_by_name(k32_name);
    if (!k32) goto skip_veh;

    /* --- Resolve APIs --- */
    char s_CreateFileA[] = { 'C','r','e','a','t','e','F','i','l','e','A', 0 };
    char s_WriteFile[]   = { 'W','r','i','t','e','F','i','l','e', 0 };
    char s_VirtualProtect[] = { 'V','i','r','t','u','a','l','P','r','o','t','e','c','t', 0 };
    char s_AddVEH[]      = { 'A','d','d','V','e','c','t','o','r','e','d',
                             'E','x','c','e','p','t','i','o','n',
                             'H','a','n','d','l','e','r', 0 };

    CreateFileA_t pCreateFileA = (CreateFileA_t)get_func_by_name(k32, s_CreateFileA);
    WriteFile_t pWriteFile     = (WriteFile_t)get_func_by_name(k32, s_WriteFile);
    VirtualProtect_t pVP       = (VirtualProtect_t)get_func_by_name(k32, s_VirtualProtect);
    AddVectoredExceptionHandler_t pAddVEH =
        (AddVectoredExceptionHandler_t)get_func_by_name(k32, s_AddVEH);

    if (!pCreateFileA || !pWriteFile || !pVP || !pAddVEH)
        goto skip_veh;

    /* --- Locate checkpoint data via sentinel LEA (0xDEADBEEF) --- */
    {
        BYTE* data_ptr;
        __asm__ volatile (
            ".byte 0x48, 0x8D, 0x05, 0xEF, 0xBE, 0xAD, 0xDE\n"
            "movq %%rax, %0\n"
            : "=r"(data_ptr)
            :
            : "rax"
        );

        /* Parse trailer */
        DWORD chk_count;
        CheckpointEntry* chk_table;
        BYTE* shellcode_base;
        BYTE* pipe_handle_slot;
        parse_trailer(data_ptr, &chk_count, &chk_table, &shellcode_base, 0, &pipe_handle_slot);

        /* Get pipe_name from trailer */
        char* pipe_name = (char*)(data_ptr + 4 + chk_count * sizeof(CheckpointEntry));

        /* Open checkpoint pipe */
        HANDLE pipe_handle = pCreateFileA(
            pipe_name,
            GENERIC_WRITE,
            0,     /* no sharing */
            0,     /* no security */
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            0      /* no template */
        );

        /* Write pipe handle into the trailer slot for veh_handler to read */
        *(HANDLE*)pipe_handle_slot = pipe_handle;

        /* Install VEH handler (first in chain) */
        pAddVEH(1, (void*)veh_handler);
    }

skip_veh:
    /* --- Jump to decode stub --- */
    /* JMP rel32 with displacement 0xCAFEBABE = E9 BE BA FE CA */
    __asm__ volatile (
        ".byte 0xE9, 0xBE, 0xBA, 0xFE, 0xCA\n"
    );

    __builtin_unreachable();
}

/* ======================================================================
 * VEH Handler — catches INT3 in shellcode region
 *
 * Locates the data trailer via its OWN sentinel (0xBAADF00D).
 * Re-resolves WriteFile and VirtualProtect via PEB walk each time
 * (the helpers are always_inline so they're inlined here).
 * Reads pipe_handle from the extended trailer slot.
 *
 * CONTEXT layout on x64 Windows: Rip is at offset 0xF8.
 * ====================================================================== */

#define CONTEXT_RIP_OFFSET 0xF8

static LONG __stdcall veh_handler(EXCEPTION_POINTERS* ep) {
    if (ep->ExceptionRecord->ExceptionCode != EXCEPTION_BREAKPOINT)
        return EXCEPTION_CONTINUE_SEARCH;

    /* --- Locate data trailer via sentinel LEA (0xBAADF00D) --- */
    BYTE* data_ptr;
    __asm__ volatile (
        ".byte 0x48, 0x8D, 0x05, 0x0D, 0xF0, 0xAD, 0xBA\n"
        "movq %%rax, %0\n"
        : "=r"(data_ptr)
        :
        : "rax"
    );

    /* Parse trailer to get checkpoint data and pipe handle */
    DWORD chk_count;
    CheckpointEntry* chk_table;
    BYTE* shellcode_base;
    HANDLE pipe_handle;
    parse_trailer(data_ptr, &chk_count, &chk_table, &shellcode_base, &pipe_handle, 0);

    if (!shellcode_base || !chk_table)
        return EXCEPTION_CONTINUE_SEARCH;

    ULONG_PTR exc_addr = (ULONG_PTR)ep->ExceptionRecord->ExceptionAddress;
    ULONG_PTR base = (ULONG_PTR)shellcode_base;

    if (exc_addr < base)
        return EXCEPTION_CONTINUE_SEARCH;

    DWORD offset = (DWORD)(exc_addr - base);

    for (DWORD i = 0; i < chk_count; i++) {
        if (chk_table[i].offset == offset) {
            /* Build JSON on stack:
             * {"ts_us":0,"checkpoint":"sc_checkpoint_N","type":"artifact_checkpoint"}\n */
            char json[128];
            int pos = 0;
            pos += str_copy(json + pos, "{\"ts_us\":0,\"checkpoint\":\"sc_checkpoint_");
            pos += uint_to_str(json + pos, i);
            pos += str_copy(json + pos, "\",\"type\":\"artifact_checkpoint\"}\n");

            /* Write to pipe (best-effort — skip if no pipe server) */
            if (pipe_handle && pipe_handle != INVALID_HANDLE_VALUE) {
                WCHAR k32w_name[] = { L'k',L'e',L'r',L'n',L'e',L'l',L'3',L'2',L'.',L'd',L'l',L'l', 0 };
                LPVOID k32w = get_module_by_name(k32w_name);
                if (k32w) {
                    char s_WF[] = { 'W','r','i','t','e','F','i','l','e', 0 };
                    WriteFile_t pWF = (WriteFile_t)get_func_by_name(k32w, s_WF);
                    if (pWF) {
                        DWORD written;
                        pWF(pipe_handle, json, (DWORD)pos, &written, 0);
                    }
                }
            }

            /* ALWAYS restore original byte — must happen even without pipe,
             * otherwise the INT3 fires again in an infinite loop. */
            {
                WCHAR k32r_name[] = { L'k',L'e',L'r',L'n',L'e',L'l',L'3',L'2',L'.',L'd',L'l',L'l', 0 };
                LPVOID k32r = get_module_by_name(k32r_name);
                if (k32r) {
                    char s_VP[] = { 'V','i','r','t','u','a','l','P','r','o','t','e','c','t', 0 };
                    VirtualProtect_t pVP = (VirtualProtect_t)get_func_by_name(k32r, s_VP);
                    if (pVP) {
                        DWORD old_protect;
                        pVP((LPVOID)exc_addr, 1, PAGE_EXECUTE_READWRITE, &old_protect);
                        *(BYTE*)exc_addr = chk_table[i].orig_byte;
                        pVP((LPVOID)exc_addr, 1, old_protect, &old_protect);
                    }
                }
            }

            /* Resume at the restored instruction */
            DWORD64* rip_ptr = (DWORD64*)((BYTE*)ep->ContextRecord + CONTEXT_RIP_OFFSET);
            *rip_ptr = exc_addr;
            return EXCEPTION_CONTINUE_EXECUTION;
        }
    }

    return EXCEPTION_CONTINUE_SEARCH;
}
