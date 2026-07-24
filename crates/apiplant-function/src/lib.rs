//! # apiplant-function
//!
//! Write an apiplant function without the ABI boilerplate.
//!
//! Instead of hand-implementing the [`apiplant_abi`] traits, exporting a root
//! module, and shuttling JSON in and out by hand, you write one ordinary typed
//! function and call [`function!`]:
//!
//! ```no_run
//! use apiplant_function::prelude::*;
//!
//! #[derive(serde::Deserialize, Default)]
//! struct Config { #[serde(default)] greeting: String }
//!
//! #[derive(serde::Deserialize, JsonSchema)]
//! struct Input { name: String }
//!
//! #[derive(serde::Serialize, JsonSchema)]
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
//! # fn main() {}
//! ```
//!
//! The macro generates the root module, reads/writes JSON, resolves typed config
//! and input, and turns your `Err(_)` into a `400`. Types are inferred from the
//! handler's signature — you never name them twice. With the default `schema`
//! feature the input and output types must also derive [`JsonSchema`](prelude::JsonSchema)
//! so the endpoint shows up typed in the OpenAPI docs.
//!
//! Use [`functions!`] to export several from one library — each with its own
//! name, manifest and handler.
//!
//! ## Functions as lifecycle hooks
//!
//! A function can also be attached to a resource's lifecycle from
//! `models/<name>.toml`, in which case [`Context::hook`] carries the operation's
//! context — the row created or fetched, the rows a list returned, the request
//! URL, the caller's auth status — and the [`reply`] helpers say what should
//! happen next. One function per event, so a handler never has to work out why
//! it was called:
//!
//! ```no_run
//! # use apiplant_function::prelude::*;
//! fn post_after_create(ctx: &Context<()>, row: serde_json::Value) -> Result<serde_json::Value, String> {
//!     let actor = ctx.hook().and_then(|hook| hook.principal_id.clone());
//!     ctx.info(&format!("post {} created by {actor:?}", row["id"]));
//!     Ok(reply::proceed())
//! }
//!
//! apiplant_function::functions! {
//!     {
//!         name: "post_after_create",
//!         description: "Records a newly created post",
//!         method: Post,
//!         visibility: Private,
//!         handler: post_after_create,
//!     },
//! }
//! # fn main() {}
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
    /// ```no_run
    /// # use apiplant_function::prelude::*;
    /// # fn validate(_data: &serde_json::Value) {}
    /// # fn audit(_event: &str, _row: &serde_json::Value) {}
    /// # fn example(ctx: &Context<()>) {
    /// match ctx.hook() {
    ///     Some(h) if h.is_before() => validate(h.data()),
    ///     Some(h) => audit(&h.event, h.row()),
    ///     None => {} // plain HTTP call
    /// }
    /// # }
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
/// ```no_run
/// # use apiplant_function::prelude::*;
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
///
/// Also the crate's panic firewall. [`apiplant_abi::Function::invoke`] is
/// reached through an `extern "C"` function pointer, and a panic that escapes
/// one of those does not unwind into the host — `abi_stable` detects it and
/// aborts the process. A `panic!`, `unwrap()` or index-out-of-bounds anywhere in
/// a handler would therefore take the whole server down with it, dropping every
/// other in-flight request. So the handler runs inside [`catch_unwind`] here,
/// while it is still on the function's side of the boundary, and a panic becomes
/// an [`INTERNAL_ERROR_PREFIX`](apiplant_abi::INTERNAL_ERROR_PREFIX) error that
/// the host reports as a `500`.
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
    use std::panic::{catch_unwind, AssertUnwindSafe};

    // `AssertUnwindSafe`: nothing observable is shared across the boundary that a
    // half-finished handler could leave inconsistent. `host` is borrowed and its
    // methods are the host's own business, the config and input are moved in and
    // dropped on unwind, and on a panic we return immediately without touching
    // anything the handler may have left mid-update.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        run_handler::<C, I, O, E, F>(host, input, handler)
    }));

    match outcome {
        Ok(result) => result,
        // The default panic hook has already printed the message and backtrace to
        // stderr, so the detail is in the operator's log either way; this carries
        // enough for the host to log a useful line without echoing it to the caller.
        // `&*payload`, not `&payload`: `Box<dyn Any + Send>` is itself `Any`, so
        // `&payload` would coerce by erasing the *box* and every downcast below
        // would miss, turning every panic message into "panicked".
        Err(payload) => RResult::RErr(RString::from(format!(
            "{}{}",
            apiplant_abi::INTERNAL_ERROR_PREFIX,
            panic_message(&*payload)
        ))),
    }
}

/// [`invoke_handler`] minus the panic firewall — everything here may unwind, and
/// [`invoke_handler`] is what stops it from reaching the ABI boundary.
fn run_handler<C, I, O, E, F>(
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

/// Recover the text from a caught panic payload. `panic!` with a literal yields
/// a `&str` and the formatting forms yield a `String`; anything else (a
/// `panic_any` with a custom type) has no text to show.
fn panic_message(payload: &(dyn core::any::Any + Send)) -> &str {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        s
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.as_str()
    } else {
        "panicked"
    }
}

/// One exported function: a manifest plus the handler that serves it.
///
/// Generated code builds one of these per entry in [`functions!`], which is what
/// lets a single library export several independently-named functions without
/// declaring a type for each. The `C`/`I`/`O`/`E` parameters are inferred from
/// the handler's signature, exactly as they are for a lone [`function!`].
/// The handler shape an [`Exported`] stands for, held only as a marker so the
/// inferred type parameters stay pinned to the struct.
type Signature<C, I, O, E> = fn(C, I) -> Result<O, E>;

#[doc(hidden)]
pub struct Exported<C, I, O, E, F> {
    manifest: apiplant_abi::FunctionManifest,
    handler: F,
    _signature: core::marker::PhantomData<Signature<C, I, O, E>>,
}

impl<C, I, O, E, F> Exported<C, I, O, E, F> {
    pub fn new(manifest: apiplant_abi::FunctionManifest, handler: F) -> Self {
        Exported {
            manifest,
            handler,
            _signature: core::marker::PhantomData,
        }
    }
}

impl<C, I, O, E, F> apiplant_abi::Function for Exported<C, I, O, E, F>
where
    C: serde::de::DeserializeOwned + Default,
    I: serde::de::DeserializeOwned,
    O: serde::Serialize,
    E: core::fmt::Display,
    F: Fn(&Context<'_, '_, C>, I) -> Result<O, E> + Send + Sync,
{
    fn manifest(&self) -> apiplant_abi::FunctionManifest {
        self.manifest.clone()
    }

    fn invoke(
        &self,
        host: HostApi_TO<'_, RBox<()>>,
        input: RStr<'_>,
    ) -> RResult<abi_stable::std_types::RString, abi_stable::std_types::RString> {
        invoke_handler(&host, input, &self.handler)
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
    pub use crate::{
        input_schema_json, invoke_handler, output_schema_json, Context, Exported, Hook,
    };
    pub use abi_stable::export_root_module;
    pub use abi_stable::prefix_type::PrefixTypeTrait;
    pub use abi_stable::sabi_extern_fn;
    pub use abi_stable::sabi_trait::TD_Opaque;
    pub use abi_stable::std_types::{RBox, RResult, RStr, RString, RVec};
    pub use apiplant_abi::{
        BoxedFunction, Function, FunctionMod, FunctionMod_Ref, FunctionManifest, Function_TO,
        HostApi_TO, HttpMethod, Visibility,
    };
}

/// Define and export **one** apiplant function from a plain handler.
///
/// Fields (`version` and `role` optional):
///
/// ```no_run
/// # use apiplant_function::prelude::*;
/// # type Json = serde_json::Value;
/// # fn greet(_ctx: &Context<()>, input: Json) -> Result<Json, String> { Ok(input) }
/// apiplant_function::function! {
///     name: "greet",               // URL segment → /functions/greet
///     version: "1.2.0",            // optional; defaults to CARGO_PKG_VERSION
///     description: "Greets people",
///     method: Post,                // Get | Post | Put | Delete
///     visibility: RoleGated,       // Public | Authenticated | RoleGated | Private
///     role: "admin",               // optional; required when visibility: RoleGated
///     handler: greet,              // fn(&Context<C>, I) -> Result<O, E>
/// }
/// # fn main() {}
/// ```
///
/// To export several functions from one library, use [`functions!`] — this is
/// exactly that macro with a single entry.
#[macro_export]
macro_rules! function {
    ( $($definition:tt)* ) => {
        $crate::functions! { { $($definition)* } }
    };
}

/// Define and export **several** apiplant functions from one library.
///
/// Each entry is an independent function with its own name, manifest and
/// handler — there is no shared dispatcher and no matching inside a handler.
/// This is how one crate provides a set of related endpoints, or a resource's
/// whole set of lifecycle hooks:
///
/// ```no_run
/// # use apiplant_function::prelude::*;
/// # type Json = serde_json::Value;
/// # fn post_before_create(_ctx: &Context<()>, input: Json) -> Result<Json, String> { Ok(input) }
/// # fn post_after_create(_ctx: &Context<()>, input: Json) -> Result<Json, String> { Ok(input) }
/// apiplant_function::functions! {
///     {
///         name: "post_before_create",
///         description: "Validates a post before it is stored.",
///         method: Post,
///         visibility: Private,
///         handler: post_before_create,
///     },
///     {
///         name: "post_after_create",
///         description: "Records a newly created post.",
///         method: Post,
///         visibility: Private,
///         handler: post_after_create,
///     },
/// }
/// # fn main() {}
/// ```
///
/// Then, in `models/post.toml`:
///
/// ```toml
/// [hooks]
/// before_create = "post_before_create"
/// after_create  = "post_after_create"
/// ```
///
/// Every entry takes the same fields as [`function!`], and each handler keeps
/// its own inferred `Config`/`Input`/`Output` types. Names must be unique within
/// a library; the host rejects duplicates at load time.
#[macro_export]
macro_rules! functions {
    (
        $(
            {
                name: $name:expr,
                $(version: $version:expr,)?
                description: $description:expr,
                method: $method:ident,
                visibility: $visibility:ident,
                $(role: $role:expr,)?
                handler: $handler:path
                $(,)?
            }
        ),+
        $(,)?
    ) => {
        #[doc(hidden)]
        pub mod __apiplant_generated_functions {
            use super::*;

            #[$crate::__rt::export_root_module]
            fn __apiplant_root_module() -> $crate::__rt::FunctionMod_Ref {
                use $crate::__rt::PrefixTypeTrait as _;
                $crate::__rt::FunctionMod {
                    new_functions: __apiplant_new_functions,
                }
                .leak_into_prefix()
            }

            #[$crate::__rt::sabi_extern_fn]
            fn __apiplant_new_functions() -> $crate::__rt::RVec<$crate::__rt::BoxedFunction> {
                let mut exported = $crate::__rt::RVec::new();
                $(
                    exported.push({
                        #[allow(unused_mut)]
                        let mut version =
                            $crate::__rt::RString::from(::core::env!("CARGO_PKG_VERSION"));
                        $( version = $crate::__rt::RString::from($version); )?

                        #[allow(unused_mut)]
                        let mut role = $crate::__rt::RString::new();
                        $( role = $crate::__rt::RString::from($role); )?

                        let manifest = $crate::__rt::FunctionManifest {
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
                        };
                        $crate::__rt::Function_TO::from_value(
                            $crate::__rt::Exported::new(manifest, $handler),
                            $crate::__rt::TD_Opaque,
                        )
                    });
                )+
                exported
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

    /// A panic must not escape as a panic: `Function::invoke` is reached through
    /// an `extern "C"` pointer, and `abi_stable` aborts the process rather than
    /// letting one unwind into the host.
    #[test]
    fn invoke_handler_turns_a_panicking_handler_into_an_internal_error() {
        let host = MockHost::success("{}", "u1", serde_json::json!([]));
        let host = HostApi_TO::from_value(host, TD_Opaque);

        let result = invoke_handler::<Config, Input, Output, String, _>(
            &host,
            RStr::from_str(r#"{"name":"Ann"}"#),
            |_ctx, _input| panic!("handler exploded"),
        );

        match result {
            RResult::ROk(v) => panic!("unexpected success: {}", v.into_string()),
            RResult::RErr(e) => {
                let msg = e.into_string();
                let detail = msg
                    .strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX)
                    .expect("a panic must be marked internal so the host answers 500, not 400");
                // The real message has to survive, or the operator's log says nothing.
                assert_eq!(detail, "handler exploded");
            }
        }
    }

    /// The same for the implicit panics people actually hit.
    #[test]
    fn invoke_handler_catches_panics_from_unwrap_and_indexing() {
        for (label, handler) in [
            (
                "unwrap",
                Box::new(|_: &Context<'_, '_, Config>, input: Input| -> Result<Output, String> {
                    // Derived from the input so clippy sees a real `Option`
                    // rather than a literal `None` it can flag at the call site.
                    let missing = input.name.strip_prefix("nonexistent-prefix");
                    Ok(Output {
                        message: missing.unwrap().to_string(),
                    })
                }) as Box<dyn Fn(&Context<'_, '_, Config>, Input) -> Result<Output, String>>,
            ),
            (
                "index",
                Box::new(|_: &Context<'_, '_, Config>, input: Input| -> Result<Output, String> {
                    // Indexed by input length so the compiler can't prove it's
                    // out of bounds and reject the test with `unconditional_panic`.
                    let empty: Vec<u8> = Vec::new();
                    let _ = empty[input.name.len()];
                    unreachable!()
                }),
            ),
        ] {
            let host = MockHost::success("{}", "u1", serde_json::json!([]));
            let host = HostApi_TO::from_value(host, TD_Opaque);
            let result = invoke_handler::<Config, Input, Output, String, _>(
                &host,
                RStr::from_str(r#"{"name":"Ann"}"#),
                handler,
            );
            match result {
                RResult::ROk(_) => panic!("{label}: expected an error"),
                RResult::RErr(e) => {
                    let msg = e.into_string();
                    assert!(
                        msg.starts_with(apiplant_abi::INTERNAL_ERROR_PREFIX),
                        "{label}: not marked internal: {msg}"
                    );
                    // "panicked" is the fallback for payloads with no text; these
                    // both carry a real message, so seeing it means the downcast
                    // erased the Box instead of its contents.
                    assert_ne!(
                        msg,
                        format!("{}panicked", apiplant_abi::INTERNAL_ERROR_PREFIX),
                        "{label}: panic message was lost"
                    );
                }
            }
        }
    }

    /// A handler that merely *returns* an error keeps the plain (400) channel —
    /// only faults get the internal marker.
    #[test]
    fn a_returned_error_is_not_marked_internal() {
        let host = MockHost::success("{}", "u1", serde_json::json!([]));
        let host = HostApi_TO::from_value(host, TD_Opaque);

        let result = invoke_handler::<Config, Input, Output, String, _>(
            &host,
            RStr::from_str(r#"{"name":"Ann"}"#),
            |_ctx, _input| Err("name is taken".to_string()),
        );

        match result {
            RResult::ROk(_) => panic!("expected an error"),
            RResult::RErr(e) => assert_eq!(e.into_string(), "name is taken"),
        }
    }

    /// The whole point: the same panic driven through the `extern "C"` vtable
    /// `abi_stable` builds. Before the firewall this aborted the test process.
    #[test]
    fn a_panic_does_not_cross_the_abi_boundary() {
        let manifest = apiplant_abi::FunctionManifest {
            name: "boom".into(),
            version: "0.0.0".into(),
            description: RString::new(),
            visibility: apiplant_abi::Visibility::Public,
            role: RString::new(),
            method: apiplant_abi::HttpMethod::Post,
            config_schema: RString::new(),
            input_schema: RString::new(),
            output_schema: RString::new(),
        };
        let exported = Exported::<Config, Input, Output, String, _>::new(
            manifest,
            |_ctx: &Context<'_, '_, Config>, _input: Input| -> Result<Output, String> {
                panic!("handler exploded")
            },
        );

        // Erase it exactly as a real library does, so `invoke` below travels
        // through the generated `extern "C"` function pointer.
        let boxed: apiplant_abi::BoxedFunction =
            apiplant_abi::Function_TO::from_value(exported, TD_Opaque);
        assert_eq!(boxed.manifest().name.as_str(), "boom");

        let host = HostApi_TO::from_value(
            MockHost::success("{}", "u1", serde_json::json!([])),
            TD_Opaque,
        );
        match boxed.invoke(host, RStr::from_str(r#"{"name":"Ann"}"#)) {
            RResult::ROk(v) => panic!("unexpected success: {}", v.into_string()),
            RResult::RErr(e) => assert!(e
                .into_string()
                .starts_with(apiplant_abi::INTERNAL_ERROR_PREFIX)),
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

    #[test]
    fn exported_functions_carry_their_own_manifest_and_handler() {
        use apiplant_abi::{Function, FunctionManifest, HttpMethod, Visibility};

        fn manifest(name: &str) -> FunctionManifest {
            FunctionManifest {
                name: RString::from(name),
                version: RString::from("1.0.0"),
                description: RString::from("test"),
                visibility: Visibility::Private,
                role: RString::new(),
                method: HttpMethod::Post,
                config_schema: RString::new(),
                input_schema: RString::new(),
                output_schema: RString::new(),
            }
        }

        // Two functions with different handlers — and different inferred input
        // types — as `functions!` builds them.
        let before: Exported<(), Input, Output, String, _> = Exported::new(
            manifest("post_before_create"),
            |_ctx: &Context<'_, '_, ()>, input: Input| {
                Ok(Output {
                    message: format!("before {}", input.name),
                })
            },
        );
        let after: Exported<(), Vec<i64>, Output, String, _> = Exported::new(
            manifest("post_after_list"),
            |_ctx: &Context<'_, '_, ()>, rows: Vec<i64>| {
                Ok(Output {
                    message: format!("after {}", rows.len()),
                })
            },
        );

        assert_eq!(before.manifest().name.as_str(), "post_before_create");
        assert_eq!(after.manifest().name.as_str(), "post_after_list");
        assert_eq!(before.manifest().version.as_str(), "1.0.0");

        let new_host = || {
            HostApi_TO::from_value(
                MockHost::success("{}", "u1", serde_json::json!([])),
                TD_Opaque,
            )
        };

        let first = match before.invoke(new_host(), RStr::from_str(r#"{"name":"Ann"}"#)) {
            RResult::ROk(v) => v.into_string(),
            RResult::RErr(e) => panic!("unexpected error: {}", e.into_string()),
        };
        assert!(first.contains("before Ann"));

        let second = match after.invoke(new_host(), RStr::from_str("[1,2,3]")) {
            RResult::ROk(v) => v.into_string(),
            RResult::RErr(e) => panic!("unexpected error: {}", e.into_string()),
        };
        assert!(second.contains("after 3"));
    }

    #[test]
    fn exported_functions_are_abi_trait_objects() {
        use apiplant_abi::{Function_TO, FunctionManifest, HttpMethod, Visibility};

        let exported: Exported<Config, Input, Output, String, _> = Exported::new(
            FunctionManifest {
                name: RString::from("greet"),
                version: RString::from("0.1.0"),
                description: RString::from("test"),
                visibility: Visibility::Public,
                role: RString::new(),
                method: HttpMethod::Post,
                config_schema: RString::new(),
                input_schema: RString::new(),
                output_schema: RString::new(),
            },
            |ctx: &Context<'_, '_, Config>, input: Input| {
                Ok(Output {
                    message: format!("{}, {}!", ctx.config().greeting, input.name),
                })
            },
        );

        // This is the exact conversion the `functions!` macro performs per entry.
        let boxed = Function_TO::from_value(exported, TD_Opaque);
        assert_eq!(boxed.manifest().name.as_str(), "greet");

        let host = MockHost::success(r#"{"greeting":"Hi"}"#, "u1", serde_json::json!([]));
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let reply = match boxed.invoke(host, RStr::from_str(r#"{"name":"Ann"}"#)) {
            RResult::ROk(v) => v.into_string(),
            RResult::RErr(e) => panic!("unexpected error: {}", e.into_string()),
        };
        assert!(reply.contains("Hi, Ann!"));
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
