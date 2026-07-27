//! Loading and invoking dynamically-compiled function libraries.
//!
//! At boot the framework scans the app's `functions/` directory for artifacts
//! `apiplant build` produced: shared libraries, loaded through the
//! [`apiplant_abi`] contract, and `.js` modules, loaded into V8 isolates by
//! [`apiplant_js`]. Both arrive as the same [`BoxedFunction`], so the difference
//! stops at the loader.
//!
//! At request time the framework constructs a [`HostBridge`] (database + config +
//! caller identity) and calls the function on a blocking worker so the function
//! author never has to deal with async.

use std::collections::BTreeMap;
use std::path::Path;

use abi_stable::library::{lib_header_from_raw_library, RawLibrary};
use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{
    BoxedFunction, FunctionManifest, FunctionMod_Ref, HostApi, HostApi_TO, LogLevel,
};
use apiplant_cache::Cache;
use apiplant_db::Db;
use apiplant_email::Mailer;

/// A function implemented in the framework itself rather than loaded from a
/// library: an ordinary Rust `fn` over the same [`HostBridge`] a dynamic
/// function sees. See [`crate::builtins`].
pub type BuiltinHandler = fn(&HostBridge, &str) -> Result<String, String>;

/// Where a registered function's code lives. Nothing outside this module cares
/// which it is: both arrive through [`LoadedFunction::invoke`].
enum Body {
    /// Loaded from a shared library in `functions/`.
    Dynamic(BoxedFunction),
    /// Compiled into the server (see [`crate::builtins`]).
    Builtin(BuiltinHandler),
}

/// One loaded function plus its resolved config.
pub struct LoadedFunction {
    pub manifest: FunctionManifest,
    /// Config JSON merged from `functions/<name>.toml` (or `{}` if absent).
    /// For a built-in, whatever the framework handed it at registration.
    pub config_json: String,
    body: Body,
}

impl LoadedFunction {
    /// Wrap an already-constructed function instance. Used by [`FunctionRegistry::load_dir`]
    /// and by hosts that link functions in statically instead of loading `.so`s.
    pub fn new(func: BoxedFunction, config_json: String) -> Self {
        LoadedFunction {
            manifest: func.manifest(),
            config_json,
            body: Body::Dynamic(func),
        }
    }

    /// Wrap a built-in: a handler the framework provides, with a manifest it
    /// declares rather than one read across the ABI.
    pub fn builtin(
        manifest: FunctionManifest,
        handler: BuiltinHandler,
        config_json: String,
    ) -> Self {
        LoadedFunction {
            manifest,
            config_json,
            body: Body::Builtin(handler),
        }
    }

    /// Invoke the function. Must be called from a blocking context (see
    /// [`FunctionRegistry`] docs) because the host bridge blocks on the DB.
    pub fn invoke(&self, bridge: HostBridge, input: &str) -> Result<String, String> {
        match &self.body {
            Body::Builtin(handler) => handler(&bridge, input),
            Body::Dynamic(func) => {
                let host = HostApi_TO::from_value(bridge, TD_Opaque);
                match func.invoke(host, RStr::from_str(input)) {
                    RResult::ROk(s) => Ok(s.into_string()),
                    RResult::RErr(e) => Err(e.into_string()),
                }
            }
        }
    }
}

/// All loaded functions, keyed by manifest name.
#[derive(Default)]
pub struct FunctionRegistry {
    functions: BTreeMap<String, LoadedFunction>,
}

impl FunctionRegistry {
    /// The registry an app runs with: the framework's [built-ins](crate::builtins)
    /// first, then everything in the app's `functions/` directory.
    ///
    /// Built-ins live in the reserved [`apiplant_`](crate::builtins::PREFIX)
    /// namespace, so an app function can't shadow one by accident. Naming one
    /// into that namespace on purpose still replaces the built-in — the escape
    /// hatch for an app that wants the hook but not our version of it — and says
    /// so in the log.
    pub fn load(app: &apiplant_core::App) -> Self {
        let mut registry = FunctionRegistry::default();
        crate::builtins::register_all(&mut registry, app);
        for (name, f) in Self::load_dir(&app.functions_dir).functions {
            if registry.functions.contains_key(&name) {
                tracing::warn!(function = %name, "app function replaces the built-in of the same name");
            }
            registry.functions.insert(name, f);
        }
        registry
    }

    /// Add a built-in under its manifest name. See [`crate::builtins`].
    pub fn register_builtin(
        &mut self,
        manifest: FunctionManifest,
        handler: BuiltinHandler,
        config_json: String,
    ) {
        let loaded = LoadedFunction::builtin(manifest, handler, config_json);
        self.functions
            .insert(loaded.manifest.name.to_string(), loaded);
    }

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
            // Two kinds of function artifact live here: a shared library, and
            // the JavaScript `apiplant build` produced from a `.ts`. Loading is
            // the only place the difference shows.
            let loadable = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("so") | Some("dylib") | Some("dll") | Some(apiplant_js::EXTENSION)
            );
            if !loadable {
                continue;
            }
            match Self::load_library(&path) {
                Ok(loaded) => {
                    for f in loaded {
                        tracing::info!(
                            function = %f.manifest.name,
                            version = %f.manifest.version,
                            library = %path.display(),
                            "loaded function"
                        );
                        registry.functions.insert(f.manifest.name.to_string(), f);
                    }
                }
                Err(e) => {
                    tracing::error!(path = %path.display(), error = %e, "failed to load function")
                }
            }
        }
        registry
    }

    /// Load every function a library exports. One library commonly provides a
    /// set of related functions — a resource's lifecycle hooks, say — each with
    /// its own name and manifest.
    ///
    /// Two ABIs are accepted. A library built with `apiplant-function` exports an
    /// [`abi_stable`] root module and is tried first; one written in C, Zig or Go
    /// exports the [plain C symbols](apiplant_abi::c) instead. Both arrive here as
    /// [`BoxedFunction`]s, so nothing downstream knows the difference.
    fn load_library(path: &Path) -> Result<Vec<LoadedFunction>, String> {
        // A `.js` never speaks either native ABI: it is a module for a V8
        // isolate, and `apiplant_js` gives back the same `BoxedFunction`s, so
        // everything below this line is shared with the compiled languages.
        let exported = if path.extension().and_then(|e| e.to_str()) == Some(apiplant_js::EXTENSION)
        {
            apiplant_js::load(path)?.into()
        } else {
            Self::load_native(path)?
        };
        Self::wrap(path, exported)
    }

    /// Load a shared library through whichever of the two native ABIs it speaks.
    fn load_native(path: &Path) -> Result<abi_stable::std_types::RVec<BoxedFunction>, String> {
        let exported = match Self::open(path) {
            Ok(module) => module.new_functions()(),
            Err(rust_abi_error) => match crate::cabi::load(path)? {
                Some(functions) => functions.into(),
                // Not a C-ABI library either, so the original failure is the
                // one worth reporting.
                None => return Err(rust_abi_error),
            },
        };
        Ok(exported)
    }

    /// Turn the functions a library exported into registry entries: check the
    /// names, then resolve each one's config file.
    fn wrap(
        path: &Path,
        exported: abi_stable::std_types::RVec<BoxedFunction>,
    ) -> Result<Vec<LoadedFunction>, String> {
        if exported.is_empty() {
            return Err("library exports no functions".to_string());
        }

        let mut loaded: Vec<LoadedFunction> = Vec::with_capacity(exported.len());
        for func in exported {
            let manifest = func.manifest();
            let name = manifest.name.to_string();
            if loaded.iter().any(|f| f.manifest.name == manifest.name) {
                return Err(format!("library exports two functions named `{name}`"));
            }

            // Per-deployment config: functions/<name>.toml → JSON. Each function
            // in a library reads its own file.
            let config_path = path.with_file_name(format!("{name}.toml"));
            // Expanded like every other app-directory TOML, so a function's
            // config can hold `api_key = "$STRIPE_KEY"` rather than the key.
            let config_json = std::fs::read_to_string(&config_path)
                .ok()
                .and_then(|t| toml::from_str::<toml::Value>(&t).ok())
                .map(|mut v| {
                    apiplant_core::expand_document(&mut v, &format!("{name}.toml"));
                    v
                })
                .and_then(|v| serde_json::to_string(&v).ok())
                .unwrap_or_else(|| "{}".to_string());

            loaded.push(LoadedFunction {
                manifest,
                config_json,
                body: Body::Dynamic(func),
            });
        }
        Ok(loaded)
    }

    /// Open one library and return its root module, with the ABI version and
    /// layout checked.
    ///
    /// Deliberately *not* [`RootModule::load_from_file`]: that caches the first
    /// library it ever loads in a process-wide static and hands the same root
    /// module back for every later path, so an app with more than one library in
    /// `functions/` would silently get the first one's functions repeatedly.
    /// Going through the header directly keeps each library separate.
    fn open(path: &Path) -> Result<FunctionMod_Ref, String> {
        let library = RawLibrary::load_at(path).map_err(|e| e.to_string())?;

        // The library must outlive every function it exports; abi_stable never
        // unloads, so leaking it is the supported way to keep its code mapped.
        let library: &'static RawLibrary = Box::leak(Box::new(library));

        // SAFETY: `library` is leaked above, so the `&'static LibHeader` this
        // returns stays valid for the rest of the process.
        let header = unsafe { lib_header_from_raw_library(library).map_err(|e| e.to_string())? };
        header
            .init_root_module::<FunctionMod_Ref>()
            .map_err(|e| e.to_string())
    }

    /// Add a function that wasn't loaded from disk, replacing any function of
    /// the same name. Lets a host embed functions directly.
    pub fn register(&mut self, func: BoxedFunction, config_json: String) {
        let loaded = LoadedFunction::new(func, config_json);
        self.functions
            .insert(loaded.manifest.name.to_string(), loaded);
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
/// [`HostApi::send_email`] and [`HostApi::cache`] work the same way.
pub struct HostBridge {
    db: Db,
    handle: tokio::runtime::Handle,
    /// The app's configured mailer, when it has one. Built once at boot and
    /// shared, so a function sending mail reuses pooled connections.
    mailer: Option<Mailer>,
    /// The app's configured cache, when it has one.
    cache: Option<Cache>,
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
            mailer: None,
            cache: None,
            config_json,
            principal_id,
            hook_json: String::new(),
        }
    }

    /// Lend the function the app's email provider and cache.
    ///
    /// Both are optional and both stay `None` when the app configured neither,
    /// which is what makes `send_email` and `cache` fail with "not configured"
    /// rather than silently doing nothing.
    pub fn with_services(mut self, mailer: Option<Mailer>, cache: Option<Cache>) -> Self {
        self.mailer = mailer;
        self.cache = cache;
        self
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

    fn send_email(&self, request: RStr<'_>) -> RResult<RString, RString> {
        let Some(mailer) = &self.mailer else {
            return RResult::RErr(
                "no email provider configured — set [email] provider in main.toml"
                    .to_string()
                    .into(),
            );
        };
        let message: apiplant_email::Message = match serde_json::from_str(request.as_str()) {
            Ok(m) => m,
            Err(e) => return RResult::RErr(format!("invalid email: {e}").into()),
        };
        match self.handle.block_on(mailer.send(&message)) {
            Ok(sent) => RResult::ROk(
                serde_json::to_string(&sent)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ),
            Err(e) => RResult::RErr(e.to_string().into()),
        }
    }

    fn cache(&self, request: RStr<'_>) -> RResult<RString, RString> {
        let Some(cache) = &self.cache else {
            return RResult::RErr(
                "no cache configured — set [cache] url in main.toml"
                    .to_string()
                    .into(),
            );
        };
        match self.handle.block_on(cache.execute(request.as_str())) {
            Ok(value) => RResult::ROk(value.to_string().into()),
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
