//! A plain C ABI, for functions written in something other than Rust.
//!
//! The [main contract](crate) is expressed in [`abi_stable`] types — `RString`,
//! `RResult`, `#[sabi_trait]` vtables, a root module whose header carries type
//! layout metadata the host verifies at load time. That machinery is what makes
//! the Rust-to-Rust boundary safe across compiler versions, but it is not
//! something you can hand-write in C, Zig or Go.
//!
//! So a library may instead export the six plain C symbols below. The host tries
//! the `abi_stable` root module first and falls back to these, wrapping whatever
//! it finds in the same [`Function`](crate::Function) trait object — so a C
//! function is mounted, authenticated, documented in the OpenAPI spec and usable
//! as a lifecycle hook exactly like a Rust one.
//!
//! ## What the library exports
//!
//! ```c
//! uint32_t     apiplant_abi_version(void);
//! const char  *apiplant_manifest(void);
//! int32_t      apiplant_invoke(const char *name, const char *input_json,
//!                              const ApiplantHost *host, char **out);
//! void         apiplant_free(char *string);
//! ```
//!
//! `apiplant_manifest` returns a JSON **array** — one object per function the
//! library provides, mirroring what `function!` generates on the Rust side:
//!
//! ```json
//! [{ "name": "hello", "version": "1.0.0", "description": "Greets someone.",
//!    "visibility": "public", "method": "POST",
//!    "input_schema": { "type": "object" }, "output_schema": { "type": "object" } }]
//! ```
//!
//! Only `name` is required. `visibility` takes the same strings as a resource's
//! permissions (`"public"`, `"authenticated"`, `"role:admin"`, `"private"`, and
//! it defaults to `"private"` — the safe direction, so a typo hides an endpoint
//! rather than exposing it). `method` defaults to `"POST"`. The two schema fields
//! are optional and may be given as an object or as a JSON string; they only feed
//! the generated docs.
//!
//! ## Memory
//!
//! Each side frees what it allocated, because the two do not share an allocator:
//!
//! * The string `apiplant_invoke` writes to `*out` is released by the host
//!   calling the library's own [`apiplant_free`](FreeFn).
//! * Strings the host returns from [`Host::config`], [`Host::query`],
//!   [`Host::principal_id`], [`Host::hook`], [`Host::send_email`] and
//!   [`Host::cache`], [`Host::payments`] and [`Host::ai`] are released by the library calling
//!   [`Host::free_string`].
//!
//! The pointer from `apiplant_manifest` is never freed, so it must be static.
//!
//! ## Growing the host
//!
//! [`Host`] gains callbacks at its **end**, never in the middle: the host is
//! what allocates the struct, so a library compiled against an older, shorter
//! definition still finds every field it knows at the offset it expects, and
//! simply never reads the ones added since. That is why [`ABI_VERSION`] does
//! not change when a callback is appended — and why it must change if one is
//! ever removed or reordered.
//!
//! ## Faults
//!
//! [`Host`]'s callbacks never unwind — the host catches its own panics before
//! they reach C. In the other direction the return code separates a bad request
//! from a broken function, which is what the string-prefix convention
//! ([`INTERNAL_ERROR_PREFIX`](crate::INTERNAL_ERROR_PREFIX)) expresses in Rust:
//!
//! | code | meaning | response |
//! |------|---------|----------|
//! | [`OK`] | `*out` is the JSON response body | `200` |
//! | [`ERR_REQUEST`] | `*out` is a message for the caller | `400` |
//! | [`ERR_INTERNAL`] | `*out` is a message for the log | `500`, message withheld |

use core::ffi::{c_char, c_void};

/// Version of this C contract. The host refuses a library reporting anything
/// else, so a breaking change here is a clean load error rather than a crash.
pub const ABI_VERSION: u32 = 1;

/// `*out` holds the JSON response body.
pub const OK: i32 = 0;
/// `*out` holds a message describing what was wrong with the request (`400`).
pub const ERR_REQUEST: i32 = 1;
/// `*out` holds a message describing how the function broke (`500`). The host
/// logs it and does not echo it to the caller.
pub const ERR_INTERNAL: i32 = 2;

/// Severities accepted by [`Host::log`]; matches [`LogLevel`](crate::LogLevel).
pub mod log_level {
    pub const TRACE: i32 = 0;
    pub const DEBUG: i32 = 1;
    pub const INFO: i32 = 2;
    pub const WARN: i32 = 3;
    pub const ERROR: i32 = 4;
}

/// Services the host lends to a C function for the duration of one call.
///
/// The host fills this in and passes a pointer that is valid **only** until
/// `apiplant_invoke` returns; `ctx` must be handed back to every callback
/// untouched. Each `char *` the host returns is owned by the callee and must be
/// released with [`free_string`](Host::free_string).
#[repr(C)]
pub struct Host {
    /// Opaque host state. Pass it back to every callback; never dereference it.
    pub ctx: *mut c_void,

    /// Run a query. `request_json` is `{"sql": "…", "params": [ … ]}`. Returns a
    /// JSON array of rows, `{"rows_affected": n}`, or `{"error": "…"}` when the
    /// query failed — the shape distinguishes them, so there is no out-param.
    pub query: Option<extern "C" fn(ctx: *mut c_void, request_json: *const c_char) -> *mut c_char>,

    /// Emit a log line through the host's `tracing` subscriber. `level` is one of
    /// [`log_level`]; anything else is treated as `INFO`.
    pub log: Option<extern "C" fn(ctx: *mut c_void, level: i32, message: *const c_char)>,

    /// The function's resolved configuration, as a JSON object.
    pub config: Option<extern "C" fn(ctx: *mut c_void) -> *mut c_char>,

    /// Id of the authenticated caller, or an empty string when anonymous.
    pub principal_id: Option<extern "C" fn(ctx: *mut c_void) -> *mut c_char>,

    /// Lifecycle-hook context as JSON, or an empty string for a plain HTTP call.
    /// See [`HostApi::hook`](crate::HostApi::hook) for the shape.
    pub hook: Option<extern "C" fn(ctx: *mut c_void) -> *mut c_char>,

    /// Release a string one of the callbacks above returned.
    pub free_string: Option<extern "C" fn(ctx: *mut c_void, string: *mut c_char)>,

    /// Send an email. `request_json` is the message
    /// (`{"to":…,"subject":…,"text":…}`); returns the receipt
    /// `{"provider":…,"id":…,"recipients":n}` or `{"error":"…"}`. As with
    /// [`query`](Self::query) the shape distinguishes them.
    /// See [`HostApi::send_email`](crate::HostApi::send_email).
    pub send_email:
        Option<extern "C" fn(ctx: *mut c_void, request_json: *const c_char) -> *mut c_char>,

    /// Run one cache operation. `request_json` is `{"op":"get","key":"…"}` and
    /// friends; returns the operation's reply or `{"error":"…"}`.
    /// See [`HostApi::cache`](crate::HostApi::cache).
    pub cache: Option<extern "C" fn(ctx: *mut c_void, request_json: *const c_char) -> *mut c_char>,

    /// Run one payment operation. `request_json` is
    /// `{"op":"checkout","stripe_price_id":"…"}` and friends; returns the
    /// operation's reply or `{"error":"…"}`.
    /// See [`HostApi::payments`](crate::HostApi::payments).
    pub payments:
        Option<extern "C" fn(ctx: *mut c_void, request_json: *const c_char) -> *mut c_char>,

    /// Ask the AI assistant. `request_json` is a conversation
    /// (`{"messages":[{"role":"user","content":"…"}]}`); returns the answer
    /// `{"text":…,"provider":…,"model":…}` or `{"error":"…"}`.
    /// See [`HostApi::ai`](crate::HostApi::ai).
    pub ai: Option<extern "C" fn(ctx: *mut c_void, request_json: *const c_char) -> *mut c_char>,

    /// Push a chunk of the response to the caller before the call returns.
    /// Returns non-zero while it is still worth producing more, and zero once
    /// the caller has hung up. See [`HostApi::emit`](crate::HostApi::emit).
    pub emit: Option<extern "C" fn(ctx: *mut c_void, chunk: *const c_char) -> i32>,
}

/// Reports the contract the library was built against; must return
/// [`ABI_VERSION`].
pub type AbiVersionFn = unsafe extern "C" fn() -> u32;

/// Returns a static, NUL-terminated JSON array of manifests. Never freed.
pub type ManifestFn = unsafe extern "C" fn() -> *const c_char;

/// Handles one request. Writes a NUL-terminated string to `*out` and returns
/// [`OK`], [`ERR_REQUEST`] or [`ERR_INTERNAL`]. Must not unwind or longjmp.
pub type InvokeFn = unsafe extern "C" fn(
    name: *const c_char,
    input_json: *const c_char,
    host: *const Host,
    out: *mut *mut c_char,
) -> i32;

/// Releases a string produced by [`InvokeFn`].
pub type FreeFn = unsafe extern "C" fn(string: *mut c_char);

/// Symbol the host looks up to decide a library speaks this ABI.
pub const SYM_ABI_VERSION: &[u8] = b"apiplant_abi_version\0";
/// Symbol for [`ManifestFn`].
pub const SYM_MANIFEST: &[u8] = b"apiplant_manifest\0";
/// Symbol for [`InvokeFn`].
pub const SYM_INVOKE: &[u8] = b"apiplant_invoke\0";
/// Symbol for [`FreeFn`].
pub const SYM_FREE: &[u8] = b"apiplant_free\0";
