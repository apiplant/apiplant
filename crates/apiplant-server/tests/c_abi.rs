//! End-to-end test of the [plain C ABI](apiplant_abi::c).
//!
//! Compiles a C fixture with the system compiler, loads it the way the server
//! does, and drives it through every part of the contract: each status code,
//! every host callback, and the two-way ownership rules for strings. Unit tests
//! can check manifest parsing, but only a real shared library exercises the
//! marshalling and the allocator boundary.
//!
//! The last two tests build the *same* contract from Zig and Go, which is the
//! only way to know the ABI is genuinely language-agnostic rather than
//! accidentally C-shaped. Go is the interesting one: its runtime has to survive
//! being `dlopen`ed and called from the host's blocking thread pool.
//!
//! Every test skips (rather than fails) when its toolchain is missing, since none
//! of them are needed to build or test apiplant itself.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{BoxedFunction, HostApi, HostApi_TO, LogLevel};

/// Exercises the whole contract: config, query, log, principal_id, hook,
/// send_email, cache, every status code, and freeing what the host hands over.
const FIXTURE: &str = r#"
#include <apiplant.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

static char *dup_string(const char *s) {
    size_t n = strlen(s) + 1;
    char *c = malloc(n);
    if (c) memcpy(c, s, n);
    return c;
}

static const char *const MANIFEST =
    "[{\"name\":\"echo\",\"version\":\"2.1.0\",\"description\":\"Echoes.\","
    "  \"visibility\":\"public\",\"method\":\"POST\","
    "  \"input_schema\":{\"type\":\"object\"}},"
    " {\"name\":\"gated\",\"visibility\":\"role:admin\",\"method\":\"GET\"},"
    " {\"name\":\"notify\",\"visibility\":\"public\",\"method\":\"POST\"}]";

uint32_t apiplant_abi_version(void) { return APIPLANT_ABI_VERSION; }
const char *apiplant_manifest(void) { return MANIFEST; }
void apiplant_free(char *s) { free(s); }

int32_t apiplant_invoke(const char *name, const char *input,
                        const ApiplantHost *host, char **out) {
    *out = NULL;

    if (strcmp(name, "echo") == 0) {
        /* "bad" and "boom" select the two error channels. */
        if (strcmp(input, "\"bad\"") == 0) {
            *out = dup_string("that input was unacceptable");
            return APIPLANT_ERR_REQUEST;
        }
        if (strcmp(input, "\"boom\"") == 0) {
            *out = dup_string("internals: /etc/secret missing");
            return APIPLANT_ERR_INTERNAL;
        }
        /* Returning without setting *out must not crash the host. */
        if (strcmp(input, "\"silent\"") == 0) return APIPLANT_OK;

        host->log(host->ctx, APIPLANT_WARN, "echo was called");

        char *config = host->config(host->ctx);
        char *principal = host->principal_id(host->ctx);
        char *hook = host->hook(host->ctx);
        char *rows = host->query(host->ctx, "{\"sql\":\"SELECT 1\",\"params\":[]}");

        size_t n = strlen(input) + strlen(config) + strlen(principal)
                 + strlen(hook) + strlen(rows) + 128;
        char *body = malloc(n);
        snprintf(body, n,
                 "{\"input\":%s,\"config\":%s,\"principal\":\"%s\","
                 "\"hook_len\":%zu,\"rows\":%s}",
                 input, config, principal, strlen(hook), rows);

        /* Everything the host handed us goes back through free_string. */
        host->free_string(host->ctx, config);
        host->free_string(host->ctx, principal);
        host->free_string(host->ctx, hook);
        host->free_string(host->ctx, rows);

        *out = body;
        return APIPLANT_OK;
    }

    /* The two services the host lends beyond the database. */
    if (strcmp(name, "notify") == 0) {
        char *receipt = host->send_email(host->ctx,
            "{\"to\":\"ann@example.com\",\"subject\":\"Hi\",\"text\":\"Hello\"}");
        char *cached = host->cache(host->ctx, "{\"op\":\"get\",\"key\":\"hits\"}");

        size_t n = strlen(receipt) + strlen(cached) + 64;
        char *body = malloc(n);
        snprintf(body, n, "{\"sent\":%s,\"cached\":%s}", receipt, cached);

        host->free_string(host->ctx, receipt);
        host->free_string(host->ctx, cached);

        *out = body;
        return APIPLANT_OK;
    }

    if (strcmp(name, "gated") == 0) {
        *out = dup_string("{\"ok\":true}");
        return APIPLANT_OK;
    }

    *out = dup_string("unknown function");
    return APIPLANT_ERR_INTERNAL;
}
"#;

/// A library that reports an ABI version the host cannot speak.
const WRONG_VERSION: &str = r#"
#include <stdint.h>
uint32_t apiplant_abi_version(void) { return 9999u; }
const char *apiplant_manifest(void) { return "[]"; }
void apiplant_free(char *s) { (void)s; }
int32_t apiplant_invoke(const char *n, const char *i, const void *h, char **o) {
    (void)n; (void)i; (void)h; (void)o; return 2;
}
"#;

/// Declares the version symbol but nothing else — a broken C-ABI library, which
/// must be reported rather than mistaken for "not a C library".
const INCOMPLETE: &str = r#"
#include <stdint.h>
uint32_t apiplant_abi_version(void) { return 1u; }
"#;

struct MockHost {
    logs: Mutex<Vec<(LogLevel, String)>>,
    queries: Mutex<Vec<String>>,
    emails: Mutex<Vec<String>>,
    cache_ops: Mutex<Vec<String>>,
    payment_ops: Mutex<Vec<String>>,
    chats: Mutex<Vec<String>>,
    chunks: Mutex<Vec<String>>,
    published: Mutex<Vec<String>>,
    hook_json: String,
}

impl MockHost {
    fn new() -> Self {
        MockHost {
            logs: Mutex::new(Vec::new()),
            queries: Mutex::new(Vec::new()),
            emails: Mutex::new(Vec::new()),
            cache_ops: Mutex::new(Vec::new()),
            payment_ops: Mutex::new(Vec::new()),
            chats: Mutex::new(Vec::new()),
            chunks: Mutex::new(Vec::new()),
            published: Mutex::new(Vec::new()),
            hook_json: String::new(),
        }
    }
}

impl HostApi for MockHost {
    fn query(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.queries.lock().unwrap().push(request.as_str().into());
        RResult::ROk(RString::from(r#"[{"n":7}]"#))
    }
    fn log(&self, level: LogLevel, message: RStr<'_>) {
        self.logs
            .lock()
            .unwrap()
            .push((level, message.as_str().into()));
    }
    fn send_email(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.emails.lock().unwrap().push(request.as_str().into());
        RResult::ROk(RString::from(
            r#"{"provider":"smtp","id":"msg-1","recipients":1}"#,
        ))
    }
    fn cache(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.cache_ops.lock().unwrap().push(request.as_str().into());
        RResult::ROk(RString::from(r#"{"hit":true,"value":42}"#))
    }
    fn payments(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.payment_ops
            .lock()
            .unwrap()
            .push(request.as_str().into());
        RResult::ROk(RString::from(r#"{"url":"https://checkout.stripe.test/s"}"#))
    }
    fn ai(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.chats.lock().unwrap().push(request.as_str().into());
        RResult::ROk(RString::from(
            r#"{"text":"hello","provider":"custom","model":"local"}"#,
        ))
    }
    fn publish(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.published.lock().unwrap().push(request.as_str().into());
        RResult::ROk(RString::from(
            r#"{"id":"m-1","topic":"order.paid","delivered":1}"#,
        ))
    }
    fn emit(&self, chunk: RStr<'_>) -> bool {
        self.chunks.lock().unwrap().push(chunk.as_str().into());
        true
    }
    fn config(&self) -> RString {
        RString::from(r#"{"greeting":"Ciao"}"#)
    }
    fn principal_id(&self) -> RString {
        RString::from("user-42")
    }
    fn hook(&self) -> RString {
        RString::from(self.hook_json.as_str())
    }
}

/// A [`MockHost`] the test still holds after the ABI has taken ownership —
/// `invoke` consumes its host, and the recordings are the point.
struct SharedHost(std::sync::Arc<MockHost>);

impl HostApi for SharedHost {
    fn query(&self, r: RStr<'_>) -> RResult<RString, RString> {
        self.0.query(r)
    }
    fn log(&self, l: LogLevel, m: RStr<'_>) {
        self.0.log(l, m)
    }
    fn send_email(&self, r: RStr<'_>) -> RResult<RString, RString> {
        self.0.send_email(r)
    }
    fn cache(&self, r: RStr<'_>) -> RResult<RString, RString> {
        self.0.cache(r)
    }
    fn payments(&self, r: RStr<'_>) -> RResult<RString, RString> {
        self.0.payments(r)
    }
    fn ai(&self, r: RStr<'_>) -> RResult<RString, RString> {
        self.0.ai(r)
    }
    fn publish(&self, r: RStr<'_>) -> RResult<RString, RString> {
        self.0.publish(r)
    }
    fn emit(&self, chunk: RStr<'_>) -> bool {
        self.0.emit(chunk)
    }
    fn config(&self) -> RString {
        self.0.config()
    }
    fn principal_id(&self) -> RString {
        self.0.principal_id()
    }
    fn hook(&self) -> RString {
        self.0.hook()
    }
}

/// A host that fails every query, to check the `{"error": …}` envelope.
struct FailingHost;
impl HostApi for FailingHost {
    fn query(&self, _r: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr(RString::from("relation does not exist"))
    }
    fn log(&self, _l: LogLevel, _m: RStr<'_>) {}
    fn send_email(&self, _r: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr(RString::from("no email provider configured"))
    }
    fn cache(&self, _r: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr(RString::from("no cache configured"))
    }
    fn payments(&self, _r: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr(RString::from("no payment provider configured"))
    }
    fn ai(&self, _r: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr(RString::from("no ai provider configured"))
    }
    fn publish(&self, _r: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr(RString::from("no queue in this test"))
    }
    fn emit(&self, _chunk: RStr<'_>) -> bool {
        false
    }
    fn config(&self) -> RString {
        RString::from("{}")
    }
    fn principal_id(&self) -> RString {
        RString::new()
    }
    fn hook(&self) -> RString {
        RString::new()
    }
}

fn scratch(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "apiplant-cabi-{}-{label}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn include_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../apiplant-abi/include")
}

/// Compile `source` into a shared library, or `None` when no compiler is around.
fn compile(dir: &Path, source: &str) -> Option<PathBuf> {
    let c_file = dir.join("fixture.c");
    let library = dir.join(if cfg!(target_os = "macos") {
        "libfixture.dylib"
    } else {
        "libfixture.so"
    });
    std::fs::write(&c_file, source).unwrap();

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".into());
    let output = Command::new(&cc)
        .arg(if cfg!(target_os = "macos") {
            "-dynamiclib"
        } else {
            "-shared"
        })
        .arg("-fPIC")
        .arg("-I")
        .arg(include_dir())
        .arg("-o")
        .arg(&library)
        .arg(&c_file)
        .output()
        .ok()?;

    if !output.status.success() {
        panic!(
            "fixture failed to compile:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Some(library)
}

/// `load`'s success type isn't `Debug`, so `unwrap_err` is unavailable.
fn load_error(library: &Path) -> String {
    match apiplant_server::cabi::load(library) {
        Err(e) => e,
        Ok(_) => panic!("expected the library to be refused"),
    }
}

fn load_one(library: &Path, name: &str) -> BoxedFunction {
    let functions = apiplant_server::cabi::load(library)
        .expect("library should load")
        .expect("library should speak the C ABI");
    functions
        .into_iter()
        .find(|f| f.manifest().name.as_str() == name)
        .unwrap_or_else(|| panic!("no function named `{name}`"))
}

fn invoke(f: &BoxedFunction, host: impl HostApi + 'static, input: &str) -> Result<String, String> {
    let host = HostApi_TO::from_value(host, TD_Opaque);
    match f.invoke(host, RStr::from_str(input)) {
        RResult::ROk(s) => Ok(s.into_string()),
        RResult::RErr(e) => Err(e.into_string()),
    }
}

#[test]
fn a_c_library_serves_requests_and_reaches_every_host_callback() {
    let dir = scratch("ok");
    let Some(library) = compile(&dir, FIXTURE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };

    let echo = load_one(&library, "echo");

    // The manifest crossed intact.
    let manifest = echo.manifest();
    assert_eq!(manifest.name.as_str(), "echo");
    assert_eq!(manifest.version.as_str(), "2.1.0");
    assert_eq!(manifest.description.as_str(), "Echoes.");
    assert_eq!(manifest.visibility, apiplant_abi::Visibility::Public);
    assert_eq!(manifest.method, apiplant_abi::HttpMethod::Post);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(manifest.input_schema.as_str()).unwrap(),
        serde_json::json!({ "type": "object" })
    );

    // A second function from the same library, with its own visibility.
    let gated = load_one(&library, "gated");
    assert_eq!(
        gated.manifest().visibility,
        apiplant_abi::Visibility::RoleGated
    );
    assert_eq!(gated.manifest().role.as_str(), "admin");
    assert_eq!(gated.manifest().method, apiplant_abi::HttpMethod::Get);
    // An absent version falls back rather than failing the load.
    assert_eq!(gated.manifest().version.as_str(), "0.0.0");

    let body = invoke(&echo, MockHost::new(), r#"{"name":"Ann"}"#).expect("should succeed");
    let body: serde_json::Value = serde_json::from_str(&body).expect("valid JSON came back");

    assert_eq!(body["input"], serde_json::json!({ "name": "Ann" }));
    assert_eq!(body["config"], serde_json::json!({ "greeting": "Ciao" }));
    assert_eq!(body["principal"], "user-42");
    // Not a hook invocation, so the hook context is the empty string.
    assert_eq!(body["hook_len"], 0);
    assert_eq!(body["rows"], serde_json::json!([{ "n": 7 }]));

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_two_error_codes_map_to_caller_faults_and_function_faults() {
    let dir = scratch("errors");
    let Some(library) = compile(&dir, FIXTURE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };
    let echo = load_one(&library, "echo");

    // ERR_REQUEST: the message is the caller's to see, so it travels unmarked
    // and the host answers 400.
    let err = invoke(&echo, MockHost::new(), r#""bad""#).unwrap_err();
    assert_eq!(err, "that input was unacceptable");
    assert!(!err.starts_with(apiplant_abi::INTERNAL_ERROR_PREFIX));

    // ERR_INTERNAL: marked, so the host logs it and answers a generic 500
    // instead of echoing internals back.
    let err = invoke(&echo, MockHost::new(), r#""boom""#).unwrap_err();
    let detail = err
        .strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX)
        .expect("an internal fault must be marked");
    assert_eq!(detail, "internals: /etc/secret missing");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_function_that_sets_no_output_yields_an_empty_body() {
    let dir = scratch("silent");
    let Some(library) = compile(&dir, FIXTURE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };
    let echo = load_one(&library, "echo");

    // Forgetting to set *out must not crash the host or the request.
    assert_eq!(invoke(&echo, MockHost::new(), r#""silent""#).unwrap(), "");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_failed_query_reaches_c_as_an_error_envelope() {
    let dir = scratch("query-err");
    let Some(library) = compile(&dir, FIXTURE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };
    let echo = load_one(&library, "echo");

    // The fixture splices the query result straight into its response, so a
    // failure has to arrive as `{"error": …}` rather than as a null pointer.
    let body = invoke(&echo, FailingHost, r#"{"name":"Ann"}"#).unwrap();
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        body["rows"],
        serde_json::json!({ "error": "relation does not exist" })
    );

    std::fs::remove_dir_all(dir).unwrap();
}

/// The email and cache callbacks, driven from C. They were appended to
/// `ApiplantHost` after the original five, so this is also the check that
/// appending didn't disturb the offsets of the fields already there.
#[test]
fn a_c_function_can_send_email_and_use_the_cache() {
    let dir = scratch("services");
    let Some(library) = compile(&dir, FIXTURE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };
    let notify = load_one(&library, "notify");

    let host = std::sync::Arc::new(MockHost::new());
    let body = invoke(&notify, SharedHost(host.clone()), "{}").expect("should succeed");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(body["sent"]["provider"], "smtp");
    assert_eq!(body["sent"]["id"], "msg-1");
    assert_eq!(body["cached"]["value"], 42);

    let emails = host.emails.lock().unwrap();
    assert_eq!(emails.len(), 1);
    let sent: serde_json::Value = serde_json::from_str(&emails[0]).unwrap();
    assert_eq!(sent["to"], "ann@example.com");
    assert_eq!(sent["subject"], "Hi");

    let ops = host.cache_ops.lock().unwrap();
    assert_eq!(ops.len(), 1);
    assert!(ops[0].contains(r#""op":"get""#), "{}", ops[0]);

    std::fs::remove_dir_all(dir).unwrap();
}

/// A failing service must reach C as the in-band `{"error": …}` envelope, not
/// as a null pointer the fixture would dereference.
#[test]
fn a_service_failure_reaches_c_as_an_error_object() {
    let dir = scratch("services-fail");
    let Some(library) = compile(&dir, FIXTURE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };
    let notify = load_one(&library, "notify");

    let body = invoke(&notify, FailingHost, "{}").expect("the call itself should succeed");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["sent"]["error"], "no email provider configured");
    assert_eq!(body["cached"]["error"], "no cache configured");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn many_invocations_do_not_leak_or_corrupt_state() {
    let dir = scratch("repeat");
    let Some(library) = compile(&dir, FIXTURE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };
    let echo = load_one(&library, "echo");

    // Each call allocates on both sides and frees across the boundary; a mistake
    // in the ownership rules tends to show up as corruption once repeated.
    for i in 0..200 {
        let input = format!(r#"{{"name":"caller-{i}"}}"#);
        let body = invoke(&echo, MockHost::new(), &input).expect("should succeed");
        let body: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["input"]["name"], format!("caller-{i}"));
    }

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn host_callbacks_are_reached_with_the_right_level_and_sql() {
    let dir = scratch("callbacks");
    let Some(library) = compile(&dir, FIXTURE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };
    let echo = load_one(&library, "echo");

    // Keep the mock so its recordings can be inspected after the call.
    let host = std::sync::Arc::new(MockHost::new());
    invoke(&echo, SharedHost(host.clone()), r#"{"name":"Ann"}"#).unwrap();

    let logs = host.logs.lock().unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].0, LogLevel::Warn, "APIPLANT_WARN must map to Warn");
    assert_eq!(logs[0].1, "echo was called");

    let queries = host.queries.lock().unwrap();
    assert_eq!(queries.len(), 1);
    assert!(queries[0].contains("SELECT 1"), "{}", queries[0]);

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_library_from_a_future_abi_is_refused_by_version() {
    let dir = scratch("version");
    let Some(library) = compile(&dir, WRONG_VERSION) else {
        eprintln!("skipping: no C compiler available");
        return;
    };

    let err = load_error(&library);
    assert!(err.contains("9999"), "{err}");
    assert!(err.contains("this host speaks"), "{err}");

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_c_library_missing_the_other_symbols_is_an_error_not_a_miss() {
    let dir = scratch("incomplete");
    let Some(library) = compile(&dir, INCOMPLETE) else {
        eprintln!("skipping: no C compiler available");
        return;
    };

    // It claims the C ABI, so a missing `apiplant_manifest` is a real failure —
    // not `Ok(None)`, which would hide it behind the abi_stable error instead.
    let err = load_error(&library);
    assert!(err.contains("apiplant_manifest"), "{err}");

    std::fs::remove_dir_all(dir).unwrap();
}

// ---- the same contract, from other languages -------------------------------

/// A Zig function reaching the ABI through `@cImport`.
const ZIG_FIXTURE: &str = r#"
const std = @import("std");
const c = @cImport({ @cInclude("apiplant.h"); });

const manifest = "[{\"name\":\"zecho\",\"version\":\"3.0.0\",\"visibility\":\"public\"}]";

export fn apiplant_abi_version() u32 { return c.APIPLANT_ABI_VERSION; }
export fn apiplant_manifest() [*:0]const u8 { return manifest; }
export fn apiplant_free(s: ?[*:0]u8) void { if (s) |p| std.c.free(p); }

fn toHost(text: []const u8) ?[*:0]u8 {
    const buf = std.heap.c_allocator.allocSentinel(u8, text.len, 0) catch return null;
    @memcpy(buf, text);
    return buf.ptr;
}

export fn apiplant_invoke(
    name: [*:0]const u8,
    input: [*:0]const u8,
    host: *const c.ApiplantHost,
    out: *?[*:0]u8,
) i32 {
    out.* = null;
    _ = name;

    if (std.mem.eql(u8, std.mem.span(input), "\"bad\"")) {
        out.* = toHost("zig says no");
        return c.APIPLANT_ERR_REQUEST;
    }

    host.log.?(host.ctx, c.APIPLANT_WARN, "zig was here");

    // Round-trip every host string, freeing each as the contract requires.
    const config = host.config.?(host.ctx);
    defer host.free_string.?(host.ctx, config);
    const principal = host.principal_id.?(host.ctx);
    defer host.free_string.?(host.ctx, principal);
    const rows = host.query.?(host.ctx, "{\"sql\":\"SELECT 1\",\"params\":[]}");
    defer host.free_string.?(host.ctx, rows);

    const body = std.fmt.allocPrint(
        std.heap.c_allocator,
        "{{\"config\":{s},\"principal\":\"{s}\",\"rows\":{s}}}",
        .{ std.mem.span(config), std.mem.span(principal), std.mem.span(rows) },
    ) catch return c.APIPLANT_ERR_INTERNAL;
    defer std.heap.c_allocator.free(body);

    out.* = toHost(body);
    return c.APIPLANT_OK;
}
"#;

/// A Go function reaching the ABI through cgo. `recover()` is what keeps a Go
/// panic from unwinding into the host.
const GO_FIXTURE: &str = r#"
package main

/*
#define APIPLANT_NO_PROTOTYPES
#include <apiplant.h>
#include <stdlib.h>
static char *ap_query(const ApiplantHost *h, const char *r) { return h->query(h->ctx, r); }
static void  ap_log(const ApiplantHost *h, int32_t l, const char *m) { h->log(h->ctx, l, m); }
static char *ap_config(const ApiplantHost *h) { return h->config(h->ctx); }
static char *ap_principal(const ApiplantHost *h) { return h->principal_id(h->ctx); }
static void  ap_free(const ApiplantHost *h, char *s) { h->free_string(h->ctx, s); }
*/
import "C"

import (
	"fmt"
	"unsafe"
)

var manifestC *C.char

func init() {
	manifestC = C.CString(`[{"name":"gecho","version":"4.0.0","visibility":"public"}]`)
}

//export apiplant_abi_version
func apiplant_abi_version() C.uint32_t { return C.uint32_t(C.APIPLANT_ABI_VERSION) }

//export apiplant_manifest
func apiplant_manifest() *C.char { return manifestC }

//export apiplant_free
func apiplant_free(s *C.char) { C.free(unsafe.Pointer(s)) }

func take(host *C.ApiplantHost, raw *C.char) string {
	if raw == nil {
		return ""
	}
	defer C.ap_free(host, raw)
	return C.GoString(raw)
}

//export apiplant_invoke
func apiplant_invoke(name, input *C.char, host *C.ApiplantHost, out **C.char) C.int32_t {
	*out = nil

	body, status := "", int32(C.APIPLANT_ERR_INTERNAL)
	func() {
		defer func() {
			if r := recover(); r != nil {
				body, status = fmt.Sprintf("panic: %v", r), int32(C.APIPLANT_ERR_INTERNAL)
			}
		}()

		switch in := C.GoString(input); in {
		case `"bad"`:
			body, status = "go says no", C.APIPLANT_ERR_REQUEST
		case `"boom"`:
			panic("go exploded")
		default:
			msg := C.CString("go was here")
			C.ap_log(host, C.APIPLANT_WARN, msg)
			C.free(unsafe.Pointer(msg))

			req := C.CString(`{"sql":"SELECT 1","params":[]}`)
			rows := take(host, C.ap_query(host, req))
			C.free(unsafe.Pointer(req))

			body = fmt.Sprintf(`{"config":%s,"principal":"%s","rows":%s}`,
				take(host, C.ap_config(host)),
				take(host, C.ap_principal(host)),
				rows)
			status = C.APIPLANT_OK
		}
	}()

	*out = C.CString(body)
	return C.int32_t(status)
}

func main() {}
"#;

/// Build a Zig fixture, or `None` when zig isn't installed.
fn compile_zig(dir: &Path, source: &str) -> Option<PathBuf> {
    let zig_file = dir.join("fixture.zig");
    let library = dir.join("libfixture.so");
    std::fs::write(&zig_file, source).unwrap();

    let output = Command::new(std::env::var("ZIG").unwrap_or_else(|_| "zig".into()))
        .arg("build-lib")
        .arg("-dynamic")
        .arg("-lc")
        .arg("-I")
        .arg(include_dir())
        .arg("--cache-dir")
        .arg(dir.join("cache"))
        .arg("--name")
        .arg("fixture")
        .arg("-O")
        .arg("ReleaseSafe")
        .arg(format!("-femit-bin={}", library.display()))
        .arg(&zig_file)
        .output()
        .ok()?;

    if !output.status.success() {
        panic!(
            "zig fixture failed to compile:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Some(library)
}

/// Build a Go fixture, or `None` when go isn't installed.
fn compile_go(dir: &Path, source: &str) -> Option<PathBuf> {
    std::fs::write(dir.join("fixture.go"), source).unwrap();
    std::fs::write(dir.join("go.mod"), "module fixture\n\ngo 1.21\n").unwrap();

    let output = Command::new(std::env::var("GO").unwrap_or_else(|_| "go".into()))
        .current_dir(dir)
        .arg("build")
        .arg("-buildvcs=false")
        .arg("-buildmode=c-shared")
        .arg("-o")
        .arg("libfixture.so")
        .arg(".")
        .env("CGO_ENABLED", "1")
        .env("CGO_CFLAGS", format!("-I{}", include_dir().display()))
        .output()
        .ok()?;

    if !output.status.success() {
        panic!(
            "go fixture failed to compile:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Some(dir.join("libfixture.so"))
}

#[test]
fn a_zig_library_speaks_the_same_abi() {
    let dir = scratch("zig");
    let Some(library) = compile_zig(&dir, ZIG_FIXTURE) else {
        eprintln!("skipping: zig not installed");
        return;
    };

    let echo = load_one(&library, "zecho");
    assert_eq!(echo.manifest().version.as_str(), "3.0.0");
    assert_eq!(echo.manifest().visibility, apiplant_abi::Visibility::Public);

    let body = invoke(&echo, MockHost::new(), r#"{"name":"Ann"}"#).expect("should succeed");
    let body: serde_json::Value = serde_json::from_str(&body).expect("valid JSON came back");
    assert_eq!(body["config"], serde_json::json!({ "greeting": "Ciao" }));
    assert_eq!(body["principal"], "user-42");
    assert_eq!(body["rows"], serde_json::json!([{ "n": 7 }]));

    // The caller-fault channel works the same from Zig.
    let err = invoke(&echo, MockHost::new(), r#""bad""#).unwrap_err();
    assert_eq!(err, "zig says no");
    assert!(!err.starts_with(apiplant_abi::INTERNAL_ERROR_PREFIX));

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_go_library_speaks_the_same_abi_and_survives_dlopen() {
    let dir = scratch("go");
    let Some(library) = compile_go(&dir, GO_FIXTURE) else {
        eprintln!("skipping: go not installed");
        return;
    };

    let echo = load_one(&library, "gecho");
    assert_eq!(echo.manifest().version.as_str(), "4.0.0");

    let body = invoke(&echo, MockHost::new(), r#"{"name":"Ann"}"#).expect("should succeed");
    let body: serde_json::Value = serde_json::from_str(&body).expect("valid JSON came back");
    assert_eq!(body["config"], serde_json::json!({ "greeting": "Ciao" }));
    assert_eq!(body["principal"], "user-42");
    assert_eq!(body["rows"], serde_json::json!([{ "n": 7 }]));

    let err = invoke(&echo, MockHost::new(), r#""bad""#).unwrap_err();
    assert_eq!(err, "go says no");

    // A Go panic must be recovered on the Go side. Without the `recover()` in the
    // fixture this would abort the test process rather than fail one call.
    let err = invoke(&echo, MockHost::new(), r#""boom""#).unwrap_err();
    let detail = err
        .strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX)
        .expect("a recovered panic must be marked internal");
    assert!(detail.contains("go exploded"), "{detail}");

    // Go's runtime is the part most likely to object to being called repeatedly
    // from a foreign host, so hammer it a little.
    for i in 0..50 {
        let body = invoke(&echo, MockHost::new(), &format!(r#"{{"i":{i}}}"#)).unwrap();
        assert!(body.contains("user-42"), "{body}");
    }

    std::fs::remove_dir_all(dir).unwrap();
}
