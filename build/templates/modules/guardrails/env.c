/* MODULE: GUARDRAIL
 * TYPE: env
 * DESC: Checks for environment variables using case-insensitive substring match.
 * CONFIG: Define ENV_KEY and ENV_NEEDLE during build.
 */
#include "../header/definitions.h"
#include <stdio.h>

// Defaults if not defined by build system
#ifndef ENV_KEY
#define ENV_KEY "USERNAME"
#endif

#ifndef ENV_NEEDLE
#define ENV_NEEDLE "Sandbox"
#endif

char my_tolower(char c) {
    if (c >= 'A' && c <= 'Z') {
        return c + ('a' - 'A');
    }
    return c;
}

// Returns 1 if 'needle' is found in 'haystack' (case-insensitive), 0 otherwise
int contains_case_insensitive(const char* haystack, const char* needle) {
    if (!haystack || !needle)
        return 0;

    for (; *haystack; haystack++) {
        const char* h = haystack;
        const char* n = needle;

        while (*h && *n && my_tolower((unsigned char)*h) == my_tolower((unsigned char)*n)) {
            h++;
            n++;
        }

        if (*n == '\0') {
            return 1;  // Match found
        }
    }

    return 0;  // No match
}

int guardrail() {
    // Execution Guardrail: Env Check
    const char* envVarName = ENV_KEY;
    const char* tocheck = ENV_NEEDLE;
    char buffer[1024]; 
    
    DWORD result = GetEnvironmentVariableA(envVarName, buffer, 1024);
    if (result == 0) {
        return 6; // Fail if env var not found
    }
    
    // In original SuperMega:
    // If we are looking for "Admin" in "Administrator", contains returns 1.
    // Logic: If NOT found, return 6 (Fail). 
    // Wait, the original template logic was:
    // if (! contains_case_insensitive(buffer, tocheck)) return 6;
    // Meaning we WANT the needle to be present.
    // e.g. Key="USERNAME", Needle="Admin" -> Runs only if user is Admin.
    
    // BUT the previous implementation I wrote was:
    // if (strcmp(buffer, "Sandbox") == 0) return 1; (Bail if sandbox)
    
    // Let's stick to the ORIGINAL SuperMega logic which is usually an "Inclusion Check"
    // (Run only if target matches). 
    // OR it could remain an exclusion check if the user negates the logic in the template.
    //
    // Looking at data/source/guardrails/env.c:
    // if (! contains_case_insensitive(buffer, tocheck)) { return 6; }
    // This implies "Must Contain".
    
    if (!contains_case_insensitive(buffer, tocheck)) {
        return 6;
    }
    
    return 0; // Success
}
