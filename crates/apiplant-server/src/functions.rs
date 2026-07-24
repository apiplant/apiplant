//! Loading and invoking dynamically-compiled function libraries.
//!
//! At boot the framework scans the app's `functions/` directory for shared
//! libraries, loads each through the [`apiplant_abi`] contract, and records its
//! manifest. At request time it constructs a [`HostBridge`] (database + config +
//! caller identity) and calls the function on a blocking worker so the function
//! author never has to deal with async.

use std::collections::BTreeMap;
use std::path::Path;

use abi_stable::library::RootModule;
use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{
    BoxedFunction, FunctionManifest, FunctionMod_Ref, HostApi, HostApi_TO, LogLevel,
};
use apiplant_db::Db;

/// One loaded function plus its resolved config.
pub struct LoadedFunction {
    pub manifest: FunctionManifest,
    /// Config JSON merged from `functions/<name>.toml` (or `{}` if absent).
    pub config_json: String,
    func: BoxedFunction,
}

impl LoadedFunction {
    /// Wrap an already-constructed function instance. Used by [`FunctionRegistry::load_dir`]
    /// and by hosts that link functions in statically instead of loading `.so`s.
    pub fn new(func: BoxedFunction, config_json: String) -> Self {
        LoadedFunction {
            manifest: func.manifest(),
            config_json,
            func,
        }
    }

    /// Invoke the function. Must be called from a blocking context (see
    /// [`FunctionRegistry`] docs) because the host bridge blocks on the DB.
    pub fn invoke(&self, bridge: HostBridge, input: &str) -> Result<String, String> {
        let host = HostApi_TO::from_value(bridge, TD_Opaque);
        match self.func.invoke(host, RStr::from_str(input)) {
            RResult::ROk(s) => Ok(s.into_string()),
            RResult::RErr(e) => Err(e.into_string()),
        }
    }
}

/// All loaded functions, keyed by manifest name.
#[derive(Default)]
pub struct FunctionRegistry {
    functions: BTreeMap<String, LoadedFunction>,
}

impl FunctionRegistry {
    /// Scan a directory for function libraries and load them all. Missing dir =
    /// empty registry. A single bad library is logged and skipped, never fatal.
    pub fn load_dir(dir: &Path) -> Self {
        let mut registry = FunctionRegistry::default();
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => {
                tracing::info!(dir = %dir.display(), "no functions/ directory");
                return registry;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_lib = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("so") | Some("dylib") | Some("dll")
            );
            if !is_lib {
                continue;
            }
            match Self::load_one(&path) {
                Ok(f) => {
                    tracing::info!(
                        function = %f.manifest.name,
                        version = %f.manifest.version,
                        "loaded function"
                    );
                    registry
                        .functions
                        .insert(f.manifest.name.to_string(), f);
                }
                Err(e) => tracing::error!(path = %path.display(), error = %e, "failed to load function"),
            }
        }
        registry
    }

    fn load_one(path: &Path) -> Result<LoadedFunction, String> {
        let module = FunctionMod_Ref::load_from_file(path).map_err(|e| e.to_string())?;
        let func = module.new()();
        let manifest = func.manifest();

        // Per-deployment config: functions/<name>.toml → JSON.
        let config_path = path
            .with_file_name(format!("{}.toml", manifest.name))
            .to_path_buf();
        let config_json = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
            .and_then(|v| serde_json::to_string(&v).ok())
            .unwrap_or_else(|| "{}".to_string());

        Ok(LoadedFunction {
            manifest,
            config_json,
            func,
        })
    }

    /// Add a function that wasn't loaded from disk, replacing any function of
    /// the same name. Lets a host embed functions directly.
    pub fn register(&mut self, func: BoxedFunction, config_json: String) {
        let loaded = LoadedFunction::new(func, config_json);
        self.functions.insert(loaded.manifest.name.to_string(), loaded);
    }

    pub fn get(&self, name: &str) -> Option<&LoadedFunction> {
        self.functions.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &LoadedFunction> {
        self.functions.values()
    }
}

/// Host-side services handed to a function for one invocation.
///
/// Constructed on the async side (capturing a runtime [`Handle`]) and moved into
/// the blocking worker; its [`HostApi::query`] blocks on the async database via
/// that handle, which is safe because functions run outside any async context.
pub struct HostBridge {
    db: Db,
    handle: tokio::runtime::Handle,
    config_json: String,
    principal_id: String,
    /// Lifecycle-hook context JSON, or empty for a plain HTTP invocation.
    hook_json: String,
}

impl HostBridge {
    pub fn new(
        db: Db,
        handle: tokio::runtime::Handle,
        config_json: String,
        principal_id: String,
    ) -> Self {
        HostBridge {
            db,
            handle,
            config_json,
            principal_id,
            hook_json: String::new(),
        }
    }

    /// Mark this invocation as a resource lifecycle hook and attach its context.
    pub fn with_hook(mut self, hook_json: String) -> Self {
        self.hook_json = hook_json;
        self
    }
}

impl HostApi for HostBridge {
    fn query(&self, request: RStr<'_>) -> RResult<RString, RString> {
        #[derive(serde::Deserialize)]
        struct Req {
            sql: String,
            #[serde(default)]
            params: Vec<serde_json::Value>,
        }
        let req: Req = match serde_json::from_str(request.as_str()) {
            Ok(r) => r,
            Err(e) => return RResult::RErr(format!("invalid query request: {e}").into()),
        };
        let result = self
            .handle
            .block_on(async { self.db.raw_json(&req.sql, &req.params).await });
        match result {
            Ok(v) => RResult::ROk(v.to_string().into()),
            Err(e) => RResult::RErr(e.to_string().into()),
        }
    }

    fn log(&self, level: LogLevel, message: RStr<'_>) {
        let msg = message.as_str();
        match level {
            LogLevel::Trace => tracing::trace!(target: "apiplant::function", "{msg}"),
            LogLevel::Debug => tracing::debug!(target: "apiplant::function", "{msg}"),
            LogLevel::Info => tracing::info!(target: "apiplant::function", "{msg}"),
            LogLevel::Warn => tracing::warn!(target: "apiplant::function", "{msg}"),
            LogLevel::Error => tracing::error!(target: "apiplant::function", "{msg}"),
        }
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
