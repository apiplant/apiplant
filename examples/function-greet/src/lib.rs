//! Example apiplant function.
//!
//! Build it with `cargo build -p function-greet --release` and drop the
//! resulting `libfunction_greet.so` into an app's `functions/` directory. The
//! framework loads it at boot and mounts it at `POST /functions/greet`.
//!
//! It reads a `greeting` string from its config, counts how many times it has
//! been called by asking the host database, and returns a greeting.
#![allow(non_local_definitions)]

use abi_stable::{
    export_root_module,
    prefix_type::PrefixTypeTrait,
    sabi_extern_fn,
    sabi_trait::TD_Opaque,
    std_types::{RResult, RStr, RString},
};
use apiplant_abi::{
    BoxedFunction, Function, FunctionMod, FunctionMod_Ref, FunctionManifest, Function_TO,
    HostApi_TO, HttpMethod, LogLevel, Visibility,
};
use serde::Deserialize;

/// Exported entry point discovered by the host loader.
#[export_root_module]
fn instantiate_root_module() -> FunctionMod_Ref {
    FunctionMod { new }.leak_into_prefix()
}

#[sabi_extern_fn]
fn new() -> BoxedFunction {
    Function_TO::from_value(Greet, TD_Opaque)
}

struct Greet;

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default = "default_greeting")]
    greeting: String,
}

fn default_greeting() -> String {
    "Hello".to_string()
}

#[derive(Deserialize)]
struct Input {
    name: String,
}

impl Function for Greet {
    fn manifest(&self) -> FunctionManifest {
        FunctionManifest {
            name: "greet".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Greets a person and counts total greetings.".into(),
            visibility: Visibility::Public,
            role: RString::new(),
            method: HttpMethod::Post,
            config_schema: r#"{"type":"object","properties":{"greeting":{"type":"string"}}}"#
                .into(),
        }
    }

    fn invoke(
        &self,
        host: HostApi_TO<'_, abi_stable::std_types::RBox<()>>,
        input: RStr<'_>,
    ) -> RResult<RString, RString> {
        let cfg: Config = serde_json::from_str(host.config().as_str()).unwrap_or_default();

        let input: Input = match serde_json::from_str(input.as_str()) {
            Ok(v) => v,
            Err(e) => return RResult::RErr(format!("invalid input: {e}").into()),
        };

        // Reach into the host database. The host owns the connection; we only
        // speak JSON to it. Counting registered users demonstrates real access.
        let req = r#"{"sql":"SELECT count(*)::int AS n FROM apiplant_user","params":[]}"#;
        let user_count = match host.query(RStr::from_str(req)) {
            RResult::ROk(rows) => serde_json::from_str::<serde_json::Value>(rows.as_str())
                .ok()
                .and_then(|v| v.get(0).and_then(|r| r.get("n")).and_then(|n| n.as_i64()))
                .unwrap_or(0),
            RResult::RErr(_) => 0,
        };

        host.log(LogLevel::Info, RStr::from_str("greet invoked"));

        let body = serde_json::json!({
            "message": format!("{}, {}!", cfg.greeting, input.name),
            "registered_users": user_count,
        });
        RResult::ROk(body.to_string().into())
    }
}
