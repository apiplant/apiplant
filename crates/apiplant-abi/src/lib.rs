//! # apiplant-abi
//!
//! The stable, C-compatible ABI contract that sits between the `apiplant`
//! host and every dynamically-loaded *function* library (`.so`/`.dylib`/`.dll`).
//!
//! Both sides depend *only* on this crate and on [`abi_stable`], which means a
//! function compiled against version `X` of this crate keeps working against a
//! host compiled against the same major version — no matter which compiler,
//! allocator or std version each side was built with. That is the whole point
//! of routing everything through `#[repr(C)]` + [`StableAbi`] types.
//!
//! ## The shape of a function
//!
//! A function library exports exactly one root module ([`FunctionMod`]) whose
//! `new_functions` constructor yields one or more [`Function`] trait objects —
//! a single library can provide a whole set of independently-named functions.
//! For each of them the host:
//!
//! 1. reads its [`FunctionManifest`] (name, visibility, HTTP method, config
//!    schema),
//! 2. mounts it on an HTTP endpoint according to its [`Visibility`],
//! 3. on each request calls [`Function::invoke`], passing a [`HostApi`] handle
//!    (database access, logging, resolved config) plus a JSON input string,
//! 4. returns the JSON the function produced.
//!
//! Everything crosses the boundary as JSON ([`RString`]) or as small
//! `#[repr(C)]` enums. Nothing sea-orm / ntex / tokio ever touches the ABI, so
//! the contract stays tiny and genuinely stable.
//!
// abi_stable's macros generate types (`Function_TO`, `FunctionMod_Ref`, …) and
// impls that trip these lints; the naming/scoping conventions are the crate's.
#![allow(non_camel_case_types, non_local_definitions)]

pub mod c;

/// `apiplant.h` — the C declaration of this ABI, embedded so a binary with no
/// checkout beside it can still hand the header to a C or Zig compiler.
pub const HEADER: &str = include_str!("../include/apiplant.h");

#[cfg(feature = "manifest-json")]
mod manifest_json;
#[cfg(feature = "manifest-json")]
pub use manifest_json::manifest_from_json;

use abi_stable::{
    declare_root_module_statics,
    library::RootModule,
    package_version_strings, sabi_trait,
    std_types::{RResult, RStr, RString},
    StableAbi,
};

/// Who is allowed to call a function's (or resource's) endpoint.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, StableAbi)]
pub enum Visibility {
    /// Anyone, no authentication required.
    Public,
    /// Any authenticated principal (session, api-key or oauth).
    Authenticated,
    /// Authenticated *and* holding a specific role (see [`FunctionManifest::role`]).
    RoleGated,
    /// Never exposed over HTTP; only callable internally by other functions.
    Private,
}

/// HTTP verb a function endpoint responds to.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, StableAbi)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

/// Severity for [`HostApi::log`].
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, StableAbi)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Static description of a function, read once at load time.
#[repr(C)]
#[derive(Debug, Clone, StableAbi)]
pub struct FunctionManifest {
    /// URL-safe identifier, e.g. `"greet"` → mounted at `/functions/greet`.
    pub name: RString,
    /// Semver of the function itself (independent of the ABI version).
    pub version: RString,
    /// Human description, surfaced in generated API docs.
    pub description: RString,
    /// Access-control policy for the generated endpoint.
    pub visibility: Visibility,
    /// Required role name when `visibility == RoleGated`, else empty.
    pub role: RString,
    /// HTTP method the endpoint answers.
    pub method: HttpMethod,
    /// Access policy in the same string grammar a resource's `[permissions]`
    /// uses — `"public"`, `"authenticated"`, `"member"`, `"role:admin"`,
    /// `"private"`/`"none"`. Empty means "derive it from `visibility` and `role`", which
    /// is what a function that predates this field gets.
    ///
    /// This exists because [`Visibility`] cannot express `member` — "anyone in
    /// the active organisation" — which is the level most operator-facing
    /// actions actually want, and because sharing one grammar with resources
    /// means an app has exactly one thing to learn about access.
    pub permission: RString,
    /// Dashboard presentation as a JSON object, or empty for the defaults:
    ///
    /// ```json
    /// { "visible": true, "roles": ["admin"], "label": "Reindex catalogue",
    ///   "group": "Maintenance", "confirm": "Reindex every product?",
    ///   "run_label": "Reindex", "order": 10 }
    /// ```
    ///
    /// Carried as JSON rather than as struct fields so that adding a
    /// presentation knob later never changes this struct's layout — and so
    /// never invalidates an already-compiled function library.
    pub admin: RString,
    /// Optional JSON-Schema describing the function's config object. Empty = none.
    pub config_schema: RString,
    /// Optional JSON-Schema for the request body. Empty = untyped. Surfaced in
    /// the generated OpenAPI document so function I/O is typed in the docs.
    pub input_schema: RString,
    /// Optional JSON-Schema for the response body. Empty = untyped.
    pub output_schema: RString,
}

/// A function's effective access policy — the resolved form of
/// [`FunctionManifest::permission`], falling back to [`Visibility`].
///
/// Deliberately *not* `StableAbi`: it is derived on the host from fields that
/// are, so it can grow a variant without touching the wire contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunctionAccess {
    /// Anyone, authenticated or not.
    Public,
    /// Any authenticated principal.
    Authenticated,
    /// Any member of the caller's active organisation.
    Member,
    /// A member holding this role in the active organisation.
    Role(String),
    /// Not exposed over HTTP at all.
    Private,
}

impl FunctionAccess {
    /// Parse the string grammar shared with a resource's `[permissions]`.
    /// Returns `None` for anything unrecognised, so a caller can decide whether
    /// a typo is an error (at build time) or should close the door (at load).
    pub fn parse(value: &str) -> Option<FunctionAccess> {
        match value.trim() {
            "public" => Some(FunctionAccess::Public),
            "authenticated" => Some(FunctionAccess::Authenticated),
            "member" => Some(FunctionAccess::Member),
            "private" | "none" => Some(FunctionAccess::Private),
            other => other
                .strip_prefix("role:")
                .filter(|role| !role.is_empty())
                .map(|role| FunctionAccess::Role(role.to_string())),
        }
    }

    /// The canonical string form, round-tripping [`FunctionAccess::parse`].
    pub fn as_string(&self) -> String {
        match self {
            FunctionAccess::Public => "public".to_string(),
            FunctionAccess::Authenticated => "authenticated".to_string(),
            FunctionAccess::Member => "member".to_string(),
            FunctionAccess::Role(role) => format!("role:{role}"),
            FunctionAccess::Private => "private".to_string(),
        }
    }

    /// Whether the endpoint is reachable without credentials.
    pub fn is_public(&self) -> bool {
        matches!(self, FunctionAccess::Public)
    }
}

/// What separates an access level from the organisation class it is narrowed
/// to, in every access string the framework parses.
pub const ORG_CLASS_SUFFIX: &str = "@org_class=";

/// A [`FunctionAccess`] plus the optional organisation class it is narrowed to
/// — the function-side twin of `apiplant_core::Policy`, spelled the same way:
/// `"role:admin@org_class=school"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionPolicy {
    pub access: FunctionAccess,
    pub org_class: Option<String>,
}

impl FunctionPolicy {
    /// Parse the shared grammar. `None` for anything unrecognised — including
    /// an empty class, which would otherwise read as "no qualifier" and widen
    /// the policy rather than narrowing it.
    pub fn parse(value: &str) -> Option<FunctionPolicy> {
        match value.trim().split_once(ORG_CLASS_SUFFIX) {
            Some((access, class)) => {
                let class = class.trim();
                if class.is_empty() {
                    return None;
                }
                Some(FunctionPolicy {
                    access: FunctionAccess::parse(access)?,
                    org_class: Some(class.to_string()),
                })
            }
            None => Some(FunctionPolicy {
                access: FunctionAccess::parse(value)?,
                org_class: None,
            }),
        }
    }

    /// The canonical string form, round-tripping [`FunctionPolicy::parse`].
    pub fn as_string(&self) -> String {
        match &self.org_class {
            Some(class) => format!("{}{ORG_CLASS_SUFFIX}{class}", self.access.as_string()),
            None => self.access.as_string(),
        }
    }

    /// Whether an organisation carrying `class` satisfies the qualifier.
    pub fn matches_org_class(&self, class: Option<&str>) -> bool {
        match &self.org_class {
            None => true,
            Some(required) => class.is_some_and(|c| c == required),
        }
    }
}

impl From<FunctionAccess> for FunctionPolicy {
    fn from(access: FunctionAccess) -> FunctionPolicy {
        FunctionPolicy {
            access,
            org_class: None,
        }
    }
}

impl FunctionManifest {
    /// The policy the host enforces for this function, class qualifier
    /// included. An unparseable `permission` collapses to `private`.
    pub fn policy(&self) -> FunctionPolicy {
        if !self.permission.is_empty() {
            return FunctionPolicy::parse(self.permission.as_str())
                .unwrap_or_else(|| FunctionAccess::Private.into());
        }
        self.access().into()
    }

    /// The policy the host enforces for this function.
    ///
    /// An explicit, parseable `permission` wins. Otherwise the legacy
    /// `visibility` + `role` pair is used, so libraries compiled before
    /// `permission` existed keep exactly the access they always had. An
    /// *unparseable* `permission` collapses to [`FunctionAccess::Private`] —
    /// the safe direction, matching how the rest of apiplant treats a typo in
    /// an access string.
    ///
    /// This is the *level* only: a `@org_class=` qualifier is dropped, so a
    /// caller that enforces a class must use [`policy`](Self::policy).
    pub fn access(&self) -> FunctionAccess {
        if !self.permission.is_empty() {
            return FunctionPolicy::parse(self.permission.as_str())
                .map(|p| p.access)
                .unwrap_or(FunctionAccess::Private);
        }
        match self.visibility {
            Visibility::Public => FunctionAccess::Public,
            Visibility::Authenticated => FunctionAccess::Authenticated,
            Visibility::Private => FunctionAccess::Private,
            Visibility::RoleGated => FunctionAccess::Role(self.role.to_string()),
        }
    }
}

/// Services the host lends to a function for the duration of one invocation.
///
/// Implemented on the host side and handed across the boundary as an
/// [`abi_stable`] trait object ([`HostApi_TO`]). Every method is synchronous
/// from the function's point of view — the host is responsible for bridging to
/// its async database internally (functions run on a blocking worker), so
/// function authors never touch `async`.
#[sabi_trait]
pub trait HostApi: Send + Sync {
    /// Run a query. `request` is a JSON object of the form
    /// `{ "sql": "...", "params": [ ... ] }` and the reply is a JSON array of
    /// row objects (for `SELECT`) or `{ "rows_affected": n }`.
    fn query(&self, request: RStr<'_>) -> RResult<RString, RString>;

    /// Emit a structured log line through the host's `tracing` subscriber.
    fn log(&self, level: LogLevel, message: RStr<'_>);

    /// Send an email through whichever provider the app configured.
    ///
    /// `request` is the message as a JSON object; the reply is a receipt:
    ///
    /// ```json
    /// // request
    /// { "to": "ann@example.com", "cc": [], "subject": "Hi", "text": "Hello",
    ///   "html": "<p>Hello</p>", "from": null, "reply_to": null }
    /// // reply
    /// { "provider": "sendgrid", "id": "…", "recipients": 1 }
    /// ```
    ///
    /// `to`/`cc`/`bcc` each accept a bare string, `"Ann <ann@example.com>"`, an
    /// `{ "email": …, "name": … }` object, or a list of any of those. `from`
    /// and `reply_to` default to the app's `[email]` configuration.
    ///
    /// Errors when the app configured no provider, when the message can't be
    /// sent (no recipient, no sender), or when the provider refused it.
    fn send_email(&self, request: RStr<'_>) -> RResult<RString, RString>;

    /// Run one cache operation against the app's Redis, if it configured one.
    ///
    /// `request` names the operation; the reply's shape depends on it:
    ///
    /// ```json
    /// { "op": "get",    "key": "k" }                        → { "hit": true, "value": … }
    /// { "op": "set",    "key": "k", "value": …, "ttl": 60 } → { "ok": true }
    /// { "op": "delete", "key": "k" }                        → { "deleted": true }
    /// { "op": "exists", "key": "k" }                        → { "exists": true }
    /// { "op": "incr",   "key": "k", "by": 1, "ttl": 60 }    → { "value": 3 }
    /// { "op": "ttl",    "key": "k" }                        → { "ttl": 42 }
    /// ```
    ///
    /// Errors when no cache is configured, when the request isn't one of the
    /// operations above, or when Redis is unreachable. Cached data is by
    /// definition reconstructible, so treating an error as a miss is a valid
    /// and usually correct thing for a function to do.
    fn cache(&self, request: RStr<'_>) -> RResult<RString, RString>;

    /// Run one payment operation against the app's provider, if it configured
    /// one.
    ///
    /// `request` names the operation; the reply's shape depends on it:
    ///
    /// ```json
    /// { "op": "checkout", "stripe_price_id": "price_…", "recurring": true,
    ///   "organization_id": "…" }                → { "url": "https://checkout.stripe.com/…", … }
    /// { "op": "portal", "stripe_customer_id": "cus_…" }  → { "url": "https://billing.stripe.com/…" }
    /// { "op": "customer", "organization_id": "…", "email": "…" }
    ///                                            → { "stripe_customer_id": "cus_…", … }
    /// { "op": "product", "name": "Pro" }         → { "stripe_product_id": "prod_…" }
    /// { "op": "price", "stripe_product_id": "prod_…", "unit_amount": 1000 }
    ///                                            → { "stripe_price_id": "price_…", "replaced": false }
    /// { "op": "subscription", "id": "sub_…" }    → { "status": "active", "entitled": true, … }
    /// { "op": "cancel", "id": "sub_…" }          → the subscription's new state
    /// ```
    ///
    /// This is the *provider*, not the app's tables. Reading whether an
    /// organisation is subscribed is an ordinary query against
    /// `billing_subscription`, which the webhook keeps current and which costs
    /// no round trip; come here to make something happen, or when a decision
    /// is worth asking Stripe directly about.
    ///
    /// Errors when the app configured no provider, when the request isn't one
    /// of the operations above, or when the provider refused it.
    fn payments(&self, request: RStr<'_>) -> RResult<RString, RString>;

    /// Ask the app's AI assistant something, if it configured one.
    ///
    /// `request` is a conversation; the reply is the whole answer:
    ///
    /// ```json
    /// // request
    /// { "messages": [{ "role": "user", "content": "Summarise this." }],
    ///   "model": null, "system": null, "temperature": null, "max_tokens": null }
    /// // reply
    /// { "text": "…", "provider": "openai", "model": "gpt-4o-mini",
    ///   "finish_reason": "stop", "input_tokens": 42, "output_tokens": 96 }
    /// ```
    ///
    /// Everything but `messages` falls back to the app's `[ai]` configuration,
    /// so a function that only has a question writes only the question.
    ///
    /// This call waits for the complete answer, because a function returns one
    /// value. A function that wants to *stream* an answer to its caller reads
    /// it here and re-emits it through [`emit`](Self::emit) — see the
    /// `<base>/functions/<name>/stream` endpoint.
    ///
    /// Errors when the app configured no provider, when the conversation is
    /// empty, or when the provider refused it.
    fn ai(&self, request: RStr<'_>) -> RResult<RString, RString>;

    /// Push a chunk of the response to the caller *before* this invocation
    /// returns.
    ///
    /// Only meaningful when the function was called through
    /// `<base>/functions/<name>/stream`, which answers as `text/event-stream`
    /// and forwards each chunk as it arrives. Everywhere else — an ordinary
    /// invocation, a lifecycle hook — this is a no-op: nobody is listening, and
    /// a function should not have to know which way it was called.
    ///
    /// `chunk` is arbitrary text. A function streaming JSON objects one per
    /// chunk, and one streaming plain prose, are both ordinary uses; the host
    /// does not interpret what it forwards.
    ///
    /// Returns whether it is still worth producing more. `false` means the
    /// caller hung up — a function's cue to stop generating rather than an
    /// error, since nobody will read the rest of it. An invocation that is not
    /// being streamed answers `true`: the chunk went nowhere, but the caller
    /// is still waiting for the return value, so stopping would be wrong.
    fn emit(&self, chunk: RStr<'_>) -> bool;

    /// The function's resolved configuration as a JSON object (merged defaults +
    /// per-deployment overrides from the app's `functions/<name>.toml`).
    fn config(&self) -> RString;

    /// Id of the authenticated principal calling this function, or empty when
    /// the endpoint is [`Visibility::Public`] and the caller is anonymous.
    fn principal_id(&self) -> RString;

    /// Lifecycle-hook context as a JSON object when this invocation is a
    /// resource hook, or empty when the function was called over HTTP.
    ///
    /// The object carries the event (`"before_create"`, `"after_list"`, …), the
    /// resource it fired for, the request URL and method, the caller's auth
    /// status, and the subject of the operation — the submitted `data`, the
    /// `row` created/fetched/deleted, or the `rows` a list returned:
    ///
    /// ```json
    /// {
    ///   "event": "after_create", "action": "create", "phase": "after",
    ///   "resource": "post", "url": "/api/post", "method": "POST",
    ///   "query": {}, "authenticated": true,
    ///   "principal_id": "…", "organization_id": "…", "role": "admin",
    ///   "record_id": null,
    ///   "data": null, "row": { "id": "…", "title": "…" }, "rows": null
    /// }
    /// ```
    ///
    /// A function running as a queue *subscriber* reads its delivery here
    /// instead, under `"event": "message"` — the topic, the message's id, and
    /// which attempt this is:
    ///
    /// ```json
    /// { "event": "message", "topic": "order.paid", "message_id": "…",
    ///   "subscriber": "fulfil_order", "attempts": 1, "principal_id": "…" }
    /// ```
    ///
    /// `attempts` is the one worth branching on: delivery is at-least-once, so
    /// anything above `1` is a message whose side effects may have partly
    /// happened already.
    fn hook(&self) -> RString;

    /// Queue a message for whichever functions subscribe to a topic, to be
    /// handled after this invocation returns.
    ///
    /// ```json
    /// // request
    /// { "op": "publish", "topic": "order.paid", "message": { "order_id": "…" } }
    /// // reply
    /// { "id": "…", "topic": "order.paid", "delivered": 2 }
    /// ```
    ///
    /// `delivered` is how many subscribers it was queued for, and **zero is not
    /// an error**: the message is recorded either way, so that a topic nobody
    /// listens to is a row to find rather than a silence to guess at. A
    /// publisher that would rather know can check it.
    ///
    /// This returns once the message is *committed*, not once it has been
    /// handled — that is the entire point, and what makes it different from
    /// calling the other function directly. The handler runs on a subscriber,
    /// possibly in another process, possibly after a retry, and its failure has
    /// no effect on this invocation.
    ///
    /// Errors when the topic isn't a usable name, or when the database refused
    /// the write. It does *not* error when no subscriber is configured.
    fn publish(&self, request: RStr<'_>) -> RResult<RString, RString>;
}

/// Marks an [`Function::invoke`] error as *the function's own fault* — a panic
/// or an internal fault — rather than a complaint about the caller's input.
///
/// The error channel is a bare string, so without a marker the host cannot tell
/// "you sent me nonsense" (a `400`, and the message is safe to echo back) from
/// "I broke" (a `500`, and the message may name internals the caller has no
/// business seeing). A function prefixes the latter with this constant; the host
/// strips it, logs the detail at `ERROR`, and answers with a generic `500`.
///
/// The leading `\x01` cannot occur in a JSON string or a `Display` message
/// anyone writes deliberately, so an unprefixed error is unambiguous — which is
/// what keeps this backward compatible with functions that never set it.
///
/// Functions built with `apiplant-function` get this for free: its generated
/// `invoke` catches unwinding panics and prefixes them. Functions written
/// against the raw ABI (in C, Zig, Go, …) should do the same.
pub const INTERNAL_ERROR_PREFIX: &str = "\x01apiplant-internal:";

/// A loaded function instance. Constructed once per library via
/// [`FunctionMod::new_functions`] and reused across requests, so it must be
/// `Send + Sync`.
#[sabi_trait]
pub trait Function: Send + Sync {
    /// Static metadata; called once right after construction.
    fn manifest(&self) -> FunctionManifest;

    /// Handle a single request. `input` is the request body as JSON; the return
    /// value is the JSON response body, or an error message the host turns into
    /// a `400` — or a `500` when prefixed with [`INTERNAL_ERROR_PREFIX`].
    ///
    /// **Must not unwind.** This crosses an `extern "C"` boundary, so a panic
    /// escaping it aborts the whole host process rather than failing the one
    /// request; catch panics here and return [`INTERNAL_ERROR_PREFIX`] instead.
    fn invoke(
        &self,
        host: HostApi_TO<'_, abi_stable::std_types::RBox<()>>,
        input: RStr<'_>,
    ) -> RResult<RString, RString>;
}

/// The root module every function library exports.
///
/// Use [`FunctionMod_Ref`] together with [`abi_stable::export_root_module`] on
/// the function side; the host loads it with [`FunctionMod_Ref::load_from_file`].
///
/// A library exports **one root module** but may carry any number of functions
/// through it — each with its own name, manifest and handler. That is what lets
/// one crate provide a whole set of related endpoints or
/// [lifecycle hooks](FunctionManifest) without a shared dispatcher.
#[repr(C)]
#[derive(StableAbi)]
#[sabi(kind(Prefix(prefix_ref = FunctionMod_Ref)))]
#[sabi(missing_field(panic))]
pub struct FunctionMod {
    /// Construct every function this library provides. Called exactly once by
    /// the host, which then reads each function's [`FunctionManifest`].
    /// Duplicate names within one library are a load error.
    #[sabi(last_prefix_field)]
    pub new_functions: extern "C" fn() -> abi_stable::std_types::RVec<BoxedFunction>,
}

impl RootModule for FunctionMod_Ref {
    declare_root_module_statics! {FunctionMod_Ref}
    const BASE_NAME: &'static str = "apiplant_function";
    const NAME: &'static str = "apiplant_function";
    const VERSION_STRINGS: abi_stable::sabi_types::VersionStrings = package_version_strings!();
}

/// Convenience alias for the boxed function trait object the host works with.
pub type BoxedFunction = Function_TO<'static, abi_stable::std_types::RBox<()>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(visibility: Visibility, role: &str, permission: &str) -> FunctionManifest {
        FunctionManifest {
            name: "act".into(),
            version: "0.0.0".into(),
            description: RString::new(),
            visibility,
            role: role.into(),
            method: HttpMethod::Post,
            permission: permission.into(),
            admin: RString::new(),
            config_schema: RString::new(),
            input_schema: RString::new(),
            output_schema: RString::new(),
        }
    }

    #[test]
    fn access_round_trips_through_its_string_form() {
        for value in [
            FunctionAccess::Public,
            FunctionAccess::Authenticated,
            FunctionAccess::Member,
            FunctionAccess::Role("buyer".into()),
            FunctionAccess::Private,
        ] {
            assert_eq!(FunctionAccess::parse(&value.as_string()), Some(value));
        }
        assert_eq!(
            FunctionAccess::parse("  member "),
            Some(FunctionAccess::Member)
        );
        // A bare `role:` names nobody, so it is not a role.
        assert_eq!(FunctionAccess::parse("role:"), None);
        assert_eq!(FunctionAccess::parse("owner"), None);
        assert_eq!(FunctionAccess::parse("wat"), None);
    }

    #[test]
    fn permission_wins_over_visibility_and_a_typo_closes_the_door() {
        // `member` is the level `visibility` cannot express.
        assert_eq!(
            manifest(Visibility::Public, "", "member").access(),
            FunctionAccess::Member
        );
        assert_eq!(
            manifest(Visibility::Private, "", "role:admin").access(),
            FunctionAccess::Role("admin".into())
        );
        // An unreadable permission hides the endpoint rather than exposing it.
        assert_eq!(
            manifest(Visibility::Public, "", "membre").access(),
            FunctionAccess::Private
        );
    }

    #[test]
    fn a_manifest_without_permission_keeps_the_access_visibility_gave_it() {
        assert_eq!(
            manifest(Visibility::Public, "", "").access(),
            FunctionAccess::Public
        );
        assert_eq!(
            manifest(Visibility::Authenticated, "", "").access(),
            FunctionAccess::Authenticated
        );
        assert_eq!(
            manifest(Visibility::RoleGated, "buyer", "").access(),
            FunctionAccess::Role("buyer".into())
        );
        assert_eq!(
            manifest(Visibility::Private, "", "").access(),
            FunctionAccess::Private
        );
    }
}
