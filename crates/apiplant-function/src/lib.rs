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

    /// Send an email through whichever provider the app configured in
    /// `[email]`, and get the provider's receipt back.
    ///
    /// The function doesn't know or care which provider that is: the same call
    /// goes out through SES, SendGrid, Brevo, Mailjet or a plain SMTP relay
    /// depending on one line of `main.toml`.
    ///
    /// ```no_run
    /// # use apiplant_function::prelude::*;
    /// # fn example(ctx: &Context<()>) -> Result<(), String> {
    /// ctx.send_email(
    ///     Email::to("ann@example.com")
    ///         .subject("Welcome")
    ///         .text("Glad you're here.")
    ///         .html("<p>Glad you're here.</p>"),
    /// )?;
    /// # Ok(()) }
    /// ```
    ///
    /// Errors when no provider is configured, when the message has no
    /// recipient, or when the provider refuses it. Whether that should fail the
    /// request is the caller's decision — a failed welcome email usually
    /// shouldn't undo the signup that triggered it.
    pub fn send_email(&self, email: Email) -> Result<Sent, String> {
        let request = serde_json::to_string(&email).map_err(|e| e.to_string())?;
        match self.host.send_email(RStr::from_str(&request)) {
            RResult::ROk(receipt) => serde_json::from_str(receipt.as_str())
                .map_err(|e| format!("unreadable email receipt: {e}")),
            RResult::RErr(e) => Err(e.into_string()),
        }
    }

    /// Read a value from the app's Redis cache. `None` for a miss.
    ///
    /// ```no_run
    /// # use apiplant_function::prelude::*;
    /// # fn fetch() -> serde_json::Value { serde_json::Value::Null }
    /// # fn example(ctx: &Context<()>) -> Result<serde_json::Value, String> {
    /// if let Some(hit) = ctx.cache_get("rates:eur")? {
    ///     return Ok(hit);
    /// }
    /// let rates = fetch();
    /// ctx.cache_set("rates:eur", &rates, Some(900))?;
    /// # Ok(rates) }
    /// ```
    ///
    /// Errors when the app configured no cache, or when Redis is unreachable.
    /// Since a cache holds only what can be recomputed, treating an error like
    /// a miss (`ctx.cache_get(k).ok().flatten()`) is a reasonable choice — and
    /// the one that keeps the endpoint working while Redis restarts.
    pub fn cache_get(&self, key: &str) -> Result<Option<serde_json::Value>, String> {
        let reply = self.cache(serde_json::json!({ "op": "get", "key": key }))?;
        match reply.get("value") {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(value) => Ok(Some(value.clone())),
        }
    }

    /// Read a value and deserialize it. A miss and a value of the wrong shape
    /// both come back as `None`, because a cache entry written by an older
    /// version of the function is a miss in every way that matters.
    pub fn cache_get_as<T: serde::de::DeserializeOwned>(
        &self,
        key: &str,
    ) -> Result<Option<T>, String> {
        Ok(self
            .cache_get(key)?
            .and_then(|value| serde_json::from_value(value).ok()))
    }

    /// Write a value, expiring after `ttl_secs`. `None` uses the app's
    /// `[cache] default_ttl_secs`; `Some(0)` means "keep it until deleted".
    pub fn cache_set<T: serde::Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl_secs: Option<u64>,
    ) -> Result<(), String> {
        let value = serde_json::to_value(value).map_err(|e| e.to_string())?;
        self.cache(serde_json::json!({
            "op": "set", "key": key, "value": value, "ttl": ttl_secs
        }))?;
        Ok(())
    }

    /// Drop a key. `true` when it was there.
    pub fn cache_delete(&self, key: &str) -> Result<bool, String> {
        let reply = self.cache(serde_json::json!({ "op": "delete", "key": key }))?;
        Ok(reply
            .get("deleted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Add `by` to a counter and return its new value, starting from zero.
    ///
    /// The increment happens on the server, so this counts correctly across
    /// every worker and every host — which is what makes it usable for rate
    /// limiting, and what a `get` + `set` pair could not do. `ttl_secs` is
    /// applied only when the counter is created, so a window doesn't extend
    /// itself on every hit.
    pub fn cache_incr(&self, key: &str, by: i64, ttl_secs: Option<u64>) -> Result<i64, String> {
        let reply = self.cache(serde_json::json!({
            "op": "incr", "key": key, "by": by, "ttl": ttl_secs
        }))?;
        Ok(reply.get("value").and_then(|v| v.as_i64()).unwrap_or(0))
    }

    /// Seconds until `key` expires; `None` when it is absent or set to persist.
    pub fn cache_ttl(&self, key: &str) -> Result<Option<i64>, String> {
        let reply = self.cache(serde_json::json!({ "op": "ttl", "key": key }))?;
        Ok(reply.get("ttl").and_then(|v| v.as_i64()))
    }

    /// Send one operation to the host's cache and parse its reply.
    fn cache(&self, request: serde_json::Value) -> Result<serde_json::Value, String> {
        match self.host.cache(RStr::from_str(&request.to_string())) {
            RResult::ROk(reply) => serde_json::from_str(reply.as_str())
                .map_err(|e| format!("unreadable cache reply: {e}")),
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

/// A message to send with [`Context::send_email`].
///
/// Addresses are written the way you'd write them in a mail client — either
/// `"ann@example.com"` or `"Ann Lee <ann@example.com>"`. `from` and `reply_to`
/// are left unset unless this particular message needs to differ from the app's
/// `[email]` defaults.
///
/// Deliberately not the host's own message type: this crate is compiled into
/// every function library, and a function has no business linking an HTTP
/// client, an SMTP stack and a request signer to describe an email. What
/// crosses the ABI is the JSON below.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Email {
    pub to: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub cc: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub bcc: Vec<String>,
    pub subject: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub html: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reply_to: Option<String>,
}

impl Email {
    /// Start a message to one recipient.
    pub fn to(recipient: impl Into<String>) -> Email {
        Email {
            to: vec![recipient.into()],
            ..Email::default()
        }
    }

    /// Start a message to several recipients.
    pub fn to_all<S: Into<String>>(recipients: impl IntoIterator<Item = S>) -> Email {
        Email {
            to: recipients.into_iter().map(Into::into).collect(),
            ..Email::default()
        }
    }

    pub fn cc(mut self, address: impl Into<String>) -> Email {
        self.cc.push(address.into());
        self
    }

    pub fn bcc(mut self, address: impl Into<String>) -> Email {
        self.bcc.push(address.into());
        self
    }

    pub fn subject(mut self, subject: impl Into<String>) -> Email {
        self.subject = subject.into();
        self
    }

    /// The plain-text body. Send at least one of this and [`html`](Self::html);
    /// sending both produces a `multipart/alternative`, which is what a mail
    /// client expects.
    pub fn text(mut self, body: impl Into<String>) -> Email {
        self.text = body.into();
        self
    }

    pub fn html(mut self, body: impl Into<String>) -> Email {
        self.html = body.into();
        self
    }

    /// Override the app's configured sender for this message.
    pub fn from(mut self, address: impl Into<String>) -> Email {
        self.from = Some(address.into());
        self
    }

    pub fn reply_to(mut self, address: impl Into<String>) -> Email {
        self.reply_to = Some(address.into());
        self
    }
}

/// The receipt [`Context::send_email`] returns: which provider took the
/// message, and what it called it.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Sent {
    /// The provider that accepted it, e.g. `"ses"`.
    pub provider: String,
    /// The provider's identifier for the message; empty when it returns none.
    pub id: String,
    /// How many addresses it went to, across `to`, `cc` and `bcc`.
    pub recipients: usize,
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

/// Turn a `permission` string into the nearest legacy [`Visibility`] + role.
///
/// The manifest carries both because [`Visibility`] is the older, coarser field
/// that generated docs and pre-`permission` tooling still read. `member` has no
/// `Visibility` of its own, so it degrades to `Authenticated` — the closest
/// truthful statement, and never a *wider* one.
#[doc(hidden)]
pub fn derive_visibility(permission: &str) -> (apiplant_abi::Visibility, String) {
    use apiplant_abi::{FunctionAccess, Visibility};
    match FunctionAccess::parse(permission) {
        Some(FunctionAccess::Public) => (Visibility::Public, String::new()),
        Some(FunctionAccess::Authenticated) | Some(FunctionAccess::Member) => {
            (Visibility::Authenticated, String::new())
        }
        Some(FunctionAccess::Role(role)) => (Visibility::RoleGated, role),
        // Unparseable or absent: closed, like every other access default here.
        Some(FunctionAccess::Private) | None => (Visibility::Private, String::new()),
    }
}

/// Collects the optional `admin { … }` block of a [`functions!`] entry and
/// serialises it into [`apiplant_abi::FunctionManifest::admin`].
///
/// An entry that declares nothing produces the empty string rather than `{}`,
/// so "said nothing" stays distinguishable from "said the defaults out loud".
#[doc(hidden)]
#[derive(Default)]
pub struct AdminBuilder {
    pub visible: Option<bool>,
    pub roles: Vec<String>,
    pub label: Option<String>,
    pub group: Option<String>,
    pub description: Option<String>,
    pub confirm: Option<String>,
    pub run_label: Option<String>,
    pub order: Option<i64>,
}

impl AdminBuilder {
    pub fn finish(self) -> String {
        let mut object = serde_json::Map::new();
        let mut put = |key: &str, value: Option<serde_json::Value>| {
            if let Some(value) = value {
                object.insert(key.to_string(), value);
            }
        };
        put("visible", self.visible.map(serde_json::Value::from));
        put("label", self.label.map(serde_json::Value::from));
        put("group", self.group.map(serde_json::Value::from));
        put("description", self.description.map(serde_json::Value::from));
        put("confirm", self.confirm.map(serde_json::Value::from));
        put("run_label", self.run_label.map(serde_json::Value::from));
        put("order", self.order.map(serde_json::Value::from));
        if !self.roles.is_empty() {
            object.insert("roles".to_string(), serde_json::Value::from(self.roles));
        }
        if object.is_empty() {
            return String::new();
        }
        serde_json::to_string(&object).unwrap_or_default()
    }
}

/// A curated set of imports for function authors: `use apiplant_function::prelude::*;`.
pub mod prelude {
    pub use crate::{reply, Context, Email, Hook, Sent};
    pub use apiplant_abi::{HttpMethod, LogLevel, Visibility};
    /// `#[derive(JsonSchema)]` for typed OpenAPI (with the `schema` feature).
    #[cfg(feature = "schema")]
    pub use schemars::JsonSchema;
}

/// Re-exports the generated code depends on. Not a stable public API.
#[doc(hidden)]
pub mod __rt {
    pub use crate::{
        derive_visibility, input_schema_json, invoke_handler, output_schema_json, AdminBuilder,
        Context, Exported, Hook,
    };
    pub use abi_stable::export_root_module;
    pub use abi_stable::prefix_type::PrefixTypeTrait;
    pub use abi_stable::sabi_extern_fn;
    pub use abi_stable::sabi_trait::TD_Opaque;
    pub use abi_stable::std_types::{RBox, RResult, RStr, RString, RVec};
    pub use apiplant_abi::{
        BoxedFunction, Function, FunctionManifest, FunctionMod, FunctionMod_Ref, Function_TO,
        HostApi_TO, HttpMethod, Visibility,
    };
}

/// Define and export **one** apiplant function from a plain handler.
///
/// Only `name`, `description`, `method` and `handler` are required:
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
///     permission: "role:admin",    // public | authenticated | member | role:<name> | private
///     handler: greet,              // fn(&Context<C>, I) -> Result<O, E>
/// }
/// # fn main() {}
/// ```
///
/// # Access
///
/// `permission` uses the same grammar as a resource's `[permissions]`, so an
/// app has one access vocabulary rather than two. The older
/// `visibility: RoleGated` + `role: "admin"` pair still works and means exactly
/// what it always did; give one or the other, not both.
///
/// `member` — any member of the caller's active organisation — is the level
/// most operator-facing actions want and the reason `permission` exists;
/// `visibility` cannot express it.
///
/// # Appearing in the dashboard
///
/// The optional `admin` block controls how `apiplant admin` presents the
/// function. Every key is optional:
///
/// ```no_run
/// # use apiplant_function::prelude::*;
/// # type Json = serde_json::Value;
/// # fn reindex(_ctx: &Context<()>, input: Json) -> Result<Json, String> { Ok(input) }
/// apiplant_function::function! {
///     name: "reindex_catalogue",
///     description: "Rebuilds the product search index.",
///     method: Post,
///     permission: "role:admin",
///     admin: {
///         visible: true,                        // default: true unless private
///         roles: ["admin", "manager"],          // who sees it; default: anyone who may call it
///         label: "Rebuild search index",
///         group: "Maintenance",
///         description: "Run this after a bulk import.",
///         confirm: "Rebuild the index for every product?",
///         run_label: "Rebuild index",
///         order: 10,
///     },
///     handler: reindex,
/// }
/// # fn main() {}
/// ```
///
/// This is presentation only — hiding a function from the dashboard does not
/// close its endpoint. `permission` is what does that.
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
                $(visibility: $visibility:ident,)?
                $(permission: $permission:expr,)?
                $(role: $role:expr,)?
                $(admin: {
                    $(visible: $admin_visible:expr,)?
                    $(roles: $admin_roles:expr,)?
                    $(label: $admin_label:expr,)?
                    $(group: $admin_group:expr,)?
                    $(description: $admin_description:expr,)?
                    $(confirm: $admin_confirm:expr,)?
                    $(run_label: $admin_run_label:expr,)?
                    $(order: $admin_order:expr,)?
                },)?
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
                        let mut role = ::std::string::String::new();
                        $( role = ::std::string::String::from($role); )?

                        // `permission` is the current spelling and `visibility`
                        // + `role` the original one. Whichever the author used,
                        // both fields end up populated and agreeing.
                        #[allow(unused_mut)]
                        let mut permission = ::std::string::String::new();
                        $( permission = ::std::string::String::from($permission); )?

                        #[allow(unused_mut)]
                        let mut declared_visibility:
                            ::core::option::Option<$crate::__rt::Visibility> =
                            ::core::option::Option::None;
                        $(
                            declared_visibility = ::core::option::Option::Some(
                                $crate::__rt::Visibility::$visibility,
                            );
                        )?

                        let (visibility, role) = match declared_visibility {
                            ::core::option::Option::Some(visibility) => {
                                if permission.is_empty() {
                                    permission = match visibility {
                                        $crate::__rt::Visibility::Public =>
                                            "public".to_string(),
                                        $crate::__rt::Visibility::Authenticated =>
                                            "authenticated".to_string(),
                                        $crate::__rt::Visibility::Private =>
                                            "private".to_string(),
                                        $crate::__rt::Visibility::RoleGated =>
                                            ::std::format!("role:{}", role),
                                    };
                                }
                                (visibility, role)
                            }
                            ::core::option::Option::None => {
                                let (visibility, derived_role) =
                                    $crate::__rt::derive_visibility(&permission);
                                let role = if role.is_empty() { derived_role } else { role };
                                (visibility, role)
                            }
                        };

                        #[allow(unused_mut)]
                        let mut admin = $crate::__rt::AdminBuilder::default();
                        $(
                            $( admin.visible = ::core::option::Option::Some($admin_visible); )?
                            $(
                                admin.roles = $admin_roles
                                    .iter()
                                    .map(|role| ::std::string::ToString::to_string(role))
                                    .collect();
                            )?
                            $( admin.label =
                                ::core::option::Option::Some($admin_label.to_string()); )?
                            $( admin.group =
                                ::core::option::Option::Some($admin_group.to_string()); )?
                            $( admin.description =
                                ::core::option::Option::Some($admin_description.to_string()); )?
                            $( admin.confirm =
                                ::core::option::Option::Some($admin_confirm.to_string()); )?
                            $( admin.run_label =
                                ::core::option::Option::Some($admin_run_label.to_string()); )?
                            $( admin.order = ::core::option::Option::Some($admin_order); )?
                        )?

                        let manifest = $crate::__rt::FunctionManifest {
                            name: $crate::__rt::RString::from($name),
                            version,
                            description: $crate::__rt::RString::from($description),
                            visibility,
                            role: $crate::__rt::RString::from(role),
                            method: $crate::__rt::HttpMethod::$method,
                            permission: $crate::__rt::RString::from(permission),
                            admin: $crate::__rt::RString::from(admin.finish()),
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
        /// Reply to `send_email`/`cache`, or an error to hand back instead.
        service_result: Result<String, String>,
        requests: Mutex<Vec<String>>,
        /// Every `send_email` and `cache` request, as sent.
        service_requests: Mutex<Vec<String>>,
        logs: Mutex<Vec<(LogLevel, String)>>,
    }

    impl MockHost {
        fn success(config_json: &str, principal_id: &str, response: serde_json::Value) -> Self {
            Self {
                config_json: config_json.into(),
                principal_id: principal_id.into(),
                hook_json: String::new(),
                query_result: Ok(response.to_string()),
                service_result: Ok("{}".to_string()),
                requests: Mutex::new(Vec::new()),
                service_requests: Mutex::new(Vec::new()),
                logs: Mutex::new(Vec::new()),
            }
        }

        fn with_hook(mut self, hook: serde_json::Value) -> Self {
            self.hook_json = hook.to_string();
            self
        }

        /// What `send_email`/`cache` should answer.
        fn replying(mut self, reply: serde_json::Value) -> Self {
            self.service_result = Ok(reply.to_string());
            self
        }

        fn failing(mut self, error: &str) -> Self {
            self.service_result = Err(error.to_string());
            self
        }

        /// The last request made through `send_email`/`cache`.
        fn last_service_request(&self) -> serde_json::Value {
            let requests = self.service_requests.lock().unwrap();
            serde_json::from_str(requests.last().expect("no service request was made")).unwrap()
        }

        /// `send_email` and `cache` are the same shape — record the request,
        /// hand back the canned answer.
        fn service(&self, request: RStr<'_>) -> RResult<RString, RString> {
            self.service_requests
                .lock()
                .unwrap()
                .push(request.as_str().to_string());
            match &self.service_result {
                Ok(reply) => RResult::ROk(RString::from(reply.as_str())),
                Err(error) => RResult::RErr(RString::from(error.as_str())),
            }
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

        fn send_email(&self, request: RStr<'_>) -> RResult<RString, RString> {
            self.service(request)
        }

        fn cache(&self, request: RStr<'_>) -> RResult<RString, RString> {
            self.service(request)
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

    /// A mock the test still holds a handle to after the ABI has taken it —
    /// the only way to assert on what a `Context` method actually sent.
    struct Shared(std::sync::Arc<MockHost>);

    impl HostApi for Shared {
        fn query(&self, request: RStr<'_>) -> RResult<RString, RString> {
            self.0.query(request)
        }

        fn log(&self, level: LogLevel, message: RStr<'_>) {
            self.0.log(level, message)
        }

        fn send_email(&self, request: RStr<'_>) -> RResult<RString, RString> {
            self.0.send_email(request)
        }

        fn cache(&self, request: RStr<'_>) -> RResult<RString, RString> {
            self.0.cache(request)
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

    /// A mock kept alive alongside the trait object built from it.
    fn shared(mock: MockHost) -> (std::sync::Arc<MockHost>, HostApi_TO<'static, RBox<()>>) {
        let mock = std::sync::Arc::new(mock);
        let host = HostApi_TO::from_value(Shared(mock.clone()), TD_Opaque);
        (mock, host)
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

        let request = &host.config().into_string();
        assert_eq!(request, "{}");
    }

    #[test]
    fn context_execute_and_logging_use_host_bridge() {
        let host = MockHost::success("{}", "user-123", serde_json::json!({ "rows_affected": 3 }));
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let ctx = Context::__new(&host, (), "user-123".into(), None);

        assert_eq!(ctx.execute("DELETE FROM apiplant_post", &[]).unwrap(), 3);
        ctx.warn("careful");
    }

    #[test]
    fn send_email_hands_the_host_the_message_and_reads_the_receipt() {
        let host = MockHost::success("{}", "u1", serde_json::json!([]))
            .replying(serde_json::json!({ "provider": "ses", "id": "abc", "recipients": 2 }));
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let ctx = Context::__new(&host, (), "u1".into(), None);

        let sent = ctx
            .send_email(
                Email::to("Ann <ann@example.com>")
                    .cc("bo@example.com")
                    .subject("Welcome")
                    .text("Hello")
                    .reply_to("help@example.com"),
            )
            .unwrap();

        assert_eq!(sent.provider, "ses");
        assert_eq!(sent.id, "abc");
        assert_eq!(sent.recipients, 2);
    }

    /// Empty parts must not appear on the wire at all: a provider that sees
    /// `"html": ""` may send a blank body instead of the text one.
    #[test]
    fn an_email_only_carries_the_fields_it_was_given() {
        let (mock, host) = shared(
            MockHost::success("{}", "u1", serde_json::json!([]))
                .replying(serde_json::json!({ "provider": "smtp", "id": "", "recipients": 1 })),
        );
        let ctx = Context::__new(&host, (), "u1".into(), None);

        ctx.send_email(Email::to("ann@example.com").subject("Hi").text("Hello"))
            .unwrap();

        let request = mock.last_service_request();
        assert_eq!(request["to"][0], "ann@example.com");
        assert_eq!(request["subject"], "Hi");
        assert_eq!(request["text"], "Hello");
        assert!(request.get("html").is_none());
        assert!(request.get("cc").is_none());
        assert!(request.get("from").is_none());
    }

    #[test]
    fn a_provider_failure_surfaces_as_an_error() {
        let host = MockHost::success("{}", "u1", serde_json::json!([]))
            .failing("sendgrid rejected the message (401): unauthorized");
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let ctx = Context::__new(&host, (), "u1".into(), None);

        let err = ctx
            .send_email(Email::to("ann@example.com").subject("Hi").text("Hello"))
            .unwrap_err();
        assert!(err.contains("401"), "{err}");
    }

    #[test]
    fn cache_get_distinguishes_a_hit_from_a_miss() {
        let hit = MockHost::success("{}", "u1", serde_json::json!([]))
            .replying(serde_json::json!({ "hit": true, "value": { "eur": 1.1 } }));
        let hit = HostApi_TO::from_value(hit, TD_Opaque);
        let ctx = Context::__new(&hit, (), "u1".into(), None);
        assert_eq!(
            ctx.cache_get("rates").unwrap(),
            Some(serde_json::json!({ "eur": 1.1 }))
        );

        let miss = MockHost::success("{}", "u1", serde_json::json!([]))
            .replying(serde_json::json!({ "hit": false, "value": null }));
        let miss = HostApi_TO::from_value(miss, TD_Opaque);
        let ctx = Context::__new(&miss, (), "u1".into(), None);
        assert_eq!(ctx.cache_get("rates").unwrap(), None);
    }

    /// A cached value written by an older version of a function is a miss, not
    /// an error — otherwise every deployment breaks its own endpoint.
    #[test]
    fn cache_get_as_treats_an_unreadable_value_as_a_miss() {
        #[derive(serde::Deserialize)]
        struct Rates {
            #[allow(dead_code)]
            eur: f64,
        }

        let host = MockHost::success("{}", "u1", serde_json::json!([]))
            .replying(serde_json::json!({ "hit": true, "value": { "old_shape": true } }));
        let host = HostApi_TO::from_value(host, TD_Opaque);
        let ctx = Context::__new(&host, (), "u1".into(), None);

        assert!(ctx.cache_get_as::<Rates>("rates").unwrap().is_none());
    }

    #[test]
    fn cache_writes_name_their_operation_key_and_ttl() {
        let (mock, host) = shared(
            MockHost::success("{}", "u1", serde_json::json!([])).replying(
                serde_json::json!({ "ok": true, "deleted": true, "value": 3, "ttl": 42 }),
            ),
        );
        let ctx = Context::__new(&host, (), "u1".into(), None);

        ctx.cache_set("rates", &serde_json::json!({ "eur": 1.1 }), Some(900))
            .unwrap();
        let request = mock.last_service_request();
        assert_eq!(request["op"], "set");
        assert_eq!(request["key"], "rates");
        assert_eq!(request["value"]["eur"], 1.1);
        assert_eq!(request["ttl"], 900);

        // No TTL is `null`, meaning "use the app's default" — not zero, which
        // would mean "never expire".
        ctx.cache_set("rates", &1, None).unwrap();
        assert!(mock.last_service_request()["ttl"].is_null());

        assert_eq!(ctx.cache_incr("hits", 1, Some(60)).unwrap(), 3);
        assert_eq!(mock.last_service_request()["op"], "incr");

        assert!(ctx.cache_delete("rates").unwrap());
        assert_eq!(mock.last_service_request()["op"], "delete");

        assert_eq!(ctx.cache_ttl("rates").unwrap(), Some(42));
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
            |_ctx, _input| {
                Ok(Output {
                    message: "never".into(),
                })
            },
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
                Box::new(
                    |_: &Context<'_, '_, Config>, input: Input| -> Result<Output, String> {
                        // Derived from the input so clippy sees a real `Option`
                        // rather than a literal `None` it can flag at the call site.
                        let missing = input.name.strip_prefix("nonexistent-prefix");
                        Ok(Output {
                            message: missing.unwrap().to_string(),
                        })
                    },
                )
                    as Box<dyn Fn(&Context<'_, '_, Config>, Input) -> Result<Output, String>>,
            ),
            (
                "index",
                Box::new(
                    |_: &Context<'_, '_, Config>, input: Input| -> Result<Output, String> {
                        // Indexed by input length so the compiler can't prove it's
                        // out of bounds and reject the test with `unconditional_panic`.
                        let empty: Vec<u8> = Vec::new();
                        let _ = empty[input.name.len()];
                        unreachable!()
                    },
                ),
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
            permission: RString::new(),
            admin: RString::new(),
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
        assert_eq!(
            before.field("title").and_then(|v| v.as_str()),
            Some("Draft")
        );
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
                permission: RString::new(),
                admin: RString::new(),
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
        use apiplant_abi::{FunctionManifest, Function_TO, HttpMethod, Visibility};

        let exported: Exported<Config, Input, Output, String, _> = Exported::new(
            FunctionManifest {
                name: RString::from("greet"),
                version: RString::from("0.1.0"),
                description: RString::from("test"),
                visibility: Visibility::Public,
                role: RString::new(),
                method: HttpMethod::Post,
                permission: RString::new(),
                admin: RString::new(),
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
        let handler =
            |_ctx: &Context<'_, '_, ()>, input: SchemaInput| -> Result<SchemaOutput, String> {
                Ok(SchemaOutput {
                    ok: !input.name.is_empty(),
                })
            };

        let input_schema = input_schema_json::<(), SchemaInput, SchemaOutput, String, _>(&handler);
        let output_schema =
            output_schema_json::<(), SchemaInput, SchemaOutput, String, _>(&handler);

        assert!(input_schema.contains("\"name\""));
        assert!(output_schema.contains("\"ok\""));
    }
}
