/**
 * Starting points for a new function, one per language and layout.
 *
 * Each template compiles as-is with `apiplant build` and mounts at
 * `<base>/functions/<name>`. The Rust ones use the `function!` macro; C, Zig and
 * Go export the four C symbols in `crates/apiplant-abi/include/apiplant.h`; and
 * TypeScript imports the `apiplant` module the host provides. All are kept
 * deliberately short — the full-fat versions live in `examples/09`–`12` and
 * `examples/17`.
 */

import { LANGUAGE_EXT, type Language } from "./types";

export type TemplateKind = "endpoint" | "hook";

export interface GeneratedFile {
  path: string;
  text: string;
}

const rustEndpoint = (name: string) => `//! \`${name}\` — an apiplant function.
//!
//! Build it with \`apiplant build <app-dir>\`, which wraps this file in a cdylib
//! crate and drops lib${name}.so beside it. Mounted at POST /functions/${name}.

use apiplant_function::prelude::*;
use serde::{Deserialize, Serialize};

/// Per-deployment configuration, read from \`functions/${name}.toml\`.
/// Use \`Context<()>\` instead if this function needs none.
#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    greeting: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct Input {
    /// Who to greet.
    name: String,
}

#[derive(Serialize, JsonSchema)]
struct Output {
    /// The composed greeting.
    message: String,
}

fn ${name}(ctx: &Context<Config>, input: Input) -> Result<Output, String> {
    ctx.info("${name} invoked");

    // The host's database is one call away:
    //   let rows = ctx.query("SELECT id FROM apiplant_user LIMIT 1", &[])?;
    // and \`ctx.principal_id()\` is the caller.

    let greeting = ctx
        .config()
        .greeting
        .clone()
        .unwrap_or_else(|| "Hello".to_string());

    Ok(Output {
        message: format!("{greeting}, {}!", input.name),
    })
}

apiplant_function::function! {
    name: "${name}",
    description: "Describe what ${name} does — this shows up in the OpenAPI docs.",
    method: Post,
    visibility: Public,   // public | authenticated | role-gated | private
    handler: ${name},
}
`;

const rustHook = (name: string) => `//! \`${name}\` — an apiplant lifecycle hook.
//!
//! Wire it to a resource by naming it in that resource's \`[hooks]\` section, e.g.
//!
//!   [hooks]
//!   before_create = "${name}"
//!
//! Hooks ignore visibility, so \`Private\` keeps it off the HTTP surface while
//! still running inside the CRUD lifecycle.

use apiplant_function::prelude::*;
use serde_json::Value;

fn ${name}(ctx: &Context<()>, input: Value) -> Result<Value, String> {
    // \`hook\` describes the operation: .event, .resource, .url, .method, .query,
    // .authenticated, .principal_id, .organization_id, .role, and the payload
    // accessors .data() / .row() / .rows().
    let Some(hook) = ctx.hook() else {
        // Called over HTTP rather than from the lifecycle.
        return Ok(reply::proceed());
    };
    ctx.info(&format!("{} on {}", hook.event, hook.resource));

    // Reject the request outright:
    //   return Ok(reply::abort(422, "title is required"));

    // Rewrite what gets stored (before_*) or returned (after_*):
    //   return Ok(reply::replace(input));

    let _ = input;
    Ok(reply::proceed())
}

apiplant_function::function! {
    name: "${name}",
    description: "Lifecycle hook.",
    method: Post,
    visibility: Private,
    handler: ${name},
}
`;

const cSource = (name: string) => `/* \`${name}\` — an apiplant function in C.
 *
 * Exports the four symbols in apiplant.h; \`apiplant build\` compiles this with
 * \`cc -shared -fPIC\` and drops lib${name}.so beside it.
 *
 * There is no JSON parser here on purpose — the ABI is strings in, strings out.
 * Reach for cJSON, jansson or yyjson when the payload gets real; see
 * examples/09-c-functions for a version that reads its input and queries the DB.
 */
#include <apiplant.h>

#include <stdlib.h>
#include <string.h>

/* Static: the host never frees the manifest. "visibility" defaults to
 * "private", so state it explicitly. */
static const char *const MANIFEST =
    "[{"
    "\\"name\\": \\"${name}\\","
    "\\"version\\": \\"1.0.0\\","
    "\\"description\\": \\"Describe what ${name} does.\\","
    "\\"visibility\\": \\"public\\","
    "\\"method\\": \\"POST\\","
    "\\"output_schema\\": {"
    "  \\"type\\": \\"object\\","
    "  \\"properties\\": { \\"message\\": { \\"type\\": \\"string\\" } }"
    "}"
    "}]";

uint32_t apiplant_abi_version(void) { return APIPLANT_ABI_VERSION; }

const char *apiplant_manifest(void) { return MANIFEST; }

/* The host hands back whatever we wrote to *out, so free it the way we made it. */
void apiplant_free(char *string) { free(string); }

static char *dup_string(const char *s) {
    size_t n = strlen(s) + 1;
    char *copy = malloc(n);
    if (copy) memcpy(copy, s, n);
    return copy;
}

int32_t apiplant_invoke(const char *name, const char *input_json,
                        const ApiplantHost *host, char **out) {
    (void)input_json; /* the request body, as JSON text */
    *out = NULL;

    if (strcmp(name, "${name}") != 0) {
        *out = dup_string("no such function in this library");
        return APIPLANT_ERR_INTERNAL;
    }

    host->log(host->ctx, APIPLANT_INFO, "${name} invoked from C");

    /* Querying the host, when you need it:
     *   char *rows = host->query(host->ctx,
     *       "{\\"sql\\":\\"SELECT count(*)::int AS n FROM apiplant_user\\",\\"params\\":[]}");
     *   ... then host->free_string(host->ctx, rows);
     */

    *out = dup_string("{\\"message\\":\\"hello from ${name}\\"}");
    return *out ? APIPLANT_OK : APIPLANT_ERR_INTERNAL;
}
`;

const zigSource = (name: string) => `//! \`${name}\` — an apiplant function in Zig.
//!
//! Zig reaches the ABI by \`@cImport\`ing the same apiplant.h a C function
//! includes, so the header is the binding. \`apiplant build\` compiles this with
//! \`zig build-lib -dynamic -lc\` and drops lib${name}.so beside it.
//!
//! See examples/10-zig-functions for reading input and querying the database.

const std = @import("std");

const c = @cImport({
    @cInclude("apiplant.h");
});

/// Everything crossing the boundary is malloc'd, so \`apiplant_free\` is \`free\`.
const allocator = std.heap.c_allocator;

/// Static: the host never frees the manifest. "visibility" defaults to
/// "private", so state it explicitly.
const manifest =
    \\\\[{
    \\\\  "name": "${name}",
    \\\\  "version": "1.0.0",
    \\\\  "description": "Describe what ${name} does.",
    \\\\  "visibility": "public",
    \\\\  "method": "POST",
    \\\\  "output_schema": {
    \\\\    "type": "object",
    \\\\    "properties": { "message": { "type": "string" } }
    \\\\  }
    \\\\}]
;

export fn apiplant_abi_version() u32 {
    return c.APIPLANT_ABI_VERSION;
}

export fn apiplant_manifest() [*:0]const u8 {
    return manifest;
}

export fn apiplant_free(string: ?[*:0]u8) void {
    if (string) |s| std.c.free(s);
}

/// Copy a slice into malloc'd, NUL-terminated storage for the host.
fn toHost(text: []const u8) ?[*:0]u8 {
    const buffer = allocator.allocSentinel(u8, text.len, 0) catch return null;
    @memcpy(buffer, text);
    return buffer.ptr;
}

export fn apiplant_invoke(
    name: [*:0]const u8,
    input_json: [*:0]const u8,
    host: *const c.ApiplantHost,
    out: *?[*:0]u8,
) i32 {
    _ = input_json; // the request body, as JSON text
    out.* = null;

    if (!std.mem.eql(u8, std.mem.span(name), "${name}")) {
        out.* = toHost("no such function in this library");
        return c.APIPLANT_ERR_INTERNAL;
    }

    host.log.?(host.ctx, c.APIPLANT_INFO, "${name} invoked from Zig");

    out.* = toHost("{\\"message\\":\\"hello from ${name}\\"}");
    return if (out.* == null) c.APIPLANT_ERR_INTERNAL else c.APIPLANT_OK;
}
`;

const goSource = (name: string) => `// \`${name}\` — an apiplant function in Go.
//
// \`apiplant build\` wraps this in a generated module and runs
// \`go build -buildmode=c-shared\`, dropping lib${name}.so beside it.
//
// See examples/11-go-functions for reading input, config and the database.
package main

/*
// cgo emits its own prototypes for the exported symbols and they disagree with
// the header's \`const\`, so take the types and constants and skip the
// declarations. The one-line shims exist because cgo cannot call a C function
// pointer directly.
#define APIPLANT_NO_PROTOTYPES
#include <apiplant.h>
#include <stdlib.h>

static void ap_log(const ApiplantHost *h, int32_t lvl, const char *m) { h->log(h->ctx, lvl, m); }
static char *ap_query(const ApiplantHost *h, const char *req) { return h->query(h->ctx, req); }
static char *ap_config(const ApiplantHost *h) { return h->config(h->ctx); }
static char *ap_principal_id(const ApiplantHost *h) { return h->principal_id(h->ctx); }
static void ap_free_string(const ApiplantHost *h, char *s) { h->free_string(h->ctx, s); }
*/
import "C"

import (
	"encoding/json"
	"fmt"
	"unsafe"
)

// Only "name" is required; "visibility" defaults to "private", so state it.
var manifest = []map[string]any{
	{
		"name":        "${name}",
		"version":     "1.0.0",
		"description": "Describe what ${name} does.",
		"visibility":  "public",
		"method":      "POST",
		"output_schema": map[string]any{
			"type":       "object",
			"properties": map[string]any{"message": map[string]any{"type": "string"}},
		},
	},
}

// The host never frees the manifest, so it is allocated once and kept alive by
// this package-level reference.
var manifestC *C.char

func init() {
	encoded, err := json.Marshal(manifest)
	if err != nil {
		encoded = []byte("[]")
	}
	manifestC = C.CString(string(encoded))
}

//export apiplant_abi_version
func apiplant_abi_version() C.uint32_t { return C.uint32_t(C.APIPLANT_ABI_VERSION) }

//export apiplant_manifest
func apiplant_manifest() *C.char { return manifestC }

//export apiplant_free
func apiplant_free(s *C.char) { C.free(unsafe.Pointer(s)) }

func hostLog(host *C.ApiplantHost, level int32, message string) {
	c := C.CString(message)
	defer C.free(unsafe.Pointer(c))
	C.ap_log(host, C.int32_t(level), c)
}

//export apiplant_invoke
func apiplant_invoke(name *C.char, inputJSON *C.char, host *C.ApiplantHost, out **C.char) C.int32_t {
	_ = inputJSON // the request body, as JSON text
	*out = nil

	// A panic escaping an exported cgo function crashes the host, so it is
	// caught here and reported as a fault the host turns into a 500.
	body, status := "", int32(C.APIPLANT_ERR_INTERNAL)
	func() {
		defer func() {
			if r := recover(); r != nil {
				body, status = fmt.Sprintf("panic: %v", r), C.APIPLANT_ERR_INTERNAL
			}
		}()

		switch which := C.GoString(name); which {
		case "${name}":
			hostLog(host, C.APIPLANT_INFO, "${name} invoked from Go")
			encoded, err := json.Marshal(map[string]any{"message": "hello from ${name}"})
			if err != nil {
				body, status = err.Error(), C.APIPLANT_ERR_INTERNAL
				return
			}
			body, status = string(encoded), C.APIPLANT_OK
		default:
			body = fmt.Sprintf("no function named \`%s\` in this library", which)
			status = C.APIPLANT_ERR_INTERNAL
		}
	}()

	// C.CString allocates with malloc, which is what apiplant_free releases.
	*out = C.CString(body)
	return C.int32_t(status)
}

// Required by -buildmode=c-shared; never called.
func main() {}
`;


const typescriptSource = (name: string, kind: TemplateKind) =>
  kind === "hook"
    ? `/**
 * \`${name}\` — an apiplant lifecycle hook in TypeScript.
 *
 * Point a resource's [hooks] at it (models/<resource>.toml), and it runs around
 * that resource's CRUD. \`apiplant build\` strips the types and writes
 * ${name}.js beside this file; the server runs it in a V8 isolate.
 *
 * Return \`{ data }\` to replace the body, throw \`BadRequest\` to reject the
 * request, or return nothing to let it through unchanged.
 */

import { defineFunctions, hook, log, BadRequest } from "apiplant";

export default defineFunctions({
  ${name}: {
    description: "Describe what ${name} guards or fills in.",

    handler() {
      const context = hook();
      if (!context) throw new Error("${name} only runs as a lifecycle hook");

      log.info(\`${name} running for \${context.event}\`);

      const data = context.data ?? {};
      if (typeof data.title === "string" && data.title.trim() === "") {
        throw new BadRequest("title cannot be blank");
      }

      // Nothing to change: let the request through as it was.
      return {};
    },
  },
});
`
    : `/**
 * \`${name}\` — an apiplant function in TypeScript.
 *
 * \`apiplant build\` strips the types and writes ${name}.js beside this file; the
 * server runs it in a V8 isolate. Nothing to install: the \`apiplant\` module is
 * provided by the host, and its types come from the apiplant.d.ts that
 * \`apiplant build\` writes into functions/.
 *
 * Mounted at POST /functions/${name}.
 */

import { config, db, defineFunctions, log, s } from "apiplant";

/**
 * The request body, declared once: it becomes the JSON Schema in the generated
 * docs, the check that runs before the handler, and the type of \`input\`.
 */
const Input = s.object({
  name: s.string({ minLength: 1, description: "Who to greet." }),
});

export default defineFunctions({
  ${name}: {
    version: "1.0.0",
    description: "Describe what ${name} does.",
    permission: "public",
    method: "POST",
    input: Input,
    output: s.object({ message: s.string() }),

    handler(input) {
      // functions/${name}.toml, if there is one.
      const { greeting = "Hello" } = config<{ greeting?: string }>();

      log.info(\`${name} invoked for \${input.name}\`);

      // The host, when you need it — synchronous, no pool to open:
      //   const rows = db.query("SELECT id FROM apiplant_user LIMIT 1");
      //   db.query(sql\`SELECT … WHERE id = \${input.id}\`)  binds its values.

      return { message: \`\${greeting}, \${input.name}!\` };
    },
  },
});
`;

const typescriptPackage = (name: string) => `{
  "name": "${name}",
  "version": "1.0.0",
  "private": true,
  "type": "module",
  "//": "A function directory is an npm project you own. apiplant runs \`install\` once, then \`build\`, and copies the bundle to ../${name}.js.",
  "module": "dist/${name}.js",
  "scripts": {
    "build": "esbuild src/index.ts --bundle --format=esm --platform=neutral --main-fields=module,main --external:apiplant --outfile=dist/${name}.js"
  },
  "dependencies": {},
  "devDependencies": {
    "esbuild": "^0.25.0"
  }
}
`;

const typescriptTsconfig = () => `{
  "//": "For your editor. apiplant builds this directory with the \`build\` script in package.json.",
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "lib": ["ES2022"],
    "types": [],
    "strict": true,
    "noEmit": true,
    "allowImportingTsExtensions": true
  },
  "include": ["src", "../apiplant.d.ts"]
}
`;

const cargoManifest = (name: string) => `# A function directory is a crate you own: \`apiplant build\` runs this manifest
# as written and copies the cdylib it produces to ../lib${name}.so.
[package]
name = "${name}"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
# TODO: point this at your apiplant checkout, or a published version.
apiplant-function = { path = "../../../../crates/apiplant-function" }
abi_stable = "0.11"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
schemars = "0.8"

# Add whatever else this function needs — a directory exists so that it can.

# apiplant build injects these for single-file functions; a directory is your
# crate, so set them yourself to keep the shipped library small.
[profile.dev]
strip = "debuginfo"

[profile.release]
strip = "symbols"
lto = "fat"
codegen-units = 1

# Detach from any surrounding cargo workspace so this crate builds standalone.
[workspace]
`;

const goModule = (name: string) => `// Your own module: bring dependencies in with \`require\` and split the code
// across as many files as you like. apiplant builds it with
// \`go build -buildmode=c-shared\` and copies the result to ../lib${name}.so.
module ${name}

go 1.21
`;

const configTemplate = (name: string) => `# Per-deployment configuration for the \`${name}\` function. The framework
# converts this to JSON and hands it to the function as its typed \`Config\`.
# Delete the file if the function takes no configuration.
greeting = "Hello"
`;

/** Every file a new function starts with. */
export function scaffoldFunction(
  name: string,
  language: Language,
  layout: "file" | "directory",
  kind: TemplateKind,
  withConfig: boolean,
): GeneratedFile[] {
  const files: GeneratedFile[] = [];
  const source = (): string => {
    switch (language) {
      case "rust":
        return kind === "hook" ? rustHook(name) : rustEndpoint(name);
      case "typescript":
        return typescriptSource(name, kind);
      case "c":
        return cSource(name);
      case "zig":
        return zigSource(name);
      case "go":
        return goSource(name);
    }
  };

  if (layout === "file") {
    const ext = LANGUAGE_EXT[language];
    files.push({ path: `functions/${name}.${ext}`, text: source() });
  } else {
    switch (language) {
      case "rust":
        files.push({ path: `functions/${name}/Cargo.toml`, text: cargoManifest(name) });
        files.push({ path: `functions/${name}/src/lib.rs`, text: source() });
        break;
      case "go":
        files.push({ path: `functions/${name}/go.mod`, text: goModule(name) });
        files.push({ path: `functions/${name}/main.go`, text: source() });
        break;
      case "typescript":
        // An npm project: your dependencies, your bundler. apiplant runs the
        // `build` script and copies out what it produced.
        files.push({ path: `functions/${name}/package.json`, text: typescriptPackage(name) });
        files.push({ path: `functions/${name}/tsconfig.json`, text: typescriptTsconfig() });
        files.push({ path: `functions/${name}/src/index.ts`, text: source() });
        break;
      case "c":
        files.push({ path: `functions/${name}/${name}.c`, text: source() });
        break;
      case "zig":
        // The root file must be named for the directory; siblings are @imported.
        files.push({ path: `functions/${name}/${name}.zig`, text: source() });
        break;
    }
  }

  if (withConfig) files.push({ path: `functions/${name}.toml`, text: configTemplate(name) });
  return files;
}

/** A default `main.toml` for a directory that doesn't have one yet. */
export function scaffoldMainToml(appName: string): string {
  return `# apiplant configuration. Every key is optional — delete the file and the
# server still boots with safe defaults.

[app]
name = "${appName}"

[server]
host = "127.0.0.1"
port = 8099
base_path = "/api"

[database]
url = "postgres://postgres@127.0.0.1:55432/${appName.replace(/[^a-zA-Z0-9_]/g, "_")}"
auto_migrate = true

[auth]
jwt_secret = "change-me-in-production"
allow_registration = true

[docs]
enabled = true
path = "/docs"
title = "${appName}"
`;
}
