//! # apiplant-function
//!
//! Write an apiplant function without the ABI boilerplate.
//!
//! Instead of hand-implementing the [`apiplant_abi`] traits, exporting a root
//! module, and shuttling JSON in and out by hand, you write one ordinary typed
//! function and call [`function!`]:
//!
//! ```ignore
//! use apiplant_function::prelude::*;
//!
//! #[derive(serde::Deserialize, Default)]
//! struct Config { #[serde(default)] greeting: String }
//!
//! #[derive(serde::Deserialize)]
//! struct Input { name: String }
//!
//! #[derive(serde::Serialize)]
//! struct Output { message: String }
//!
//! fn greet(ctx: &Context<Config>, input: Input) -> Result<Output, String> {
//!     Ok(Output { message: format!("{}, {}!", ctx.config().greeting, input.name) })
//! }
//!
//! apiplant_function::function! {
//!     name: "greet",
//!     description: "Greets a person",
//!     method: Post,
//!     visibility: Public,
//!     handler: greet,
//! }
//! ```
//!
//! The macro generates the root module, reads/writes JSON, resolves typed config
//! and input, and turns your `Err(_)` into a `400`. Types are inferred from the
//! handler's signature — you never name them twice.
//!
//! ## Functions as lifecycle hooks
//!
//! The same function can also be attached to a resource's lifecycle from
//! `models/<name>.toml`, in which case [`Context::hook`] carries the operation's
//! context — the row created or fetched, the rows a list returned, the request
//! URL, the caller's auth status — and the [`reply`] helpers say what should
//! happen next:
//!
//! ```ignore
//! fn audit(ctx: &Context<()>, _input: serde_json::Value) -> Result<serde_json::Value, String> {
//!     let Some(hook) = ctx.hook() else { return Ok(reply::proceed()) };
//!     ctx.info(&format!("{} on {} by {:?}", hook.event, hook.resource, hook.principal_id));
//!     Ok(reply::proceed())
//! }
//! ```

use abi_stable::std_types::{RBox, RResult, RStr};
use apiplant_abi::{HostApi_TO, LogLevel};

/// The handle a function receives for one invocation.
///
/// It carries the function's typed, already-deserialized [config](Self::config),
/// the [caller's id](Self::principal_id), and a borrow of the host so you can
/// [query the database](Self::query). Construct it via the [`function!`] macro —
/// you won't build one yourself.
pub struct Context<'a, 'h, C> {
    host: &'a HostApi_TO<'h, RBox<()>>,
    config: C,
    principal_id: String,
    hook: Option<Hook>,
}

impl<'a, 'h, C> Context<'a, 'h, C> {
    /// Internal constructor used by generated code.
    #[doc(hidden)]
    pub fn __new(
        host: &'a HostApi_TO<'h, RBox<()>>,
        config: C,
        principal_id: String,
        hook: Option<Hook>,
    ) -> Self {
        Context {
            host,
            config,
            principal_id,
            hook,
        }
    }

    /// The lifecycle-hook context when this call came from a resource hook, or
    /// `None` when the function was invoked directly over HTTP.
    ///
    /// This is where the data *around* the operation lives: the row that was
    /// created, fetched or deleted, the rows a list returned, the request URL,
    /// and the caller's auth status.
    ///
    /// ```ignore
    /// match ctx.hook() {
    ///     Some(h) if h.is_before() => validate(h.data()),
    ///     Some(h) => audit(h.event(), h.row()),
    ///     None => {} // plain HTTP call
    /// }
    /// ```
    pub fn hook(&self) -> Option<&Hook> {
        self.hook.as_ref()
    }

    /// The function's resolved, typed configuration (`functions/<name>.toml`).
    pub fn config(&self) -> &C {
        &self.config
    }

    /// The authenticated caller's user id, or `""` when the endpoint is public
    /// and the caller is anonymous.
    pub fn principal_id(&self) -> &str {
        &self.principal_id
    }

    /// Run a `SELECT` (or `WITH`) and get the rows as JSON objects.
    pub fn query(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<Vec<serde_json::Value>, String> {
        match self.raw(sql, params)? {
            serde_json::Value::Array(rows) => Ok(rows),
            other => Err(format!("expected rows, got {other}")),
        }
    }

    /// Run a query expected to return at most one row.
    pub fn query_one(
        &self,
        sql: &str,
        params: &[serde_json::Value],
    ) -> Result<Option<serde_json::Value>, String> {
        Ok(self.query(sql, params)?.into_iter().next())
    }

    /// Run an `INSERT`/`UPDATE`/`DELETE` and get the number of affected rows.
    pub fn execute(&self, sql: &str, params: &[serde_json::Value]) -> Result<u64, String> {
        match self.raw(sql, params)? {
            serde_json::Value::Object(map) => Ok(map
                .get("rows_affected")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)),
            serde_json::Value::Array(rows) => Ok(rows.len() as u64),
            _ => Ok(0),
        }
    }

    fn raw(&self, sql: &str, params: &[serde_json::Value]) -> Result<serde_json::Value, String> {
        let request = serde_json::json!({ "sql": sql, "params": params }).to_string();
        match self.host.query(RStr::from_str(request.as_str())) {
            RResult::ROk(s) => serde_json::from_str(s.as_str()).map_err(|e| e.to_string()),
            RResult::RErr(e) => Err(e.into_string()),
        }
    }

    /// Log through the host's `tracing` subscriber.
    pub fn log(&self, level: LogLevel, message: &str) {
        self.host.log(level, RStr::from_str(message));
    }

    /// Log at INFO.
    pub fn info(&self, message: &str) {
        self.log(LogLevel::Info, message);
    }

    /// Log at WARN.
    pub fn warn(&self, message: &str) {
        self.log(LogLevel::Warn, message);
    }

    /// Log at ERROR.
    pub fn error(&self, message: &str) {
        self.log(LogLevel::Error, message);
    }

    /// Log at DEBUG.
    pub fn debug(&self, message: &str) {
        self.log(LogLevel::Debug, message);
    }
}

/// Everything the host knows about the operation a hook fired for.
///
/// Reachable through [`Context::hook`]. Every field is optional on the wire, so
/// a function written against an older host still loads; unknown fields are
/// ignored, so a newer host can add more.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct Hook {
    /// The lifecycle event, e.g. `"before_create"` or `"after_list"`.
    pub event: String,
    /// The operation: `"list"`, `"read"`, `"create"`, `"update"` or `"delete"`.
    pub action: String,
    /// `"before"` or `"after"`.
    pub phase: String,
    /// The resource the hook is attached to, e.g. `"post"`.
    pub resource: String,
    /// Path and query string of the request that triggered the hook.
    pub url: String,
    /// HTTP method of that request.
    pub method: String,
    /// Parsed query parameters.
    pub query: std::collections::BTreeMap<String, String>,
    /// Whether the caller is authenticated.
    pub authenticated: bool,
    /// The caller's user id, when authenticated.
    pub principal_id: Option<String>,
    /// The caller's active organisation, when one is resolved.
    pub organization_id: Option<String>,
    /// The caller's role in that organisation, when they have one.
    pub role: Option<String>,
    /// The id in the URL for single-record operations (read/update/delete).
    pub record_id: Option<String>,
    /// The submitted body on `before_create` / `before_update`.
    pub data: Option<serde_json::Value>,
    /// The row created, fetched, updated or about to be deleted.
    pub row: Option<serde_json::Value>,
    /// The rows a list returned, on `after_list`.
    pub rows: Option<Vec<serde_json::Value>>,
}

impl Hook {
    /// Parse a hook context, or `None` when the string is empty or malformed
    /// (i.e. this was a plain HTTP invocation).
    pub fn parse(json: &str) -> Option<Hook> {
        if json.trim().is_empty() {
            return None;
        }
        serde_json::from_str(json).ok()
    }

    /// Whether this hook runs before the database operation (and so can still
    /// rewrite the payload or abort).
    pub fn is_before(&self) -> bool {
        self.phase == "before"
    }

    /// Whether this hook runs after the operation succeeded.
    pub fn is_after(&self) -> bool {
        self.phase == "after"
    }

    /// The submitted body, or `null` when the event carries none.
    pub fn data(&self) -> &serde_json::Value {
        self.data.as_ref().unwrap_or(&serde_json::Value::Null)
    }

    /// The row in play, or `null` when the event carries none.
    pub fn row(&self) -> &serde_json::Value {
        self.row.as_ref().unwrap_or(&serde_json::Value::Null)
    }

    /// The rows a list returned; empty for every other event.
    pub fn rows(&self) -> &[serde_json::Value] {
        self.rows.as_deref().unwrap_or(&[])
    }

    /// Read a field from whichever subject the event carries — the submitted
    /// `data` for `before_create`/`before_update`, else the `row`.
    pub fn field(&self, name: &str) -> Option<&serde_json::Value> {
        let subject = if self.data.is_some() {
            self.data()
        } else {
            self.row()
        };
        subject.get(name)
    }
}

/// What a hook handler returns to the host.
///
/// A hook's `Ok` value is a JSON object the host reads as an instruction. These
/// helpers build it; anything else (including `{}` or `null`) means "carry on
/// unchanged", so an observational hook can simply return
/// `Ok(serde_json::Value::Null)`.
///
/// ```ignore
/// fn guard(ctx: &Context<()>, _input: serde_json::Value) -> Result<serde_json::Value, String> {
///     let Some(h) = ctx.hook() else { return Ok(reply::proceed()) };
///     if h.field("title").and_then(|t| t.as_str()).unwrap_or("").is_empty() {
///         return Ok(reply::abort(422, "title is required"));
///     }
///     Ok(reply::proceed())
/// }
/// ```
pub mod reply {
    use serde_json::{json, Value};

    /// Continue with the payload unchanged.
    pub fn proceed() -> Value {
        json!({})
    }

    /// Replace the payload (`before_create`/`before_update`) or the response
    /// body (any `after_*` hook) with `data`.
    pub fn replace(data: Value) -> Value {
        json!({ "data": data })
    }

    /// Abort the request with an HTTP status and message. Statuses outside
    /// `400..=599` are clamped to `400` by the host.
    pub fn abort(status: u16, message: impl Into<String>) -> Value {
        json!({ "error": { "status": status, "message": message.into() } })
    }
}

/// The glue every generated `invoke` calls: parse config + input, run the
/// handler, serialize the result. Type parameters are inferred from `handler`.
#[doc(hidden)]
pub fn invoke_handler<C, I, O, E, F>(
    host: &HostApi_TO<'_, RBox<()>>,
    input: RStr<'_>,
    handler: F,
) -> RResult<abi_stable::std_types::RString, abi_stable::std_types::RString>
where
    C: serde::de::DeserializeOwned + Default,
    I: serde::de::DeserializeOwned,
    O: serde::Serialize,
    E: core::fmt::Display,
    F: FnOnce(&Context<'_, '_, C>, I) -> Result<O, E>,
{
    use abi_stable::std_types::RString;

    let config: C = serde_json::from_str(host.config().as_str()).unwrap_or_default();
    let principal_id = host.principal_id().into_string();
    let hook = Hook::parse(host.hook().as_str());

    let input: I = match serde_json::from_str(input.as_str()) {
        Ok(v) => v,
        Err(e) => return RResult::RErr(RString::from(format!("invalid input: {e}"))),
    };

    let ctx = Context::__new(host, config, principal_id, hook);
    match handler(&ctx, input) {
        Ok(output) => match serde_json::to_string(&output) {
            Ok(s) => RResult::ROk(RString::from(s)),
            Err(e) => RResult::RErr(RString::from(format!("failed to serialize output: {e}"))),
        },
        Err(e) => RResult::RErr(RString::from(e.to_string())),
    }
}

/// Produce the JSON Schema for a handler's `Input` type, inferred from the
/// handler's signature. Used by [`function!`] to type the request body in the
/// OpenAPI docs. Returns `""` when the `schema` feature is off.
#[doc(hidden)]
#[cfg(feature = "schema")]
pub fn input_schema_json<C, I, O, E, F>(_handler: &F) -> String
where
    F: Fn(&Context<'_, '_, C>, I) -> Result<O, E>,
    I: schemars::JsonSchema,
{
    serde_json::to_string(&schemars::schema_for!(I)).unwrap_or_default()
}

/// Produce the JSON Schema for a handler's `Output` (the `Ok` type).
#[doc(hidden)]
#[cfg(feature = "schema")]
pub fn output_schema_json<C, I, O, E, F>(_handler: &F) -> String
where
    F: Fn(&Context<'_, '_, C>, I) -> Result<O, E>,
    O: schemars::JsonSchema,
{
    serde_json::to_string(&schemars::schema_for!(O)).unwrap_or_default()
}

#[doc(hidden)]
#[cfg(not(feature = "schema"))]
pub fn input_schema_json<C, I, O, E, F>(_handler: &F) -> String
where
    F: Fn(&Context<'_, '_, C>, I) -> Result<O, E>,
{
    String::new()
}

#[doc(hidden)]
#[cfg(not(feature = "schema"))]
pub fn output_schema_json<C, I, O, E, F>(_handler: &F) -> String
where
    F: Fn(&Context<'_, '_, C>, I) -> Result<O, E>,
{
    String::new()
}

/// A curated set of imports for function authors: `use apiplant_function::prelude::*;`.
pub mod prelude {
    pub use crate::{reply, Context, Hook};
    pub use apiplant_abi::{HttpMethod, LogLevel, Visibility};
    /// `#[derive(JsonSchema)]` for typed OpenAPI (with the `schema` feature).
    #[cfg(feature = "schema")]
    pub use schemars::JsonSchema;
}

/// Re-exports the generated code depends on. Not a stable public API.
#[doc(hidden)]
pub mod __rt {
    pub use crate::{input_schema_json, invoke_handler, output_schema_json, Context, Hook};
    pub use abi_stable::export_root_module;
    pub use abi_stable::prefix_type::PrefixTypeTrait;
    pub use abi_stable::sabi_extern_fn;
    pub use abi_stable::sabi_trait::TD_Opaque;
    pub use abi_stable::std_types::{RBox, RResult, RStr, RString};
    pub use apiplant_abi::{
        BoxedFunction, Function, FunctionMod, FunctionMod_Ref, FunctionManifest, Function_TO,
        HostApi_TO, HttpMethod, Visibility,
    };
}

/// Define and export an apiplant function from a plain handler.
///
/// Fields (`version` and `role` optional):
///
/// ```ignore
/// apiplant_function::function! {
///     name: "greet",               // URL segment → /functions/greet
///     version: "1.2.0",            // optional; defaults to CARGO_PKG_VERSION
///     description: "Greets people",
///     method: Post,                // Get | Post | Put | Delete
///     visibility: Public,          // Public | Authenticated | RoleGated | Private
///     role: "admin",              // optional; required when visibility: RoleGated
///     handler: greet,              // fn(&Context<C>, I) -> Result<O, E>
/// }
/// ```
#[macro_export]
macro_rules! function {
    (
        name: $name:expr,
        $(version: $version:expr,)?
        description: $description:expr,
        method: $method:ident,
        visibility: $visibility:ident,
        $(role: $role:expr,)?
        handler: $handler:path
        $(,)?
    ) => {
        #[doc(hidden)]
        pub mod __apiplant_generated_function {
            use super::*;

            struct __ApiplantFunction;

            impl $crate::__rt::Function for __ApiplantFunction {
                fn manifest(&self) -> $crate::__rt::FunctionManifest {
                    #[allow(unused_mut)]
                    let mut version =
                        $crate::__rt::RString::from(::core::env!("CARGO_PKG_VERSION"));
                    $( version = $crate::__rt::RString::from($version); )?

                    #[allow(unused_mut)]
                    let mut role = $crate::__rt::RString::new();
                    $( role = $crate::__rt::RString::from($role); )?

                    $crate::__rt::FunctionManifest {
                        name: $crate::__rt::RString::from($name),
                        version,
                        description: $crate::__rt::RString::from($description),
                        visibility: $crate::__rt::Visibility::$visibility,
                        role,
                        method: $crate::__rt::HttpMethod::$method,
                        config_schema: $crate::__rt::RString::new(),
                        input_schema: $crate::__rt::RString::from(
                            $crate::__rt::input_schema_json(&$handler),
                        ),
                        output_schema: $crate::__rt::RString::from(
                            $crate::__rt::output_schema_json(&$handler),
                        ),
                    }
                }

                fn invoke(
                    &self,
                    host: $crate::__rt::HostApi_TO<'_, $crate::__rt::RBox<()>>,
                    input: $crate::__rt::RStr<'_>,
                ) -> $crate::__rt::RResult<$crate::__rt::RString, $crate::__rt::RString> {
                    $crate::__rt::invoke_handler(&host, input, $handler)
                }
            }

            #[$crate::__rt::export_root_module]
            fn __apiplant_root_module() -> $crate::__rt::FunctionMod_Ref {
                use $crate::__rt::PrefixTypeTrait as _;
                $crate::__rt::FunctionMod {
                    new: __apiplant_new,
                }
                .leak_into_prefix()
            }

            #[$crate::__rt::sabi_extern_fn]
            fn __apiplant_new() -> $crate::__rt::BoxedFunction {
                $crate::__rt::Function_TO::from_value(__ApiplantFunction, $crate::__rt::TD_Opaque)
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use abi_stable::sabi_trait::TD_Opaque;
    use abi_stable::std_types::{RResult, RStr, RString};
    use apiplant_abi::{HostApi, HostApi_TO, LogLevel};
    use serde::{Deserialize, Serialize};
    use std::sync::Mutex;

    struct MockHost {
        config_json: String,
        principal_id: String,
        hook_json: String,
        query_result: Result<String, String>,
        requests: Mutex<Vec<String>>,
        logs: Mutex<Vec<(LogLevel, String)>>,
    }

    impl MockHost {
        fn success(config_json: &str, principal_id: &str, response: serde_json::Value) -> Self {
            Self {
                config_json: config_json.into(),
                principal_id: principal_id.into(),
                hook_json: String::new(),
                query_result: Ok(response.to_string()),
                requests: Mutex::new(Vec::new()),
                logs: Mutex::new(Vec::new()),
            }
        }

        fn with_hook(mut self, hook: serde_json::Value) -> Self {
            self.hook_json = hook.to_string();
            self
        }
    }

    impl HostApi for MockHost {
        fn query(&self, request: RStr<'_>) -> RResult<RString, RString> {
            self.requests
                .lock()
                .unwrap()
                .push(request.as_str().to_string());
            match &self.query_result {
                Ok(json) => RResult::ROk(RString::from(json.as_str())),
                Err(err) => RResult::RErr(RString::from(err.as_str())),
            }
        }

        fn log(&self, level: LogLevel, message: RStr<'_>) {
            self.logs
                .lock()
                .unwrap()
                .push((level, message.as_str().to_string()));
        }

        fn config(&self) -> RString {
            self.config_json.clone().into()
        }

        fn principal_id(&self) -> RString {
            self.principal_id.clone().into()
        }

        fn hook(&self) -> RString {
            self.hook_json.clone().into()
        }
    }

    #[derive(Deserialize)]
    struct Config {
        greeting: String,
    }

    impl Default for Config {
        fn default() -> Self {
            Self {
                greeting: "Hello".into(),
            }
        }
    }

    #[derive(Deserialize)]
    struct Input {
        name: String,
    }

    #[derive(Serialize, serde::Deserialize, schemars::JsonSchema)]
    struct Output {
        message: String,
    }

    #[test]
    fn context_bridges_queries_execution_and_principal_id() {
        let host = MockHost::success("{}", "user-123", serde_json::json!([{ "n": 1 }]));
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let ctx = Context::__new(&host, (), "user-123".into(), None);

        let rows = ctx
            .query("SELECT count(*) AS n", &[serde_json::json!(true)])
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(ctx.principal_id(), "user-123");

        let request = &host
            .config()
            .into_string();
        assert_eq!(request, "{}");
    }

    #[test]
    fn context_execute_and_logging_use_host_bridge() {
        let host = MockHost::success("{}", "user-123", serde_json::json!({ "rows_affected": 3 }));
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let ctx = Context::__new(&host, (), "user-123".into(), None);

        assert_eq!(
            ctx.execute("DELETE FROM apiplant_post", &[]).unwrap(),
            3
        );
        ctx.warn("careful");
    }

    #[test]
    fn invoke_handler_uses_default_config_when_host_config_is_invalid() {
        let host = MockHost::success("{not-json", "u1", serde_json::json!([]));
        let host = HostApi_TO::from_value(host, TD_Opaque);

        let result = invoke_handler::<Config, Input, Output, String, _>(
            &host,
            RStr::from_str(r#"{"name":"Ann"}"#),
            |ctx, input| {
                Ok(Output {
                    message: format!("{}, {}!", ctx.config().greeting, input.name),
                })
            },
        );

        let json = match result {
            RResult::ROk(v) => v.into_string(),
            RResult::RErr(e) => panic!("unexpected error: {}", e.into_string()),
        };
        assert!(json.contains("Hello, Ann!"));
    }

    #[test]
    fn invoke_handler_rejects_invalid_input_json() {
        let host = MockHost::success("{}", "u1", serde_json::json!([]));
        let host = HostApi_TO::from_value(host, TD_Opaque);

        let result = invoke_handler::<Config, Input, Output, String, _>(
            &host,
            RStr::from_str("{"),
            |_ctx, _input| Ok(Output {
                message: "never".into(),
            }),
        );

        match result {
            RResult::ROk(v) => panic!("unexpected success: {}", v.into_string()),
            RResult::RErr(e) => assert!(e.into_string().contains("invalid input")),
        }
    }

    fn hook_context() -> serde_json::Value {
        serde_json::json!({
            "event": "after_create",
            "action": "create",
            "phase": "after",
            "resource": "post",
            "url": "/api/post?draft=true",
            "method": "POST",
            "query": { "draft": "true" },
            "authenticated": true,
            "principal_id": "11111111-1111-1111-1111-111111111111",
            "organization_id": "22222222-2222-2222-2222-222222222222",
            "role": "admin",
            "record_id": null,
            "data": null,
            "row": { "id": "33333333-3333-3333-3333-333333333333", "title": "Hi" },
            "rows": null,
        })
    }

    #[test]
    fn context_exposes_hook_data_when_invoked_as_a_hook() {
        let host = MockHost::success("{}", "u1", serde_json::json!([])).with_hook(hook_context());
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let hook = Hook::parse(host.hook().as_str());
        let ctx = Context::__new(&host, (), "u1".into(), hook);

        let hook = ctx.hook().expect("hook context should be present");
        assert_eq!(hook.event, "after_create");
        assert_eq!(hook.action, "create");
        assert!(hook.is_after());
        assert!(!hook.is_before());
        assert_eq!(hook.resource, "post");
        assert_eq!(hook.url, "/api/post?draft=true");
        assert_eq!(hook.method, "POST");
        assert_eq!(hook.query.get("draft").map(String::as_str), Some("true"));
        assert!(hook.authenticated);
        assert_eq!(hook.role.as_deref(), Some("admin"));
        assert!(hook.organization_id.is_some());
        assert_eq!(hook.record_id, None);
        assert_eq!(hook.row()["title"], "Hi");
        assert!(hook.data().is_null());
        assert!(hook.rows().is_empty());
        // `field` reads the row when no submitted data is present.
        assert_eq!(hook.field("title").and_then(|v| v.as_str()), Some("Hi"));
    }

    #[test]
    fn hook_is_absent_for_plain_http_invocations() {
        let host = MockHost::success("{}", "u1", serde_json::json!([]));
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let ctx = Context::__new(&host, (), "u1".into(), Hook::parse(host.hook().as_str()));

        assert!(ctx.hook().is_none());
        assert!(Hook::parse("").is_none());
        assert!(Hook::parse("   ").is_none());
        assert!(Hook::parse("{not json").is_none());
    }

    #[test]
    fn hook_reads_submitted_data_on_before_events_and_lists_on_after_list() {
        let before = Hook::parse(
            &serde_json::json!({
                "event": "before_create",
                "phase": "before",
                "data": { "title": "Draft" },
            })
            .to_string(),
        )
        .unwrap();
        assert!(before.is_before());
        assert_eq!(before.field("title").and_then(|v| v.as_str()), Some("Draft"));
        assert!(before.row().is_null());

        let listed = Hook::parse(
            &serde_json::json!({
                "event": "after_list",
                "phase": "after",
                "rows": [{ "id": "a" }, { "id": "b" }],
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(listed.rows().len(), 2);
        assert_eq!(listed.rows()[1]["id"], "b");
    }

    #[test]
    fn hook_tolerates_missing_and_unknown_fields() {
        let sparse = Hook::parse(r#"{"event":"before_delete","surprise":42}"#).unwrap();
        assert_eq!(sparse.event, "before_delete");
        assert_eq!(sparse.resource, "");
        assert!(!sparse.authenticated);
        assert!(sparse.principal_id.is_none());
    }

    #[test]
    fn reply_helpers_build_the_host_protocol() {
        assert_eq!(reply::proceed(), serde_json::json!({}));
        assert_eq!(
            reply::replace(serde_json::json!({ "title": "clean" })),
            serde_json::json!({ "data": { "title": "clean" } })
        );
        assert_eq!(
            reply::abort(422, "title is required"),
            serde_json::json!({ "error": { "status": 422, "message": "title is required" } })
        );
    }

    #[test]
    fn invoke_handler_passes_hook_context_through_to_the_handler() {
        let host = MockHost::success("{}", "u1", serde_json::json!([])).with_hook(hook_context());
        let host = HostApi_TO::from_value(host, TD_Opaque);

        let result = invoke_handler::<(), serde_json::Value, serde_json::Value, String, _>(
            &host,
            RStr::from_str(r#"{"id":"33333333-3333-3333-3333-333333333333","title":"Hi"}"#),
            |ctx, input| {
                let hook = ctx.hook().ok_or("expected a hook context")?;
                assert_eq!(input["title"], "Hi");
                Ok(reply::replace(serde_json::json!({
                    "event": hook.event,
                    "title": hook.row()["title"],
                })))
            },
        );

        let json = match result {
            RResult::ROk(v) => v.into_string(),
            RResult::RErr(e) => panic!("unexpected error: {}", e.into_string()),
        };
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["data"]["event"], "after_create");
        assert_eq!(value["data"]["title"], "Hi");
    }

    #[derive(Deserialize, schemars::JsonSchema)]
    struct SchemaInput {
        name: String,
    }

    #[derive(Serialize, schemars::JsonSchema)]
    struct SchemaOutput {
        ok: bool,
    }

    #[test]
    fn schema_generation_is_typed() {
        let handler = |_ctx: &Context<'_, '_, ()>, input: SchemaInput| -> Result<SchemaOutput, String> {
            Ok(SchemaOutput { ok: !input.name.is_empty() })
        };

        let input_schema = input_schema_json::<(), SchemaInput, SchemaOutput, String, _>(&handler);
        let output_schema =
            output_schema_json::<(), SchemaInput, SchemaOutput, String, _>(&handler);

        assert!(input_schema.contains("\"name\""));
        assert!(output_schema.contains("\"ok\""));
    }
}
