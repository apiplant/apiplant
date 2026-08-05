//! # apiplant-queue
//!
//! Work that happens *after* the response, without a broker.
//!
//! A function calls `publish("order.paid", …)`, the request returns, and some
//! milliseconds later another function runs with that message. Nothing new is
//! deployed to make that work: the transport is the Postgres the app already
//! has.
//!
//! ## Two halves, for two different reasons
//!
//! Publishing does two things, and it is worth being clear about which does
//! what — because the usual mistake is to build only one of them:
//!
//! * **A row in `queue_message`.** This is the message. It survives a restart,
//!   it records that an attempt failed and when the next one is due, and it is
//!   there to be looked at when somebody asks why a welcome email never went
//!   out. Everything about *reliability* lives here.
//! * **A `NOTIFY`.** This is only a tap on the shoulder. It carries no payload
//!   worth trusting and losing it costs nothing but latency, because the sweep
//!   in [`Queue::claim`] would have found the row anyway. Everything about
//!   *promptness* lives here.
//!
//! A design with only the notification (the tempting one — no table, no
//! migration) drops every message published while nothing was listening, and
//! has nowhere to put "this failed, try again in 20 seconds". A design with
//! only the table polls, and a one-second poll is both too slow and too chatty.
//! Together they are a queue.
//!
//! ## What is guaranteed
//!
//! **At-least-once.** A handler that succeeds and then dies before its row is
//! marked `done` runs again when the lease expires. This is not a rough edge to
//! be fixed later; it is the only honest guarantee a queue can give without the
//! handler taking part, since "did my side effect happen?" is a question only
//! the handler can answer. Write handlers that can run twice — check for the
//! row you were going to insert, use the message id as an idempotency key, make
//! the update the same update. `billing_event` exists for exactly this reason.
//!
//! **One subscriber, one claim.** Rows are taken with `FOR UPDATE SKIP LOCKED`,
//! so N replicas share the work rather than each doing all of it, and no two
//! ever hold the same message.
//!
//! **Order is not promised.** Messages are claimed oldest-first, but two
//! replicas handling two messages will finish in whatever order they finish.
//! A topic that needs strict ordering wants one subscriber and `batch = 1`,
//! and even then a retry moves a message behind its successors.

use apiplant_core::{App, QueuesConfig};
use apiplant_db::Db;
use sea_orm::sea_query::Value as SqlValue;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde::Serialize;
use serde_json::{json, Value};

mod listener;
pub use listener::Listener;

/// What went wrong handling a message.
#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    /// The request a function made isn't a queue operation.
    #[error("invalid queue request: {0}")]
    Request(String),

    /// The database refused, or was unreachable.
    #[error("queue: {0}")]
    Backend(String),
}

impl From<sea_orm::DbErr> for QueueError {
    fn from(e: sea_orm::DbErr) -> Self {
        QueueError::Backend(e.to_string())
    }
}

/// A published message, as the publisher hears about it.
#[derive(Debug, Clone, Serialize)]
pub struct Publication {
    /// Id of the first row written. A message with several subscribers has
    /// several rows and several ids; this is the one to quote in a log line.
    pub id: String,
    pub topic: String,
    /// How many subscribers it was queued for. **Zero is not an error** — the
    /// message is still recorded — but it is almost always a typo in a topic
    /// name, so a publisher that cares should look at it.
    pub delivered: usize,
}

/// One message, claimed and waiting to be handled.
#[derive(Debug, Clone)]
pub struct Delivery {
    pub id: String,
    pub topic: String,
    /// The function that subscribed to this topic.
    pub subscriber: String,
    /// What the publisher sent.
    pub payload: Value,
    /// Which attempt this is, counting from 1.
    pub attempts: u32,
    /// The principal that published it, or empty.
    pub published_by: String,
}

impl Delivery {
    /// The delivery context a handler sees through its hook, alongside the
    /// payload it gets as input.
    ///
    /// `attempts` is the field worth writing a handler against: a message on
    /// its fourth attempt is one whose side effects may already have happened.
    pub fn context(&self) -> Value {
        json!({
            "event": "message",
            "topic": self.topic,
            "message_id": self.id,
            "subscriber": self.subscriber,
            "attempts": self.attempts,
            "principal_id": self.published_by,
            "published_by": self.published_by,
        })
    }
}

/// The app's queue: publish here, claim from here.
///
/// Cloning is cheap — the [`DatabaseConnection`] inside is a pooled handle —
/// so every worker holds one.
#[derive(Clone)]
pub struct Queue {
    conn: DatabaseConnection,
    /// The physical table, resolved from the app's `queue_message` resource so
    /// that an app which overrides it with its own model still works.
    table: String,
    config: QueuesConfig,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("table", &self.table)
            .field("topics", &self.config.subscribe.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Queue {
    /// The queue for an app. Never fails and is never optional: `publish` works
    /// in an app whose `main.toml` says nothing about queues, because the table
    /// is a built-in and a message with no subscriber is still worth recording.
    pub fn new(db: &Db, app: &App) -> Self {
        let table = app
            .resources
            .get("queue_message")
            .map(|r| r.table_name())
            .unwrap_or_else(|| "apiplant_queue_message".to_string());
        Queue {
            conn: db.connection().clone(),
            table,
            config: app.config.queues.clone(),
        }
    }

    pub fn config(&self) -> &QueuesConfig {
        &self.config
    }

    /// Every topic this app subscribes to.
    pub fn topics(&self) -> Vec<String> {
        self.config.subscribe.keys().cloned().collect()
    }

    /// Add the index the claim query needs, if it isn't there.
    ///
    /// Not left to the migrator: the migrator's job is to make columns match
    /// the resource declarations, and this index is not a property of the
    /// schema but of one query — the `(status, available_at)` lookup every
    /// subscriber runs on every sweep, which is a sequential scan of the whole
    /// ledger without it. Retention keeps that table small in a healthy app and
    /// large in exactly the app that is having a bad day.
    pub async fn prepare(&self) -> Result<(), QueueError> {
        let sql = format!(
            "CREATE INDEX IF NOT EXISTS {index} ON {table} (status, available_at)",
            index = quote(&format!("idx_{}_claim", self.table))?,
            table = quote(&self.table)?,
        );
        self.execute_sql(sql, vec![]).await?;
        Ok(())
    }

    /// Publish a message: one row per subscriber, then one notification.
    ///
    /// The order matters and is not an accident. The rows are committed first,
    /// so a subscriber woken by the notification always finds them; notifying
    /// first would race, and the loser would be a wakeup for work that isn't
    /// visible yet — which looks exactly like a queue that randomly adds 30
    /// seconds of latency.
    pub async fn publish(
        &self,
        topic: &str,
        message: &Value,
        published_by: &str,
    ) -> Result<Publication, QueueError> {
        let topic = topic.trim();
        if !QueuesConfig::valid_topic(topic) {
            return Err(QueueError::Request(format!(
                "`{topic}` is not a topic: use letters, digits, `.`, `_`, `-` or `:`"
            )));
        }

        let subscribers = self.config.subscribers(topic);
        // A topic nobody listens to still gets a row. The alternative is
        // publishing into silence, and then the only evidence that a message
        // was ever sent is the absence of its effect — which is the hardest
        // kind of bug to be handed.
        let rows: Vec<&str> = match subscribers.is_empty() {
            true => vec![""],
            false => subscribers.iter().map(String::as_str).collect(),
        };

        let mut ids = Vec::with_capacity(rows.len());
        for subscriber in &rows {
            let id = uuid::Uuid::new_v4();
            // A row for nobody is born finished: it is a record, not work.
            let status = match subscriber.is_empty() {
                true => "done",
                false => "pending",
            };
            let sql = format!(
                "INSERT INTO {table} \
                 (\"id\", \"topic\", \"subscriber\", \"status\", \"payload\", \"attempts\", \
                  \"available_at\", \"processed_at\", \"published_by\", \"created_at\", \"updated_at\") \
                 VALUES ($1, $2, $3, $4, $5, 0, now(), \
                         CASE WHEN $4 = 'done' THEN now() ELSE NULL END, $6, now(), now())",
                table = quote(&self.table)?,
            );
            self.execute_sql(
                sql,
                vec![
                    SqlValue::from(id),
                    SqlValue::from(topic.to_string()),
                    SqlValue::from(subscriber.to_string()),
                    SqlValue::from(status.to_string()),
                    SqlValue::from(message.clone()),
                    SqlValue::from(published_by.to_string()),
                ],
            )
            .await?;
            ids.push(id.to_string());
        }

        if subscribers.is_empty() {
            tracing::warn!(
                topic,
                "published to a topic nothing subscribes to — the message is recorded in \
                 queue_message but no function will run; check [queues.subscribe]"
            );
        } else {
            // Only now, and only when there is something to find.
            self.notify(topic).await?;
        }

        Ok(Publication {
            id: ids.first().cloned().unwrap_or_default(),
            topic: topic.to_string(),
            delivered: subscribers.len(),
        })
    }

    /// Wake every listening subscriber, in this process and every other.
    ///
    /// Failing to notify is logged, not returned: the message is already
    /// committed, so the worst case is that it waits for the next sweep instead
    /// of running now. Turning that into an error would fail a request whose
    /// work is safely queued.
    async fn notify(&self, topic: &str) -> Result<(), QueueError> {
        let sql = "SELECT pg_notify($1, $2)".to_string();
        let result = self
            .execute_sql(
                sql,
                vec![
                    SqlValue::from(self.config.channel()),
                    SqlValue::from(topic.to_string()),
                ],
            )
            .await;
        if let Err(e) = result {
            tracing::warn!(topic, error = %e, "could not notify subscribers; the message will be picked up by the next sweep");
        }
        Ok(())
    }

    /// Take up to `[queues] batch` messages for this app's topics.
    ///
    /// One statement, and that is the point: the `SELECT … FOR UPDATE SKIP
    /// LOCKED` runs inside the `UPDATE`'s own transaction, so the rows are
    /// claimed and the locks released in a single commit. Holding a transaction
    /// open across the handler instead would mean one database connection tied
    /// up per in-flight message, and a long handler blocking `VACUUM` on the
    /// whole table.
    ///
    /// The `running` rows a dead worker left behind are swept back in by
    /// [`Queue::reclaim`] rather than here, so a stuck message costs a lease
    /// rather than being invisible.
    pub async fn claim(&self, worker: &str) -> Result<Vec<Delivery>, QueueError> {
        let topics = self.topics();
        if topics.is_empty() {
            return Ok(Vec::new());
        }
        let table = quote(&self.table)?;
        // `= ANY($1)` rather than an IN-list built by string concatenation:
        // topics come from config, but the day one is templated from anything
        // else this is already the safe shape.
        let sql = format!(
            "UPDATE {table} SET \
                \"status\" = 'running', \
                \"attempts\" = \"attempts\" + 1, \
                \"claimed_at\" = now(), \
                \"claimed_by\" = $2, \
                \"updated_at\" = now() \
             WHERE \"id\" IN ( \
                SELECT \"id\" FROM {table} \
                WHERE \"status\" = 'pending' \
                  AND \"available_at\" <= now() \
                  AND \"subscriber\" <> '' \
                  AND \"topic\" = ANY($1) \
                ORDER BY \"available_at\" \
                FOR UPDATE SKIP LOCKED \
                LIMIT {limit} \
             ) \
             RETURNING \"id\"::text AS id, \"topic\", \"subscriber\", \"payload\", \
                       \"attempts\", coalesce(\"published_by\", '') AS published_by",
            limit = self.config.batch.max(1),
        );

        let rows = self
            .conn
            .query_all(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                vec![topic_array(&topics), SqlValue::from(worker.to_string())],
            ))
            .await?;

        rows.into_iter()
            .map(|row| {
                Ok(Delivery {
                    id: row.try_get::<String>("", "id")?,
                    topic: row.try_get::<String>("", "topic")?,
                    subscriber: row.try_get::<String>("", "subscriber")?,
                    payload: row.try_get::<Value>("", "payload")?,
                    attempts: row.try_get::<i32>("", "attempts")?.max(0) as u32,
                    published_by: row.try_get::<String>("", "published_by")?,
                })
            })
            .collect()
    }

    /// Seconds until the next scheduled message becomes claimable, if there is
    /// one waiting.
    ///
    /// This is what makes a retry honour the backoff it was given rather than
    /// the poll interval. A failure schedules itself for `now() + 10s`, but
    /// nothing publishes when a backoff expires — there is no `NOTIFY` for "a
    /// timer went off" — so a subscriber that always waited the full
    /// `poll_secs` would round every retry up to the next 30-second boundary.
    /// Asking the database when to come back costs one indexed query per cycle
    /// and makes the configured number mean what it says.
    ///
    /// `None` means nothing is scheduled, and the caller should wait its full
    /// interval. `Some(0)` means something is due now.
    pub async fn next_due(&self) -> Result<Option<u64>, QueueError> {
        let topics = self.topics();
        if topics.is_empty() {
            return Ok(None);
        }
        let sql = format!(
            "SELECT ceil(extract(epoch FROM (min(\"available_at\") - now())))::bigint AS wait \
             FROM {table} \
             WHERE \"status\" = 'pending' AND \"subscriber\" <> '' AND \"topic\" = ANY($1)",
            table = quote(&self.table)?,
        );
        let row = self
            .conn
            .query_one(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                vec![topic_array(&topics)],
            ))
            .await?;
        // `min()` over no rows is NULL, which is "nothing is waiting" — not
        // "come back immediately".
        let wait: Option<i64> = match row {
            Some(row) => row.try_get("", "wait").ok(),
            None => None,
        };
        Ok(wait.map(|seconds| seconds.max(0) as u64))
    }

    /// Mark a message handled.
    pub async fn complete(&self, id: &str) -> Result<(), QueueError> {
        let sql = format!(
            "UPDATE {table} SET \"status\" = 'done', \"processed_at\" = now(), \
                    \"claimed_by\" = NULL, \"updated_at\" = now() \
             WHERE \"id\" = $1::uuid",
            table = quote(&self.table)?,
        );
        self.execute_sql(sql, vec![SqlValue::from(id.to_string())])
            .await?;
        Ok(())
    }

    /// Record a failed attempt: schedule the retry, or give up.
    ///
    /// Giving up leaves the row `failed` with its error rather than deleting
    /// it. A dead-letter you have to go and look at is the point — a queue that
    /// quietly discards what it could not handle is a queue that loses orders.
    pub async fn fail(&self, delivery: &Delivery, error: &str) -> Result<bool, QueueError> {
        let exhausted = delivery.attempts >= self.config.max_attempts.max(1);
        let delay = self.config.retry_delay_secs(delivery.attempts);

        let sql = match exhausted {
            true => format!(
                "UPDATE {table} SET \"status\" = 'failed', \"error\" = $2, \
                        \"processed_at\" = now(), \"claimed_by\" = NULL, \"updated_at\" = now() \
                 WHERE \"id\" = $1::uuid",
                table = quote(&self.table)?,
            ),
            false => format!(
                "UPDATE {table} SET \"status\" = 'pending', \"error\" = $2, \
                        \"available_at\" = now() + make_interval(secs => {delay}), \
                        \"claimed_by\" = NULL, \"updated_at\" = now() \
                 WHERE \"id\" = $1::uuid",
                table = quote(&self.table)?,
            ),
        };
        self.execute_sql(
            sql,
            vec![
                SqlValue::from(delivery.id.clone()),
                SqlValue::from(truncate(error, 4000)),
            ],
        )
        .await?;

        match exhausted {
            true => tracing::error!(
                topic = %delivery.topic,
                subscriber = %delivery.subscriber,
                message_id = %delivery.id,
                attempts = delivery.attempts,
                %error,
                "message failed for the last time — left in queue_message with status 'failed'"
            ),
            false => tracing::warn!(
                topic = %delivery.topic,
                subscriber = %delivery.subscriber,
                message_id = %delivery.id,
                attempt = delivery.attempts,
                retry_in_secs = delay,
                %error,
                "message failed; will retry"
            ),
        }
        Ok(!exhausted)
    }

    /// Offer up messages whose handler never came back.
    ///
    /// Returns how many were taken back. The attempt is *not* undone: a handler
    /// that reliably kills its process — the classic out-of-memory loop — has
    /// spent an attempt, and will run out of them and land in the dead-letter
    /// instead of retrying until somebody notices the restart count.
    pub async fn reclaim(&self) -> Result<u64, QueueError> {
        let sql = format!(
            "UPDATE {table} SET \"status\" = 'pending', \"claimed_by\" = NULL, \
                    \"error\" = 'the subscriber holding this message stopped responding', \
                    \"updated_at\" = now() \
             WHERE \"status\" = 'running' \
               AND \"claimed_at\" < now() - make_interval(secs => {lease})",
            table = quote(&self.table)?,
            lease = self.config.lease_secs.max(1),
        );
        let affected = self.execute_sql(sql, vec![]).await?;
        if affected > 0 {
            tracing::warn!(
                messages = affected,
                "reclaimed messages whose subscriber died mid-handler"
            );
        }
        Ok(affected)
    }

    /// Delete handled messages older than `[queues] retain_hours`. `0` keeps
    /// them forever.
    ///
    /// Only `done` rows. A `failed` row is the whole reason the ledger exists
    /// and is never swept — if it were, the dead-letter would empty itself
    /// overnight and the evidence would go with it.
    pub async fn prune(&self) -> Result<u64, QueueError> {
        if self.config.retain_hours == 0 {
            return Ok(0);
        }
        let sql = format!(
            "DELETE FROM {table} WHERE \"status\" = 'done' \
             AND \"processed_at\" < now() - make_interval(hours => {hours})",
            table = quote(&self.table)?,
            hours = self.config.retain_hours,
        );
        self.execute_sql(sql, vec![]).await
    }

    /// Run one operation on behalf of a function. The JSON surface behind
    /// [`HostApi::publish`](apiplant_abi::HostApi::publish).
    pub async fn execute(&self, request: &str, published_by: &str) -> Result<Value, QueueError> {
        let request: Value = serde_json::from_str(request)
            .map_err(|e| QueueError::Request(format!("not JSON: {e}")))?;

        let op = request.get("op").and_then(Value::as_str).unwrap_or("publish");
        match op {
            "publish" => {
                let topic = request
                    .get("topic")
                    .and_then(Value::as_str)
                    .ok_or_else(|| QueueError::Request("`topic` is required".into()))?;
                // A publish with no message is a signal, and a signal is a
                // perfectly good message — `{}` rather than an error.
                let message = request
                    .get("message")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let publication = self.publish(topic, &message, published_by).await?;
                Ok(serde_json::to_value(publication).unwrap_or(Value::Null))
            }
            other => Err(QueueError::Request(format!(
                "`{other}` is not a queue operation; expected `publish`"
            ))),
        }
    }

    async fn execute_sql(&self, sql: String, params: Vec<SqlValue>) -> Result<u64, QueueError> {
        let result = self
            .conn
            .execute(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                params,
            ))
            .await?;
        Ok(result.rows_affected())
    }
}

/// A `text[]` parameter for the topic filter.
fn topic_array(topics: &[String]) -> SqlValue {
    SqlValue::Array(
        sea_orm::sea_query::ArrayType::String,
        Some(Box::new(
            topics
                .iter()
                .map(|t| SqlValue::from(t.clone()))
                .collect::<Vec<_>>(),
        )),
    )
}

/// Quote an identifier for interpolation into SQL.
///
/// The table name comes from the app's own resource declaration rather than
/// from a request, but it is still the one thing here that is pasted into a
/// statement rather than bound — so it is checked rather than trusted.
fn quote(ident: &str) -> Result<String, QueueError> {
    if ident.is_empty()
        || !ident
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(QueueError::Backend(format!(
            "`{ident}` is not a usable table name"
        )));
    }
    Ok(format!("\"{ident}\""))
}

/// Keep an error message inside the column, on a character boundary.
fn truncate(text: &str, max: usize) -> String {
    match text.len() <= max {
        true => text.to_string(),
        false => {
            let mut end = max;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &text[..end])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_topic_is_an_identifier_not_free_text() {
        assert!(QueuesConfig::valid_topic("order.paid"));
        assert!(QueuesConfig::valid_topic("user:signed_up"));
        assert!(QueuesConfig::valid_topic("a-b_c.d:e"));

        assert!(!QueuesConfig::valid_topic(""));
        assert!(!QueuesConfig::valid_topic("   "));
        assert!(!QueuesConfig::valid_topic("order paid"));
        assert!(!QueuesConfig::valid_topic("order'; DROP TABLE"));
        assert!(!QueuesConfig::valid_topic(&"x".repeat(201)));
    }

    #[test]
    fn only_identifiers_can_be_interpolated_as_a_table() {
        assert_eq!(quote("apiplant_queue_message").unwrap(), "\"apiplant_queue_message\"");
        assert!(quote("").is_err());
        assert!(quote("queue\"; DROP TABLE x --").is_err());
        assert!(quote("public.queue").is_err());
    }

    /// The backoff is what stops a broken downstream being hammered, so its
    /// shape matters: doubling, and capped so a retry can't be scheduled past
    /// the point anyone is still watching.
    #[test]
    fn the_retry_backoff_doubles_and_is_capped() {
        let config = QueuesConfig {
            retry_backoff_secs: 10,
            ..QueuesConfig::default()
        };
        assert_eq!(config.retry_delay_secs(1), 10);
        assert_eq!(config.retry_delay_secs(2), 20);
        assert_eq!(config.retry_delay_secs(3), 40);
        assert_eq!(config.retry_delay_secs(4), 80);
        // An hour, however many attempts have gone by — including absurd ones,
        // which must not overflow into a tiny delay.
        assert_eq!(config.retry_delay_secs(50), 3600);
        assert_eq!(config.retry_delay_secs(u32::MAX), 3600);
    }

    #[test]
    fn a_long_error_is_truncated_on_a_character_boundary() {
        let long = "é".repeat(3000);
        let cut = truncate(&long, 4000);
        // 4000 bytes of message plus the three-byte ellipsis.
        assert!(cut.len() <= 4003, "{} bytes", cut.len());
        assert!(cut.ends_with('…'));
        // The point of the boundary walk: this must not panic or produce
        // invalid UTF-8.
        assert!(cut.chars().count() > 1);
    }

    #[test]
    fn subscriptions_are_read_as_one_name_or_several() {
        let config: QueuesConfig = toml::from_str(
            r#"
            [subscribe]
            "order.paid" = "fulfil"
            "user.signed_up" = ["welcome", "crm_sync"]
            "ignored" = []
        "#,
        )
        .unwrap();
        assert_eq!(config.subscribers("order.paid"), ["fulfil"]);
        assert_eq!(config.subscribers("user.signed_up"), ["welcome", "crm_sync"]);
        // A topic with no subscriber is not a subscription.
        assert!(config.subscribers("ignored").is_empty());
        assert!(config.subscribers("never.declared").is_empty());
    }
}
