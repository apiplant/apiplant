//! The watchdog, in a test binary of its own.
//!
//! It tunes `APIPLANT_JS_WORKERS` and `APIPLANT_JS_TIMEOUT_MS`, which are process
//! wide — so it gets its own process rather than racing every other test that
//! loads a module.

use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{BoxedFunction, HostApi, HostApi_TO, LogLevel};

/// A host that answers nothing: the function under test never asks it anything.
#[derive(Clone)]
struct SilentHost;

impl HostApi for SilentHost {
    fn query(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no database in this test".into())
    }
    fn send_email(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no mailer in this test".into())
    }
    fn cache(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no cache in this test".into())
    }
    fn payments(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no payments in this test".into())
    }
    fn ai(&self, _request: RStr<'_>) -> RResult<RString, RString> {
        RResult::RErr("no assistant in this test".into())
    }
    fn emit(&self, _chunk: RStr<'_>) -> bool {
        false
    }
    fn log(&self, _level: LogLevel, _message: RStr<'_>) {}
    fn config(&self) -> RString {
        "{}".into()
    }
    fn principal_id(&self) -> RString {
        RString::new()
    }
    fn hook(&self) -> RString {
        RString::new()
    }
}

/// Transpile, write and load, as `apiplant build` and the server would.
fn build(label: &str, source: &str) -> Result<Vec<BoxedFunction>, String> {
    let js = apiplant_js::transpile::to_js(label, source)?;
    let dir = std::env::temp_dir().join(format!("apiplant-js-watchdog-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{label}.js"));
    std::fs::write(&path, js).unwrap();
    apiplant_js::load(&path)
}

fn call(functions: &[BoxedFunction], name: &str) -> Result<String, String> {
    let function = functions
        .iter()
        .find(|f| f.manifest().name.as_str() == name)
        .unwrap();
    match function.invoke(
        HostApi_TO::from_value(SilentHost, TD_Opaque),
        RStr::from_str("{}"),
    ) {
        RResult::ROk(out) => Ok(out.into_string()),
        RResult::RErr(e) => Err(e.into_string()),
    }
}

/// An isolate is not preemptible, so a runaway loop would hold its worker until
/// the process died. The watchdog is what makes that one failed request.
#[test]
fn a_runaway_function_is_terminated_and_its_worker_recovers() {
    std::env::set_var("APIPLANT_JS_TIMEOUT_MS", "300");
    // One worker, so the recovery below is provably the *same* isolate.
    std::env::set_var("APIPLANT_JS_WORKERS", "1");

    let functions = build(
        "runaway",
        r#"
        export const manifest = [
            { name: "spin", permission: "public" },
            { name: "fine", permission: "public" },
        ];
        export function spin() { for (;;) {} }
        export function fine() { return { ok: true }; }
        "#,
    )
    .unwrap();

    let err = call(&functions, "spin").unwrap_err();
    assert!(
        err.starts_with(apiplant_abi::INTERNAL_ERROR_PREFIX),
        "{err}"
    );

    // The isolate was interrupted, not poisoned.
    assert_eq!(call(&functions, "fine").unwrap(), r#"{"ok":true}"#);

    std::env::remove_var("APIPLANT_JS_TIMEOUT_MS");
    std::env::remove_var("APIPLANT_JS_WORKERS");
}
