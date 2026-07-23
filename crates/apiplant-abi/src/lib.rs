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
//! `new` constructor yields a [`Function`] trait object. The host:
//!
//! 1. loads the library and reads the [`FunctionManifest`] (name, visibility,
//!    HTTP method, config schema),
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

use abi_stable::{
    declare_root_module_statics,
    library::RootModule,
    package_version_strings,
    sabi_trait,
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
    /// Optional JSON-Schema describing the function's config object. Empty = none.
    pub config_schema: RString,
    /// Optional JSON-Schema for the request body. Empty = untyped. Surfaced in
    /// the generated OpenAPI document so function I/O is typed in the docs.
    pub input_schema: RString,
    /// Optional JSON-Schema for the response body. Empty = untyped.
    pub output_schema: RString,
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

    /// The function's resolved configuration as a JSON object (merged defaults +
    /// per-deployment overrides from the app's `functions/<name>.toml`).
    fn config(&self) -> RString;

    /// Id of the authenticated principal calling this function, or empty when
    /// the endpoint is [`Visibility::Public`] and the caller is anonymous.
    fn principal_id(&self) -> RString;
}

/// A loaded function instance. Constructed once per library via
/// [`FunctionMod::new`] and reused across requests, so it must be `Send + Sync`.
#[sabi_trait]
pub trait Function: Send + Sync {
    /// Static metadata; called once right after construction.
    fn manifest(&self) -> FunctionManifest;

    /// Handle a single request. `input` is the request body as JSON; the return
    /// value is the JSON response body, or an error message the host turns into
    /// a `400`/`500`.
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
#[repr(C)]
#[derive(StableAbi)]
#[sabi(kind(Prefix(prefix_ref = FunctionMod_Ref)))]
#[sabi(missing_field(panic))]
pub struct FunctionMod {
    /// Construct the function instance. Called exactly once by the host.
    #[sabi(last_prefix_field)]
    pub new: extern "C" fn() -> Function_TO<'static, abi_stable::std_types::RBox<()>>,
}

impl RootModule for FunctionMod_Ref {
    declare_root_module_statics! {FunctionMod_Ref}
    const BASE_NAME: &'static str = "apiplant_function";
    const NAME: &'static str = "apiplant_function";
    const VERSION_STRINGS: abi_stable::sabi_types::VersionStrings = package_version_strings!();
}

/// Convenience alias for the boxed function trait object the host works with.
pub type BoxedFunction = Function_TO<'static, abi_stable::std_types::RBox<()>>;
