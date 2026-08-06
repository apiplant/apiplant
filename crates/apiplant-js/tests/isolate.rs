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
    fn payments(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no payment provider configured".into())
    }
    fn ai(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no ai provider configured".into())
    }
    fn publish(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no queue in this test".into())
    }
    fn emit(&self, _chunk: RStr<'_>) -> bool {
        false
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

/// The Web platform globals are actually there, and actually work.
///
/// `deno_core` supplies none of these — no `TextEncoder`, no `URL`, not even
/// `setTimeout`. They arrive from `deno_web` through the startup snapshot, and
/// the failure mode when that wiring breaks is not a compile error but a
/// `ReferenceError` at the first request, so it is worth one round trip through
/// a real isolate. Each expression below is chosen to fail loudly on a
/// half-installed global rather than on a missing one.
#[test]
fn the_web_globals_are_installed_and_working() {
    let functions = build(
        "web",
        r#"
        export const manifest = [{ name: "web", permission: "public" }];

        export async function web() {
            const encoded = new TextEncoder().encode("héllo");
            const url = new URL("https://example.com/a/b?x=1&x=2");
            const pattern = new URLPattern({ pathname: "/a/:rest" });

            // A refed timer: the invocation must not settle before it fires.
            let ticked = false;
            await new Promise((resolve) => setTimeout(() => { ticked = true; resolve(); }, 5));

            const cloned = structuredClone({ nested: { set: new Set([1, 2]) } });
            const stream = new ReadableStream({
                start(c) { c.enqueue("chunk"); c.close(); },
            });
            const chunk = await stream.getReader().read();

            return {
                encodedLength: encoded.length,
                decoded: new TextDecoder().decode(encoded),
                base64: btoa("hi"),
                host: url.host,
                search: url.searchParams.getAll("x"),
                matches: pattern.test("https://example.com/a/b"),
                ticked,
                clonedSet: cloned.nested.set instanceof Set,
                chunk: chunk.value,
                blobSize: new Blob(["1234"]).size,
                measures: typeof performance.now(),
            };
        }
        "#,
    )
    .unwrap();

    let out = call(&functions, "web", "{}", FakeHost::new()).unwrap();
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();

    // "héllo" is six UTF-8 bytes, not five: proof this is real encoding rather
    // than a stub that counted the characters.
    assert_eq!(out["encodedLength"], 6);
    assert_eq!(out["decoded"], "héllo");
    assert_eq!(out["base64"], "aGk=");
    assert_eq!(out["host"], "example.com");
    assert_eq!(out["search"], serde_json::json!(["1", "2"]));
    assert_eq!(out["matches"], true);
    assert_eq!(out["ticked"], true, "the invocation outran its own timer");
    assert_eq!(out["clonedSet"], true, "structuredClone flattened a Set");
    assert_eq!(out["chunk"], "chunk");
    assert_eq!(out["blobSize"], 4);
    assert_eq!(out["measures"], "number");
}

/// `Intl` is present *and* has locale data behind it.
///
/// V8 always exposes the `Intl` constructors, so a test that only checks they
/// exist proves nothing: without ICU data every locale silently falls back to
/// root, and `de-DE` formats like English. Each assertion below is picked to
/// differ between locales, so a fallback fails rather than passes quietly. This
/// is what `deno_core`'s `include_icu_data` feature buys.
#[test]
fn intl_has_real_locale_data() {
    let functions = build(
        "intl",
        r#"
        export const manifest = [{ name: "intl", permission: "public" }];

        export function intl() {
            const when = new Date(Date.UTC(2026, 2, 9, 15, 45, 0));
            return {
                // Decimal comma and a euro symbol, not "1,234.50 EUR".
                euros: new Intl.NumberFormat("de-DE", {
                    style: "currency", currency: "EUR",
                }).format(1234.5),
                yen: new Intl.NumberFormat("ja-JP", {
                    style: "currency", currency: "JPY",
                }).format(1234),
                // Day before month in en-GB, month before day in en-US.
                british: new Intl.DateTimeFormat("en-GB", {
                    timeZone: "UTC", day: "2-digit", month: "2-digit", year: "numeric",
                }).format(when),
                american: new Intl.DateTimeFormat("en-US", {
                    timeZone: "UTC", day: "2-digit", month: "2-digit", year: "numeric",
                }).format(when),
                // A real timezone database, not just UTC offsets.
                tokyo: new Intl.DateTimeFormat("en-US", {
                    timeZone: "Asia/Tokyo", hour: "2-digit", minute: "2-digit", hour12: false,
                }).format(when),
                // Locale-aware collation: in Swedish "ä" sorts after "z".
                swedish: ["ä", "z", "a"].sort(new Intl.Collator("sv").compare),
                german: ["ä", "z", "a"].sort(new Intl.Collator("de").compare),
                plural: new Intl.PluralRules("cy").select(3),
                relative: new Intl.RelativeTimeFormat("es").format(-1, "day"),
                list: new Intl.ListFormat("en", { type: "conjunction" })
                    .format(["a", "b", "c"]),
            };
        }
        "#,
    )
    .unwrap();

    let out = call(&functions, "intl", "{}", FakeHost::new()).unwrap();
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();

    // CLDR separates the amount from the symbol with a *non-breaking* space —
    // U+00A0, not U+0020. Worth spelling out: it is the usual reason a
    // hand-written assertion against `Intl` output fails.
    assert_eq!(out["euros"], "1.234,50\u{a0}€");
    assert_eq!(out["yen"], "￥1,234");
    assert_eq!(out["british"], "09/03/2026");
    assert_eq!(out["american"], "03/09/2026");
    assert_eq!(out["tokyo"], "00:45");
    assert_eq!(out["swedish"], serde_json::json!(["a", "z", "ä"]));
    assert_eq!(out["german"], serde_json::json!(["a", "ä", "z"]));
    // Welsh has six plural categories; a root fallback would say "other".
    assert_eq!(out["plural"], "few");
    assert_eq!(out["relative"], "hace 1 día");
    assert_eq!(out["list"], "a, b, and c");
}

/// A throwaway HTTP server on a random port.
///
/// Hand-rolled on `std::net` rather than pulled from a crate because the tests
/// need to answer with things a polite server would not: a redirect chain, a
/// bare 404, a header repeated twice. It answers each connection once from a
/// canned table keyed by path, and records what it was asked.
mod server {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub struct Server {
        pub base: String,
        /// Every request, as `METHOD /path` plus the body when there was one.
        pub seen: Arc<Mutex<Vec<String>>>,
    }

    /// Start a server that answers `routes` and stops when the test ends.
    pub fn start(routes: Vec<(&'static str, &'static str)>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("cannot bind a test port");
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);

        // A thread per connection, and each connection served until the client
        // closes it. Both matter: HTTP/1.1 keep-alive means one client uses a
        // connection for several requests, and `Promise.all` means it opens
        // several at once. Answering one request per accept and hanging up would
        // give reqwest a "connection closed before message completed" instead of
        // a response, which is a property of this stub rather than of `fetch`.
        std::thread::spawn(move || {
            let routes = Arc::new(routes);
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let recorded = Arc::clone(&recorded);
                let routes = Arc::clone(&routes);

                std::thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    loop {
                        let mut request_line = String::new();
                        // An empty read is the client hanging up, which is the
                        // normal way out of this loop.
                        match reader.read_line(&mut request_line) {
                            Ok(0) | Err(_) => return,
                            Ok(_) => {}
                        }
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().unwrap_or("").to_string();
                        let path = parts.next().unwrap_or("/").to_string();

                        let mut length = 0usize;
                        loop {
                            let mut header = String::new();
                            if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                                break;
                            }
                            if let Some(value) =
                                header.to_ascii_lowercase().strip_prefix("content-length:")
                            {
                                length = value.trim().parse().unwrap_or(0);
                            }
                        }
                        let mut body = vec![0u8; length];
                        if length > 0 && reader.read_exact(&mut body).is_err() {
                            return;
                        }

                        let mut entry = format!("{method} {path}");
                        if !body.is_empty() {
                            entry.push(' ');
                            entry.push_str(&String::from_utf8_lossy(&body));
                        }
                        recorded.lock().unwrap().push(entry);

                        let response = routes
                            .iter()
                            .find(|(route, _)| *route == path)
                            .map(|(_, response)| *response)
                            .unwrap_or("HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
                        if stream.write_all(response.as_bytes()).is_err() {
                            return;
                        }
                        let _ = stream.flush();
                    }
                });
            }
        });

        Server {
            base: format!("http://127.0.0.1:{port}"),
            seen,
        }
    }
}

/// `fetch` works, and behaves like `fetch` rather than like a bespoke helper.
///
/// The assertions are about the parts that are easy to get wrong when
/// reimplementing it: a non-2xx is a *resolved* promise with `ok: false` rather
/// than a rejection, headers are case-insensitive, a body can only be read once,
/// and `Response.url` reports where the response actually came from.
#[test]
fn fetch_speaks_the_web_api() {
    let server = server::start(vec![
        (
            "/json",
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 17\r\n\r\n{\"hello\":\"world\"}",
        ),
        (
            "/echo",
            "HTTP/1.1 201 Created\r\ncontent-length: 2\r\n\r\nok",
        ),
        (
            "/moved",
            "HTTP/1.1 302 Found\r\nlocation: /json\r\ncontent-length: 0\r\n\r\n",
        ),
        ("/teapot", "HTTP/1.1 418 I'm a Teapot\r\ncontent-length: 0\r\n\r\n"),
    ]);

    let functions = build(
        "net",
        r#"
        export const manifest = [{ name: "net", permission: "public" }];

        export async function net(input: { base: string }) {
            const base = input.base;

            const json = await fetch(`${base}/json`);
            const body = await json.json();

            const posted = await fetch(`${base}/echo`, {
                method: "POST",
                headers: { "X-Custom": "yes" },
                body: JSON.stringify({ a: 1 }),
            });

            const redirected = await fetch(`${base}/moved`);
            const teapot = await fetch(`${base}/teapot`);

            // Concurrency: both are in flight, not queued behind each other.
            const pair = await Promise.all([
                fetch(`${base}/json`).then((r) => r.status),
                fetch(`${base}/teapot`).then((r) => r.status),
            ]);

            // The body is readable exactly once.
            const once = await fetch(`${base}/json`);
            await once.text();
            let secondRead = "no error";
            try { await once.text(); } catch (e) { secondRead = e.constructor.name; }

            return {
                ok: json.ok,
                status: json.status,
                statusText: json.statusText,
                body,
                // Header lookup ignores case.
                contentType: json.headers.get("CONTENT-TYPE"),
                hasHeader: json.headers.has("Content-Type"),
                postedStatus: posted.status,
                postedText: await posted.text(),
                redirected: redirected.redirected,
                redirectedUrl: redirected.url.endsWith("/json"),
                redirectedBody: await redirected.json(),
                teapotOk: teapot.ok,
                teapotStatus: teapot.status,
                pair,
                secondRead,
                // A Request is a first-class value.
                requestMethod: new Request(`${base}/json`, { method: "PUT" }).method,
                // And Response.json() is the static shortcut.
                madeJson: await Response.json({ x: 1 }).json(),
            };
        }
        "#,
    )
    .unwrap();

    let out = call(
        &functions,
        "net",
        &serde_json::json!({ "base": server.base }).to_string(),
        FakeHost::new(),
    )
    .unwrap();
    let out: serde_json::Value = serde_json::from_str(&out).unwrap();

    assert_eq!(out["ok"], true);
    assert_eq!(out["status"], 200);
    assert_eq!(out["statusText"], "OK");
    assert_eq!(out["body"], serde_json::json!({ "hello": "world" }));
    assert_eq!(out["contentType"], "application/json");
    assert_eq!(out["hasHeader"], true);
    assert_eq!(out["postedStatus"], 201);
    assert_eq!(out["postedText"], "ok");
    assert_eq!(out["redirected"], true, "the redirect was not followed");
    assert_eq!(out["redirectedUrl"], true, "Response.url is the pre-redirect URL");
    assert_eq!(out["redirectedBody"], serde_json::json!({ "hello": "world" }));
    // A 418 is a perfectly good response: `fetch` rejects only on network
    // failure, never on a status the server chose deliberately.
    assert_eq!(out["teapotOk"], false);
    assert_eq!(out["teapotStatus"], 418);
    assert_eq!(out["pair"], serde_json::json!([200, 418]));
    assert_eq!(out["secondRead"], "TypeError");
    assert_eq!(out["requestMethod"], "PUT");
    assert_eq!(out["madeJson"], serde_json::json!({ "x": 1 }));

    let seen = server.seen.lock().unwrap();
    assert!(
        seen.iter().any(|r| r == r#"POST /echo {"a":1}"#),
        "the request body never arrived: {seen:?}",
    );
    assert!(seen.iter().any(|r| r == "GET /moved"), "{seen:?}");
    assert!(seen.iter().any(|r| r == "GET /json"), "{seen:?}");
}
