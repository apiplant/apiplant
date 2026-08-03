//! TypeScript and JavaScript functions.
//!
//! A function written in Rust, C, Zig or Go arrives as a shared library. A
//! function written in TypeScript cannot: there is nothing to link. So this
//! crate provides the other half of what a `.so` gives the host — a manifest to
//! read at boot and something to call per request — backed by V8 isolates
//! instead of `dlopen`.
//!
//! ```text
//! functions/greet.ts   ← what you write
//! functions/greet.js   ← `apiplant build` strips the types (swc, at build time)
//!                        and the server loads *this*, like it loads libgreet.so
//! ```
//!
//! ## Two stages, on purpose
//!
//! Types are stripped **at build time**, so the server never parses TypeScript
//! and a syntax error is a build failure rather than a boot failure. What runs at
//! request time is plain JavaScript in a V8 isolate — the same split Deno and Bun
//! make internally, just with the first half hoisted into `apiplant build`.
//!
//! No type *checking* happens: swc strips annotations without consulting them,
//! exactly like `deno run --no-check` or `bun`. `apiplant build` writes an
//! `apiplant.d.ts` beside your sources so your editor (and `tsc --noEmit`, if you
//! want it in CI) does the checking with real types.
//!
//! ## What a module looks like
//!
//! ```ts
//! import { defineFunctions, db, s } from "apiplant";
//!
//! export default defineFunctions({
//!   greet: {
//!     permission: "public",
//!     input: s.object({ name: s.string() }),
//!     handler(input) {
//!       const notes = db.value("SELECT count(*)::int AS n FROM apiplant_note");
//!       return { message: `Hello, ${input.name}!`, notes };
//!     },
//!   },
//! });
//! ```
//!
//! One module may declare any number of functions, like one `.so` may export
//! any number. `apiplant` is the only module a function can import; it is
//! compiled into this crate from `typescript/` at the repository root and served
//! to the isolate by [`module`], so nothing is installed and nothing can be out
//! of step with the host. A module that would rather import nothing declares
//! `export const manifest = [...]` and one export per entry instead; both forms
//! arrive here the same way.
//!
//! ## Concurrency
//!
//! An isolate is single-threaded, so a module is loaded into a small pool of them
//! ([`workers`], `APIPLANT_JS_WORKERS`) that share one job queue. Requests run
//! concurrently up to the pool size and queue beyond it. Isolates share nothing:
//! module-level state in a function is per-worker and must not be treated as
//! shared state — use the database or the cache for that.
//!
//! An invocation that runs longer than `APIPLANT_JS_TIMEOUT_MS` (30s by default)
//! has its isolate terminated and fails that one request; the worker recovers.

mod module;
mod worker;

#[cfg(feature = "transpile")]
pub mod transpile;

use std::path::Path;
use std::sync::Arc;

use abi_stable::sabi_trait::TD_Opaque;
use abi_stable::std_types::{RResult, RStr, RString};
use apiplant_abi::{BoxedFunction, Function, FunctionManifest, HostApi_TO, LogLevel};
use crossbeam_channel::{bounded, Sender};
use serde_json::Value;

use worker::{Job, Message};

/// The extension a JavaScript function library has on disk.
pub const EXTENSION: &str = "js";

/// How many isolates a module is loaded into.
///
/// Each one is a full V8 heap, so this is deliberately small: it trades memory
/// for concurrency, and most functions spend their time waiting on the host
/// (which happens on the *caller's* thread, not the isolate's) rather than
/// running JavaScript.
fn workers() -> usize {
    std::env::var("APIPLANT_JS_WORKERS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2)
}

/// A pool of isolates, all holding the same module, behind one job queue.
///
/// Whichever isolate is free takes the next job, so a slow function does not
/// block a fast one behind it while another worker sits idle.
struct Pool {
    jobs: Sender<Job>,
    /// Only for log messages: which library these isolates hold.
    label: String,
}

impl Pool {
    /// Load `code` into [`workers()`] isolates, returning the pool and the
    /// manifest they all declare.
    fn load(label: &str, code: &str) -> Result<(Pool, String), String> {
        // Unbounded would let a queue of doomed requests grow without limit; the
        // bound makes back-pressure visible as a rejected request instead.
        let (jobs, incoming) = bounded::<Job>(1024);

        let mut manifest = None;
        for worker in 0..workers() {
            let declared = worker::spawn(
                format!("{label}#{worker}"),
                code.to_string(),
                incoming.clone(),
            )?;
            // Every isolate runs the same code, so the first answer is the
            // manifest; the rest are only checked for having started.
            manifest = manifest.or(declared);
        }

        let manifest = manifest.ok_or_else(|| {
            "the module exports no `manifest`; add \
             `export const manifest = [{ name: \"…\", permission: \"…\" }]`"
                .to_string()
        })?;

        Ok((
            Pool {
                jobs,
                label: label.to_string(),
            },
            manifest,
        ))
    }

    /// Run one function and serve the host calls it makes along the way.
    ///
    /// Must be called from a thread that may block on the async runtime — the
    /// same requirement every other function body has, for the same reason: the
    /// host API is synchronous and the database is not.
    fn invoke(
        &self,
        name: &str,
        input: &str,
        host: &HostApi_TO<'_, abi_stable::std_types::RBox<()>>,
    ) -> Result<String, String> {
        let (replies, incoming) = bounded::<Message>(1);
        let job = Job {
            name: name.to_string(),
            input: input.to_string(),
            replies,
        };
        if self.jobs.try_send(job).is_err() {
            return Err(format!(
                "{}javascript function `{name}` is overloaded; try again",
                apiplant_abi::INTERNAL_ERROR_PREFIX
            ));
        }

        // Everything the function asks for comes back here, on this thread,
        // until the isolate says it is done. See `worker`'s module docs.
        loop {
            match incoming.recv() {
                Ok(Message::Host {
                    kind,
                    payload,
                    answer,
                }) => {
                    let _ = answer.send(serve(host, &kind, &payload));
                }
                Ok(Message::Done(result)) => return result,
                Err(_) => {
                    tracing::error!(library = %self.label, function = %name, "javascript worker died");
                    return Err(format!(
                        "{}the javascript worker died",
                        apiplant_abi::INTERNAL_ERROR_PREFIX
                    ));
                }
            }
        }
    }
}

/// Answer one host request from an isolate.
///
/// Failures are reported **in band**, as `{"error": …}`, which is the same
/// convention the C ABI uses — see `apiplant_abi::c::Host::query`. The bootstrap
/// turns that back into a thrown `Error`, so a function author sees an ordinary
/// exception and never a magic value.
fn serve(
    host: &HostApi_TO<'_, abi_stable::std_types::RBox<()>>,
    kind: &str,
    payload: &str,
) -> String {
    let in_band = |result: RResult<RString, RString>| match result {
        RResult::ROk(reply) => reply.into_string(),
        RResult::RErr(e) => serde_json::json!({ "error": e.as_str() }).to_string(),
    };

    match kind {
        "query" => in_band(host.query(RStr::from_str(payload))),
        "send_email" => in_band(host.send_email(RStr::from_str(payload))),
        "cache" => in_band(host.cache(RStr::from_str(payload))),
        "payments" => in_band(host.payments(RStr::from_str(payload))),
        "ai" => in_band(host.ai(RStr::from_str(payload))),
        // The one host call whose payload is text rather than an object, and
        // whose answer is a fact rather than a document: was anybody there to
        // receive it. The bootstrap sends every payload as JSON, so the chunk
        // arrives quoted and has to be read back out.
        "emit" => {
            let chunk: String = serde_json::from_str(payload).unwrap_or_default();
            serde_json::json!({ "delivered": host.emit(RStr::from_str(&chunk)) }).to_string()
        }
        "config" => host.config().into_string(),
        "principal_id" => host.principal_id().into_string(),
        "hook" => host.hook().into_string(),
        "log" => {
            let entry: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
            let message = entry.get("message").and_then(Value::as_str).unwrap_or("");
            let level = match entry.get("level").and_then(Value::as_str) {
                Some("trace") => LogLevel::Trace,
                Some("debug") => LogLevel::Debug,
                Some("warn") => LogLevel::Warn,
                Some("error") => LogLevel::Error,
                _ => LogLevel::Info,
            };
            host.log(level, RStr::from_str(message));
            String::new()
        }
        other => serde_json::json!({ "error": format!("unknown host call `{other}`") }).to_string(),
    }
}

/// One exported function, presented to the host as an ABI function object.
///
/// Every function from a module shares the module's [`Pool`] — they are exports
/// of the same code, so they are the same isolates.
struct JsFunction {
    manifest: FunctionManifest,
    pool: Arc<Pool>,
}

impl Function for JsFunction {
    fn manifest(&self) -> FunctionManifest {
        self.manifest.clone()
    }

    fn invoke(
        &self,
        host: HostApi_TO<'_, abi_stable::std_types::RBox<()>>,
        input: RStr<'_>,
    ) -> RResult<RString, RString> {
        // This is called across an `extern "C"` boundary: a panic escaping it
        // would abort the process instead of failing one request.
        let called = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.pool
                .invoke(self.manifest.name.as_str(), input.as_str(), &host)
        }));
        match called {
            Ok(Ok(output)) => RResult::ROk(output.into()),
            Ok(Err(e)) => RResult::RErr(e.into()),
            Err(_) => RResult::RErr(
                format!(
                    "{}panic while invoking a javascript function",
                    apiplant_abi::INTERNAL_ERROR_PREFIX
                )
                .into(),
            ),
        }
    }
}

/// Load a `.js` function library: every function its `manifest` declares.
///
/// The counterpart of the C-ABI loader, and it fails the same way — a module
/// that cannot be compiled, that exports no manifest, or whose manifest names a
/// function it does not export is an error here rather than a surprise at the
/// first request.
pub fn load(path: &Path) -> Result<Vec<BoxedFunction>, String> {
    let code = std::fs::read_to_string(path).map_err(|e| format!("cannot read: {e}"))?;
    let label = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "function".to_string());

    let (pool, manifest) = Pool::load(&label, &code)?;
    let pool = Arc::new(pool);

    let entries: Vec<Value> = serde_json::from_str(&manifest)
        .map_err(|e| format!("`manifest` is not valid JSON: {e}"))?;
    if entries.is_empty() {
        return Err("`manifest` is empty; it must describe at least one function".to_string());
    }

    let mut functions = Vec::with_capacity(entries.len());
    for entry in &entries {
        // The same reader the C loader uses: a manifest is a manifest, and an
        // app porting a function from C to TypeScript should not have to rewrite
        // the part that has nothing to do with either language.
        let manifest = apiplant_abi::manifest_from_json(entry)?;
        functions.push(BoxedFunction::from_value(
            JsFunction {
                manifest,
                pool: pool.clone(),
            },
            TD_Opaque,
        ));
    }
    Ok(functions)
}
