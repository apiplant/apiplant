//! Invoking one function without serving anything — the `apiplant call`
//! command, and what a Kubernetes CronJob runs.
//!
//! A scheduled job wants exactly what an HTTP call to `<base>/functions/{name}`
//! does, minus the server: the same function, the same database, the same
//! email/cache/payments/AI services built from the same `main.toml`. So this
//! builds those services the way [`crate::run_with`] does, hands the function a
//! [`HostBridge`] over them, and returns what it returned.
//!
//! Two deliberate differences from the HTTP path:
//!
//! * **No access check.** There is no request to authenticate and no session to
//!   read; whoever can run the binary against the database has already got more
//!   than any endpoint would give them. `--as <USER_ID>` sets the principal the
//!   function sees, for a function that reads it.
//! * **Visibility is ignored.** A `Private` function has no route on purpose —
//!   it exists to be called from a hook — and a cron job is that same kind of
//!   caller, so it can call one.
//!
//! Migrations are *not* run: a job that starts by migrating is a job that can
//! migrate a production database at 3am because it was scheduled to.

use apiplant_core::App;
use apiplant_db::Db;

use crate::functions::{FunctionRegistry, HostBridge};

/// How to call.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The function's input, as JSON. Empty means `{}`, matching the HTTP
    /// endpoint's treatment of an empty body.
    pub input: String,
    /// The user id the function sees as its caller, if any.
    pub principal: Option<String>,
    /// Forward what the function `emit`s to stderr as it is produced, rather
    /// than dropping it. Keeps a long job's progress visible in `kubectl logs`
    /// without mixing into the result on stdout.
    pub emit_to_stderr: bool,
}

/// Run one of the app's functions and return its JSON result.
///
/// The error is the function's own message, or the reason it couldn't be run.
pub async fn call(app: &App, name: &str, options: Options) -> anyhow::Result<String> {
    let registry = FunctionRegistry::load(app);
    // Checked before anything is connected, so a typo'd name costs a database
    // connection and a Stripe client rather than reporting after them.
    let config_json = match registry.get(name) {
        Some(f) => f.config_json.clone(),
        None => {
            let known = registry
                .iter()
                .map(|f| f.manifest.name.to_string())
                .collect::<Vec<_>>();
            anyhow::bail!(
                "unknown function `{name}` — this app has: {}",
                match known.is_empty() {
                    true => "none".to_string(),
                    false => known.join(", "),
                }
            );
        }
    };

    let db = Db::connect(
        &app.config.database.resolved_url(),
        app.config.database.max_connections,
    )
    .await?;

    let mailer = apiplant_email::Mailer::from_config(&app.config.email)?;
    let email_templates =
        std::sync::Arc::new(crate::email_templates::EmailTemplates::load(&app.root)?);
    let cache = apiplant_cache::Cache::connect(&app.config.cache).await?;
    let ai = apiplant_ai::Ai::from_config(&app.config.ai)?;
    let payments = apiplant_payments::Payments::from_config(
        &app.config.payments,
        &app.config.server.public_origin(),
    )?;
    // A job that publishes is a normal thing to want — a nightly sweep queuing
    // one message per row it found. Nothing here *subscribes*, though: the
    // messages are handled by the running server, not by this process, which
    // exits as soon as the function returns.
    let queue = apiplant_queue::Queue::new(&db, app);

    // The chunks are drained on this task while the function runs on a blocking
    // one, so a chatty function can't fill the channel unread.
    let (printer, chunks) = match options.emit_to_stderr {
        true => {
            let (chunks, mut receiver) = tokio::sync::mpsc::unbounded_channel::<String>();
            let printer = tokio::spawn(async move {
                while let Some(chunk) = receiver.recv().await {
                    eprint!("{chunk}");
                }
            });
            (Some(printer), Some(chunks))
        }
        false => (None, None),
    };

    let mut bridge = HostBridge::new(
        db,
        tokio::runtime::Handle::current(),
        config_json,
        options.principal.unwrap_or_default(),
    )
    .with_services(mailer, cache, payments, ai)
    .with_email_templates(email_templates)
    .with_queue(queue);
    if let Some(chunks) = chunks {
        bridge = bridge.streaming(chunks);
    }

    let input = match options.input.trim().is_empty() {
        true => "{}".to_string(),
        false => options.input,
    };
    let name = name.to_string();
    let result = tokio::task::spawn_blocking(move || {
        let f = registry.get(&name).expect("checked above");
        f.invoke(bridge, &input)
    })
    .await
    .map_err(|_| anyhow::anyhow!("the function task panicked"))?;

    if let Some(printer) = printer {
        // The sender dropped with the bridge, so this ends on its own.
        let _ = printer.await;
    }

    result.map_err(|message| {
        match message.strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX) {
            // Unlike the HTTP path there is nobody to hide internals from: an
            // operator reading `kubectl logs` is exactly who the detail is for.
            Some(detail) => anyhow::anyhow!("function faulted: {detail}"),
            None => anyhow::anyhow!("{message}"),
        }
    })
}
