//! End-to-end tests for TypeScript functions: transpile a source, load the
//! JavaScript into isolates, call it, and check what comes back — including the
//! parts that are easy to get subtly wrong (who is at fault for an error, what a
//! host call sees, what happens to a function that never returns).
//!
//! The host here is a stub rather than a database, which is the point: it
//! records what the isolate asked for, so the tests can assert on the *contract*
//! between the two rather than on Postgres.

use std::sync::{Arc, Mutex};

use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{BoxedFunction, HostApi, HostApi_TO, LogLevel};

/// A host that answers from canned values and remembers what it was asked.
#[derive(Clone, Default)]
struct FakeHost {
    config: String,
    principal: String,
    /// Every `{sql, params}` request the function made, in order.
    queries: Arc<Mutex<Vec<String>>>,
    /// Every log line, as `level: message`.
    logs: Arc<Mutex<Vec<String>>>,
    /// What `query` answers with; an `{"error": …}` object to fail.
    rows: String,
}

impl FakeHost {
    fn new() -> FakeHost {
        FakeHost {
            config: "{}".into(),
            principal: String::new(),
            rows: "[]".into(),
            ..Default::default()
        }
    }
}

impl HostApi for FakeHost {
    fn query(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.queries.lock().unwrap().push(request.as_str().into());
        RResult::ROk(self.rows.clone().into())
    }
    fn send_email(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no email provider configured".into())
    }
    fn cache(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no cache configured".into())
    }
    fn log(&self, level: LogLevel, message: RStr<'_>) {
        self.logs
            .lock()
            .unwrap()
            .push(format!("{level:?}: {}", message.as_str()));
    }
    fn config(&self) -> RString {
        self.config.clone().into()
    }
    fn principal_id(&self) -> RString {
        self.principal.clone().into()
    }
    fn hook(&self) -> RString {
        RString::new()
    }
}

/// Transpile `source`, write it where `load` expects it, and load it.
fn build(label: &str, source: &str) -> Result<Vec<BoxedFunction>, String> {
    let js = apiplant_js::transpile::to_js(label, source)?;

    let dir = std::env::temp_dir().join(format!(
        "apiplant-js-test-{}-{label}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{label}.js"));
    std::fs::write(&path, js).unwrap();

    apiplant_js::load(&path)
}

/// Call one exported function by name.
fn call(
    functions: &[BoxedFunction],
    name: &str,
    input: &str,
    host: FakeHost,
) -> Result<String, String> {
    let function = functions
        .iter()
        .find(|f| f.manifest().name.as_str() == name)
        .unwrap_or_else(|| panic!("no function named `{name}`"));
    match function.invoke(
        HostApi_TO::from_value(host, TD_Opaque),
        RStr::from_str(input),
    ) {
        RResult::ROk(out) => Ok(out.into_string()),
        RResult::RErr(e) => Err(e.into_string()),
    }
}

const GREET: &str = r#"
export const manifest = [
    { name: "greet", permission: "public", description: "Greets someone." },
    { name: "notes", permission: "authenticated", method: "GET" },
];

interface Input { name?: string }

export function greet(input: Input, ctx: Ctx) {
    if (!input?.name) throw new BadRequest("`name` is required");
    const greeting = (ctx.config() as { greeting?: string }).greeting ?? "Hello";
    ctx.log.info("greeting " + input.name);
    return { message: `${greeting}, ${input.name}!` };
}

export async function notes(_input: unknown, ctx: Ctx) {
    const rows = ctx.query("SELECT count(*)::int AS n FROM apiplant_note") as Row[];
    return { notes: rows[0]?.n ?? 0, caller: ctx.principalId() };
}
"#;

#[test]
fn a_module_declares_several_functions_and_each_becomes_one() {
    let functions = build("greet", GREET).unwrap();
    let names: Vec<String> = functions
        .iter()
        .map(|f| f.manifest().name.to_string())
        .collect();
    assert_eq!(names, vec!["greet".to_string(), "notes".to_string()]);

    let greet = functions[0].manifest();
    assert_eq!(greet.visibility, apiplant_abi::Visibility::Public);
    assert_eq!(greet.method, apiplant_abi::HttpMethod::Post);
    assert_eq!(greet.description.as_str(), "Greets someone.");

    // The manifest's own fields decide the endpoint, not the export order.
    let notes = functions[1].manifest();
    assert_eq!(notes.method, apiplant_abi::HttpMethod::Get);
    assert_eq!(notes.visibility, apiplant_abi::Visibility::Authenticated);
}

#[test]
fn input_output_and_config_cross_the_boundary_as_json() {
    let functions = build("greet_io", GREET).unwrap();
    let mut host = FakeHost::new();
    host.config = r#"{"greeting":"Ciao"}"#.into();

    let out = call(&functions, "greet", r#"{"name":"world"}"#, host.clone()).unwrap();
    assert_eq!(out, r#"{"message":"Ciao, world!"}"#);
    assert_eq!(
        host.logs.lock().unwrap().as_slice(),
        ["Info: greeting world"]
    );
}

/// The host call runs on the *caller's* thread while the isolate blocks, so this
/// is the test that the round trip through the two channels actually works.
#[test]
fn a_function_can_query_the_host_and_read_the_caller() {
    let functions = build("greet_query", GREET).unwrap();
    let mut host = FakeHost::new();
    host.rows = r#"[{"n":3}]"#.into();
    host.principal = "user-1".into();

    let out = call(&functions, "notes", "{}", host.clone()).unwrap();
    assert_eq!(out, r#"{"notes":3,"caller":"user-1"}"#);

    let queries = host.queries.lock().unwrap();
    assert_eq!(queries.len(), 1);
    assert!(queries[0].contains("apiplant_note"), "{}", queries[0]);
    // Parameters are always sent, so the host never has to guess.
    assert!(queries[0].contains(r#""params":[]"#), "{}", queries[0]);
}

/// `BadRequest` is a 400: the message goes back to the caller bare.
#[test]
fn bad_request_is_the_callers_fault() {
    let functions = build("greet_bad", GREET).unwrap();
    let err = call(&functions, "greet", "{}", FakeHost::new()).unwrap_err();
    assert_eq!(err, "`name` is required");
    assert!(!err.starts_with(apiplant_abi::INTERNAL_ERROR_PREFIX));
}

/// Anything else thrown is a 500, which the host recognises by the prefix.
#[test]
fn any_other_throw_is_the_functions_fault() {
    let functions = build(
        "boom",
        r#"
        export const manifest = [{ name: "boom", permission: "public" }];
        export function boom() { throw new Error("nope"); }
        "#,
    )
    .unwrap();

    let err = call(&functions, "boom", "{}", FakeHost::new()).unwrap_err();
    assert!(
        err.starts_with(apiplant_abi::INTERNAL_ERROR_PREFIX),
        "{err}"
    );
    assert!(err.contains("nope"), "{err}");
}

/// A failed host call surfaces as an ordinary exception, not a magic value, so a
/// function that ignores it cannot accidentally return `{"error": …}` as data.
#[test]
fn a_host_failure_becomes_a_thrown_error() {
    let functions = build(
        "mailer",
        r#"
        export const manifest = [{ name: "mail", permission: "public" }];
        export function mail(_input: unknown, ctx: Ctx) {
            ctx.sendEmail({ to: "a@b.c", subject: "hi", text: "hi" });
            return { sent: true };
        }
        "#,
    )
    .unwrap();

    let err = call(&functions, "mail", "{}", FakeHost::new()).unwrap_err();
    assert!(err.contains("no email provider configured"), "{err}");
}

#[test]
fn an_async_function_is_awaited() {
    let functions = build(
        "slow",
        r#"
        export const manifest = [{ name: "slow", permission: "public" }];
        export async function slow() {
            await new Promise((resolve) => setTimeout(resolve, 10));
            return { done: true };
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        call(&functions, "slow", "{}", FakeHost::new()).unwrap(),
        r#"{"done":true}"#
    );
}

/// A module without a manifest is rejected at load, where the fix is obvious —
/// not at the first request, where it would look like a routing bug.
#[test]
fn a_module_without_a_manifest_is_refused() {
    let Err(err) = build("nameless", "export function greet() { return {}; }") else {
        panic!("a module with no manifest was loaded anyway");
    };
    assert!(err.contains("manifest"), "{err}");
}

/// A manifest naming an export that isn't there fails the request rather than
/// the whole app: the other functions in the module still work.
#[test]
fn a_manifest_entry_without_an_export_fails_only_its_own_calls() {
    let functions = build(
        "typo",
        r#"
        export const manifest = [
            { name: "here", permission: "public" },
            { name: "gone", permission: "public" },
        ];
        export function here() { return { ok: true }; }
        "#,
    )
    .unwrap();

    assert_eq!(
        call(&functions, "here", "{}", FakeHost::new()).unwrap(),
        r#"{"ok":true}"#
    );
    let err = call(&functions, "gone", "{}", FakeHost::new()).unwrap_err();
    assert!(err.contains("no function named `gone`"), "{err}");
}

/// A function returning nothing is a valid function; the endpoint answers with
/// `null` rather than with an error.
#[test]
fn returning_nothing_is_null_not_a_failure() {
    let functions = build(
        "quiet",
        r#"
        export const manifest = [{ name: "quiet", permission: "public" }];
        export function quiet() {}
        "#,
    )
    .unwrap();

    assert_eq!(
        call(&functions, "quiet", "{}", FakeHost::new()).unwrap(),
        "null"
    );
}
