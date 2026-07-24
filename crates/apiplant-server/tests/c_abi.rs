//! End-to-end test of the [plain C ABI](apiplant_abi::c).
//!
//! Compiles a C fixture with the system compiler, loads it the way the server
//! does, and drives it through every part of the contract: each status code, all
//! five host callbacks, and the two-way ownership rules for strings. Unit tests
//! can check manifest parsing, but only a real shared library exercises the
//! marshalling and the allocator boundary.
//!
//! Skipped (not failed) when there is no C compiler, since one is not otherwise
//! needed to build or test apiplant.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{BoxedFunction, HostApi, HostApi_TO, LogLevel};

/// Exercises the whole contract: config, query, log, principal_id, hook, every
/// status code, and freeing what the host hands over.
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
    " {\"name\":\"gated\",\"visibility\":\"role:admin\",\"method\":\"GET\"}]";

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
    hook_json: String,
}

impl MockHost {
    fn new() -> Self {
        MockHost {
            logs: Mutex::new(Vec::new()),
            queries: Mutex::new(Vec::new()),
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

/// A host that fails every query, to check the `{"error": …}` envelope.
struct FailingHost;
impl HostApi for FailingHost {
    fn query(&self, _r: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr(RString::from("relation does not exist"))
    }
    fn log(&self, _l: LogLevel, _m: RStr<'_>) {}
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
    struct Shared(std::sync::Arc<MockHost>);
    impl HostApi for Shared {
        fn query(&self, r: RStr<'_>) -> RResult<RString, RString> {
            self.0.query(r)
        }
        fn log(&self, l: LogLevel, m: RStr<'_>) {
            self.0.log(l, m)
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

    invoke(&echo, Shared(host.clone()), r#"{"name":"Ann"}"#).unwrap();

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
