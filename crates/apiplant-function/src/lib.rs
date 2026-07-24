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
}

impl<'a, 'h, C> Context<'a, 'h, C> {
    /// Internal constructor used by generated code.
    #[doc(hidden)]
    pub fn __new(host: &'a HostApi_TO<'h, RBox<()>>, config: C, principal_id: String) -> Self {
        Context {
            host,
            config,
            principal_id,
        }
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

    let input: I = match serde_json::from_str(input.as_str()) {
        Ok(v) => v,
        Err(e) => return RResult::RErr(RString::from(format!("invalid input: {e}"))),
    };

    let ctx = Context::__new(host, config, principal_id);
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
    pub use crate::Context;
    pub use apiplant_abi::{HttpMethod, LogLevel, Visibility};
    /// `#[derive(JsonSchema)]` for typed OpenAPI (with the `schema` feature).
    #[cfg(feature = "schema")]
    pub use schemars::JsonSchema;
}

/// Re-exports the generated code depends on. Not a stable public API.
#[doc(hidden)]
pub mod __rt {
    pub use crate::{input_schema_json, invoke_handler, output_schema_json, Context};
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
        query_result: Result<String, String>,
        requests: Mutex<Vec<String>>,
        logs: Mutex<Vec<(LogLevel, String)>>,
    }

    impl MockHost {
        fn success(config_json: &str, principal_id: &str, response: serde_json::Value) -> Self {
            Self {
                config_json: config_json.into(),
                principal_id: principal_id.into(),
                query_result: Ok(response.to_string()),
                requests: Mutex::new(Vec::new()),
                logs: Mutex::new(Vec::new()),
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

        fn config(&self) -> RString {
            self.config_json.clone().into()
        }

        fn principal_id(&self) -> RString {
            self.principal_id.clone().into()
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
        let ctx = Context::__new(&host, (), "user-123".into());

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
        let ctx = Context::__new(&host, (), "user-123".into());

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
