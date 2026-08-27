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
use std::sync::Arc;

use abi_stable::library::{lib_header_from_raw_library, RawLibrary};
use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{
    BoxedFunction, FunctionManifest, FunctionMod_Ref, HostApi, HostApi_TO, LogLevel,
};
use apiplant_ai::Ai;
use apiplant_cache::Cache;
use apiplant_core::RateLimitRule;
use apiplant_db::Db;
use apiplant_email::Mailer;
use apiplant_payments::Payments;
use apiplant_queue::Queue;

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

/// The extension of a TypeScript function's build output.
///
/// Named here rather than taken from `apiplant_js`, because a build without
/// TypeScript support still has to recognise the file in order to say what is
/// wrong with it — see [`load_javascript`].
const JS_EXTENSION: &str = "js";

/// Hand a `.js` to the V8 isolate that runs it.
#[cfg(feature = "typescript")]
fn load_javascript(path: &Path) -> Result<abi_stable::std_types::RVec<BoxedFunction>, String> {
    debug_assert_eq!(JS_EXTENSION, apiplant_js::EXTENSION);
    Ok(apiplant_js::load(path)?.into())
}

/// The slim build's answer: there is no isolate to run it in.
///
/// The file is still found and still reported, one error per function, because
/// the alternative is an app that starts cleanly and 404s on endpoints whose
/// sources its author can see sitting in `functions/`.
#[cfg(not(feature = "typescript"))]
fn load_javascript(path: &Path) -> Result<abi_stable::std_types::RVec<BoxedFunction>, String> {
    Err(format!(
        "{} is a TypeScript function, and this build of apiplant was made without \
         TypeScript support — use a full build, or drop the function",
        path.display()
    ))
}

/// One loaded function plus its resolved config.
pub struct LoadedFunction {
    pub manifest: FunctionManifest,
    /// Config JSON merged from `functions/<name>.toml` (or `{}` if absent).
    /// For a built-in, whatever the framework handed it at registration.
    pub config_json: String,
    /// How often one caller may invoke it, from the same file's `rate_limit`
    /// key. [`Inherit`](RateLimitRule::Inherit) leaves it to `main.toml`.
    pub rate_limit: RateLimitRule,
    body: Body,
}

/// Read a function's `rate_limit` out of its resolved config.
///
/// It lives in `functions/<name>.toml` rather than in the manifest because the
/// manifest crosses the ABI boundary compiled into the library: how often a
/// deployment lets people call a function is the deployment's decision, and it
/// should not take a recompile. A value that is not a rate limit is logged and
/// ignored — a function that refuses to load is a worse answer than one that
/// runs at the app-wide rate while somebody fixes a typo.
fn declared_rate_limit(name: &str, config_json: &str) -> RateLimitRule {
    let declared = serde_json::from_str::<serde_json::Value>(config_json)
        .ok()
        .and_then(|config| config.get("rate_limit")?.as_str().map(str::to_string));
    let Some(declared) = declared else {
        return RateLimitRule::Inherit;
    };
    RateLimitRule::parse(&declared).unwrap_or_else(|| {
        tracing::warn!(
            function = %name,
            value = %declared,
            "`rate_limit` is not a rate limit (`100/1m`, `off`, `inherit`); ignoring it"
        );
        RateLimitRule::Inherit
    })
}

impl LoadedFunction {
    /// Wrap an already-constructed function instance. Used by [`FunctionRegistry::load_dir`]
    /// and by hosts that link functions in statically instead of loading `.so`s.
    pub fn new(func: BoxedFunction, config_json: String) -> Self {
        let manifest = func.manifest();
        LoadedFunction {
            rate_limit: declared_rate_limit(manifest.name.as_str(), &config_json),
            manifest,
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
            rate_limit: declared_rate_limit(manifest.name.as_str(), &config_json),
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
                Some("so") | Some("dylib") | Some("dll") | Some(JS_EXTENSION)
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
        let exported = if path.extension().and_then(|e| e.to_str()) == Some(JS_EXTENSION) {
            load_javascript(path)?
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
                rate_limit: declared_rate_limit(&name, &config_json),
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
    /// The app's own email templates, from its `emails/` directory. Lets a
    /// function send one by name instead of composing a message itself, so the
    /// copy an app mails lives in one place whoever sends it.
    email_templates: Option<Arc<crate::email_templates::EmailTemplates>>,
    /// The app's configured cache, when it has one.
    cache: Option<Cache>,
    /// The app's configured payment provider, when it has one.
    payments: Option<Payments>,
    /// The app's configured AI assistant, when it has one.
    ai: Option<Ai>,
    /// The app's queue. Unlike the four above this is not optional: `publish`
    /// needs no configuration to work, because the table it writes to is a
    /// built-in. It is `Option` only for the handful of call sites that build a
    /// bridge without an app around it — a test, mainly — and those get the
    /// same "not configured" message the others give.
    queue: Option<Queue>,
    /// Where [`HostApi::emit`] sends what a function produces mid-invocation,
    /// when this call is being streamed to somebody. `None` for every other
    /// invocation, which is what makes `emit` a no-op there rather than an
    /// error a function has to guard against.
    chunks: Option<tokio::sync::mpsc::UnboundedSender<String>>,
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
            email_templates: None,
            cache: None,
            payments: None,
            ai: None,
            queue: None,
            chunks: None,
            config_json,
            principal_id,
            hook_json: String::new(),
        }
    }

    /// Lend the function the app's email templates, so `send_email` can name
    /// one instead of spelling out a subject and a body.
    pub fn with_email_templates(
        mut self,
        templates: Arc<crate::email_templates::EmailTemplates>,
    ) -> Self {
        self.email_templates = Some(templates);
        self
    }

    /// Lend the function the app's email provider, cache, payments and AI
    /// assistant.
    ///
    /// All four are optional and each stays `None` when the app configured
    /// none, which is what makes `send_email`, `cache`, `payments` and `ai`
    /// fail with "not configured" rather than silently doing nothing.
    pub fn with_services(
        mut self,
        mailer: Option<Mailer>,
        cache: Option<Cache>,
        payments: Option<Payments>,
        ai: Option<Ai>,
    ) -> Self {
        self.mailer = mailer;
        self.cache = cache;
        self.payments = payments;
        self.ai = ai;
        self
    }

    /// Lend the function the app's queue, so it can `publish`.
    pub fn with_queue(mut self, queue: Queue) -> Self {
        self.queue = Some(queue);
        self
    }

    /// Stream this invocation: everything the function `emit`s goes to
    /// `chunks` as it is produced, rather than nowhere.
    pub fn streaming(mut self, chunks: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        self.chunks = Some(chunks);
        self
    }

    /// Ask the assistant and pass every token to whoever is listening, on its
    /// way to assembling the complete answer.
    ///
    /// This is what makes a *function* able to stream a model's output rather
    /// than only relay it: the function still gets one return value, and its
    /// caller still gets the answer as it is written. Without it, a function
    /// wrapping the assistant — to check permissions, to look something up
    /// first, to log the exchange — would turn a streaming provider into a
    /// blocking endpoint, and nobody would wrap it.
    async fn relay(
        &self,
        ai: &apiplant_ai::Ai,
        request: &apiplant_ai::ChatRequest,
    ) -> Result<apiplant_ai::ChatReply, apiplant_ai::AiError> {
        use futures_util::StreamExt;

        let mut stream = Box::pin(ai.stream(request).await?);
        let mut text = String::new();
        let mut done = apiplant_ai::Done::default();
        while let Some(event) = stream.next().await {
            match event? {
                apiplant_ai::Event::Delta(delta) => {
                    text.push_str(&delta);
                    // A caller who has closed the connection stops the
                    // generation: there is nobody left to read it, and the
                    // provider is still being paid by the token.
                    if !self.emit(abi_stable::std_types::RStr::from_str(&delta)) {
                        break;
                    }
                }
                // The model's thinking is not the answer, so it is neither
                // returned to the function nor forwarded: a function that
                // relays a stream is relaying a reply.
                apiplant_ai::Event::Reasoning(_) => {}
                apiplant_ai::Event::Done(end) => {
                    done = end;
                    break;
                }
            }
        }
        Ok(apiplant_ai::ChatReply {
            text,
            reasoning: String::new(),
            provider: ai.provider().as_str().to_string(),
            model: request
                .model
                .clone()
                .unwrap_or_else(|| ai.model().to_string()),
            done,
            tool_calls: Vec::new(),
        })
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
        let mut message: apiplant_email::Message = match serde_json::from_str(request.as_str()) {
            Ok(m) => m,
            Err(e) => return RResult::RErr(format!("invalid email: {e}").into()),
        };

        // A request may name a template from the app's `emails/` directory
        // instead of carrying a body — the point of the directory being that
        // the copy lives in one place whoever sends it. A subject spelled out
        // beside the template still wins, so one template can serve several
        // messages that differ only in their subject line.
        if let Some(name) = template_name(request.as_str()) {
            let Some(templates) = &self.email_templates else {
                return RResult::RErr(
                    format!("no email template `{name}`: this app has no emails/ directory").into(),
                );
            };
            let vars = template_vars(request.as_str());
            match templates.render(&name, &vars, &message.subject) {
                Ok(rendered) => {
                    message.subject = rendered.subject;
                    message.text = rendered.text;
                    message.html = rendered.html;
                }
                Err(e) => return RResult::RErr(format!("{e:#}").into()),
            }
        }
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

    fn payments(&self, request: RStr<'_>) -> RResult<RString, RString> {
        let Some(payments) = &self.payments else {
            return RResult::RErr(
                "no payment provider configured — set [payments] provider in main.toml"
                    .to_string()
                    .into(),
            );
        };
        match self.handle.block_on(payments.execute(request.as_str())) {
            Ok(value) => RResult::ROk(value.to_string().into()),
            Err(e) => RResult::RErr(e.to_string().into()),
        }
    }

    fn ai(&self, request: RStr<'_>) -> RResult<RString, RString> {
        let Some(ai) = &self.ai else {
            return RResult::RErr(
                "no ai provider configured — set [ai] provider in main.toml"
                    .to_string()
                    .into(),
            );
        };
        let raw: serde_json::Value = match serde_json::from_str(request.as_str()) {
            Ok(r) => r,
            Err(e) => return RResult::RErr(format!("invalid chat request: {e}").into()),
        };
        // `stream` is an instruction to the host, not part of the conversation:
        // it says "forward the answer to my caller as it arrives", and only
        // means anything when somebody is listening.
        let forward = raw.get("stream").and_then(serde_json::Value::as_bool) == Some(true);
        let request: apiplant_ai::ChatRequest = match serde_json::from_value(raw) {
            Ok(r) => r,
            Err(e) => return RResult::RErr(format!("invalid chat request: {e}").into()),
        };

        let result = match (forward, &self.chunks) {
            (true, Some(_)) => self.handle.block_on(self.relay(ai, &request)),
            _ => self.handle.block_on(ai.chat(&request)),
        };
        match result {
            Ok(reply) => RResult::ROk(
                serde_json::to_string(&reply)
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
            ),
            Err(e) => RResult::RErr(e.to_string().into()),
        }
    }

    fn publish(&self, request: RStr<'_>) -> RResult<RString, RString> {
        let Some(queue) = &self.queue else {
            return RResult::RErr("this invocation has no queue attached".to_string().into());
        };
        // The publisher is recorded as the caller of *this* function, so a
        // message queued while handling somebody's request carries their id
        // through to the handler — which is what makes the eventual work
        // attributable to the person who caused it.
        match self
            .handle
            .block_on(queue.execute(request.as_str(), &self.principal_id))
        {
            Ok(value) => RResult::ROk(value.to_string().into()),
            Err(e) => RResult::RErr(e.to_string().into()),
        }
    }

    fn emit(&self, chunk: RStr<'_>) -> bool {
        match &self.chunks {
            // A closed channel is a caller who hung up: the one thing a
            // streaming function genuinely wants to hear about.
            Some(chunks) => chunks.send(chunk.as_str().to_string()).is_ok(),
            // No channel means nobody asked for this call to be streamed. The
            // chunk is dropped, and the answer is still `true` — because what
            // a function does with `false` is *stop working*, and on a plain
            // invocation the return value is exactly what the caller is
            // waiting for. "Nobody is streaming" and "everybody has left" look
            // the same to a handler otherwise, and only one of them is a
            // reason to give up.
            None => true,
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

/// The `template` a `send_email` request names, if it names one.
///
/// Read off the raw JSON rather than off [`apiplant_email::Message`], because a
/// template is a property of the *request* — how the body is to be produced —
/// and not of the message that comes out of it.
fn template_name(request: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(request)
        .ok()?
        .get("template")?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// The `vars` object a `send_email` request carries, as Liquid values.
///
/// Absent or malformed means no variables, not an error: a template that names
/// none is perfectly ordinary, and one that names a variable it was not given
/// renders it as empty, which is Liquid's own answer.
fn template_vars(request: &str) -> liquid::Object {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(request) else {
        return liquid::Object::new();
    };
    match value.get("vars") {
        Some(vars) => liquid::model::to_object(vars).unwrap_or_default(),
        None => liquid::Object::new(),
    }
}

#[cfg(test)]
mod slim_tests {
    use super::*;

    /// A `.js` is still recognised as something to load, in both builds.
    ///
    /// This is the half of the slim behaviour that matters at boot: the file
    /// has to be picked up in order to be complained about, and an extension
    /// list that quietly stopped matching it would produce an app missing an
    /// endpoint with nothing in the log.
    #[test]
    fn javascript_is_always_a_loadable_artifact() {
        assert_eq!(JS_EXTENSION, "js");
        #[cfg(feature = "typescript")]
        assert_eq!(JS_EXTENSION, apiplant_js::EXTENSION);
    }

    /// Without TypeScript support, loading one fails by name.
    #[cfg(not(feature = "typescript"))]
    #[test]
    fn a_slim_build_refuses_javascript_and_says_why() {
        // `unwrap_err` is out: the success type is a boxed FFI trait object
        // with no `Debug`, so the error has to be taken by hand.
        let error = match load_javascript(Path::new("/app/functions/greet.js")) {
            Err(error) => error,
            Ok(_) => panic!("a slim build has no isolate to run it in"),
        };
        assert!(error.contains("greet.js"), "{error}");
        assert!(error.contains("TypeScript support"), "{error}");
    }
}
