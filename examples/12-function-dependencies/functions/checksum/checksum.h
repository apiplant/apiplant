/* A second translation unit's public interface. A C function *directory*
 * compiles every .c file it holds together and adds itself to the include path,
 * so `hello.c` can #include this and call across files. */
#ifndef CHECKSUM_H
#define CHECKSUM_H

#include <stddef.h>
#include <stdint.h>

/* A tiny FNV-1a hash, defined in checksum.c. */
uint32_t fnv1a(const char *data, size_t len);

#endif
