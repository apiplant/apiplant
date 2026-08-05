//! The `apiplant` module, exercised the way a function uses it.
//!
//! The unit tests next door check the plumbing; these check the promises the
//! package makes to an author -- that `defineFunctions` produces the manifest
//! the host reads, that a declared schema rejects a bad body with a 400 before
//! the handler runs, and that `db`, `cache`, `email`, `payments` and `ai` send
//! the host exactly what the Rust side expects to receive.
//!
//! The host is a stub that records requests, so what is asserted is the wire
//! format between the two -- which is the thing that silently breaks when either
//! side changes.

use std::sync::{Arc, Mutex};

use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{BoxedFunction, HostApi, HostApi_TO, LogLevel};
use serde_json::{json, Value};

/// A host that answers from canned replies and remembers every request.
#[derive(Clone, Default)]
struct Recorder {
    /// `kind: payload` for every host call, in order.
    calls: Arc<Mutex<Vec<String>>>,
    /// What `query` answers with, in order; the last one repeats.
    query_replies: Arc<Mutex<Vec<Value>>>,
    /// What `cache` answers with, in order; the last one repeats.
    cache_replies: Arc<Mutex<Vec<Value>>>,
    /// What `payments` answers with, in order; the last one repeats.
    payments_replies: Arc<Mutex<Vec<Value>>>,
    /// What `ai` answers with, in order; the last one repeats.
    ai_replies: Arc<Mutex<Vec<Value>>>,
    /// What `publish` answers with, in order; the last one repeats.
    publish_replies: Arc<Mutex<Vec<Value>>>,
    config: String,
    principal: String,
    hook: String,
}

impl Recorder {
    fn new() -> Recorder {
        Recorder {
            config: "{}".into(),
            ..Default::default()
        }
    }

    fn record(&self, kind: &str, payload: &str) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{kind}: {payload}"));
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// The payload of the one call of this kind.
    fn call(&self, kind: &str) -> Value {
        let prefix = format!("{kind}: ");
        let calls = self.calls.lock().unwrap();
        let mut matching = calls.iter().filter_map(|c| c.strip_prefix(&prefix));
        let payload = matching
            .next()
            .unwrap_or_else(|| panic!("no `{kind}` call in {calls:?}"));
        serde_json::from_str(payload).unwrap()
    }

    fn answering(mut self, replies: Vec<Value>) -> Recorder {
        self.query_replies = Arc::new(Mutex::new(replies));
        self
    }

    fn caching(mut self, replies: Vec<Value>) -> Recorder {
        self.cache_replies = Arc::new(Mutex::new(replies));
        self
    }

    fn paying(mut self, replies: Vec<Value>) -> Recorder {
        self.payments_replies = Arc::new(Mutex::new(replies));
        self
    }

    fn chatting(mut self, replies: Vec<Value>) -> Recorder {
        self.ai_replies = Arc::new(Mutex::new(replies));
        self
    }
}

/// Take the next canned reply, keeping the last one for any further calls.
fn next_reply(replies: &Mutex<Vec<Value>>, fallback: Value) -> Value {
    let mut replies = replies.lock().unwrap();
    if replies.is_empty() {
        return fallback;
    }
    if replies.len() == 1 {
        return replies[0].clone();
    }
    replies.remove(0)
}

impl HostApi for Recorder {
    fn query(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.record("query", request.as_str());
        RResult::ROk(
            next_reply(&self.query_replies, json!([]))
                .to_string()
                .into(),
        )
    }
    fn send_email(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.record("send_email", request.as_str());
        RResult::ROk(
            json!({ "provider": "smtp", "id": "abc", "recipients": 2 })
                .to_string()
                .into(),
        )
    }
    fn payments(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.record("payments", request.as_str());
        RResult::ROk(
            next_reply(
                &self.payments_replies,
                json!({ "url": "https://checkout.stripe.test/s" }),
            )
            .to_string()
            .into(),
        )
    }

    fn ai(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.record("ai", request.as_str());
        RResult::ROk(
            next_reply(
                &self.ai_replies,
                json!({ "text": "hello", "provider": "custom", "model": "local" }),
            )
            .to_string()
            .into(),
        )
    }

    fn emit(&self, chunk: RStr<'_>) -> bool {
        self.record("emit", chunk.as_str());
        true
    }

    fn publish(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.record("publish", request.as_str());
        RResult::ROk(
            next_reply(
                &self.publish_replies,
                json!({ "id": "m-1", "topic": "t", "delivered": 1 }),
            )
            .to_string()
            .into(),
        )
    }

    fn cache(&self, request: RStr<'_>) -> RResult<RString, RString> {
        self.record("cache", request.as_str());
        RResult::ROk(
            next_reply(&self.cache_replies, json!({}))
                .to_string()
                .into(),
        )
    }
    fn log(&self, level: LogLevel, message: RStr<'_>) {
        self.record("log", &format!("{level:?} {}", message.as_str()));
    }
    fn config(&self) -> RString {
        self.config.clone().into()
    }
    fn principal_id(&self) -> RString {
        self.principal.clone().into()
    }
    fn hook(&self) -> RString {
        self.hook.clone().into()
    }
}

/// Transpile and load, as `apiplant build` and the server do.
fn build(label: &str, source: &str) -> Result<Vec<BoxedFunction>, String> {
    let js = apiplant_js::transpile::to_js(label, source)?;
    let dir = std::env::temp_dir().join(format!(
        "apiplant-js-package-{}-{label}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{label}.js"));
    std::fs::write(&path, js).unwrap();
    apiplant_js::load(&path)
}

fn call(
    functions: &[BoxedFunction],
    name: &str,
    input: &str,
    host: Recorder,
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

const NOTES: &str = r#"
import { defineFunctions, db, s, sql, principalId, log } from "apiplant";

const NewNote = s.object({
    title: s.string({ minLength: 1 }),
    tags: s.optional(s.array(s.string())),
});

export default defineFunctions({
    createNote: {
        permission: "authenticated",
        description: "Files a note.",
        method: "POST",
        input: NewNote,
        output: s.object({ id: s.string() }),
        handler(input) {
            log.info(`filing ${input.title}`);
            const row = db.one(
                sql`INSERT INTO apiplant_note (title, owner) VALUES (${input.title}, ${principalId()}) RETURNING id`,
            );
            return { id: row.id, tags: input.tags ?? [] };
        },
    },
    countNotes: {
        permission: "public",
        method: "GET",
        handler() {
            return { notes: db.value("SELECT count(*)::int AS n FROM apiplant_note") };
        },
    },
});
"#;

/// `defineFunctions` is the manifest the host reads: names, methods and access
/// all come from the same place the handlers do.
#[test]
fn define_functions_produces_the_manifest_the_host_loads() {
    let functions = build("notes", NOTES).unwrap();

    let names: Vec<String> = functions
        .iter()
        .map(|f| f.manifest().name.to_string())
        .collect();
    // Declaration order, not alphabetical: the manifest reads like the file.
    assert_eq!(
        names,
        vec!["createNote".to_string(), "countNotes".to_string()]
    );

    let create = functions
        .iter()
        .find(|f| f.manifest().name.as_str() == "createNote")
        .unwrap()
        .manifest();
    assert_eq!(create.visibility, apiplant_abi::Visibility::Authenticated);
    assert_eq!(create.method, apiplant_abi::HttpMethod::Post);
    assert_eq!(create.description.as_str(), "Files a note.");

    // A declared schema reaches the generated OpenAPI as JSON Schema, which is
    // the whole reason it is declared rather than checked inline.
    let input: Value = serde_json::from_str(create.input_schema.as_str()).unwrap();
    assert_eq!(input["type"], "object");
    assert_eq!(input["properties"]["title"]["type"], "string");
    assert_eq!(input["properties"]["title"]["minLength"], 1);
    // `s.optional` is the only thing that makes a field optional.
    assert_eq!(input["required"], json!(["title"]));

    let output: Value = serde_json::from_str(create.output_schema.as_str()).unwrap();
    assert_eq!(output["properties"]["id"]["type"], "string");
}

/// The check runs before the handler, so a handler never sees a body its schema
/// would have rejected -- and the caller gets a 400 naming the field.
#[test]
fn a_declared_schema_rejects_a_bad_body_before_the_handler_runs() {
    let functions = build("notes_validation", NOTES).unwrap();
    let host = Recorder::new();

    let missing = call(&functions, "createNote", "{}", host.clone()).unwrap_err();
    assert_eq!(missing, "title is required");
    assert!(!missing.starts_with(apiplant_abi::INTERNAL_ERROR_PREFIX));

    let empty = call(&functions, "createNote", r#"{"title":""}"#, host.clone()).unwrap_err();
    assert_eq!(empty, "title must be at least 1 characters");

    let wrong_type = call(
        &functions,
        "createNote",
        r#"{"title":"ok","tags":"nope"}"#,
        host.clone(),
    )
    .unwrap_err();
    assert_eq!(wrong_type, "tags must be an array");

    // A rejected request never reached the database.
    assert!(
        !host.calls().iter().any(|c| c.starts_with("query:")),
        "{:?}",
        host.calls()
    );
}

/// `sql` binds values instead of interpolating them, and `db.one` unwraps the
/// row the host sent back.
#[test]
fn the_sql_template_binds_its_values() {
    let functions = build("notes_sql", NOTES).unwrap();
    let mut host = Recorder::new().answering(vec![json!([{ "id": "note-1" }])]);
    host.principal = "user-7".into();

    let out = call(
        &functions,
        "createNote",
        r#"{"title":"O'Brien","tags":["a"]}"#,
        host.clone(),
    )
    .unwrap();
    assert_eq!(out, r#"{"id":"note-1","tags":["a"]}"#);

    let query = host.call("query");
    assert_eq!(
        query["sql"],
        "INSERT INTO apiplant_note (title, owner) VALUES ($1, $2) RETURNING id"
    );
    // The apostrophe is data, because it never went near the SQL.
    assert_eq!(query["params"], json!(["O'Brien", "user-7"]));
}

/// `db.value` is the `count(*)` case: one row, one column, no unwrapping at the
/// call site.
#[test]
fn db_value_returns_the_single_column() {
    let functions = build("notes_value", NOTES).unwrap();
    let host = Recorder::new().answering(vec![json!([{ "n": 12 }])]);

    assert_eq!(
        call(&functions, "countNotes", "{}", host).unwrap(),
        r#"{"notes":12}"#
    );
}

/// Using `db.query` for a statement that returns no rows is a mistake worth
/// naming: `{rows_affected}` is not an empty result set.
#[test]
fn db_query_refuses_a_statement_that_returned_no_rows() {
    let functions = build(
        "misuse",
        r#"
        import { defineFunctions, db } from "apiplant";
        export default defineFunctions({
            wrong: { permission: "public", handler: () => db.query("DELETE FROM apiplant_note") },
            right: { permission: "public", handler: () => ({ deleted: db.execute("DELETE FROM apiplant_note") }) },
        });
        "#,
    )
    .unwrap();
    let host = Recorder::new().answering(vec![json!({ "rows_affected": 3 })]);

    let err = call(&functions, "wrong", "{}", host.clone()).unwrap_err();
    assert!(err.contains("use `db.execute`"), "{err}");

    assert_eq!(
        call(&functions, "right", "{}", host).unwrap(),
        r#"{"deleted":3}"#
    );
}

/// The cache wrapper speaks the `{op, key, …}` grammar `apiplant-cache` parses,
/// and turns its replies back into values rather than envelopes.
#[test]
fn the_cache_wrapper_speaks_the_hosts_grammar() {
    let functions = build(
        "caching",
        r#"
        import { defineFunctions, cache } from "apiplant";
        export default defineFunctions({
            warm: {
                permission: "public",
                handler() {
                    const miss = cache.get("absent");
                    const hit = cache.get("present");
                    cache.set("k", { a: 1 }, 30);
                    const total = cache.increment("hits");
                    return { miss, hit, total };
                },
            },
        });
        "#,
    )
    .unwrap();

    let host = Recorder::new().caching(vec![
        json!({ "hit": false, "value": null }),
        json!({ "hit": true, "value": { "cached": true } }),
        json!({ "ok": true }),
        json!({ "value": 5 }),
    ]);

    let out = call(&functions, "warm", "{}", host.clone()).unwrap();
    assert_eq!(out, r#"{"miss":null,"hit":{"cached":true},"total":5}"#);

    let calls = host.calls();
    assert!(
        calls[0].contains(r#"{"op":"get","key":"absent"}"#),
        "{calls:?}"
    );
    assert!(
        calls[2].contains(r#"{"op":"set","key":"k","value":{"a":1},"ttl":30}"#),
        "{calls:?}"
    );
    assert!(
        calls[3].contains(r#"{"op":"incr","key":"hits","by":1}"#),
        "{calls:?}"
    );
}

/// `cache.remember` is the read-through pattern, so the expensive part must not
/// run when the value is already there.
#[test]
fn remember_computes_only_on_a_miss() {
    let functions = build(
        "remember",
        r#"
        import { defineFunctions, cache, db } from "apiplant";
        export default defineFunctions({
            stats: {
                permission: "public",
                handler: () => ({
                    notes: cache.remember("stats", 60, () => db.value("SELECT count(*)::int AS n FROM apiplant_note")),
                }),
            },
        });
        "#,
    )
    .unwrap();

    let hit = Recorder::new().caching(vec![json!({ "hit": true, "value": 9 })]);
    assert_eq!(
        call(&functions, "stats", "{}", hit.clone()).unwrap(),
        r#"{"notes":9}"#
    );
    assert!(
        !hit.calls().iter().any(|c| c.starts_with("query:")),
        "a cache hit still queried: {:?}",
        hit.calls()
    );

    let miss = Recorder::new()
        .caching(vec![
            json!({ "hit": false, "value": null }),
            json!({ "ok": true }),
        ])
        .answering(vec![json!([{ "n": 4 }])]);
    assert_eq!(
        call(&functions, "stats", "{}", miss.clone()).unwrap(),
        r#"{"notes":4}"#
    );
    // Computed, then written back with the TTL it was given.
    assert!(
        miss.calls().iter().any(|c| c.starts_with("query:")),
        "{:?}",
        miss.calls()
    );
    assert!(
        miss.calls()
            .iter()
            .any(|c| c.contains(r#""op":"set","key":"stats","value":4,"ttl":60"#)),
        "{:?}",
        miss.calls()
    );
}

/// The email shape is `apiplant_email::Message`, which accepts one recipient or
/// a list, and answers with what the provider reported.
#[test]
fn email_send_matches_the_hosts_message() {
    let functions = build(
        "mail",
        r#"
        import { defineFunctions, email, config } from "apiplant";
        export default defineFunctions({
            notify: {
                permission: "public",
                handler() {
                    const { sender } = config();
                    const sent = email.send({
                        to: ["ops@example.com", { email: "cto@example.com", name: "CTO" }],
                        subject: "A note was filed",
                        text: "Someone filed a note.",
                        from: sender,
                    });
                    return { provider: sent.provider, recipients: sent.recipients };
                },
            },
        });
        "#,
    )
    .unwrap();

    let mut host = Recorder::new();
    host.config = r#"{"sender":"noreply@example.com"}"#.into();

    let out = call(&functions, "notify", "{}", host.clone()).unwrap();
    assert_eq!(out, r#"{"provider":"smtp","recipients":2}"#);

    let message = host.call("send_email");
    assert_eq!(message["subject"], "A note was filed");
    assert_eq!(message["to"][0], "ops@example.com");
    assert_eq!(message["to"][1]["name"], "CTO");
    assert_eq!(message["from"], "noreply@example.com");
}

/// The payments helpers map straight onto the host's request grammar.
#[test]
fn payments_helpers_match_the_hosts_requests() {
    let functions = build(
        "payments",
        r#"
        import { defineFunctions, payments } from "apiplant";
        export default defineFunctions({
            billing: {
                permission: "member",
                handler() {
                    const checkout = payments.checkout("price_123", true, "org_123");
                    const portal = payments.billingPortal("cus_123");
                    const subscription = payments.subscription("sub_123");
                    const cancelled = payments.cancelSubscription("sub_123", false);
                    const customer = payments.request({ op: "customer", organization_id: "org_123", email: "ann@example.com" });
                    return {
                        checkout: checkout.url,
                        portal: portal.url,
                        status: subscription.status,
                        cancelled: cancelled.status,
                        customer: customer.stripe_customer_id,
                    };
                },
            },
        });
        "#,
    )
    .unwrap();

    let host = Recorder::new().paying(vec![
        json!({ "url": "https://checkout.stripe.test/c" }),
        json!({ "url": "https://billing.stripe.test/p" }),
        json!({ "status": "active" }),
        json!({ "status": "canceling" }),
        json!({ "stripe_customer_id": "cus_123" }),
    ]);

    let out = call(&functions, "billing", "{}", host.clone()).unwrap();
    assert_eq!(
        out,
        r#"{"checkout":"https://checkout.stripe.test/c","portal":"https://billing.stripe.test/p","status":"active","cancelled":"canceling","customer":"cus_123"}"#
    );

    let calls = host.calls();
    assert!(
        calls[0].contains(
            r#""op":"checkout","stripe_price_id":"price_123","recurring":true,"organization_id":"org_123""#
        ),
        "{calls:?}"
    );
    assert!(
        calls[1].contains(r#""op":"portal","stripe_customer_id":"cus_123""#),
        "{calls:?}"
    );
    assert!(
        calls[2].contains(r#""op":"subscription","id":"sub_123""#),
        "{calls:?}"
    );
    assert!(
        calls[3].contains(r#""op":"cancel","id":"sub_123","at_period_end":false"#),
        "{calls:?}"
    );
    assert!(
        calls[4]
            .contains(r#""op":"customer","organization_id":"org_123","email":"ann@example.com""#),
        "{calls:?}"
    );
}

/// AI requests carry tools and tool-call messages through unchanged, and the
/// wrapper always gives the caller a `tool_calls` array back.
#[test]
fn ai_tools_round_trip_through_the_wrapper() {
    let functions = build(
        "ai_tools",
        r#"
        import { defineFunctions, ai } from "apiplant";
        export default defineFunctions({
            draft: {
                permission: "public",
                handler() {
                    const first = ai.chat("hello");
                    const second = ai.chat({
                        messages: [
                            { role: "user", content: "Look this up." },
                            {
                                role: "assistant",
                                content: "",
                                tool_calls: [{ id: "call_1", name: "lookup_note", input: { id: "note-1" } }],
                            },
                            { role: "tool", tool_call_id: "call_1", content: "{\"id\":\"note-1\"}" },
                        ],
                        tools: [{
                            name: "lookup_note",
                            description: "Load one note by id.",
                            input_schema: { type: "object", properties: { id: { type: "string" } } },
                        }],
                    });
                    return {
                        first_calls: first.tool_calls.length,
                        second_calls: second.tool_calls.length,
                        tool: second.tool_calls[0].name,
                    };
                },
            },
        });
        "#,
    )
    .unwrap();

    let host = Recorder::new().chatting(vec![
        json!({ "text": "hello", "provider": "custom", "model": "local" }),
        json!({
            "text": "",
            "provider": "custom",
            "model": "local",
            "tool_calls": [{ "id": "call_2", "name": "lookup_note", "input": { "id": "note-2" } }]
        }),
    ]);

    let out = call(&functions, "draft", "{}", host.clone()).unwrap();
    assert_eq!(
        out,
        r#"{"first_calls":0,"second_calls":1,"tool":"lookup_note"}"#
    );

    let calls = host.calls();
    assert!(
        calls[0].contains(r#"{"messages":[{"role":"user","content":"hello"}]}"#),
        "{calls:?}"
    );
    assert!(
        calls[1].contains(
            r#""tool_calls":[{"id":"call_1","name":"lookup_note","input":{"id":"note-1"}}]"#
        ),
        "{calls:?}"
    );
    assert!(
        calls[1]
            .contains(r#""role":"tool","tool_call_id":"call_1","content":"{\"id\":\"note-1\"}""#),
        "{calls:?}"
    );
    assert!(
        calls[1].contains(
            r#""tools":[{"name":"lookup_note","description":"Load one note by id.","input_schema":{"type":"object","properties":{"id":{"type":"string"}}}}]"#
        ),
        "{calls:?}"
    );
}

/// A hook reads its context through the same module, and rejects the request by
/// throwing rather than by returning a magic shape.
#[test]
fn a_hook_reads_its_context_and_can_reject() {
    let functions = build(
        "guard",
        r#"
        import { defineFunctions, hook, BadRequest } from "apiplant";
        export default defineFunctions({
            guard: {
                handler() {
                    const context = hook();
                    if (!context) throw new Error("not a hook");
                    if (context.data?.title === "spam") throw new BadRequest("no spam");
                    return { data: { ...context.data, title: context.data.title.trim() } };
                },
            },
        });
        "#,
    )
    .unwrap();

    let mut host = Recorder::new();
    host.hook = json!({
        "event": "before_create",
        "action": "create",
        "resource": "note",
        "data": { "title": "  hello  " },
    })
    .to_string();

    let out = call(&functions, "guard", "{}", host.clone()).unwrap();
    assert_eq!(out, r#"{"data":{"title":"hello"}}"#);

    host.hook = json!({ "event": "before_create", "data": { "title": "spam" } }).to_string();
    assert_eq!(
        call(&functions, "guard", "{}", host).unwrap_err(),
        "no spam"
    );
}

/// The long form still works: a module that imports nothing keeps its `manifest`
/// array and one export per entry, so nothing written before the package broke.
#[test]
fn the_import_free_form_still_loads() {
    let functions = build(
        "plain",
        r#"
        export const manifest = [{ name: "hello", permission: "public" }];
        export function hello(input: { name: string }, ctx: Ctx) {
            return { hi: input.name, caller: ctx.principalId() };
        }
        "#,
    )
    .unwrap();

    assert_eq!(
        call(&functions, "hello", r#"{"name":"world"}"#, Recorder::new()).unwrap(),
        r#"{"hi":"world","caller":""}"#
    );
}
