//! Publishing a message, and handling it.
//!
//! These run against a real database, which is the only way this feature is
//! worth testing: the claim is a `FOR UPDATE SKIP LOCKED` statement, the retry
//! backoff is an interval arithmetic expression, and the durability is the row.
//! None of that is exercised by a mock.

use super::*;

use apiplant_queue::Queue;

/// A handler that records what it was given and succeeds.
fn recorder(_host: &HostApi_TO<'_, RBox<()>>, hook: &str, input: &str) -> Result<String, String> {
    HANDLED
        .lock()
        .unwrap()
        .push((hook.to_string(), input.to_string()));
    Ok(json!({ "ok": true }).to_string())
}

/// What `recorder` saw, for one topic only.
///
/// The log is process-global and these tests run in parallel, so counting the
/// whole of it would make each test's result depend on its neighbours.
fn handled(topic: &str) -> Vec<(Value, Value)> {
    HANDLED
        .lock()
        .unwrap()
        .iter()
        .map(|(hook, input)| {
            (
                serde_json::from_str::<Value>(hook).unwrap_or(Value::Null),
                serde_json::from_str::<Value>(input).unwrap_or(Value::Null),
            )
        })
        .filter(|(hook, _)| hook["topic"] == topic)
        .collect()
}

/// A handler that always fails, for the retry and dead-letter paths.
fn always_fails(
    _host: &HostApi_TO<'_, RBox<()>>,
    _hook: &str,
    _input: &str,
) -> Result<String, String> {
    Err("the downstream service is down".to_string())
}

static HANDLED: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

fn main_toml(db_url: &str, extra: &str) -> String {
    format!(
        r#"
[server]
base_path = "/api"

[database]
url = "{db_url}"

[queues]
{extra}
"#
    )
}

/// Every row currently in the queue, newest last.
async fn rows(state: &AppState) -> Vec<Value> {
    let json = state
        .db
        .raw_json(
            "SELECT topic, subscriber, status, attempts, payload, published_by, error \
             FROM apiplant_queue_message ORDER BY created_at, subscriber",
            &[],
        )
        .await
        .unwrap();
    json.as_array().cloned().unwrap_or_default()
}

/// Publish → row → claim → invoke → done, which is the whole feature.
#[ntex::test]
async fn a_published_message_is_recorded_claimed_handled_and_marked_done() {
    let db = TempDatabase::create("queue-roundtrip").await;
    let root = temp_dir("queue-roundtrip");
    write_files(
        &root,
        &[(
            "main.toml",
            &main_toml(
                &db.url,
                r#"
[queues.subscribe]
"order.paid" = "fulfil_order"
"#,
            ),
        )],
    );

    let state = load_state_with(
        &root,
        vec![test_function("fulfil_order", Visibility::Private, recorder)],
    )
    .await;

    let publication = state
        .queue
        .publish(
            "order.paid",
            &json!({ "order_id": "o-9", "total": 4200 }),
            "user-1",
        )
        .await
        .unwrap();
    assert_eq!(publication.delivered, 1);

    // The message is a committed row before anything has handled it — this is
    // the property that makes it survive a restart.
    let queued = rows(&state).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0]["status"], "pending");
    assert_eq!(queued[0]["subscriber"], "fulfil_order");
    assert_eq!(queued[0]["attempts"], 0);
    assert_eq!(queued[0]["payload"]["order_id"], "o-9");
    // The publisher is carried through to the handler, which is what makes the
    // eventual work attributable.
    assert_eq!(queued[0]["published_by"], "user-1");

    let subscriber = subscriber(&state, &db.url);
    let claimed = subscriber.queue.claim("test-worker").await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempts, 1);

    crate::queues::handle_for_test(&subscriber, claimed[0].clone()).await;

    let handled = handled("order.paid");
    assert_eq!(handled.len(), 1, "the subscriber should have run once");
    let (hook, input) = &handled[0];
    // The message body arrives as ordinary input, so the handler is an
    // ordinary function.
    assert_eq!(input["order_id"], "o-9");
    // The envelope arrives in the hook slot.
    assert_eq!(hook["event"], "message");
    assert_eq!(hook["topic"], "order.paid");
    assert_eq!(hook["attempts"], 1);
    assert_eq!(hook["principal_id"], "user-1");

    let done = rows(&state).await;
    assert_eq!(done[0]["status"], "done");

    // And a second claim finds nothing: a handled message is not re-delivered.
    assert!(subscriber
        .queue
        .claim("test-worker")
        .await
        .unwrap()
        .is_empty());
}

/// One row per subscriber, so a failing handler cannot drag its neighbour into
/// its retries.
#[ntex::test]
async fn each_subscriber_gets_its_own_row_and_its_own_retries() {
    let db = TempDatabase::create("queue-fanout").await;
    let root = temp_dir("queue-fanout");
    write_files(
        &root,
        &[(
            "main.toml",
            &main_toml(
                &db.url,
                r#"
max_attempts = 2
retry_backoff_secs = 0

[queues.subscribe]
"user.signed_up" = ["send_welcome", "sync_crm"]
"#,
            ),
        )],
    );

    let state = load_state_with(
        &root,
        vec![
            test_function("send_welcome", Visibility::Private, recorder),
            test_function("sync_crm", Visibility::Private, always_fails),
        ],
    )
    .await;

    let publication = state
        .queue
        .publish("user.signed_up", &json!({ "email": "ann@example.com" }), "")
        .await
        .unwrap();
    assert_eq!(publication.delivered, 2);
    assert_eq!(rows(&state).await.len(), 2);

    let subscriber = subscriber(&state, &db.url);

    // First pass: one succeeds, one fails and is scheduled to retry.
    for delivery in subscriber.queue.claim("w1").await.unwrap() {
        crate::queues::handle_for_test(&subscriber, delivery).await;
    }
    let after_one = rows(&state).await;
    let welcome = after_one
        .iter()
        .find(|r| r["subscriber"] == "send_welcome")
        .unwrap();
    let crm = after_one
        .iter()
        .find(|r| r["subscriber"] == "sync_crm")
        .unwrap();
    assert_eq!(welcome["status"], "done");
    assert_eq!(crm["status"], "pending", "a failure goes back to pending");
    assert!(crm["error"].as_str().unwrap().contains("down"));

    // Second pass, with `retry_backoff_secs = 0` so it is immediately due:
    // only the failing one is claimable, and it exhausts `max_attempts = 2`.
    let retry = subscriber.queue.claim("w1").await.unwrap();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].subscriber, "sync_crm");
    assert_eq!(retry[0].attempts, 2);
    crate::queues::handle_for_test(&subscriber, retry[0].clone()).await;

    let dead = rows(&state).await;
    let crm = dead.iter().find(|r| r["subscriber"] == "sync_crm").unwrap();
    // The dead-letter: kept, with the reason, for somebody to look at. Not
    // deleted, and not retried forever.
    assert_eq!(crm["status"], "failed");
    assert_eq!(crm["attempts"], 2);
    assert!(subscriber.queue.claim("w1").await.unwrap().is_empty());

    // The successful handler ran exactly once despite its neighbour's retries.
    assert_eq!(handled("user.signed_up").len(), 1);
}

/// A topic nobody listens to still leaves a row. The whole point is that
/// "nothing happened" is answerable.
#[ntex::test]
async fn publishing_to_an_unsubscribed_topic_records_the_message_anyway() {
    let db = TempDatabase::create("queue-orphan").await;
    let root = temp_dir("queue-orphan");
    write_files(&root, &[("main.toml", &main_toml(&db.url, ""))]);
    let state = load_state(&root).await;

    let publication = state
        .queue
        .publish("nobody.listening", &json!({ "a": 1 }), "")
        .await
        .unwrap();
    assert_eq!(publication.delivered, 0);

    let queued = rows(&state).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0]["topic"], "nobody.listening");
    assert_eq!(queued[0]["subscriber"], "");
    // Born finished: it is a record, not work waiting to be done.
    assert_eq!(queued[0]["status"], "done");

    // A malformed topic, on the other hand, is refused outright.
    let err = state
        .queue
        .publish("not a topic!", &json!({}), "")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not a topic"), "{err}");
}

/// A resource's `[publish]` declaration: the row is the message, and the write
/// that produced it is not held up by anything downstream.
#[ntex::test]
async fn a_resource_announces_its_writes_on_the_topics_it_declares() {
    let db = TempDatabase::create("queue-resource").await;
    let root = temp_dir("queue-resource");
    write_files(
        &root,
        &[
            (
                "main.toml",
                &main_toml(
                    &db.url,
                    r#"
[queues.subscribe]
"note.created" = "on_note"
"#,
                ),
            ),
            (
                "models/note.toml",
                r#"
[resource]
name = "note"
scope = "global"

[permissions]
create = "public"
update = "public"
delete = "public"
read   = "public"
list   = "public"

[publish]
after_create = "note.created"
after_delete = "note.deleted"

[fields.title]
type = "string"
required = true
"#,
            ),
        ],
    );

    let state = load_state_with(
        &root,
        vec![test_function("on_note", Visibility::Private, recorder)],
    )
    .await;
    let app = init_http_app!(state.clone());

    let created = test::call_service(
        &app,
        req_json("POST", "/api/note", json!({ "title": "Buy milk" })),
    )
    .await;
    assert_eq!(created.status(), 201);
    let note = read_json(created).await;
    let id = note["id"].as_str().unwrap().to_string();

    let queued = rows(&state).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0]["topic"], "note.created");
    // The row *is* the message.
    assert_eq!(queued[0]["payload"]["title"], "Buy milk");

    let deleted = test::call_service(
        &app,
        test::TestRequest::delete()
            .uri(&format!("/api/note/{id}"))
            .to_request(),
    )
    .await;
    assert_eq!(deleted.status(), 204);

    let queued = rows(&state).await;
    assert_eq!(queued.len(), 2);
    let deleted_msg = queued
        .iter()
        .find(|r| r["topic"] == "note.deleted")
        .unwrap();
    // Nothing subscribes to `note.deleted`, so it is recorded and finished —
    // and the delete still returned 204 either way.
    assert_eq!(deleted_msg["subscriber"], "");
    assert_eq!(deleted_msg["payload"]["title"], "Buy milk");
}

/// The HTTP endpoint is absent unless `[queues] publish` opts in, and honours
/// the permission it names when it is there.
#[ntex::test]
async fn the_publish_endpoint_is_off_by_default_and_gated_when_on() {
    let db = TempDatabase::create("queue-http-off").await;
    let root = temp_dir("queue-http-off");
    write_files(&root, &[("main.toml", &main_toml(&db.url, ""))]);
    let state = load_state(&root).await;
    let app = init_http_app!(state);

    let refused = test::call_service(
        &app,
        req_json("POST", "/api/queues/order.paid", json!({ "id": 1 })),
    )
    .await;
    // Not 403: there is no such endpoint, and probing must not say otherwise.
    assert_eq!(refused.status(), 404);

    // Now an app that opted in.
    let db = TempDatabase::create("queue-http-on").await;
    let root = temp_dir("queue-http-on");
    write_files(
        &root,
        &[(
            "main.toml",
            &main_toml(&db.url, "publish = \"authenticated\"\n"),
        )],
    );
    let state = load_state(&root).await;
    let app = init_http_app!(state.clone());

    let anonymous = test::call_service(
        &app,
        req_json("POST", "/api/queues/order.paid", json!({ "id": 1 })),
    )
    .await;
    assert_eq!(anonymous.status(), 401);

    let registration = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/auth/register",
            json!({"email":"ann@example.com","password":"pw"}),
        ),
    )
    .await;
    let token = read_json(registration).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    let accepted = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/api/queues/order.paid")
            .header("authorization", format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .set_payload(json!({ "order_id": "o-1" }).to_string())
            .to_request(),
    )
    .await;
    // 202, not 200: the message is written down, and that is the whole promise.
    assert_eq!(accepted.status(), 202);
    let body = read_json(accepted).await;
    assert_eq!(body["topic"], "order.paid");

    let queued = rows(&state).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0]["payload"]["order_id"], "o-1");
    // Attributed to the caller, not anonymous.
    assert!(!queued[0]["published_by"].as_str().unwrap().is_empty());
}

/// A message whose subscriber died mid-handler comes back after the lease,
/// rather than being stuck in `running` forever.
#[ntex::test]
async fn an_abandoned_message_is_reclaimed_after_its_lease() {
    let db = TempDatabase::create("queue-lease").await;
    let root = temp_dir("queue-lease");
    write_files(
        &root,
        &[(
            "main.toml",
            &main_toml(
                &db.url,
                r#"
lease_secs = 1

[queues.subscribe]
"slow.work" = "never_returns"
"#,
            ),
        )],
    );
    let state = load_state(&root).await;

    state
        .queue
        .publish("slow.work", &json!({ "n": 1 }), "")
        .await
        .unwrap();

    // Claimed and then, as far as the database knows, abandoned.
    let claimed = state.queue.claim("doomed-worker").await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(rows(&state).await[0]["status"], "running");
    // Nobody else may take it while the lease holds.
    assert!(state.queue.claim("other").await.unwrap().is_empty());
    assert_eq!(state.queue.reclaim().await.unwrap(), 0);

    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;

    assert_eq!(state.queue.reclaim().await.unwrap(), 1);
    let retried = state.queue.claim("other").await.unwrap();
    assert_eq!(retried.len(), 1);
    // The attempt is *not* refunded: a handler that reliably kills its process
    // must still run out of attempts rather than retry forever.
    assert_eq!(retried[0].attempts, 2);
}

/// A retry has an exact time it becomes claimable and nothing notifies when it
/// arrives, so the loop has to *ask* — otherwise a 10-second backoff under a
/// 30-second poll takes 30 seconds and the configured number means nothing.
#[ntex::test]
async fn the_loop_is_told_when_the_next_retry_is_due() {
    let db = TempDatabase::create("queue-due").await;
    let root = temp_dir("queue-due");
    write_files(
        &root,
        &[(
            "main.toml",
            &main_toml(
                &db.url,
                r#"
retry_backoff_secs = 20

[queues.subscribe]
"flaky.work" = "always_fails"
"#,
            ),
        )],
    );
    let state = load_state_with(
        &root,
        vec![test_function(
            "always_fails",
            Visibility::Private,
            always_fails,
        )],
    )
    .await;

    // Nothing queued: the loop should wait its full interval.
    assert_eq!(state.queue.next_due().await.unwrap(), None);

    state
        .queue
        .publish("flaky.work", &json!({}), "")
        .await
        .unwrap();
    // Queued and immediately claimable.
    assert_eq!(state.queue.next_due().await.unwrap(), Some(0));

    let subscriber = subscriber(&state, &db.url);
    let claimed = subscriber.queue.claim("w1").await.unwrap();
    // Claimed, so nothing is *pending* — the loop has no reason to hurry back.
    assert_eq!(state.queue.next_due().await.unwrap(), None);

    crate::queues::handle_for_test(&subscriber, claimed[0].clone()).await;

    // Failed, and scheduled for the backoff rather than the poll interval.
    let due = state
        .queue
        .next_due()
        .await
        .unwrap()
        .expect("a retry is due");
    assert!(
        (18..=21).contains(&due),
        "expected the 20s backoff, got {due}s"
    );
}

/// The dashboard's whole view of the queue is `GET <base>/queue_message` under
/// the built-in's `role:admin` policy, so that request has to work — for an
/// admin, and for nobody else.
#[ntex::test]
async fn an_admin_can_list_the_queue_and_an_ordinary_member_cannot() {
    let db = TempDatabase::create("queue-admin").await;
    let root = temp_dir("queue-admin");
    write_files(&root, &[("main.toml", &main_toml(&db.url, ""))]);
    let state = load_state(&root).await;

    state
        .queue
        .publish("audit.me", &json!({ "secret": "payload" }), "")
        .await
        .unwrap();

    let (admin_token, admin_org) = member_with_role(&state, "admin@example.test", "admin").await;
    let (member_token, member_org) = member_with_role(&state, "bob@example.test", "member").await;
    let app = init_http_app!(state);

    let listed = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/queue_message")
            .header("authorization", format!("Bearer {admin_token}"))
            .header("x-organization", admin_org.as_str())
            .to_request(),
    )
    .await;
    let status = listed.status().as_u16();
    let body = read_json(listed).await;
    assert_eq!(status, 200, "an admin cannot see the queue: {body}");
    assert_eq!(body[0]["topic"], "audit.me");

    // A message payload is arbitrary app data and routinely personal, so the
    // rest of the organisation does not get to read the ledger.
    let refused = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/api/queue_message")
            .header("authorization", format!("Bearer {member_token}"))
            .header("x-organization", member_org.as_str())
            .to_request(),
    )
    .await;
    assert_eq!(refused.status().as_u16(), 403);
}

/// Build the subscriber the worker loop uses, over this test's state.
fn subscriber(state: &AppState, database_url: &str) -> crate::queues::Subscriber {
    crate::queues::Subscriber {
        db: state.db.clone(),
        queue: state.queue.clone(),
        functions: Arc::clone(&state.functions),
        mailer: state.mailer.clone(),
        cache: state.cache.clone(),
        payments: state.payments.clone(),
        ai: state.ai.clone(),
        database_url: database_url.to_string(),
        worker: "test-worker".to_string(),
    }
}

/// The queue is built from the app's own resource declaration, so an app that
/// renames the table keeps working.
#[ntex::test]
async fn the_queue_follows_the_table_the_app_declares() {
    let db = TempDatabase::create("queue-table").await;
    let root = temp_dir("queue-table");
    write_files(
        &root,
        &[
            ("main.toml", &main_toml(&db.url, "")),
            (
                "models/queue_message.toml",
                r#"
[resource]
name = "queue_message"
table = "my_own_queue"
scope = "global"
timestamps = true

[permissions]
list = "private"
read = "private"
create = "private"
update = "private"
delete = "private"

[fields.topic]
type = "string"
required = true

[fields.subscriber]
type = "string"

[fields.status]
type = "string"
required = true
default = "pending"

[fields.payload]
type = "json"
required = true

[fields.attempts]
type = "integer"
required = true
default = 0

[fields.available_at]
type = "timestamp"
required = true

[fields.claimed_at]
type = "timestamp"

[fields.claimed_by]
type = "string"

[fields.processed_at]
type = "timestamp"

[fields.error]
type = "text"

[fields.published_by]
type = "string"
"#,
            ),
        ],
    );

    let app = App::load(&root).unwrap();
    let conn = Db::connect(&app.config.database.resolved_url(), 4)
        .await
        .unwrap();
    apiplant_db::migrate(conn.connection(), &app).await.unwrap();
    let queue = Queue::new(&conn, &app);
    queue
        .publish("a.topic", &json!({ "x": 1 }), "")
        .await
        .unwrap();

    let found = conn
        .raw_json("SELECT topic FROM my_own_queue", &[])
        .await
        .unwrap();
    assert_eq!(found[0]["topic"], "a.topic");
}
