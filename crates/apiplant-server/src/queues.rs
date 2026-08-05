//! The subscriber: the loop that turns queued messages back into function calls.
//!
//! One task per process, started by [`crate::run_with`] when the app subscribes
//! to anything. It does three things in a cycle:
//!
//! 1. **Wait** — for a `NOTIFY` from a publisher, or for `[queues] poll_secs`,
//!    whichever comes first. The notification is what makes delivery feel
//!    instant; the timeout is what makes it *correct*, since a notification can
//!    be missed and a retry has no publisher to announce it.
//! 2. **Claim** — take a batch with `FOR UPDATE SKIP LOCKED`, so several
//!    replicas share the work instead of each doing all of it.
//! 3. **Run** — invoke each message's function on a blocking worker, then mark
//!    the row done or schedule the retry.
//!
//! ## Why it drains rather than handling one batch per wake
//!
//! A publisher that queues 500 messages fires notifications the listener will
//! coalesce into far fewer wakeups — Postgres is allowed to, and does. A loop
//! that handled one batch per notification would leave the rest sitting until
//! the next poll. So a wake keeps claiming until a claim comes back empty.
//!
//! ## What a failure costs
//!
//! Nothing that reaches the caller, because there is no caller: the request
//! that published this ended long ago. A handler that returns an error, panics,
//! or is missing entirely leaves a row with the reason on it and a retry
//! scheduled — and after `[queues] max_attempts`, a `failed` row somebody has
//! to come and look at. That is the design: the queue's job is to make the
//! failure *visible and re-runnable*, not to make it somebody's 500.

use std::sync::Arc;
use std::time::Duration;

use apiplant_ai::Ai;
use apiplant_cache::Cache;
use apiplant_db::Db;
use apiplant_email::Mailer;
use apiplant_payments::Payments;
use apiplant_queue::{Delivery, Listener, Queue};

use crate::functions::{FunctionRegistry, HostBridge};

/// Everything a subscriber needs to run a handler, cloned once at boot.
///
/// The same services a request-time invocation gets — a queued function is an
/// ordinary function, and a handler that sends mail or reads the cache should
/// not have to care which side of the queue it is on.
pub struct Subscriber {
    pub db: Db,
    pub queue: Queue,
    pub functions: Arc<FunctionRegistry>,
    pub mailer: Option<Mailer>,
    pub cache: Option<Cache>,
    pub payments: Option<Payments>,
    pub ai: Option<Ai>,
    /// The database URL, for the listener's own dedicated connection.
    pub database_url: String,
    /// Identifies this process in `queue_message.claimed_by`. Worth having when
    /// three replicas are up and one of them is the one that keeps dying.
    pub worker: String,
}

/// Run until the process ends.
///
/// Never returns an error: a subscriber that gave up would leave messages
/// queued with nothing to handle them, and the failure modes it could give up
/// on — the database being briefly unreachable, a listener connection dropping
/// — are all ones that fix themselves. Everything is logged and retried instead.
pub async fn run(subscriber: Subscriber) {
    let config = subscriber.queue.config().clone();
    let poll = Duration::from_secs(config.poll_secs.max(1));

    if let Err(error) = subscriber.queue.prepare().await {
        // Only the index is missing, so this is slow rather than broken.
        tracing::warn!(%error, "could not prepare the queue index; claims will be slower");
    }

    // Losing the listener is a latency problem, not a correctness one — the
    // poll below finds every message either way — so failing to connect is a
    // warning and the loop starts anyway.
    let channel = config.channel();
    let mut listener = match Listener::connect(&subscriber.database_url, &channel).await {
        Ok(listener) => Some(listener),
        Err(error) => {
            tracing::warn!(
                %error, channel,
                "queue subscriber could not LISTEN; falling back to polling every {}s",
                poll.as_secs()
            );
            None
        }
    };

    tracing::info!(
        worker = %subscriber.worker,
        channel,
        topics = ?subscriber.queue.topics(),
        "queue subscriber started"
    );

    loop {
        // A sweep on every cycle, before waiting: this is what picks up
        // messages published while this process was starting, and retries whose
        // backoff has expired since the last pass.
        drain(&subscriber).await;

        // Messages abandoned by a subscriber that died mid-handler. Cheap, and
        // only ever does anything when something went wrong elsewhere.
        if let Err(error) = subscriber.queue.reclaim().await {
            tracing::warn!(%error, "could not reclaim abandoned messages");
        }
        if let Err(error) = subscriber.queue.prune().await {
            tracing::warn!(%error, "could not prune handled messages");
        }

        // How long it is safe to sleep for. Normally the poll interval, but a
        // message already scheduled — a retry waiting out its backoff — has an
        // exact time it becomes claimable, and nothing will notify when it
        // arrives. Without this, a 10-second backoff under a 30-second poll
        // takes 30 seconds, and the configured number is a fiction.
        let wait = match subscriber.queue.next_due().await {
            Ok(Some(seconds)) => poll.min(Duration::from_secs(seconds)),
            // Nothing scheduled, or the question failed: wait normally. The
            // sweep at the top of the loop is the backstop either way.
            _ => poll,
        };

        match &mut listener {
            Some(active) => {
                // Whichever comes first. The topic the notification names is
                // ignored on purpose — see `Listener::recv`.
                match tokio::time::timeout(wait, active.recv()).await {
                    Ok(Ok(_topic)) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "queue listener failed; polling until it recovers");
                        listener = None;
                    }
                    // Nothing published; the sweep at the top of the loop is
                    // the whole reason this timeout exists.
                    Err(_) => {}
                }
            }
            None => {
                tokio::time::sleep(wait).await;
                // Try to get notifications back. Until this succeeds the queue
                // still works, just at poll speed.
                if let Ok(reconnected) =
                    Listener::connect(&subscriber.database_url, &channel).await
                {
                    tracing::info!(channel, "queue listener reconnected");
                    listener = Some(reconnected);
                }
            }
        }
    }
}

/// Claim and handle until there is nothing claimable left.
async fn drain(subscriber: &Subscriber) {
    loop {
        let batch = match subscriber.queue.claim(&subscriber.worker).await {
            Ok(batch) => batch,
            Err(error) => {
                tracing::warn!(%error, "could not claim messages; will try again on the next pass");
                return;
            }
        };
        if batch.is_empty() {
            return;
        }
        for delivery in batch {
            handle(subscriber, delivery).await;
        }
    }
}

/// [`handle`], reachable from the integration tests so they can drive one
/// message through without racing the real loop's timers.
#[cfg(test)]
pub(crate) async fn handle_for_test(subscriber: &Subscriber, delivery: Delivery) {
    handle(subscriber, delivery).await
}

/// Run one message's handler and record what happened.
async fn handle(subscriber: &Subscriber, delivery: Delivery) {
    let result = invoke(subscriber, &delivery).await;

    let outcome = match result {
        Ok(_) => subscriber.queue.complete(&delivery.id).await.map(|_| ()),
        Err(error) => subscriber.queue.fail(&delivery, &error).await.map(|_| ()),
    };

    // The one genuinely awkward case: the handler ran but its row could not be
    // marked. The work is done and the message is still `running`, so it will
    // be reclaimed after the lease and run a second time — which is exactly the
    // at-least-once contract, and worth saying out loud in the log because the
    // duplicate will otherwise look inexplicable.
    if let Err(error) = outcome {
        tracing::error!(
            message_id = %delivery.id,
            topic = %delivery.topic,
            %error,
            "handled a message but could not record the outcome; it will be delivered again"
        );
    }
}

/// Invoke the subscribed function with the message as its input.
async fn invoke(subscriber: &Subscriber, delivery: &Delivery) -> Result<String, String> {
    let Some(function) = subscriber.functions.get(&delivery.subscriber) else {
        // A subscription naming a function that isn't loaded. Reported at boot
        // too, but this is where it costs something, so it says so again with
        // the message that is now stuck behind it.
        return Err(format!(
            "`{}` is subscribed to `{}` but no such function is loaded",
            delivery.subscriber, delivery.topic
        ));
    };

    let bridge = HostBridge::new(
        subscriber.db.clone(),
        tokio::runtime::Handle::current(),
        function.config_json.clone(),
        delivery.published_by.clone(),
    )
    .with_services(
        subscriber.mailer.clone(),
        subscriber.cache.clone(),
        subscriber.payments.clone(),
        subscriber.ai.clone(),
    )
    // A handler may publish in turn — a chain of steps, each queued — so the
    // queue goes across too.
    .with_queue(subscriber.queue.clone())
    // The delivery envelope rides in the hook slot, which is where a function
    // already looks for "why am I running". See `Delivery::context`.
    .with_hook(delivery.context().to_string());

    // The message body is the function's input, exactly as if it had been
    // posted to the endpoint. That is what keeps a handler an ordinary function
    // — callable by hand, testable, and usable over HTTP as well.
    let input = delivery.payload.to_string();
    let name = delivery.subscriber.clone();
    let functions = Arc::clone(&subscriber.functions);

    let result = tokio::task::spawn_blocking(move || {
        let function = functions.get(&name).expect("checked above");
        function.invoke(bridge, &input)
    })
    .await
    .map_err(|_| "the handler panicked".to_string())?;

    result.map_err(|message| {
        // Nobody is waiting on this, so unlike the HTTP path there is no reason
        // to withhold the internal detail — it goes on the row, which is the
        // only place anybody will look for it.
        match message.strip_prefix(apiplant_abi::INTERNAL_ERROR_PREFIX) {
            Some(detail) => format!("handler faulted: {detail}"),
            None => message,
        }
    })
}
