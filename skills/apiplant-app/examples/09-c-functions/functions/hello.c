/* A complete apiplant function in C.
 *
 * Two endpoints from one file:
 *
 *   POST /api/functions/hello   public         — greets someone, using config
 *   GET  /api/functions/notes   authenticated  — counts rows via the host's DB
 *
 * `apiplant build` compiles this with `cc` and drops libhello.so next to it.
 * There is no JSON library here on purpose: the point is to show the ABI, and
 * the ABI is only strings in and strings out. Use a real parser in anger —
 * cJSON, jansson, yyjson — the contract does not change.
 */
#include <apiplant.h>

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ---- helpers --------------------------------------------------------------- */

/* Copy into freshly malloc'd storage, since the host frees whatever we return
 * through our own apiplant_free. Returns NULL if the allocation fails. */
static char *dup_string(const char *s) {
    size_t n = strlen(s) + 1;
    char *copy = malloc(n);
    if (copy) memcpy(copy, s, n);
    return copy;
}

/* printf into a fresh buffer. Two passes: measure, then format. */
static char *format(const char *fmt, ...) {
    va_list args;
    va_start(args, fmt);
    int needed = vsnprintf(NULL, 0, fmt, args);
    va_end(args);
    if (needed < 0) return NULL;

    char *buffer = malloc((size_t)needed + 1);
    if (!buffer) return NULL;

    va_start(args, fmt);
    vsnprintf(buffer, (size_t)needed + 1, fmt, args);
    va_end(args);
    return buffer;
}

/* The value of a top-level `"key": "..."` pair, decoded into fresh storage.
 *
 * Enough JSON to be correct for flat objects of strings: it decodes the
 * single-character escapes, and returns NULL rather than mangling anything it
 * does not handle. It still finds the key with a substring search, so a key name
 * appearing inside another value would fool it, and it knows nothing of nesting.
 * Use cJSON, jansson or yyjson for real work — none of this is ABI, it is just
 * what a C function needs before it can do anything.
 *
 * Returns NULL when the key is absent, is not a string, uses a `\uXXXX` escape,
 * or the value is unterminated. */
static char *json_string(const char *json, const char *key) {
    char pattern[64];
    snprintf(pattern, sizeof pattern, "\"%s\"", key);

    const char *at = strstr(json, pattern);
    if (!at) return NULL;

    at = strchr(at + strlen(pattern), ':');
    if (!at) return NULL;
    at++;
    while (*at == ' ' || *at == '\t' || *at == '\n' || *at == '\r') at++;
    if (*at != '"') return NULL; /* present, but not a string */
    at++;

    /* Decoding only ever shortens, so the remaining input is a safe bound. */
    char *value = malloc(strlen(at) + 1);
    if (!value) return NULL;

    char *w = value;
    while (*at && *at != '"') {
        if (*at != '\\') {
            *w++ = *at++;
            continue;
        }
        at++;
        switch (*at) {
        case '"':
        case '\\':
        case '/': *w++ = *at; break;
        case 'n': *w++ = '\n'; break;
        case 't': *w++ = '\t'; break;
        case 'r': *w++ = '\r'; break;
        case 'b': *w++ = '\b'; break;
        case 'f': *w++ = '\f'; break;
        /* \uXXXX needs UTF-16 surrogate handling; refuse instead of guessing. */
        default: free(value); return NULL;
        }
        at++;
    }
    if (*at != '"') { /* ran off the end without closing the string */
        free(value);
        return NULL;
    }
    *w = '\0';
    return value;
}

/* Escape the few characters that would break out of a JSON string. Keeps the
 * responses below well-formed even when the caller sends a quote. */
static char *json_escape(const char *s) {
    /* Two bytes is the worst case for anything we rewrite, one for the rest. */
    size_t out = 0;
    for (const char *p = s; *p; p++)
        out += (*p == '"' || *p == '\\' || (unsigned char)*p < 0x20) ? 2 : 1;

    char *escaped = malloc(out + 1);
    if (!escaped) return NULL;

    char *w = escaped;
    for (const char *p = s; *p; p++) {
        if (*p == '"' || *p == '\\') {
            *w++ = '\\';
            *w++ = *p;
        } else if (*p == '\n') {
            *w++ = '\\';
            *w++ = 'n';
        } else if (*p == '\r') {
            *w++ = '\\';
            *w++ = 'r';
        } else if (*p == '\t') {
            *w++ = '\\';
            *w++ = 't';
        } else if ((unsigned char)*p < 0x20) {
            /* Remaining control characters would need \u00XX; a space keeps the
             * output valid JSON, which is all this example needs. */
            *w++ = ' ';
        } else {
            *w++ = *p;
        }
    }
    *w = '\0';
    return escaped;
}

/* ---- the manifest ---------------------------------------------------------- */

/* Static, because the host never frees it. `visibility` defaults to "private",
 * so both entries state it explicitly. */
static const char *const MANIFEST =
    "["
    "  {"
    "    \"name\": \"hello\","
    "    \"version\": \"1.0.0\","
    "    \"description\": \"Greets someone from C.\","
    "    \"visibility\": \"public\","
    "    \"method\": \"POST\","
    "    \"input_schema\": {"
    "      \"type\": \"object\","
    "      \"required\": [\"name\"],"
    "      \"properties\": { \"name\": {"
    "        \"type\": \"string\", \"description\": \"Who to greet.\" } }"
    "    },"
    "    \"output_schema\": {"
    "      \"type\": \"object\","
    "      \"properties\": {"
    "        \"message\":     { \"type\": \"string\" },"
    "        \"compiled_by\": { \"type\": \"string\" }"
    "      }"
    "    }"
    "  },"
    "  {"
    "    \"name\": \"notes\","
    "    \"version\": \"1.0.0\","
    "    \"description\": \"Counts notes, to show a query from C.\","
    "    \"visibility\": \"authenticated\","
    "    \"method\": \"GET\","
    "    \"output_schema\": {"
    "      \"type\": \"object\","
    "      \"properties\": {"
    "        \"notes\":  { \"type\": \"integer\" },"
    "        \"caller\": { \"type\": \"string\"  }"
    "      }"
    "    }"
    "  }"
    "]";

uint32_t apiplant_abi_version(void) { return APIPLANT_ABI_VERSION; }

const char *apiplant_manifest(void) { return MANIFEST; }

void apiplant_free(char *string) { free(string); }

/* ---- hello ----------------------------------------------------------------- */

static int32_t hello(const char *input_json, const ApiplantHost *host, char **out) {
    char *name = json_string(input_json, "name");
    if (!name) {
        *out = dup_string("`name` is required and must be a string");
        return APIPLANT_ERR_REQUEST;
    }

    /* functions/hello.toml, converted to JSON by the host. */
    char *config = host->config(host->ctx);
    char *greeting = config ? json_string(config, "greeting") : NULL;
    if (config) host->free_string(host->ctx, config);

    char *safe_name = json_escape(name);
    free(name);

    if (!safe_name) {
        free(greeting);
        *out = dup_string("out of memory");
        return APIPLANT_ERR_INTERNAL;
    }

    host->log(host->ctx, APIPLANT_INFO, "hello invoked from C");

    *out = format("{\"message\":\"%s, %s!\",\"compiled_by\":\"%s\"}",
                  greeting ? greeting : "Hello", safe_name,
#if defined(__clang__)
                  "clang"
#elif defined(__GNUC__)
                  "gcc"
#else
                  "cc"
#endif
    );
    free(safe_name);
    free(greeting);

    if (!*out) return APIPLANT_ERR_INTERNAL;
    return APIPLANT_OK;
}

/* ---- notes ----------------------------------------------------------------- */

static int32_t notes(const ApiplantHost *host, char **out) {
    /* Same request shape the Rust side uses: {"sql": ..., "params": [...]}.
     * A SELECT comes back as a JSON array of row objects. */
    char *rows = host->query(
        host->ctx,
        "{\"sql\":\"SELECT count(*)::int AS n FROM apiplant_note\",\"params\":[]}");
    if (!rows) {
        *out = dup_string("query returned nothing");
        return APIPLANT_ERR_INTERNAL;
    }

    /* An object with "error" means the query failed; rows arrive as an array. */
    char *failure = json_string(rows, "error");
    if (failure) {
        *out = format("query failed: %s", failure);
        free(failure);
        host->free_string(host->ctx, rows);
        return APIPLANT_ERR_INTERNAL;
    }

    /* [{"n":3}] — find the number after the field name. */
    long count = 0;
    const char *n = strstr(rows, "\"n\"");
    if (n && (n = strchr(n, ':'))) count = strtol(n + 1, NULL, 10);
    host->free_string(host->ctx, rows);

    char *caller = host->principal_id(host->ctx);
    char *safe_caller = json_escape(caller ? caller : "");
    if (caller) host->free_string(host->ctx, caller);

    *out = format("{\"notes\":%ld,\"caller\":\"%s\"}", count,
                  safe_caller ? safe_caller : "");
    free(safe_caller);

    if (!*out) return APIPLANT_ERR_INTERNAL;
    return APIPLANT_OK;
}

/* ---- dispatch -------------------------------------------------------------- */

/* One library, several functions — the host passes the manifest name so this
 * routes on it, exactly as the Rust `functions!` macro does behind the scenes. */
int32_t apiplant_invoke(const char *name, const char *input_json,
                        const ApiplantHost *host, char **out) {
    *out = NULL;

    if (strcmp(name, "hello") == 0) return hello(input_json, host, out);
    if (strcmp(name, "notes") == 0) return notes(host, out);

    *out = format("no function named `%s` in this library", name);
    return APIPLANT_ERR_INTERNAL;
}
