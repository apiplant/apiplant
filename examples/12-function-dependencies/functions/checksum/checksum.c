/* The "extra file" this example is about: a helper compiled alongside the entry
 * point, not inlined into it. Split a C function across as many .c files as you
 * like — apiplant compiles them together. */
#include "checksum.h"

uint32_t fnv1a(const char *data, size_t len) {
    uint32_t hash = 2166136261u;
    for (size_t i = 0; i < len; i++) {
        hash ^= (unsigned char)data[i];
        hash *= 16777619u;
    }
    return hash;
}
