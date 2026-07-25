//! Loading function libraries that speak the [plain C ABI](apiplant_abi::c).
//!
//! The host tries the `abi_stable` root module first; a library without one is
//! offered here. What comes back is an ordinary [`BoxedFunction`], so everything
//! downstream — routing, visibility, OpenAPI, lifecycle hooks — treats a function
//! written in C exactly like one written in Rust.
//!
//! Two rules govern everything below, and both exist because the other side of
//! the boundary is not Rust:
//!
//! * **Nothing may unwind into C.** Every callback the host exposes is wrapped in
//!   [`catch_unwind`], because a panic crossing an `extern "C"` frame aborts the
//!   process instead of failing one request.
//! * **Each side frees what it allocated.** The library and the host may not
//!   share an allocator, so host strings go back through `free_string` and the
//!   library's output comes back through its own `apiplant_free`.

use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::catch_unwind;
use std::path::Path;

use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::c as cabi;
use apiplant_abi::{
    BoxedFunction, Function, FunctionAccess, FunctionManifest, HostApi_TO, HttpMethod, LogLevel,
    Visibility,
};
use libloading::{Library, Symbol};
use serde_json::Value;

/// A function living in a C shared library, presented as an ABI function object.
struct CFunction {
    manifest: FunctionManifest,
    /// The manifest name, as a C string, ready to pass to `invoke`.
    name: CString,
    invoke: cabi::InvokeFn,
    free: cabi::FreeFn,
    /// Keeps the library mapped for as long as any function from it exists.
    /// Never dropped in practice — the registry lives for the process — but
    /// holding it here is what makes that a guarantee rather than a hope.
    _library: &'static Library,
}

// SAFETY: the library is leaked, `invoke`/`free` are plain code pointers into it,
// and the ABI requires `apiplant_invoke` to be callable from several threads at
// once — the same requirement the Rust `Function` trait states with `Send + Sync`.
unsafe impl Send for CFunction {}
unsafe impl Sync for CFunction {}

impl Function for CFunction {
    fn manifest(&self) -> FunctionManifest {
        self.manifest.clone()
    }

    fn invoke(
        &self,
        host: HostApi_TO<'_, abi_stable::std_types::RBox<()>>,
        input: RStr<'_>,
    ) -> RResult<RString, RString> {
        // The input may legitimately contain interior NULs only if a client sent
        // them; C cannot represent that, so reject it rather than truncate.
        let Ok(input) = CString::new(input.as_str()) else {
            return RResult::RErr(RString::from("input contains a NUL byte"));
        };

        // `bridge` is borrowed by the callbacks for exactly this call. It never
        // escapes: `apiplant_invoke` returns before the borrow ends.
        let mut bridge = Bridge { host: &host };
        let c_host = cabi::Host {
            ctx: &mut bridge as *mut Bridge<'_, '_> as *mut c_void,
            query: Some(host_query),
            log: Some(host_log),
            config: Some(host_config),
            principal_id: Some(host_principal_id),
            hook: Some(host_hook),
            free_string: Some(host_free_string),
        };

        let mut out: *mut c_char = std::ptr::null_mut();
        // SAFETY: `invoke` came from this library's symbol table with the ABI's
        // signature; the three pointers are valid for the duration of the call.
        let status =
            unsafe { (self.invoke)(self.name.as_ptr(), input.as_ptr(), &c_host, &mut out) };

        let message = self.take_string(out);
        match status {
            cabi::OK => RResult::ROk(RString::from(message.unwrap_or_default())),
            cabi::ERR_REQUEST => RResult::RErr(RString::from(
                message.unwrap_or_else(|| "function rejected the request".to_string()),
            )),
            // Anything that isn't OK or ERR_REQUEST is the function's fault,
            // including codes from a future ABI this host doesn't know.
            _ => RResult::RErr(RString::from(format!(
                "{}{}",
                apiplant_abi::INTERNAL_ERROR_PREFIX,
                message.unwrap_or_else(|| format!("function returned status {status}"))
            ))),
        }
    }
}

impl CFunction {
    /// Copy a string the library produced, then hand the original back for it to
    /// free. Returns `None` for a null pointer, which is how a function signals
    /// "no body" (and what we get if it forgot to set `*out` at all).
    fn take_string(&self, ptr: *mut c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        // SAFETY: non-null and, per the ABI, NUL-terminated and owned by the
        // library until we return it to `apiplant_free` below.
        let owned = unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        // SAFETY: same pointer, handed straight back to its own allocator.
        unsafe { (self.free)(ptr) };
        Some(owned)
    }
}

/// What `Host::ctx` points at: the Rust host API for one in-flight call.
struct Bridge<'a, 'b> {
    host: &'a HostApi_TO<'b, abi_stable::std_types::RBox<()>>,
}

/// Recover the bridge inside a callback.
///
/// # Safety
/// `ctx` must be the pointer the host put in [`cabi::Host::ctx`], and the call
/// must still be in progress.
unsafe fn bridge<'a>(ctx: *mut c_void) -> Option<&'a Bridge<'a, 'a>> {
    (ctx as *const Bridge<'a, 'a>).as_ref()
}

/// Hand a string to C. The callee returns it to [`host_free_string`].
///
/// A string with an interior NUL cannot be represented in C; that would mean the
/// database returned one, so the empty string is the honest answer rather than a
/// silent truncation at the NUL.
fn to_c(s: &str) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

/// Wrap a callback body so a panic becomes a null pointer instead of an abort.
fn guard_string<F: FnOnce() -> *mut c_char>(f: F) -> *mut c_char {
    match catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(ptr) => ptr,
        Err(_) => {
            tracing::error!("panic in a host callback serving a C function");
            std::ptr::null_mut()
        }
    }
}

extern "C" fn host_query(ctx: *mut c_void, request: *const c_char) -> *mut c_char {
    guard_string(|| {
        // SAFETY: `ctx` is the bridge for the call in progress; `request` is a
        // NUL-terminated string the callee owns for the duration of this call.
        let (Some(bridge), Some(request)) = (unsafe { bridge(ctx) }, unsafe { cstr(request) })
        else {
            return to_c(r#"{"error":"invalid query request"}"#);
        };
        match bridge.host.query(RStr::from_str(&request)) {
            RResult::ROk(rows) => to_c(rows.as_str()),
            // Reported in-band: the shape (object with "error") is what tells a
            // failure apart from a result set. See `apiplant_abi::c::Host::query`.
            RResult::RErr(e) => {
                let body = serde_json::json!({ "error": e.as_str() });
                to_c(&body.to_string())
            }
        }
    })
}

extern "C" fn host_log(ctx: *mut c_void, level: i32, message: *const c_char) {
    let _ = catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: as in `host_query`.
        let (Some(bridge), Some(message)) = (unsafe { bridge(ctx) }, unsafe { cstr(message) })
        else {
            return;
        };
        let level = match level {
            cabi::log_level::TRACE => LogLevel::Trace,
            cabi::log_level::DEBUG => LogLevel::Debug,
            cabi::log_level::WARN => LogLevel::Warn,
            cabi::log_level::ERROR => LogLevel::Error,
            // Includes INFO and any level from a future ABI.
            _ => LogLevel::Info,
        };
        bridge.host.log(level, RStr::from_str(&message));
    }));
}

extern "C" fn host_config(ctx: *mut c_void) -> *mut c_char {
    // SAFETY: as in `host_query`.
    guard_string(|| match unsafe { bridge(ctx) } {
        Some(b) => to_c(b.host.config().as_str()),
        None => to_c("{}"),
    })
}

extern "C" fn host_principal_id(ctx: *mut c_void) -> *mut c_char {
    // SAFETY: as in `host_query`.
    guard_string(|| match unsafe { bridge(ctx) } {
        Some(b) => to_c(b.host.principal_id().as_str()),
        None => to_c(""),
    })
}

extern "C" fn host_hook(ctx: *mut c_void) -> *mut c_char {
    // SAFETY: as in `host_query`.
    guard_string(|| match unsafe { bridge(ctx) } {
        Some(b) => to_c(b.host.hook().as_str()),
        None => to_c(""),
    })
}

extern "C" fn host_free_string(_ctx: *mut c_void, string: *mut c_char) {
    if string.is_null() {
        return;
    }
    // SAFETY: every string the callbacks above return came from
    // `CString::into_raw`, so this is the matching `from_raw`. A library that
    // passes anything else violates the ABI.
    drop(unsafe { CString::from_raw(string) });
}

/// Borrow a C string as UTF-8, lossily.
///
/// # Safety
/// `ptr` must be null or a valid NUL-terminated string.
unsafe fn cstr(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
}

/// Try to load `path` as a C-ABI library, returning its functions.
///
/// `Ok(None)` means "this isn't a C-ABI library" — no `apiplant_abi_version`
/// symbol — which lets the caller report the *original* `abi_stable` failure
/// instead of a confusing second one. `Err` means it is one and is broken.
pub fn load(path: &Path) -> Result<Option<Vec<BoxedFunction>>, String> {
    // SAFETY: loading any shared library runs its initialisers; that is inherent
    // to the feature and no more unsafe here than for the `abi_stable` path.
    let library = match unsafe { Library::new(path) } {
        Ok(l) => l,
        Err(e) => return Err(format!("cannot open library: {e}")),
    };

    // Probe before committing: a missing version symbol just means this library
    // speaks the other ABI.
    // SAFETY: the symbol's type is asserted to match the ABI's `AbiVersionFn`.
    let version = unsafe { library.get::<cabi::AbiVersionFn>(cabi::SYM_ABI_VERSION) };
    let Ok(version) = version else {
        return Ok(None);
    };
    // SAFETY: calling a function the library exported under the documented name.
    let version = unsafe { version() };
    if version != cabi::ABI_VERSION {
        return Err(format!(
            "library targets apiplant C ABI version {version}, this host speaks {}",
            cabi::ABI_VERSION
        ));
    }

    let symbol = |name: &[u8]| -> Result<*const (), String> {
        // SAFETY: resolved as an untyped pointer and transmuted by the caller to
        // the signature the ABI documents for that name.
        unsafe {
            library
                .get::<*const ()>(name)
                .map(|s: Symbol<'_, *const ()>| *s)
                .map_err(|e| {
                    format!(
                        "library exports `apiplant_abi_version` but not `{}`: {e}",
                        String::from_utf8_lossy(&name[..name.len() - 1])
                    )
                })
        }
    };

    let manifest_ptr = symbol(cabi::SYM_MANIFEST)?;
    let invoke_ptr = symbol(cabi::SYM_INVOKE)?;
    let free_ptr = symbol(cabi::SYM_FREE)?;

    // SAFETY: each pointer resolved from the documented symbol name, transmuted
    // to the signature `apiplant_abi::c` specifies for it.
    let (manifest_fn, invoke, free): (cabi::ManifestFn, cabi::InvokeFn, cabi::FreeFn) = unsafe {
        (
            std::mem::transmute::<*const (), cabi::ManifestFn>(manifest_ptr),
            std::mem::transmute::<*const (), cabi::InvokeFn>(invoke_ptr),
            std::mem::transmute::<*const (), cabi::FreeFn>(free_ptr),
        )
    };

    // The manifest must outlive the borrow, and the library must stay mapped for
    // as long as any function points into it. Leaking is how the `abi_stable`
    // path does it too — nothing ever unloads a function library.
    let library: &'static Library = Box::leak(Box::new(library));

    // SAFETY: the ABI requires a static, NUL-terminated string here.
    let json = unsafe { cstr(manifest_fn()) }
        .ok_or_else(|| "`apiplant_manifest` returned NULL".to_string())?;
    let entries: Vec<Value> = serde_json::from_str::<Value>(&json)
        .map_err(|e| format!("`apiplant_manifest` is not valid JSON: {e}"))?
        .as_array()
        .cloned()
        .ok_or_else(|| "`apiplant_manifest` must return a JSON array".to_string())?;
    if entries.is_empty() {
        return Err("`apiplant_manifest` returned an empty array".to_string());
    }

    let mut functions = Vec::with_capacity(entries.len());
    for entry in &entries {
        let manifest = parse_manifest(entry)?;
        let name = CString::new(manifest.name.as_str())
            .map_err(|_| "a function name contains a NUL byte".to_string())?;
        functions.push(BoxedFunction::from_value(
            CFunction {
                manifest,
                name,
                invoke,
                free,
                _library: library,
            },
            TD_Opaque,
        ));
    }
    Ok(Some(functions))
}

/// Build a [`FunctionManifest`] from one entry of `apiplant_manifest`'s array.
fn parse_manifest(entry: &Value) -> Result<FunctionManifest, String> {
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "a manifest entry has no `name`".to_string())?;

    let string = |key: &str| -> RString {
        entry
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into()
    };

    // Schemas may be written inline as objects or handed over pre-serialised.
    let schema = |key: &str| -> RString {
        match entry.get(key) {
            None | Some(Value::Null) => RString::new(),
            Some(Value::String(s)) => s.as_str().into(),
            Some(other) => other.to_string().into(),
        }
    };

    // `permission` is the current key and `visibility` the original one; they
    // share a grammar, so either may carry the policy and `permission` wins.
    let access_str = entry
        .get("permission")
        .or_else(|| entry.get("visibility"))
        .and_then(Value::as_str)
        // Default to the closed end: an unreadable or absent policy should hide
        // an endpoint, never publish one.
        .unwrap_or("private");
    let access = FunctionAccess::parse(access_str).ok_or_else(|| {
        format!(
            "function `{name}` has unknown permission `{access_str}`; expected \
             \"public\", \"authenticated\", \"member\", \"role:<name>\" or \"private\""
        )
    })?;
    // `visibility` is kept in step so anything still reading it — generated
    // docs, older tooling — sees the nearest equivalent.
    let (visibility, role) = match &access {
        FunctionAccess::Public => (Visibility::Public, String::new()),
        FunctionAccess::Authenticated | FunctionAccess::Member => {
            (Visibility::Authenticated, String::new())
        }
        FunctionAccess::Role(role) => (Visibility::RoleGated, role.clone()),
        FunctionAccess::Private => (Visibility::Private, String::new()),
    };

    let method_str = entry
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("POST");
    let method = match method_str.to_ascii_uppercase().as_str() {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "DELETE" => HttpMethod::Delete,
        other => {
            return Err(format!(
                "function `{name}` has unsupported method `{other}`; \
                 expected GET, POST, PUT or DELETE"
            ))
        }
    };

    Ok(FunctionManifest {
        name: name.into(),
        version: match entry.get("version").and_then(Value::as_str) {
            Some(v) => v.into(),
            None => "0.0.0".into(),
        },
        description: string("description"),
        visibility,
        role: role.into(),
        method,
        permission: access.as_string().into(),
        admin: schema("admin"),
        config_schema: schema("config_schema"),
        input_schema: schema("input_schema"),
        output_schema: schema("output_schema"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(json: &str) -> Result<FunctionManifest, String> {
        parse_manifest(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn a_name_is_the_only_required_field() {
        let m = entry(r#"{"name":"hello"}"#).unwrap();
        assert_eq!(m.name.as_str(), "hello");
        assert_eq!(m.version.as_str(), "0.0.0");
        assert_eq!(m.method, HttpMethod::Post);
        assert!(m.description.is_empty());
    }

    #[test]
    fn a_missing_or_unreadable_name_is_an_error() {
        assert!(entry(r#"{"description":"no name"}"#).is_err());
        assert!(entry(r#"{"name":""}"#).is_err());
        assert!(entry(r#"{"name":42}"#).is_err());
    }

    /// The safe direction: a function that says nothing stays unreachable.
    #[test]
    fn visibility_defaults_to_private() {
        assert_eq!(
            entry(r#"{"name":"h"}"#).unwrap().visibility,
            Visibility::Private
        );
    }

    #[test]
    fn visibility_uses_the_same_strings_as_resource_permissions() {
        let vis = |v: &str| entry(&format!(r#"{{"name":"h","visibility":"{v}"}}"#)).unwrap();
        assert_eq!(vis("public").visibility, Visibility::Public);
        assert_eq!(vis("authenticated").visibility, Visibility::Authenticated);
        assert_eq!(vis("private").visibility, Visibility::Private);

        let gated = vis("role:admin");
        assert_eq!(gated.visibility, Visibility::RoleGated);
        assert_eq!(gated.role.as_str(), "admin");
    }

    /// A typo must not silently become `private` and leave the author wondering
    /// why their endpoint 404s — unlike an *absent* field, which is a choice.
    #[test]
    fn an_unknown_visibility_is_rejected() {
        let err = entry(r#"{"name":"h","visibility":"pubic"}"#).unwrap_err();
        assert!(err.contains("unknown permission"), "{err}");
        assert!(entry(r#"{"name":"h","visibility":"role:"}"#).is_err());
    }

    /// `permission` is the current key for the policy a C library declares, and
    /// it may say things `visibility` never could.
    #[test]
    fn permission_is_read_and_outranks_visibility() {
        let member = entry(r#"{"name":"h","permission":"member"}"#).unwrap();
        assert_eq!(member.access(), FunctionAccess::Member);
        // `member` has no Visibility of its own, so the legacy field carries the
        // nearest thing rather than something wider.
        assert_eq!(member.visibility, Visibility::Authenticated);

        let both = entry(r#"{"name":"h","visibility":"public","permission":"role:ops"}"#).unwrap();
        assert_eq!(both.access(), FunctionAccess::Role("ops".into()));
        assert_eq!(both.visibility, Visibility::RoleGated);
        assert_eq!(both.role.as_str(), "ops");

        // Absent means private — a library that says nothing exposes nothing.
        assert_eq!(
            entry(r#"{"name":"h"}"#).unwrap().access(),
            FunctionAccess::Private
        );
    }

    /// The dashboard block is passed through verbatim; the admin generator, not
    /// the loader, is what understands its shape.
    #[test]
    fn the_admin_block_survives_as_an_object_or_a_string() {
        let inline = entry(r#"{"name":"h","admin":{"label":"Do it","order":2}}"#).unwrap();
        let parsed: Value = serde_json::from_str(inline.admin.as_str()).unwrap();
        assert_eq!(parsed["label"], "Do it");
        assert_eq!(parsed["order"], 2);

        let preserialised = entry(r#"{"name":"h","admin":"{\"label\":\"Do it\"}"}"#).unwrap();
        assert_eq!(preserialised.admin.as_str(), r#"{"label":"Do it"}"#);

        assert!(entry(r#"{"name":"h"}"#).unwrap().admin.is_empty());
    }

    #[test]
    fn methods_are_case_insensitive_and_validated() {
        let m = |v: &str| entry(&format!(r#"{{"name":"h","method":"{v}"}}"#));
        assert_eq!(m("get").unwrap().method, HttpMethod::Get);
        assert_eq!(m("Put").unwrap().method, HttpMethod::Put);
        assert_eq!(m("DELETE").unwrap().method, HttpMethod::Delete);

        let err = m("PATCH").unwrap_err();
        assert!(err.contains("unsupported method"), "{err}");
    }

    /// Schemas are for the docs, and C has no derive to generate them — so both
    /// an inline object and an already-serialised string have to work.
    #[test]
    fn schemas_accept_an_object_or_a_string() {
        let inline = entry(r#"{"name":"h","input_schema":{"type":"object"}}"#).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(inline.input_schema.as_str()).unwrap(),
            serde_json::json!({"type":"object"})
        );

        let preserialised =
            entry(r#"{"name":"h","input_schema":"{\"type\":\"string\"}"}"#).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(preserialised.input_schema.as_str()).unwrap(),
            serde_json::json!({"type":"string"})
        );

        assert!(entry(r#"{"name":"h"}"#).unwrap().input_schema.is_empty());
        assert!(entry(r#"{"name":"h","input_schema":null}"#)
            .unwrap()
            .input_schema
            .is_empty());
    }

    /// Not a C-ABI library and not a valid library at all both have to be
    /// distinguishable from "loaded fine", or the caller reports the wrong error.
    #[test]
    fn a_library_that_is_not_c_abi_is_not_an_error() {
        let dir = std::env::temp_dir().join(format!("apiplant-cabi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("libgarbage.so");
        std::fs::write(&path, b"not an elf file").unwrap();

        // Unopenable: a real error.
        assert!(load(&path).is_err());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
