/* A C function split across two files.
 *
 * This is the entry point — it exports the four `apiplant.h` symbols — but the
 * work is done by `fnv1a`, which lives in `checksum.c`. Because this function is
 * a *directory*, `apiplant build` compiles every `.c` beside it and puts the
 * directory on the include path, so the `#include "checksum.h"` below just
 * resolves. A single-file `.c` function can't be split this way. */
#include "apiplant.h"
#include "checksum.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

uint32_t apiplant_abi_version(void) { return APIPLANT_ABI_VERSION; }

const char *apiplant_manifest(void) {
    return "[{"
           "\"name\":\"checksum\","
           "\"description\":\"Hashes the request body with a helper in a second file.\","
           "\"method\":\"POST\","
           "\"visibility\":\"public\""
           "}]";
}

int32_t apiplant_invoke(const char *name, const char *input_json,
                        const ApiplantHost *host, char **out) {
    (void)name;
    (void)host;

    /* Hash the raw request body — the point is the cross-file call, not JSON. */
    const char *body = input_json ? input_json : "";
    uint32_t sum = fnv1a(body, strlen(body));

    char *buf = malloc(32);
    if (!buf) {
        *out = NULL;
        return APIPLANT_ERR_INTERNAL;
    }
    snprintf(buf, 32, "{\"checksum\":%u}", sum);
    *out = buf;
    return APIPLANT_OK;
}

void apiplant_free(char *string) { free(string); }
