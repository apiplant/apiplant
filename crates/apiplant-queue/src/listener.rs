//! The half of the queue that makes it prompt: a Postgres `LISTEN`.
//!
//! This needs a connection of its own, which is why it is not simply another
//! method on [`Queue`](crate::Queue). `LISTEN` is a property of a *session* —
//! it lasts until that connection ends — and every other query in apiplant goes
//! through a pool that hands the connection back the moment the statement
//! finishes. Issuing `LISTEN` there would register interest on a connection
//! immediately returned to the pool and reused for something else.
//!
//! So the listener holds one connection open for the life of the process. That
//! is the cost of the feature: one connection per replica, whether or not
//! anything is ever published.
//!
//! Losing it is survivable and deliberately not fatal. [`sqlx`]'s listener
//! reconnects and re-subscribes on its own, and any notification that lands in
//! the gap costs nothing but latency, because the message is a committed row
//! that the subscriber's periodic sweep will find regardless. That is the whole
//! reason the row exists as well as the notification.

use sea_orm::sqlx::postgres::PgListener;

use crate::QueueError;

/// A live `LISTEN` on the app's queue channel.
pub struct Listener {
    inner: PgListener,
    channel: String,
}

impl Listener {
    /// Open a dedicated connection and subscribe to `channel`.
    ///
    /// Fails when the database is unreachable *right now*, which the caller
    /// should treat as "run without notifications" rather than as a reason not
    /// to start: a subscriber that polls every `poll_secs` still handles every
    /// message, just later.
    pub async fn connect(url: &str, channel: &str) -> Result<Self, QueueError> {
        let mut inner = PgListener::connect(url)
            .await
            .map_err(|e| QueueError::Backend(format!("cannot open a listener connection: {e}")))?;
        inner
            .listen(channel)
            .await
            .map_err(|e| QueueError::Backend(format!("cannot LISTEN on `{channel}`: {e}")))?;
        Ok(Listener {
            inner,
            channel: channel.to_string(),
        })
    }

    /// Wait for the next notification and return the topic it names.
    ///
    /// The topic is a hint, not an instruction: the caller sweeps for whatever
    /// is claimable rather than trusting it, because a notification can be
    /// coalesced, arrive twice, or be for a topic whose row another replica has
    /// already taken. Treating it as data to act on would make the queue's
    /// correctness depend on the one part of it that is allowed to be lost.
    pub async fn recv(&mut self) -> Result<String, QueueError> {
        // `recv` reconnects and re-issues the LISTEN internally, so this only
        // errors when the database is properly gone.
        let notification = self.inner.recv().await.map_err(|e| {
            QueueError::Backend(format!("listening on `{}` failed: {e}", self.channel))
        })?;
        Ok(notification.payload().to_string())
    }
}
