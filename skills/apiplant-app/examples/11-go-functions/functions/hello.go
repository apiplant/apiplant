// A complete apiplant function in Go.
//
// Two endpoints from one file:
//
//	POST /api/functions/hello   public         — greets someone, using config
//	GET  /api/functions/notes   authenticated  — counts rows via the host's DB
//
// `apiplant build` wraps this in a generated module and runs
// `go build -buildmode=c-shared`, dropping libhello.so next to it.
//
// Unlike the C and Zig examples, this one uses a real JSON library — Go has one
// in the standard library, so there is no reason not to. The ABI is unchanged:
// strings in, strings out.
package main

/*
// cgo generates its own prototypes for the symbols exported below, and they
// disagree with the header's `const` qualifiers — so take the types and
// constants and skip the declarations.
#define APIPLANT_NO_PROTOTYPES
#include <apiplant.h>
#include <stdlib.h>

// cgo cannot call a C function pointer directly, so the host's callbacks need
// these one-line shims. Keeping them `static` is what lets them live in the
// preamble of a file that also uses //export.
static char *ap_query(const ApiplantHost *h, const char *req) { return h->query(h->ctx, req); }
static void  ap_log(const ApiplantHost *h, int32_t lvl, const char *m) { h->log(h->ctx, lvl, m); }
static char *ap_config(const ApiplantHost *h)       { return h->config(h->ctx); }
static char *ap_principal_id(const ApiplantHost *h) { return h->principal_id(h->ctx); }
static void  ap_free_string(const ApiplantHost *h, char *s) { h->free_string(h->ctx, s); }
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"runtime/debug"
	"unsafe"
)

// ---- the manifest ----------------------------------------------------------

// Built once at load time. Only "name" is required; "visibility" defaults to
// "private", so both entries state it explicitly.
var manifest = []map[string]any{
	{
		"name":        "hello",
		"version":     "1.0.0",
		"description": "Greets someone from Go.",
		"visibility":  "public",
		"method":      "POST",
		"input_schema": map[string]any{
			"type":     "object",
			"required": []string{"name"},
			"properties": map[string]any{
				"name": map[string]any{"type": "string", "description": "Who to greet."},
			},
		},
		"output_schema": map[string]any{
			"type": "object",
			"properties": map[string]any{
				"message":     map[string]any{"type": "string"},
				"compiled_by": map[string]any{"type": "string"},
			},
		},
	},
	{
		"name":        "notes",
		"version":     "1.0.0",
		"description": "Counts notes, to show a query from Go.",
		"visibility":  "authenticated",
		"method":      "GET",
		"output_schema": map[string]any{
			"type": "object",
			"properties": map[string]any{
				"notes":  map[string]any{"type": "integer"},
				"caller": map[string]any{"type": "string"},
			},
		},
	},
}

// The host never frees what apiplant_manifest returns, so this is allocated once
// and kept alive by the package-level reference.
var manifestC *C.char

func init() {
	encoded, err := json.Marshal(manifest)
	if err != nil {
		// Unreachable for the literal above, and there is no way to report a
		// failure from here — an empty array makes the load fail cleanly.
		encoded = []byte("[]")
	}
	manifestC = C.CString(string(encoded))
}

//export apiplant_abi_version
func apiplant_abi_version() C.uint32_t {
	return C.uint32_t(C.APIPLANT_ABI_VERSION)
}

//export apiplant_manifest
func apiplant_manifest() *C.char {
	return manifestC
}

//export apiplant_free
func apiplant_free(s *C.char) {
	C.free(unsafe.Pointer(s))
}

// ---- talking to the host ---------------------------------------------------

// hostString calls one of the host's string-returning callbacks and copies the
// result into Go memory, handing the original straight back — the host owns what
// it returns, so every call has to be paired with free_string.
func hostString(host *C.ApiplantHost, get func(*C.ApiplantHost) *C.char) string {
	raw := get(host)
	if raw == nil {
		return ""
	}
	defer C.ap_free_string(host, raw)
	return C.GoString(raw)
}

func hostLog(host *C.ApiplantHost, level int32, message string) {
	c := C.CString(message)
	defer C.free(unsafe.Pointer(c))
	C.ap_log(host, C.int32_t(level), c)
}

// query runs SQL through the host and returns the raw JSON reply.
func query(host *C.ApiplantHost, sql string) (string, error) {
	request, err := json.Marshal(map[string]any{"sql": sql, "params": []any{}})
	if err != nil {
		return "", err
	}
	c := C.CString(string(request))
	defer C.free(unsafe.Pointer(c))

	raw := C.ap_query(host, c)
	if raw == nil {
		return "", fmt.Errorf("query returned nothing")
	}
	defer C.ap_free_string(host, raw)
	return C.GoString(raw), nil
}

// ---- hello -----------------------------------------------------------------

type helloInput struct {
	Name *string `json:"name"`
}

func hello(input string, host *C.ApiplantHost) (string, int32) {
	var in helloInput
	if err := json.Unmarshal([]byte(input), &in); err != nil {
		return fmt.Sprintf("invalid input: %v", err), C.APIPLANT_ERR_REQUEST
	}
	if in.Name == nil {
		return "`name` is required and must be a string", C.APIPLANT_ERR_REQUEST
	}

	// functions/hello.toml, converted to JSON by the host.
	greeting := "Hello"
	var config struct {
		Greeting *string `json:"greeting"`
	}
	raw := hostString(host, func(h *C.ApiplantHost) *C.char { return C.ap_config(h) })
	if err := json.Unmarshal([]byte(raw), &config); err == nil && config.Greeting != nil {
		greeting = *config.Greeting
	}

	hostLog(host, C.APIPLANT_INFO, "hello invoked from Go")

	body, err := json.Marshal(map[string]any{
		"message":     fmt.Sprintf("%s, %s!", greeting, *in.Name),
		"compiled_by": compiler(),
	})
	if err != nil {
		return err.Error(), C.APIPLANT_ERR_INTERNAL
	}
	return string(body), C.APIPLANT_OK
}

// compiler reports the toolchain, read from the build info the linker embeds.
func compiler() string {
	if info, ok := debug.ReadBuildInfo(); ok && info.GoVersion != "" {
		return info.GoVersion
	}
	return "go"
}

// ---- notes -----------------------------------------------------------------

func notes(host *C.ApiplantHost) (string, int32) {
	rows, err := query(host, "SELECT count(*)::int AS n FROM apiplant_note")
	if err != nil {
		return err.Error(), C.APIPLANT_ERR_INTERNAL
	}

	// An object with "error" means the query failed; rows arrive as an array.
	var failure struct {
		Error *string `json:"error"`
	}
	if err := json.Unmarshal([]byte(rows), &failure); err == nil && failure.Error != nil {
		return fmt.Sprintf("query failed: %s", *failure.Error), C.APIPLANT_ERR_INTERNAL
	}

	var counted []struct {
		N int64 `json:"n"`
	}
	if err := json.Unmarshal([]byte(rows), &counted); err != nil {
		return fmt.Sprintf("unexpected query reply: %v", err), C.APIPLANT_ERR_INTERNAL
	}

	var count int64
	if len(counted) > 0 {
		count = counted[0].N
	}

	body, err := json.Marshal(map[string]any{
		"notes":  count,
		"caller": hostString(host, func(h *C.ApiplantHost) *C.char { return C.ap_principal_id(h) }),
	})
	if err != nil {
		return err.Error(), C.APIPLANT_ERR_INTERNAL
	}
	return string(body), C.APIPLANT_OK
}

// ---- dispatch --------------------------------------------------------------

// One library, several functions — the host passes the manifest name so this
// routes on it, exactly as the Rust `functions!` macro does behind the scenes.
//
//export apiplant_invoke
func apiplant_invoke(
	name *C.char,
	inputJSON *C.char,
	host *C.ApiplantHost,
	out **C.char,
) C.int32_t {
	*out = nil

	// A Go panic must not escape into the host: unwinding out of an exported cgo
	// function crashes the process, so it is caught here and reported as a fault,
	// which the host turns into a 500. This is the same firewall the Rust side
	// puts around every handler.
	body, status := "", int32(C.APIPLANT_ERR_INTERNAL)
	func() {
		defer func() {
			if r := recover(); r != nil {
				body = fmt.Sprintf("panic: %v", r)
				status = C.APIPLANT_ERR_INTERNAL
			}
		}()

		switch which := C.GoString(name); which {
		case "hello":
			body, status = hello(C.GoString(inputJSON), host)
		case "notes":
			body, status = notes(host)
		default:
			body = fmt.Sprintf("no function named `%s` in this library", which)
			status = C.APIPLANT_ERR_INTERNAL
		}
	}()

	// C.CString allocates with malloc, which is what apiplant_free releases.
	*out = C.CString(body)
	return C.int32_t(status)
}

// Required by -buildmode=c-shared; never called.
func main() {}
