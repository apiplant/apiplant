//! One V8 isolate on one thread, and the channels that let the rest of the
//! process talk to it.
//!
//! A V8 isolate belongs to the thread that created it, so a JavaScript function
//! cannot simply be called on whatever worker the HTTP server happens to be
//! using. Instead each isolate gets a thread of its own and receives [`Job`]s;
//! the caller blocks until the answer comes back, which makes an invocation look
//! synchronous from the outside — exactly like the `.so` path next to it.
//!
//! ## Who runs the host calls
//!
//! When the function asks the host for something — a query, its config — the op
//! does **not** run it on the isolate's thread. It sends a [`Message::Host`] back
//! to the caller and blocks. The caller is a `spawn_blocking` worker that already
//! holds the [`HostApi`](apiplant_abi::HostApi) and is allowed to block on the
//! async runtime; the isolate's thread is neither. So the request travels back to
//! the one thread that can serve it, and the reply travels forward.
//!
//! That inversion is the whole design: the isolate thread only ever runs
//! JavaScript, and the ops are a mailbox.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};
use deno_core::{extension, op2, JsRuntime, OpState, PollEventLoopOptions, RuntimeOptions};

/// How long one invocation may run before the isolate is terminated.
///
/// A JavaScript function is not preemptible: `while (true) {}` would hold its
/// thread until the process ended, and a pool of them can be exhausted by one
/// bad deployment. V8 can interrupt a running isolate from another thread, which
/// is what the watchdog below does.
fn timeout() -> Duration {
    let ms = std::env::var("APIPLANT_JS_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30_000);
    Duration::from_millis(ms)
}

/// One call into a JavaScript function.
pub(crate) struct Job {
    /// The manifest name, which is also the export to call.
    pub name: String,
    /// The request body as JSON.
    pub input: String,
    /// Where the isolate sends host requests and, finally, the result.
    pub replies: Sender<Message>,
}

/// What the isolate's thread sends back over a job's channel.
pub(crate) enum Message {
    /// The function asked the host for something and is blocked until answered.
    Host {
        kind: String,
        payload: String,
        answer: Sender<String>,
    },
    /// The call finished. Always the last message of a job.
    Done(Result<String, String>),
}

/// The isolate-side end of the current job, read by the op.
type Current = Rc<RefCell<Option<Sender<Message>>>>;

#[op2]
#[string]
fn op_apiplant_host(state: &mut OpState, #[string] kind: &str, #[string] payload: &str) -> String {
    let current = state.borrow::<Current>().clone();
    let replies = current.borrow().clone();
    let Some(replies) = replies else {
        // Only reachable if user code stashed `ctx` and called it after the
        // invocation returned — from a timer, say. There is no host to ask.
        return r#"{"error":"no invocation in progress"}"#.to_string();
    };

    let (answer, wait) = bounded(1);
    let sent = replies.send(Message::Host {
        kind: kind.to_string(),
        payload: payload.to_string(),
        answer,
    });
    if sent.is_err() {
        return r#"{"error":"the host stopped listening"}"#.to_string();
    }
    wait.recv()
        .unwrap_or_else(|_| r#"{"error":"the host stopped listening"}"#.to_string())
}

extension!(
    apiplant_js,
    ops = [op_apiplant_host],
    esm_entry_point = "ext:apiplant_js/bootstrap.js",
    esm = [dir "src", "bootstrap.js"],
    options = { current: Current },
    state = |state, options| state.put::<Current>(options.current),
);

/// Start an isolate on its own thread with `code` already evaluated.
///
/// Returns the manifest the module declared, plus the channel jobs go down.
/// Failing to compile or evaluate the module fails here rather than at the first
/// request, so a broken function library is reported at boot like a broken `.so`.
pub(crate) fn spawn(
    label: String,
    code: String,
    jobs: Receiver<Job>,
) -> Result<Option<String>, String> {
    let (ready, wait) = bounded::<Result<Option<String>, String>>(1);

    std::thread::Builder::new()
        .name(format!("apiplant-js:{label}"))
        .spawn(move || run(label, code, jobs, ready))
        .map_err(|e| format!("cannot start a JavaScript worker thread: {e}"))?;

    wait.recv()
        .map_err(|_| "the JavaScript worker died during startup".to_string())?
}

/// The isolate thread: build the runtime, evaluate the module, serve jobs.
fn run(
    label: String,
    code: String,
    jobs: Receiver<Job>,
    ready: Sender<Result<Option<String>, String>>,
) {
    let current: Current = Rc::new(RefCell::new(None));
    let mut runtime = JsRuntime::new(RuntimeOptions {
        extensions: vec![apiplant_js::init(current.clone())],
        // Serves `import … from "apiplant"` and refuses everything else.
        module_loader: Some(crate::module::Loader::shared()),
        ..Default::default()
    });

    // The isolate's own event loop needs an async context to be driven in; it is
    // current-thread and single-purpose, and never blocks on anything but V8,
    // because host work happens on the *caller's* thread (see the module docs).
    let Ok(local) = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    else {
        let _ = ready.send(Err("cannot start the JavaScript event loop".into()));
        return;
    };

    let watchdog = Watchdog::spawn(runtime.v8_isolate().thread_safe_handle());

    let manifest = local.block_on(evaluate(&mut runtime, &label, code));
    let failed = manifest.is_err();
    let _ = ready.send(manifest);
    if failed {
        return;
    }

    // `__apiplantInvoke` is fetched once: it is the only entry point, and looking
    // it up per call would mean a handle scope per call for nothing.
    let entry = match global_function(&mut runtime, "__apiplantInvoke") {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(library = %label, error = %e, "javascript bootstrap is broken");
            return;
        }
    };

    // A closed job channel means the registry is gone: the process is shutting
    // down, so the isolate goes with it.
    while let Ok(job) = jobs.recv() {
        *current.borrow_mut() = Some(job.replies.clone());
        let guard = watchdog.watching();
        let result = local.block_on(invoke(&mut runtime, &entry, &job.name, &job.input));
        drop(guard);
        *current.borrow_mut() = None;

        // A terminated isolate stays terminated until told otherwise; without
        // this the worker would reject every later request too.
        runtime.v8_isolate().cancel_terminate_execution();

        let _ = job.replies.send(Message::Done(result));
    }
}

/// Load and evaluate the module, then read its manifest.
async fn evaluate(
    runtime: &mut JsRuntime,
    label: &str,
    code: String,
) -> Result<Option<String>, String> {
    // The specifier is only ever seen in stack traces, so it names the library.
    let url = deno_core::resolve_url(&format!("file:///{label}.js"))
        .map_err(|e| format!("cannot name the module: {e}"))?;

    let id = runtime
        .load_main_es_module_from_code(&url, code)
        .await
        .map_err(|e| format!("cannot compile the module: {e}"))?;
    let evaluated = runtime.mod_evaluate(id);
    runtime
        .run_event_loop(PollEventLoopOptions::default())
        .await
        .map_err(|e| format!("module failed while evaluating: {e}"))?;
    evaluated
        .await
        .map_err(|e| format!("module failed while evaluating: {e}"))?;

    // Hand the namespace to the bootstrap, which is what dispatches into it.
    let namespace = runtime
        .get_module_namespace(id)
        .map_err(|e| format!("cannot read the module's exports: {e}"))?;
    {
        deno_core::scope!(scope, runtime);
        let namespace = deno_core::v8::Local::new(scope, namespace);
        let global = scope.get_current_context().global(scope);
        let key = deno_core::v8::String::new(scope, "__apiplantModule")
            .ok_or("out of memory naming the module")?;
        global.set(scope, key.into(), namespace.into());
    }

    let manifest = global_function(runtime, "__apiplantManifest")?;
    let manifest = invoke_json(runtime, &manifest, &[]).await?;
    Ok(match manifest.as_str() {
        "" | "null" => None,
        json => Some(json.to_string()),
    })
}

/// Call `__apiplantInvoke(name, input)` and unwrap what it resolves to.
async fn invoke(
    runtime: &mut JsRuntime,
    entry: &deno_core::v8::Global<deno_core::v8::Function>,
    name: &str,
    input: &str,
) -> Result<String, String> {
    let args = {
        deno_core::scope!(scope, runtime);
        let name: deno_core::v8::Local<deno_core::v8::Value> =
            deno_core::v8::String::new(scope, name)
                .ok_or("out of memory")?
                .into();
        let input: deno_core::v8::Local<deno_core::v8::Value> =
            deno_core::v8::String::new(scope, input)
                .ok_or("out of memory")?
                .into();
        [
            deno_core::v8::Global::new(scope, name),
            deno_core::v8::Global::new(scope, input),
        ]
    };

    // The bootstrap resolves rather than rejects, so a failure here is the
    // isolate itself failing: a timeout, an out-of-memory, a top-level throw
    // from a timer. All of those are the function's fault, never the caller's.
    let reply = invoke_json(runtime, entry, &args).await.map_err(|e| {
        format!(
            "{}javascript function `{name}` failed: {e}",
            apiplant_abi::INTERNAL_ERROR_PREFIX
        )
    })?;

    let reply: serde_json::Value = serde_json::from_str(&reply).map_err(|e| {
        format!(
            "{}invoke returned invalid JSON: {e}",
            apiplant_abi::INTERNAL_ERROR_PREFIX
        )
    })?;

    if let Some(error) = reply.get("error").and_then(|e| e.as_str()) {
        // `request: true` is a 400 and goes back bare; anything else is a 500,
        // which the host recognises by the prefix.
        let caller_fault = reply.get("request").and_then(|r| r.as_bool()) == Some(true);
        return Err(if caller_fault {
            error.to_string()
        } else {
            format!("{}{error}", apiplant_abi::INTERNAL_ERROR_PREFIX)
        });
    }
    Ok(match reply.get("ok") {
        Some(value) => value.to_string(),
        None => "null".to_string(),
    })
}

/// Call a JavaScript function that returns a string (or a promise of one),
/// driving the event loop until it settles.
async fn invoke_json(
    runtime: &mut JsRuntime,
    function: &deno_core::v8::Global<deno_core::v8::Function>,
    args: &[deno_core::v8::Global<deno_core::v8::Value>],
) -> Result<String, String> {
    let call = runtime.call_with_args(function, args);
    let value = runtime
        .with_event_loop_promise(call, PollEventLoopOptions::default())
        .await
        .map_err(|e| e.to_string())?;

    deno_core::scope!(scope, runtime);
    let value = deno_core::v8::Local::new(scope, value);
    if value.is_null_or_undefined() {
        return Ok(String::new());
    }
    Ok(value.to_rust_string_lossy(scope))
}

/// Fetch a function off the global object, by name.
fn global_function(
    runtime: &mut JsRuntime,
    name: &str,
) -> Result<deno_core::v8::Global<deno_core::v8::Function>, String> {
    deno_core::scope!(scope, runtime);
    let global = scope.get_current_context().global(scope);
    let key = deno_core::v8::String::new(scope, name).ok_or("out of memory")?;
    let value = global
        .get(scope, key.into())
        .ok_or_else(|| format!("`{name}` is missing from the isolate"))?;
    let function: deno_core::v8::Local<deno_core::v8::Function> = value
        .try_into()
        .map_err(|_| format!("`{name}` is not a function"))?;
    Ok(deno_core::v8::Global::new(scope, function))
}

/// Terminates an isolate that overstays its [`timeout`].
///
/// Lives on its own thread because the isolate's thread is, by definition, busy
/// running the code that needs interrupting.
struct Watchdog {
    signals: Sender<Signal>,
    timeout: Duration,
}

enum Signal {
    Begin(Duration),
    End,
}

impl Watchdog {
    fn spawn(handle: deno_core::v8::IsolateHandle) -> Watchdog {
        let (signals, incoming) = bounded::<Signal>(1);
        std::thread::Builder::new()
            .name("apiplant-js:watchdog".into())
            .spawn(move || {
                while let Ok(Signal::Begin(limit)) = incoming.recv() {
                    // Either the call ends in time, or V8 is interrupted and the
                    // `End` that follows the failure is absorbed here.
                    if incoming.recv_timeout(limit).is_err() {
                        handle.terminate_execution();
                        if incoming.recv().is_err() {
                            return;
                        }
                    }
                }
            })
            .ok();
        Watchdog {
            signals,
            timeout: timeout(),
        }
    }

    /// Arm the watchdog for one call; disarmed when the guard drops.
    fn watching(&self) -> WatchGuard<'_> {
        let _ = self.signals.send(Signal::Begin(self.timeout));
        WatchGuard { watchdog: self }
    }
}

struct WatchGuard<'a> {
    watchdog: &'a Watchdog,
}

impl Drop for WatchGuard<'_> {
    fn drop(&mut self) {
        let _ = self.watchdog.signals.send(Signal::End);
    }
}
