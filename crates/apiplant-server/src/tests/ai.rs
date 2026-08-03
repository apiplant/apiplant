//! The assistant's endpoints: that they exist only where a provider does, and
//! that they are not open to the world by default.
//!
//! Nothing here talks to a model. What a completion *contains* is the
//! provider's business and is covered by `apiplant-ai`'s own tests; what
//! matters at this layer is who gets through the door.

use super::*;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// An app whose `main.toml` has whatever `[ai]` section the test wants.
async fn app_with(label: &str, ai_section: &str) -> (AppState, std::path::PathBuf, TempDatabase) {
    let db = TempDatabase::create(label).await;
    let root = temp_dir(label);
    write_files(
        &root,
        &[(
            "main.toml",
            &format!(
                "\n[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n{ai_section}",
                db.url
            ),
        )],
    );
    let state = load_state(&root).await;
    (state, root, db)
}

async fn app_with_files(
    label: &str,
    ai_section: &str,
    files: &[(&str, &str)],
) -> (AppState, std::path::PathBuf, TempDatabase) {
    let db = TempDatabase::create(label).await;
    let root = temp_dir(label);
    let mut all = vec![(
        "main.toml",
        format!(
            "\n[server]\nbase_path = \"/api\"\n\n[database]\nurl = \"{}\"\n{ai_section}",
            db.url
        ),
    )];
    all.extend(
        files
            .iter()
            .map(|(path, contents)| (*path, (*contents).to_string())),
    );
    let refs = all
        .iter()
        .map(|(path, contents)| (*path, contents.as_str()))
        .collect::<Vec<_>>();
    write_files(&root, &refs);
    let state = load_state(&root).await;
    (state, root, db)
}

async fn mock_ai_server() -> (String, tokio::task::JoinHandle<()>) {
    mock_ai_server_with_requests(2, None).await
}

async fn mock_ai_server_with_requests(
    expected_requests: usize,
    seen_requests: Option<Arc<Mutex<Vec<Value>>>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = Vec::new();
            let mut header_end = None;
            loop {
                let mut chunk = [0_u8; 1024];
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
                if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    header_end = Some(pos + 4);
                    break;
                }
            }
            let header_end = header_end.expect("request headers");
            let headers = String::from_utf8(buffer[..header_end].to_vec()).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length: ")
                        .or_else(|| line.strip_prefix("content-length: "))
                })
                .and_then(|value| value.trim().parse::<usize>().ok())
                .unwrap_or_default();
            while buffer.len() < header_end + content_length {
                let mut chunk = vec![0_u8; header_end + content_length - buffer.len()];
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            let body: Value =
                serde_json::from_slice(&buffer[header_end..header_end + content_length]).unwrap();
            if let Some(seen) = &seen_requests {
                seen.lock().unwrap().push(body.clone());
            }
            let is_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
            let system = body
                .get("system")
                .and_then(Value::as_str)
                .or_else(|| {
                    body.get("messages")
                        .and_then(Value::as_array)
                        .and_then(|messages| messages.first())
                        .filter(|message| {
                            message.get("role").and_then(Value::as_str) == Some("system")
                        })
                        .and_then(|message| message.get("content"))
                        .and_then(Value::as_str)
                })
                .unwrap_or_default();

            let response_body = if system.contains("backend-only rolling conversation summaries") {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "The user greeted the coach and the coach greeted them back."
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 18, "completion_tokens": 11 }
                })
                .to_string()
            } else if system.contains("emit-reasoning") {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Visible answer.",
                            "reasoning_content": "Private reasoning."
                        },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 12, "completion_tokens": 3 }
                })
                .to_string()
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "Hello back." },
                        "finish_reason": "stop"
                    }],
                    "usage": { "prompt_tokens": 12, "completion_tokens": 3 }
                })
                .to_string()
            };
            let content_type = if is_stream {
                "text/event-stream"
            } else {
                "application/json"
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    (format!("http://{addr}"), handle)
}

/// An app with no `[ai]` section has no assistant endpoints at all — the same
/// answer the mailbox flows and `/billing` give when unconfigured.
#[ntex::test]
async fn without_a_provider_there_are_no_ai_endpoints() {
    let (state, root, db) = app_with("noai", "").await;
    assert!(!state.ai_enabled());
    let app = init_http_app!(state);

    // Neither path is served. What answers instead is the generic CRUD scope,
    // which the paths fall through to and which has no `ai` resource — so the
    // assertion is that nothing *succeeds*, not which 4xx the fallthrough
    // happens to produce.
    for request in [
        test::TestRequest::get().uri("/api/ai/config").to_request(),
        req_json("POST", "/api/ai/chat", json!({"messages":[]})),
    ] {
        let status = test::call_service(&app, request).await.status().as_u16();
        assert!(
            (400..500).contains(&status),
            "an app with no assistant answered {status}"
        );
    }

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// Configured, the app describes its assistant to a front end — and asks a
/// caller who they are before spending anything on their behalf.
#[ntex::test]
async fn a_configured_assistant_is_described_publicly_and_answers_nobody_anonymous() {
    let (state, root, db) = app_with(
        "withai",
        "\n[ai]\nprovider = \"custom\"\nendpoint = \"http://localhost:8080\"\nmodel = \"local\"\n",
    )
    .await;
    assert!(state.ai_enabled());
    let app = init_http_app!(state);

    // The config is public: a page that knows the access level can show a
    // sign-in prompt rather than a chat box that would answer 401.
    let response = test::call_service(
        &app,
        test::TestRequest::get().uri("/api/ai/config").to_request(),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let config = read_json(response).await;
    assert_eq!(config["provider"], "custom");
    assert_eq!(config["model"], "local");
    assert_eq!(config["access"], "authenticated");
    // The one thing that must never be in it.
    assert!(config.get("api_key").is_none());

    // `[ai] access` defaults to `authenticated`, so an anonymous caller is
    // turned away before the request reaches any provider.
    assert_eq!(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/ai/chat",
                json!({"messages":[{"role":"user","content":"hi"}]})
            )
        )
        .await
        .status()
        .as_u16(),
        401
    );

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// `access = "public"` is a decision an app writes down, and it is honoured —
/// the request gets as far as the provider, which in a test is not there.
#[ntex::test]
async fn a_public_assistant_lets_an_anonymous_caller_reach_the_provider() {
    let (state, root, db) = app_with(
        "publicai",
        // Port 1 answers nothing, which is what makes this a transport failure
        // rather than an authorisation one — the distinction being tested.
        "\n[ai]\nprovider = \"custom\"\nendpoint = \"http://127.0.0.1:1\"\naccess = \"public\"\ntimeout_secs = 2\n",
    )
    .await;
    let app = init_http_app!(state);

    let response = test::call_service(
        &app,
        req_json(
            "POST",
            "/api/ai/chat",
            json!({"messages":[{"role":"user","content":"hi"}]}),
        ),
    )
    .await;
    // Not 401 and not 500: the app is fine, the thing behind it is not.
    assert_eq!(response.status().as_u16(), 502);

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

/// A configured agent is discoverable and, when it stores history, creates a
/// thread plus the caller's message before the provider is even reached.
#[ntex::test]
async fn a_stored_agent_persists_the_thread_even_if_the_provider_refuses() {
    let (state, root, db) = app_with_files(
        "agent",
        "\n[ai]\nprovider = \"custom\"\nendpoint = \"http://127.0.0.1:1\"\nmodel = \"local\"\ntimeout_secs = 2\n",
        &[(
            "agents/coach.toml",
            r#"
[agent]
name = "coach"
description = "A stored coach."
system = "Be concise."
storage.enabled = true

[permissions]
chat = "authenticated"
history = "owner"
"#,
        )],
    )
    .await;
    let app = init_http_app!(state);

    let registration = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ann@example.test","password":"hunter2"}),
            ),
        )
        .await,
    )
    .await;
    let token = registration["token"].as_str().unwrap().to_string();

    let config = read_json(
        test::call_service(
            &app,
            test::TestRequest::get().uri("/api/ai/config").to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(config["agents"][0]["name"], "coach");
    assert_eq!(config["agents"][0]["storage"], true);

    let response = test::call_service(
        &app,
        bearer(
            test::TestRequest::post()
                .uri("/api/ai/agents/coach/chat")
                .header(CONTENT_TYPE, "application/json")
                .set_payload(json!({"message":"hello there"}).to_string()),
            &token,
        )
        .to_request(),
    )
    .await;
    assert_eq!(response.status().as_u16(), 502);

    let threads = read_json(
        test::call_service(
            &app,
            bearer(test::TestRequest::get().uri("/api/ai_coach_thread"), &token).to_request(),
        )
        .await,
    )
    .await;
    let threads = threads.as_array().unwrap();
    assert_eq!(threads.len(), 1);

    let messages = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::get().uri("/api/ai_coach_message"),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    let messages = messages.as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], "hello there");

    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn a_stored_agent_refreshes_and_persists_hidden_thread_summaries() {
    let (endpoint, server) = mock_ai_server().await;
    let (state, root, db) = app_with_files(
        "agent-summary",
        &format!(
            "\n[ai]\nprovider = \"custom\"\nendpoint = \"{endpoint}\"\nmodel = \"local\"\ntimeout_secs = 2\n"
        ),
        &[(
            "agents/coach.toml",
            r#"
[agent]
name = "coach"
description = "A stored coach."
system = "Be concise."
storage.enabled = true
storage.summary_after_characters = 20

[permissions]
chat = "authenticated"
history = "owner"
"#,
        )],
    )
    .await;
    let app = init_http_app!(state.clone());

    let registration = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ann@example.test","password":"hunter2"}),
            ),
        )
        .await,
    )
    .await;
    let token = registration["token"].as_str().unwrap().to_string();

    let response = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/ai/agents/coach/chat")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"message":"hello there","stream":false}).to_string()),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(response["text"], "Hello back.");

    let threads = read_json(
        test::call_service(
            &app,
            bearer(test::TestRequest::get().uri("/api/ai_coach_thread"), &token).to_request(),
        )
        .await,
    )
    .await;
    let threads = threads.as_array().unwrap();
    assert_eq!(threads.len(), 1);
    assert!(threads[0].get("summary").is_none());
    assert!(threads[0].get("summary_message_count").is_none());

    let table = state.table("ai_coach_thread").unwrap();
    let rows = state
        .db
        .raw_json(
            &format!(
                "SELECT summary, summary_message_count, summary_characters \
                 FROM {table}"
            ),
            &[],
        )
        .await
        .unwrap();
    let row = rows.as_array().and_then(|rows| rows.first()).unwrap();
    assert_eq!(
        row["summary"],
        "The user greeted the coach and the coach greeted them back."
    );
    assert_eq!(row["summary_message_count"], 2);
    assert!(row["summary_characters"].as_i64().unwrap() > 0);

    server.await.unwrap();
    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn a_follow_up_request_uses_the_stored_summary_instead_of_replaying_full_history() {
    let seen_requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let (endpoint, server) = mock_ai_server_with_requests(3, Some(seen_requests.clone())).await;
    let (state, root, db) = app_with_files(
        "agent-summary-follow-up",
        &format!(
            "\n[ai]\nprovider = \"custom\"\nendpoint = \"{endpoint}\"\nmodel = \"local\"\ntimeout_secs = 2\n"
        ),
        &[(
            "agents/coach.toml",
            r#"
[agent]
name = "coach"
description = "A stored coach."
system = "Be concise."
storage.enabled = true
storage.summary_after_characters = 80

[permissions]
chat = "authenticated"
history = "owner"
"#,
        )],
    )
    .await;
    let app = init_http_app!(state.clone());

    let registration = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ann@example.test","password":"hunter2"}),
            ),
        )
        .await,
    )
    .await;
    let token = registration["token"].as_str().unwrap().to_string();

    let first =
        "hello there, this is a deliberately long first message so a stored summary is created";
    let second = "hi again";

    let first_response = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/ai/agents/coach/chat")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"message":first,"stream":false}).to_string()),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(first_response["text"], "Hello back.");

    let second_response = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/ai/agents/coach/chat")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"message":second,"thread_id":first_response["thread_id"],"stream":false}).to_string()),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(second_response["text"], "Hello back.");

    let requests = seen_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    let second_main = &requests[2];
    let messages = second_main["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], second);
    let system = messages[0]["content"].as_str().unwrap();
    assert!(system.contains("Conversation summary so far:"));
    assert!(system.contains("The user greeted the coach and the coach greeted them back."));
    assert!(!system.contains(first));

    server.await.unwrap();
    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn agent_reasoning_is_hidden_by_default() {
    let (endpoint, server) = mock_ai_server_with_requests(1, None).await;
    let (state, root, db) = app_with_files(
        "agent-reasoning-default",
        &format!(
            "\n[ai]\nprovider = \"custom\"\nendpoint = \"{endpoint}\"\nmodel = \"local\"\ntimeout_secs = 2\n"
        ),
        &[(
            "agents/coach.toml",
            r#"
[agent]
name = "coach"
description = "A stored coach."
system = "emit-reasoning"
storage.enabled = true

[permissions]
chat = "authenticated"
history = "owner"
"#,
        )],
    )
    .await;
    let app = init_http_app!(state.clone());

    let registration = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ann@example.test","password":"hunter2"}),
            ),
        )
        .await,
    )
    .await;
    let token = registration["token"].as_str().unwrap().to_string();

    let response = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/ai/agents/coach/chat")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"message":"hello there","stream":false}).to_string()),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(response["text"], "Visible answer.");
    assert!(response.get("reasoning").is_none());

    let table = state.table("ai_coach_message").unwrap();
    let rows = state
        .db
        .raw_json(
            &format!("SELECT role, content, reasoning FROM {table} WHERE role = 'assistant'"),
            &[],
        )
        .await
        .unwrap();
    let row = rows.as_array().and_then(|rows| rows.first()).unwrap();
    assert_eq!(row["content"], "Visible answer.");
    assert!(row.get("reasoning").is_none() || row["reasoning"].is_null());

    server.await.unwrap();
    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}

#[ntex::test]
async fn agent_reasoning_can_be_enabled_per_agent() {
    let (endpoint, server) = mock_ai_server_with_requests(1, None).await;
    let (state, root, db) = app_with_files(
        "agent-reasoning-enabled",
        &format!(
            "\n[ai]\nprovider = \"custom\"\nendpoint = \"{endpoint}\"\nmodel = \"local\"\ntimeout_secs = 2\n"
        ),
        &[(
            "agents/coach.toml",
            r#"
[agent]
name = "coach"
description = "A stored coach."
system = "emit-reasoning"
storage.enabled = true

[ai]
reasoning = true

[permissions]
chat = "authenticated"
history = "owner"
"#,
        )],
    )
    .await;
    let app = init_http_app!(state.clone());

    let registration = read_json(
        test::call_service(
            &app,
            req_json(
                "POST",
                "/api/auth/register",
                json!({"email":"ann@example.test","password":"hunter2"}),
            ),
        )
        .await,
    )
    .await;
    let token = registration["token"].as_str().unwrap().to_string();

    let response = read_json(
        test::call_service(
            &app,
            bearer(
                test::TestRequest::post()
                    .uri("/api/ai/agents/coach/chat")
                    .header(CONTENT_TYPE, "application/json")
                    .set_payload(json!({"message":"hello there","stream":false}).to_string()),
                &token,
            )
            .to_request(),
        )
        .await,
    )
    .await;
    assert_eq!(response["text"], "Visible answer.");
    assert_eq!(response["reasoning"], "Private reasoning.");

    let table = state.table("ai_coach_message").unwrap();
    let rows = state
        .db
        .raw_json(
            &format!("SELECT role, content, reasoning FROM {table} WHERE role = 'assistant'"),
            &[],
        )
        .await
        .unwrap();
    let row = rows.as_array().and_then(|rows| rows.first()).unwrap();
    assert_eq!(row["content"], "Visible answer.");
    assert_eq!(row["reasoning"], "Private reasoning.");

    server.await.unwrap();
    fs::remove_dir_all(root).unwrap();
    db.cleanup().await;
}
