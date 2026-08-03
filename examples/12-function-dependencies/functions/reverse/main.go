// The cgo entry point for the `reverse` function.
//
// It exports the four apiplant.h symbols and defers the actual work to
// `reverse`, which lives in strutil.go — two files, one module. The cgo
// preamble mirrors examples/11-go-functions; see that file for why each line is
// needed.
package main

/*
#define APIPLANT_NO_PROTOTYPES
#include <apiplant.h>
#include <stdlib.h>
*/
import "C"

import (
	"encoding/json"
	"runtime/debug"
	"unsafe"
)

var manifest = []map[string]any{
	{
		"name":        "reverse",
		"version":     "1.0.0",
		"description": "Reverses a string, with the helper in a second file.",
		"visibility":  "public",
		"method":      "POST",
		"input_schema": map[string]any{
			"type":     "object",
			"required": []string{"text"},
			"properties": map[string]any{
				"text": map[string]any{"type": "string"},
			},
		},
		"output_schema": map[string]any{
			"type": "object",
			"properties": map[string]any{
				"reversed": map[string]any{"type": "string"},
			},
		},
	},
}

//export apiplant_abi_version
func apiplant_abi_version() C.uint32_t { return C.APIPLANT_ABI_VERSION }

//export apiplant_manifest
func apiplant_manifest() *C.char {
	// Static for the process lifetime; the host never frees it.
	raw, _ := json.Marshal(manifest)
	return C.CString(string(raw))
}

//export apiplant_invoke
func apiplant_invoke(name *C.char, inputJSON *C.char, host *C.ApiplantHost, out **C.char) (code C.int32_t) {
	// A panic escaping an exported cgo function crashes the whole process, so
	// contain it here and report a 500 instead.
	defer func() {
		if r := recover(); r != nil {
			*out = C.CString("panic in reverse: " + string(debug.Stack()))
			code = C.APIPLANT_ERR_INTERNAL
		}
	}()

	var input struct {
		Text string `json:"text"`
	}
	body := C.GoString(inputJSON)
	if body != "" {
		if err := json.Unmarshal([]byte(body), &input); err != nil {
			*out = C.CString("invalid JSON: " + err.Error())
			return C.APIPLANT_ERR_REQUEST
		}
	}

	result, _ := json.Marshal(map[string]string{"reversed": reverse(input.Text)})
	*out = C.CString(string(result))
	return C.APIPLANT_OK
}

//export apiplant_free
func apiplant_free(string *C.char) { C.free(unsafe.Pointer(string)) }

func main() {}
