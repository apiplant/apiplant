/* apiplant.h — write an apiplant function in C (or Zig, or Go via cgo).
 *
 * Implement apiplant_abi_version, apiplant_manifest, apiplant_invoke and
 * apiplant_free, build as a shared library, and drop it in your app's
 * functions/ directory. `apiplant build` compiles every .c file there for you.
 *
 * The contract, in full, is documented in the `apiplant_abi::c` module.
 *
 * Memory: each side frees what it allocated. The string you write to *out is
 * released by the host calling your apiplant_free; strings the host hands you
 * (config, query, principal_id, hook, send_email, cache) are released by you
 * calling host->free_string.
 */
#ifndef APIPLANT_H
#define APIPLANT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Bump only on a breaking change; the host refuses anything it doesn't know. */
#define APIPLANT_ABI_VERSION 1u

/* apiplant_invoke return codes. */
#define APIPLANT_OK           0 /* *out is the JSON response body      -> 200 */
#define APIPLANT_ERR_REQUEST  1 /* *out is a message for the caller    -> 400 */
#define APIPLANT_ERR_INTERNAL 2 /* *out is a message for the log       -> 500 */

/* host->log severities. */
#define APIPLANT_TRACE 0
#define APIPLANT_DEBUG 1
#define APIPLANT_INFO  2
#define APIPLANT_WARN  3
#define APIPLANT_ERROR 4

/* Services the host lends you for the duration of one call. Valid only until
 * apiplant_invoke returns. Always pass `ctx` back untouched. */
typedef struct ApiplantHost {
    void *ctx;

    /* {"sql": "...", "params": [...]} -> JSON rows, {"rows_affected": n},
     * or {"error": "..."} if the query failed. Free with free_string. */
    char *(*query)(void *ctx, const char *request_json);

    void (*log)(void *ctx, int32_t level, const char *message);

    /* All three return a NUL-terminated string you must free_string.
     * principal_id and hook are empty strings when not applicable. */
    char *(*config)(void *ctx);
    char *(*principal_id)(void *ctx);
    char *(*hook)(void *ctx);

    void (*free_string)(void *ctx, char *string);

    /* Send an email through the provider the app configured in [email].
     * Request:  {"to": "ann@example.com", "subject": "Hi", "text": "Hello"}
     *           ("to"/"cc"/"bcc" also take a list; "html", "from" and
     *            "reply_to" are optional)
     * Returns:  {"provider": "ses", "id": "...", "recipients": 1}
     *           or {"error": "..."}. Free with free_string. */
    char *(*send_email)(void *ctx, const char *request_json);

    /* One operation against the app's [cache] Redis, if it configured one:
     *   {"op":"get","key":"k"}                     -> {"hit":true,"value":...}
     *   {"op":"set","key":"k","value":...,"ttl":60} -> {"ok":true}
     *   {"op":"delete"|"exists"|"incr"|"ttl", ...}
     * or {"error": "..."}. Free with free_string. */
    char *(*cache)(void *ctx, const char *request_json);
} ApiplantHost;

/* Callbacks are only ever appended to ApiplantHost, and the host is what
 * allocates it — so a library built against an older header keeps working
 * unchanged, and APIPLANT_ABI_VERSION does not move when one is added. */

/* ---- what your library must export ---------------------------------------- */

/* Define APIPLANT_NO_PROTOTYPES before including this header to get only the
 * types and constants above, without the four declarations below.
 *
 * Needed when your toolchain emits its own prototypes for the symbols it
 * exports and they collide with these — cgo, for instance, generates
 * `char *apiplant_manifest(void)`, which conflicts with the `const` here. Go
 * bindings should define this macro; C and Zig should not. */
#ifndef APIPLANT_NO_PROTOTYPES

/* Return APIPLANT_ABI_VERSION. */
uint32_t apiplant_abi_version(void);

/* A static JSON array, one object per function. Only "name" is required:
 *
 *   [{"name": "hello", "version": "1.0.0", "description": "Greets someone.",
 *     "visibility": "public", "method": "POST",
 *     "input_schema": {...}, "output_schema": {...}}]
 *
 * "visibility" is "public" | "authenticated" | "role:<name>" | "private" and
 * defaults to "private", so a typo hides an endpoint rather than exposing it.
 * "method" defaults to "POST". Never freed, so it must be static storage. */
const char *apiplant_manifest(void);

/* Handle one request. `name` is the manifest name being invoked (a library may
 * export several). Write a NUL-terminated string to *out and return one of the
 * APIPLANT_* codes. Must not unwind or longjmp out of this call. */
int32_t apiplant_invoke(const char *name, const char *input_json,
                        const ApiplantHost *host, char **out);

/* Release a string you produced in apiplant_invoke. */
void apiplant_free(char *string);

#endif /* APIPLANT_NO_PROTOTYPES */

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* APIPLANT_H */
