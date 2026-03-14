/**
 * VEH Checkpoint Stub — PIC x64 code for PE injection instrumentation
 *
 * Fully position-independent: no CRT, no imports, all APIs resolved via PEB walk.
 * Compiled with clang to .o, .text extracted by Rust, patched and assembled into
 * the injected section alongside the decode stub and encoded payload.
 *
 * Layout when assembled into PE section:
 *   [veh_stub_code]      This compiled code
 *   [checkpoint_data]    { u32 count, {u32 offset, u8 orig_byte}[count], pipe_name\0 }
 *   [decode_stub]        XOR/SubByte/None carrier stub
 *   [key_bytes]          Encoding key (0-256 bytes)
 *   [encoded_payload]    Encoded shellcode
 *
 * The stub:
 *   1. Resolves kernel32 APIs via PEB walk
 *   2. Opens checkpoint named pipe
 *   3. Installs VEH handler for INT3 breakpoints
 *   4. Falls through to decode stub (patched RIP-relative JMP)
 *
 * After decode runs and CALLs the decoded payload, INT3 breakpoints fire.
 * The VEH handler catches them, reports via pipe, restores original bytes.
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
typedef int                 BOOLEAN;
typedef unsigned long long  SIZE_T;
typedef wchar_t             WCHAR;
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
    BYTE  filler[102];  /* skip to DataDirectory offset */
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
    /* Partial — we only need Rip at offset 0xF8 in the full CONTEXT.
     * Rather than reproduce the full 1232-byte struct, we access Rip
     * through a pointer cast in the handler. */
    DWORD64 filler[32];  /* placeholder */
    DWORD64 Rip;         /* not at real offset — see handler for actual access */
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
 * Globals (stored in .data / .bss — will be in .text after extraction)
 * We use static vars; since this is PIC code extracted from .text,
 * the compiler will use RIP-relative addressing for these.
 * ====================================================================== */

static HANDLE           g_pipe_handle;
static WriteFile_t      g_WriteFile;
static VirtualProtect_t g_VirtualProtect;
static DWORD            g_checkpoint_count;
static CheckpointEntry* g_checkpoint_table;
static BYTE*            g_shellcode_base;

/* ======================================================================
 * PEB Walk — resolve module by name (case-insensitive wchar)
 * ====================================================================== */

static WCHAR to_lower_w(WCHAR c) {
    if (c >= L'A' && c <= L'Z') return (WCHAR)(c - L'A' + L'a');
    return c;
}

static LPVOID get_module_by_name(const WCHAR* module_name) {
    PEB* peb;
#if defined(_M_X64) || defined(__x86_64__)
    peb = (PEB*)__readgsqword(0x60);
#else
    peb = (PEB*)__readfsdword(0x30);
#endif
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
 * Export resolver — find function by name in PE export table
 * ====================================================================== */

static LPVOID get_func_by_name(LPVOID module, const char* func_name) {
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

/* ======================================================================
 * Manual integer-to-string (no snprintf in PIC)
 * ====================================================================== */

static int uint_to_str(char* buf, unsigned int val) {
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

static int str_copy(char* dst, const char* src) {
    int i = 0;
    while (src[i]) { dst[i] = src[i]; i++; }
    return i;
}

/* ======================================================================
 * VEH Handler — catches INT3 in shellcode region
 * ====================================================================== */

/*
 * CONTEXT layout on x64 Windows (from winnt.h):
 * Rip is at offset 0xF8 (248) in the full CONTEXT structure.
 * We access it via raw byte pointer to avoid reproducing the full struct.
 */
#define CONTEXT_RIP_OFFSET 0xF8

static LONG __stdcall veh_handler(EXCEPTION_POINTERS* ep) {
    if (ep->ExceptionRecord->ExceptionCode != EXCEPTION_BREAKPOINT)
        return EXCEPTION_CONTINUE_SEARCH;

    if (!g_shellcode_base || !g_checkpoint_table)
        return EXCEPTION_CONTINUE_SEARCH;

    ULONG_PTR exc_addr = (ULONG_PTR)ep->ExceptionRecord->ExceptionAddress;
    ULONG_PTR base = (ULONG_PTR)g_shellcode_base;

    if (exc_addr < base)
        return EXCEPTION_CONTINUE_SEARCH;

    DWORD offset = (DWORD)(exc_addr - base);

    for (DWORD i = 0; i < g_checkpoint_count; i++) {
        if (g_checkpoint_table[i].offset == offset) {
            /* Build JSON on stack:
             * {"ts_us":0,"checkpoint":"sc_checkpoint_N","type":"artifact_checkpoint"}\n */
            char json[128];
            int pos = 0;
            pos += str_copy(json + pos, "{\"ts_us\":0,\"checkpoint\":\"sc_checkpoint_");
            pos += uint_to_str(json + pos, i);
            pos += str_copy(json + pos, "\",\"type\":\"artifact_checkpoint\"}\n");

            /* Write to pipe (best-effort — don't fail execution if pipe is gone) */
            if (g_pipe_handle && g_pipe_handle != INVALID_HANDLE_VALUE && g_WriteFile) {
                DWORD written;
                g_WriteFile(g_pipe_handle, json, (DWORD)pos, &written, 0);
            }

            /* Restore original byte */
            if (g_VirtualProtect) {
                DWORD old_protect;
                g_VirtualProtect((LPVOID)exc_addr, 1, PAGE_EXECUTE_READWRITE, &old_protect);
                *(BYTE*)exc_addr = g_checkpoint_table[i].orig_byte;
                g_VirtualProtect((LPVOID)exc_addr, 1, old_protect, &old_protect);
            }

            /* Resume at the restored instruction */
            DWORD64* rip_ptr = (DWORD64*)((BYTE*)ep->ContextRecord + CONTEXT_RIP_OFFSET);
            *rip_ptr = exc_addr;
            return EXCEPTION_CONTINUE_EXECUTION;
        }
    }

    return EXCEPTION_CONTINUE_SEARCH;
}

/* ======================================================================
 * Stub entry point
 *
 * Called when execution is redirected to the injected section.
 * After setup, falls through to the decode stub via a JMP whose
 * displacement is patched by Rust at assembly time.
 * ====================================================================== */

/*
 * Sentinel values for Rust to find and patch:
 *   0xDEADBEEF — LEA displacement pointing to checkpoint data trailer
 *   0xCAFEBABE — JMP displacement to decode stub
 *
 * These appear as disp32 operands in RIP-relative LEA/JMP instructions.
 * The Rust assembler scans for them with iced-x86 and patches in the
 * correct offsets based on the assembled section layout.
 */

__attribute__((section(".text")))
void stub_entry(void) {
    /* Align RSP for Win64 ABI */
    /* (The compiler handles this via function prologue) */

    /* --- PEB walk: resolve kernel32.dll --- */
    WCHAR k32_name[] = { L'k',L'e',L'r',L'n',L'e',L'l',L'3',L'2',L'.',L'd',L'l',L'l', 0 };
    LPVOID k32 = get_module_by_name(k32_name);
    if (!k32) goto skip_veh;

    /* --- Resolve 5 APIs --- */
    char s_CreateFileA[] = { 'C','r','e','a','t','e','F','i','l','e','A', 0 };
    char s_WriteFile[]   = { 'W','r','i','t','e','F','i','l','e', 0 };
    char s_VirtualProtect[] = { 'V','i','r','t','u','a','l','P','r','o','t','e','c','t', 0 };
    char s_AddVEH[]      = { 'A','d','d','V','e','c','t','o','r','e','d',
                             'E','x','c','e','p','t','i','o','n',
                             'H','a','n','d','l','e','r', 0 };
    char s_RemoveVEH[]   = { 'R','e','m','o','v','e','V','e','c','t','o','r','e','d',
                             'E','x','c','e','p','t','i','o','n',
                             'H','a','n','d','l','e','r', 0 };

    CreateFileA_t pCreateFileA = (CreateFileA_t)get_func_by_name(k32, s_CreateFileA);
    g_WriteFile     = (WriteFile_t)get_func_by_name(k32, s_WriteFile);
    g_VirtualProtect = (VirtualProtect_t)get_func_by_name(k32, s_VirtualProtect);
    AddVectoredExceptionHandler_t pAddVEH =
        (AddVectoredExceptionHandler_t)get_func_by_name(k32, s_AddVEH);

    if (!pCreateFileA || !g_WriteFile || !g_VirtualProtect || !pAddVEH)
        goto skip_veh;

    /* --- Locate checkpoint data via sentinel LEA --- */
    {
        /*
         * The compiler emits: lea rax, [rip + 0xDEADBEEF]
         * Rust patches the disp32 to point to the checkpoint data trailer.
         */
        BYTE* data_ptr;
        __asm__ volatile (
            "lea %0, [rip + 0xDEADBEEF]"  /* sentinel — patched by Rust */
            : "=r"(data_ptr)
        );

        /* Parse checkpoint data trailer:
         *   [u32 count] [count * {u32 offset, u8 orig_byte}] [pipe_name\0] */
        g_checkpoint_count = *(DWORD*)data_ptr;
        g_checkpoint_table = (CheckpointEntry*)(data_ptr + 4);
        char* pipe_name = (char*)(data_ptr + 4 + g_checkpoint_count * sizeof(CheckpointEntry));

        /* Open checkpoint pipe */
        g_pipe_handle = pCreateFileA(
            pipe_name,
            GENERIC_WRITE,
            0,     /* no sharing */
            0,     /* no security */
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            0      /* no template */
        );

        /* Install VEH handler (first in chain) */
        pAddVEH(1, (void*)veh_handler);

        /*
         * Compute shellcode base address.
         * After the decode stub runs, the decoded shellcode (with INT3s) lives
         * at the encoded_payload location. We compute that from our data pointer:
         *   shellcode_base = pipe_name + strlen(pipe_name) + 1 + decode_stub_size + key_size
         * But we don't know those sizes here. Instead, Rust patches g_shellcode_base
         * into a known location, OR we set it after computing from the section layout.
         *
         * Simpler approach: Rust writes the shellcode_base_offset into the data trailer
         * as an additional field after pipe_name. But that changes the format.
         *
         * Simplest: the decode stub is located right after pipe_name. After decode runs
         * in-place, the payload region starts at a known offset from data_ptr.
         * Rust computes this offset and stores it as a u32 after pipe_name.
         *
         * Data trailer format (updated):
         *   [u32 count] [{u32,u8}*count] [pipe_name\0] [u32 shellcode_base_rel]
         *
         * shellcode_base_rel = offset from data_ptr to the start of (decoded) payload.
         */
        /* Find end of pipe_name */
        char* p = pipe_name;
        while (*p) p++;
        p++; /* skip null terminator */
        DWORD shellcode_base_rel = *(DWORD*)p;
        g_shellcode_base = data_ptr + shellcode_base_rel;
    }

skip_veh:
    /* --- Jump to decode stub --- */
    /*
     * The compiler emits: jmp [rip + 0xCAFEBABE]
     * Rust patches the displacement to jump to the decode stub.
     * We use inline asm to guarantee the sentinel appears.
     */
    __asm__ volatile (
        "jmp 0xCAFEBABE"  /* sentinel — patched by Rust to decode stub offset */
    );

    /* Unreachable — silences compiler warnings */
    __builtin_unreachable();
}
