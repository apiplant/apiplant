//! Configured chat agents, loaded from `agents/*.toml`.
//!
//! One agent is one named chat surface with a fixed system prompt and access
//! policy. A stored agent also gets two generated resources — threads and
//! messages — and this route writes them as it talks to the model.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

// The budgets, prompts and sanitiser a rolling summary needs live in
// `apiplant_ai::summary`; this module only maps an agent's configuration onto
// them and persists what comes back.
use apiplant_ai::summary::{self, Summary, SummaryLimits};
use apiplant_ai::{ChatReply, ChatRequest, Done, Event, Message, Role, ToolCall, ToolDefinition};
use apiplant_auth::{Principal, ADMIN_ROLE};
use apiplant_core::{Access, Agent, Scope};
use apiplant_db::Filter;
use chrono::Utc;
use futures_util::StreamExt;
use ntex::util::Bytes;
use ntex::web::types::{Json, Path, State};
use ntex::web::{HttpRequest, HttpResponse};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::response::{db_error, error, ok};
use crate::sse;
use crate::state::AppState;

/// A posted turn for a configured agent.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Body {
    /// The new user turn to append.
    message: String,
    /// Prior turns for a non-stored conversation. Stored agents keep their own
    /// history and reject this field.
    messages: Vec<Message>,
    /// Continue a stored thread. Omit to start one.
    thread_id: Option<Uuid>,
    /// Human title for a new stored thread. Defaults to the first message,
    /// trimmed down to one short line.
    title: Option<String>,
    /// `true` (the default) streams the answer as SSE; `false` answers with one
    /// JSON document once it is complete.
    stream: Option<bool>,
}

struct Caller {
    principal: Option<Principal>,
    active_org: Option<Uuid>,
}

/// A reasoning model can spend its whole budget thinking and answer with
/// nothing at all. Say so rather than closing the turn on silence, which used
/// to leave the user's message stored with no reply and no explanation.
const EMPTY_ANSWER: &str =
    "the model returned no answer; try again, or raise this agent's max_tokens";
/// Emitted when the model spent its whole budget thinking. The trace is on the
/// message already, behind the toggle, so the turn needs an explanation rather
/// than a copy of the thinking.
const TRUNCATED_ANSWER: &str =
    "the model ran out of tokens while thinking and never answered; its reasoning is \
below — raise this agent's max_tokens";

#[derive(Debug, Clone, Default)]
struct ThreadHistory {
    thread_id: Uuid,
    summary: Option<String>,
    summary_message_count: usize,
    recent_messages: Vec<Message>,
}

/// `POST <base>/ai/agents/{name}/chat`.
pub async fn chat(
    req: HttpRequest,
    state: State<AppState>,
    path: Path<String>,
    body: Json<Body>,
) -> HttpResponse {
    let name = path.into_inner();
    let Some(agent) = state.app.agents.get(&name).cloned() else {
        return error(404, format!("unknown ai agent `{name}`"));
    };
    let Some(ai) = state
        .agent_ais
        .get(&name)
        .cloned()
        .or_else(|| state.ai.clone())
    else {
        return error(404, "this app has no ai assistant");
    };

    let caller = match admit(&state, &req, &agent).await {
        Ok(caller) => caller,
        Err(response) => return response,
    };
    let body = body.into_inner();
    let prepared = match prepare(&state, &agent, &caller, &ai, body).await {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };

    if prepared.stream == Some(false) {
        return match chat_with_tools(&state, &prepared, &ai).await {
            Ok(reply) => {
                let warning = truncated_answer(&reply);
                if let Err(response) =
                    persist_assistant_and_summary(&state, &prepared, &ai, &reply).await
                {
                    return response;
                }
                // An empty answer is only a failure when nothing came back at
                // all. A truncated one still carries the thinking, and
                // `warning` says why the text is missing.
                if reply.text.trim().is_empty() && warning.is_none() {
                    return error(502, EMPTY_ANSWER);
                }
                let mut value = reply_json(reply, prepared.thread_id);
                if let Some(warning) = warning {
                    if let Some(map) = value.as_object_mut() {
                        map.insert("warning".into(), json!(warning));
                    }
                }
                ok(&value)
            }
            Err(e) => refused(e),
        };
    }

    if !prepared.agent.tools.is_empty() {
        return match chat_with_tools(&state, &prepared, &ai).await {
            Ok(reply) => {
                let warning = truncated_answer(&reply);
                if let Err(response) =
                    persist_assistant_and_summary(&state, &prepared, &ai, &reply).await
                {
                    return response;
                }
                let mut response = HttpResponse::Ok();
                sse::headers(&mut response);
                let thread_id = prepared.thread_id;
                let mut frames = Vec::new();
                // A tool-using turn is not streamed from the provider, so the
                // reasoning has had no chance to arrive as it was produced.
                // Send it now, or the toggle on this message would be empty
                // while the stored message has the trace.
                if !reply.reasoning.trim().is_empty() {
                    frames.push(Ok::<Bytes, sse::Never>(sse::event(
                        "reasoning",
                        &json!({ "text": reply.reasoning }),
                    )));
                }
                frames.push(Ok(sse::delta(&reply.text)));
                match warning {
                    Some(warning) => {
                        frames.push(Ok(sse::event("warning", &json!({ "text": warning }))))
                    }
                    None if reply.text.trim().is_empty() => {
                        frames.push(Ok(sse::failure(EMPTY_ANSWER)))
                    }
                    None => {}
                }
                frames.push(Ok(sse::done(&done_json(reply.done, thread_id))));
                response.streaming(Box::pin(futures_util::stream::iter(frames)))
            }
            Err(e) => refused(e),
        };
    }

    let stream = match ai.stream(&prepared.request).await {
        Ok(stream) => stream,
        Err(e) => return refused(e),
    };

    let text = Rc::new(RefCell::new(String::new()));
    let reasoning = Rc::new(RefCell::new(String::new()));
    let done = Rc::new(RefCell::new(Done::default()));
    let reply_model = prepared
        .request
        .model
        .clone()
        .unwrap_or_else(|| ai.model().to_string());
    let ended = Rc::new(Cell::new(false));
    let failed = Rc::new(Cell::new(false));

    let text_events = text.clone();
    let reasoning_events = reasoning.clone();
    let done_events = done.clone();
    let ended_events = ended.clone();
    let failed_events = failed.clone();

    let events = stream
        .map(move |event| -> Result<Bytes, sse::Never> {
            Ok(match event {
                Ok(Event::Delta(chunk)) => {
                    text_events.borrow_mut().push_str(&chunk);
                    sse::delta(&chunk)
                }
                Ok(Event::Reasoning(chunk)) => {
                    reasoning_events.borrow_mut().push_str(&chunk);
                    sse::event("reasoning", &json!({ "text": chunk }))
                }
                Ok(Event::Done(agent_done)) => {
                    *done_events.borrow_mut() = agent_done;
                    ended_events.set(true);
                    Bytes::new()
                }
                Err(e) => {
                    failed_events.set(true);
                    sse::failure(&e.to_string())
                }
            })
        })
        .chain(futures_util::stream::once(async move {
            let reply = ChatReply {
                text: text.borrow().clone(),
                reasoning: reasoning.borrow().clone(),
                provider: ai.provider().as_str().to_string(),
                model: reply_model,
                done: done.borrow().clone(),
                tool_calls: Vec::new(),
            };
            let warning = (!failed.get()).then(|| truncated_answer(&reply)).flatten();
            let empty_answer = !failed.get() && warning.is_none() && reply.text.trim().is_empty();
            let frame = match persist_assistant(&state, &prepared, &reply).await {
                Ok(()) => {
                    maybe_refresh_thread_summary(&state, &prepared, &ai).await;
                    let done = sse::done(&done_json(reply.done.clone(), prepared.thread_id));
                    if let Some(warning) = warning {
                        // No `delta`: the thinking already went out as
                        // `reasoning` events, and repeating it as the answer is
                        // how the thinking ends up being the reply.
                        let mut bytes = sse::event("warning", &json!({ "text": warning })).to_vec();
                        bytes.extend_from_slice(done.as_ref());
                        return Ok::<Bytes, sse::Never>(Bytes::from(bytes));
                    }
                    if empty_answer {
                        // A reasoning model can spend the whole budget thinking
                        // and answer with nothing. Say so rather than closing
                        // the stream on silence.
                        let mut bytes = sse::failure(EMPTY_ANSWER).to_vec();
                        bytes.extend_from_slice(done.as_ref());
                        Bytes::from(bytes)
                    } else {
                        done
                    }
                }
                Err(_) => {
                    let mut bytes = sse::failure("agent history could not be saved").to_vec();
                    bytes.extend_from_slice(
                        sse::done(&done_json(reply.done.clone(), prepared.thread_id)).as_ref(),
                    );
                    Bytes::from(bytes)
                }
            };
            Ok::<Bytes, sse::Never>(frame)
        }));

    let mut response = HttpResponse::Ok();
    sse::headers(&mut response);
    response.streaming(Box::pin(events))
}

struct Prepared {
    request: ChatRequest,
    thread_id: Option<Uuid>,
    owner_id: Option<Uuid>,
    active_org: Option<Uuid>,
    stream: Option<bool>,
    agent: Agent,
}

async fn prepare(
    state: &State<AppState>,
    agent: &Agent,
    caller: &Caller,
    ai: &apiplant_ai::Ai,
    body: Body,
) -> Result<Prepared, HttpResponse> {
    let message = body.message.trim().to_string();
    if message.is_empty() {
        return Err(error(400, "ask a message"));
    }
    if body.messages.iter().any(|entry| entry.role == Role::System) {
        return Err(error(
            400,
            "agent conversations cannot include caller-supplied system messages",
        ));
    }

    let history = if agent.meta.storage.enabled {
        if !body.messages.is_empty() {
            return Err(error(
                400,
                "stored agents keep their own history; use `thread_id` instead of `messages`",
            ));
        }
        if let Some(thread_id) = body.thread_id {
            let mut history = load_thread_history(state, agent, caller, thread_id).await?;
            let pending = [Message::user(message.clone())];
            if needs_summary_refresh(
                agent,
                &summary::with_pending_messages(&history.recent_messages, &pending),
            ) {
                match refresh_thread_summary_until_fit(state, agent, caller, ai, &history, &pending)
                    .await
                {
                    Ok(updated) => history = updated,
                    Err(response) => {
                        tracing::warn!(
                            agent = %agent.meta.name,
                            thread_id = %thread_id,
                            status = response.status().as_u16(),
                            "agent thread summary refresh failed before answering"
                        );
                    }
                }
            }
            Some(history)
        } else {
            None
        }
    } else {
        if body.thread_id.is_some() {
            return Err(error(400, "this agent does not persist history"));
        }
        None
    };
    let mut messages = history
        .as_ref()
        .map(|history| summary::drop_orphan_tool_messages(&history.recent_messages))
        .unwrap_or_else(|| body.messages.clone());
    messages.push(Message::user(message.clone()));

    let owner_id = caller.principal.as_ref().map(|principal| principal.user_id);
    let thread_id = if agent.meta.storage.enabled {
        Some(
            ensure_thread(
                state,
                agent,
                caller,
                body.thread_id,
                body.title.as_deref(),
                &message,
            )
            .await?,
        )
    } else {
        None
    };
    if let Some(thread_id) = thread_id {
        insert_message(
            state,
            agent,
            caller.active_org,
            owner_id.expect("stored agents require an owner"),
            thread_id,
            "user",
            &message,
            None,
        )
        .await?;
    }

    let mut system = render_system_prompt(
        &agent.meta.system,
        &agent_context(agent, caller, owner_id, thread_id),
    );
    if let Some(summary) = history
        .as_ref()
        .and_then(|history| history.summary.as_deref())
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        if !system.trim().is_empty() {
            system.push_str("\n\n");
        }
        system.push_str("Conversation summary so far:\n");
        system.push_str(summary);
        system.push_str(
            "\n\nUse that summary for earlier context. The message list only contains the recent tail.",
        );
    }

    let mut request = ChatRequest {
        messages,
        model: agent.meta.model.clone(),
        system: (!system.trim().is_empty()).then_some(system),
        temperature: agent.meta.temperature,
        max_tokens: agent.meta.max_tokens,
        tools: agent
            .tools
            .iter()
            .map(|tool| ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect(),
    };
    // Let `[ai]` decide what the agent file left blank.
    if request
        .model
        .as_deref()
        .is_some_and(|model| model.trim().is_empty())
    {
        request.model = None;
    }

    Ok(Prepared {
        request,
        thread_id,
        owner_id,
        active_org: caller.active_org,
        stream: body.stream,
        agent: agent.clone(),
    })
}

fn agent_context(
    agent: &Agent,
    caller: &Caller,
    owner_id: Option<Uuid>,
    thread_id: Option<Uuid>,
) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    values.insert("agent_name".to_string(), agent.meta.name.clone());
    values.insert(
        "agent_description".to_string(),
        agent.meta.description.clone(),
    );
    values.insert(
        "authenticated".to_string(),
        caller.principal.is_some().to_string(),
    );
    values.insert(
        "user_id".to_string(),
        owner_id.map(|id| id.to_string()).unwrap_or_default(),
    );
    values.insert(
        "principal_id".to_string(),
        owner_id.map(|id| id.to_string()).unwrap_or_default(),
    );
    values.insert(
        "organization_id".to_string(),
        caller
            .active_org
            .map(|id| id.to_string())
            .unwrap_or_default(),
    );
    values.insert(
        "thread_id".to_string(),
        thread_id.map(|id| id.to_string()).unwrap_or_default(),
    );
    values
}

fn render_system_prompt(template: &str, context: &BTreeMap<String, String>) -> String {
    let mut rendered = template.to_string();
    for (key, value) in context {
        rendered = rendered.replace(&format!("{{{{{key}}}}}"), value);
        rendered = rendered.replace(&format!("{{{{ {key} }}}}"), value);
    }
    rendered
}

async fn admit(
    state: &State<AppState>,
    req: &HttpRequest,
    agent: &Agent,
) -> Result<Caller, HttpResponse> {
    let principal = state.resolve_principal(req).await;

    if agent.meta.scope == Scope::Organization {
        if matches!(agent.permissions.chat.level, Access::Private) {
            return Err(error(
                404,
                format!("unknown ai agent `{}`", agent.meta.name),
            ));
        }
        let Some(principal) = principal else {
            return Err(error(401, "authentication required"));
        };
        let active_org = state.active_org(req, &Some(principal.clone()));
        let Some(org) = active_org else {
            return Err(error(
                403,
                "select an organisation with the X-Organization header",
            ));
        };
        let Some(membership) = principal.membership(org) else {
            return Err(error(403, "you are not a member of this organisation"));
        };
        // A class-qualified agent answers only inside organisations of that
        // class, whatever the level attached to it.
        if let Some(class) = agent.permissions.chat.org_class.as_deref() {
            if !membership.is_class(class) {
                return Err(error(
                    403,
                    format!("requires an organisation of class `{class}`"),
                ));
            }
        }
        if let Access::Role(role) = &agent.permissions.chat.level {
            if !membership.has_role(role) {
                return Err(error(
                    403,
                    format!("requires the `{role}` role in this organisation"),
                ));
            }
        }
        return Ok(Caller {
            principal: Some(principal),
            active_org: Some(org),
        });
    }

    // Outside an organisation-scoped agent the same qualifier means the caller
    // must have selected an organisation of the class, since there is no other
    // organisation in the request to ask about.
    if let Some(class) = agent.permissions.chat.org_class.as_deref() {
        let membership = principal.as_ref().and_then(|p| {
            state
                .active_org(req, &principal)
                .and_then(|org| p.membership(org))
                .cloned()
        });
        match membership {
            Some(m) if m.is_class(class) => {}
            Some(_) => {
                return Err(error(
                    403,
                    format!("requires an organisation of class `{class}`"),
                ))
            }
            None => {
                return Err(match principal {
                    Some(_) => error(403, "select an organisation with the X-Organization header"),
                    None => error(401, "authentication required"),
                })
            }
        }
    }

    match &agent.permissions.chat.level {
        Access::Public => Ok(Caller {
            principal,
            active_org: None,
        }),
        Access::Private => Err(error(
            404,
            format!("unknown ai agent `{}`", agent.meta.name),
        )),
        Access::Authenticated => match principal {
            Some(principal) => Ok(Caller {
                active_org: state.active_org(req, &Some(principal.clone())),
                principal: Some(principal),
            }),
            None => Err(error(401, "authentication required")),
        },
        Access::Member => match principal {
            Some(principal) if state.active_org(req, &Some(principal.clone())).is_some() => {
                Ok(Caller {
                    active_org: state.active_org(req, &Some(principal.clone())),
                    principal: Some(principal),
                })
            }
            Some(_) => Err(error(
                403,
                "select an organisation with the X-Organization header",
            )),
            None => Err(error(401, "authentication required")),
        },
        Access::Role(role) => match principal {
            Some(principal) => {
                let active_org = state.active_org(req, &Some(principal.clone()));
                let Some(org) = active_org else {
                    return Err(error(
                        403,
                        "select an organisation with the X-Organization header",
                    ));
                };
                if !principal.has_role_in(org, role) {
                    return Err(error(
                        403,
                        format!("requires the `{role}` role in this organisation"),
                    ));
                }
                Ok(Caller {
                    principal: Some(principal),
                    active_org,
                })
            }
            None => Err(error(401, "authentication required")),
        },
        Access::Owner => Err(error(500, "internal error")),
    }
}

async fn chat_with_tools(
    state: &State<AppState>,
    prepared: &Prepared,
    ai: &apiplant_ai::Ai,
) -> Result<ChatReply, apiplant_ai::AiError> {
    let mut request = prepared.request.clone();
    for _ in 0..8 {
        let reply = ai.complete(&request).await?;
        if reply.tool_calls.is_empty() {
            return Ok(reply);
        }
        let calls = reply.tool_calls.clone();
        persist_tool_calls(state, prepared, &calls)
            .await
            .map_err(|response| {
                apiplant_ai::AiError::Transport(format!(
                    "agent history could not be saved: {}",
                    response.status()
                ))
            })?;
        request
            .messages
            .push(Message::assistant_tool_calls(calls.clone()));
        for call in calls {
            let output = invoke_tool(state, prepared, &call).await?;
            persist_tool_result(state, prepared, &call, &output)
                .await
                .map_err(|response| {
                    apiplant_ai::AiError::Transport(format!(
                        "agent history could not be saved: {}",
                        response.status()
                    ))
                })?;
            request.messages.push(Message::tool_result(call.id, output));
        }
    }
    Err(apiplant_ai::AiError::Request(
        "agent exceeded the tool-call limit".to_string(),
    ))
}

async fn invoke_tool(
    state: &State<AppState>,
    prepared: &Prepared,
    call: &ToolCall,
) -> Result<String, apiplant_ai::AiError> {
    let Some(tool) = prepared
        .agent
        .tools
        .iter()
        .find(|tool| tool.name == call.name)
    else {
        return Ok(json!({ "error": format!("unknown tool `{}`", call.name) }).to_string());
    };
    let Some(loaded) = state.functions.get(&tool.function) else {
        tracing::warn!(
            agent = %prepared.agent.meta.name,
            tool = %tool.name,
            function = %tool.function,
            "agent tool names a missing function"
        );
        return Ok(
            json!({ "error": format!("tool function `{}` is not loaded", tool.function) })
                .to_string(),
        );
    };

    let functions = state.functions.clone();
    let function = tool.function.clone();
    let config_json = loaded.config_json.clone();
    let principal_id = prepared
        .owner_id
        .map(|id| id.to_string())
        .unwrap_or_default();
    let input = call.input.to_string();
    let bridge = crate::functions::HostBridge::new(
        state.db.clone(),
        tokio::runtime::Handle::current(),
        config_json,
        principal_id,
    )
    .with_services(
        state.mailer.clone(),
        state.cache.clone(),
        state.payments.clone(),
        state.ai.clone(),
    )
    .with_email_templates(state.email_templates.clone())
    .with_queue(state.queue.clone());

    tokio::task::spawn_blocking(move || {
        let f = functions.get(&function).expect("checked above");
        f.invoke(bridge, &input)
    })
    .await
    .map_err(|_| apiplant_ai::AiError::Transport("tool function task panicked".to_string()))?
    .map_err(|message| {
        tracing::warn!(
            agent = %prepared.agent.meta.name,
            tool = %tool.name,
            function = %tool.function,
            error = %message,
            "agent tool function failed"
        );
        apiplant_ai::AiError::Request(format!("tool `{}` failed: {message}", tool.name))
    })
}

async fn ensure_thread(
    state: &State<AppState>,
    agent: &Agent,
    caller: &Caller,
    thread_id: Option<Uuid>,
    explicit_title: Option<&str>,
    message: &str,
) -> Result<Uuid, HttpResponse> {
    let owner_id = caller
        .principal
        .as_ref()
        .map(|principal| principal.user_id)
        .unwrap_or_default();
    if let Some(thread_id) = thread_id {
        return load_thread_history_row(state, agent, caller, thread_id)
            .await
            .map(|history| history.thread_id);
    }

    let resource_name = agent.thread_resource_name();
    let Some(resource) = state.app.resources.get(&resource_name) else {
        return Err(error(500, "agent history resource is missing"));
    };
    let mut data = Map::new();
    data.insert("owner_id".into(), json!(owner_id));
    let title = explicit_title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| title_from(message));
    if !title.is_empty() {
        data.insert("title".into(), json!(title));
    }
    if agent.meta.scope == Scope::Organization {
        let Some(org) = caller.active_org else {
            return Err(error(
                403,
                "select an organisation with the X-Organization header",
            ));
        };
        data.insert("organization_id".into(), json!(org));
    }
    let row = state.db.create(resource, &data).await.map_err(db_error)?;
    row.get("id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or_else(|| error(500, "created thread is missing an id"))
}

async fn load_thread_history(
    state: &State<AppState>,
    agent: &Agent,
    caller: &Caller,
    thread_id: Uuid,
) -> Result<ThreadHistory, HttpResponse> {
    let mut history = load_thread_history_row(state, agent, caller, thread_id).await?;

    let Some(table) = state.table(&agent.message_resource_name()) else {
        return Err(error(500, "agent history resource is missing"));
    };
    let sql = format!(
        "SELECT role, content, tool_call_id, tool_name, tool_input FROM {table} \
         WHERE thread_id = $1::uuid ORDER BY created_at ASC, id ASC"
    );
    let rows = state
        .db
        .raw_json(&sql, &[Value::String(thread_id.to_string())])
        .await
        .map_err(|e| {
            tracing::error!(error = %e, "failed to load agent history");
            error(500, "internal error")
        })?;
    let mut messages = Vec::new();
    for row in rows.as_array().map(Vec::as_slice).unwrap_or_default() {
        let raw_role = row.get("role").and_then(Value::as_str).unwrap_or_default();
        let role = match raw_role {
            "assistant" | "tool_call" => Role::Assistant,
            "system" => Role::System,
            "tool" | "tool_result" => Role::Tool,
            _ => Role::User,
        };
        let content = row
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if raw_role == "tool_call" {
            messages.push(Message::assistant_tool_calls(vec![ToolCall {
                id: row
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: row
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                input: row.get("tool_input").cloned().unwrap_or_else(|| json!({})),
            }]));
            continue;
        }
        messages.push(Message {
            role,
            content,
            tool_call_id: row
                .get("tool_call_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            ..Message::default()
        });
    }
    history.summary_message_count = history.summary_message_count.min(messages.len());
    history.recent_messages = messages
        .into_iter()
        .skip(history.summary_message_count)
        .collect();
    Ok(history)
}

async fn load_thread_history_row(
    state: &State<AppState>,
    agent: &Agent,
    caller: &Caller,
    thread_id: Uuid,
) -> Result<ThreadHistory, HttpResponse> {
    let Some(table) = state.table(&agent.thread_resource_name()) else {
        return Err(error(500, "agent history resource is missing"));
    };
    let principal = caller
        .principal
        .as_ref()
        .ok_or_else(|| error(401, "authentication required"))?;
    let mut sql = format!(
        "SELECT id::text AS id, summary, summary_message_count FROM {table} \
         WHERE id = $1::uuid"
    );
    let mut params = vec![Value::String(thread_id.to_string())];
    if agent.meta.scope == Scope::Organization {
        let Some(org) = caller.active_org else {
            return Err(error(
                403,
                "select an organisation with the X-Organization header",
            ));
        };
        if !principal.has_role_in(org, ADMIN_ROLE) {
            sql.push_str(" AND owner_id = $2::uuid");
            params.push(Value::String(principal.user_id.to_string()));
        }
        sql.push_str(&format!(
            " AND organization_id = ${}::uuid",
            params.len() + 1
        ));
        params.push(Value::String(org.to_string()));
    } else if let Some(org) = caller.active_org {
        if principal.is_admin_of(org) {
            let ids = state.organization_user_ids(org).await;
            if ids.is_empty() {
                return Err(error(404, "unknown thread"));
            }
            let placeholders = (0..ids.len())
                .map(|index| format!("${}::uuid", index + 2))
                .collect::<Vec<_>>()
                .join(", ");
            sql.push_str(&format!(" AND owner_id IN ({placeholders})"));
            params.extend(ids.into_iter().map(|id| Value::String(id.to_string())));
        } else {
            sql.push_str(" AND owner_id = $2::uuid");
            params.push(Value::String(principal.user_id.to_string()));
        }
    } else {
        sql.push_str(" AND owner_id = $2::uuid");
        params.push(Value::String(principal.user_id.to_string()));
    }
    if matches!(agent.permissions.history.level, Access::Private) {
        return Err(error(404, "unknown thread"));
    }
    let rows = state.db.raw_json(&sql, &params).await.map_err(|e| {
        tracing::error!(error = %e, "failed to load agent thread");
        error(500, "internal error")
    })?;
    let Some(row) = rows.as_array().and_then(|rows| rows.first()).cloned() else {
        return Err(error(404, "unknown thread"));
    };
    Ok(ThreadHistory {
        thread_id: row
            .get("id")
            .and_then(Value::as_str)
            .and_then(|value| Uuid::parse_str(value).ok())
            .ok_or_else(|| error(404, "unknown thread"))?,
        summary: row
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|summary| !summary.trim().is_empty()),
        summary_message_count: row
            .get("summary_message_count")
            .and_then(Value::as_i64)
            .unwrap_or_default()
            .max(0) as usize,
        recent_messages: Vec::new(),
    })
}

async fn persist_assistant(
    state: &State<AppState>,
    prepared: &Prepared,
    reply: &ChatReply,
) -> Result<(), HttpResponse> {
    let Some(thread_id) = prepared.thread_id else {
        return Ok(());
    };
    let Some(owner_id) = prepared.owner_id else {
        return Ok(());
    };
    if reply.text.trim().is_empty() {
        return Ok(());
    }
    insert_message(
        state,
        &prepared.agent,
        prepared.active_org,
        owner_id,
        thread_id,
        "assistant",
        &reply.text,
        Some(reply),
    )
    .await
}

async fn persist_assistant_and_summary(
    state: &State<AppState>,
    prepared: &Prepared,
    ai: &apiplant_ai::Ai,
    reply: &ChatReply,
) -> Result<(), HttpResponse> {
    persist_assistant(state, prepared, reply).await?;
    maybe_refresh_thread_summary(state, prepared, ai).await;
    Ok(())
}

async fn maybe_refresh_thread_summary(
    state: &State<AppState>,
    prepared: &Prepared,
    ai: &apiplant_ai::Ai,
) {
    let Some(thread_id) = prepared.thread_id else {
        return;
    };
    let caller = Caller {
        principal: prepared.owner_id.map(owner_principal),
        active_org: prepared.active_org,
    };
    let history = match load_thread_history(state, &prepared.agent, &caller, thread_id).await {
        Ok(history) => history,
        Err(response) => {
            tracing::warn!(
                agent = %prepared.agent.meta.name,
                thread_id = %thread_id,
                status = response.status().as_u16(),
                "agent thread summary load failed"
            );
            return;
        }
    };
    if !needs_summary_refresh(&prepared.agent, &history.recent_messages) {
        return;
    }
    if let Err(response) =
        refresh_thread_summary_until_fit(state, &prepared.agent, &caller, ai, &history, &[]).await
    {
        tracing::warn!(
            agent = %prepared.agent.meta.name,
            thread_id = %thread_id,
            status = response.status().as_u16(),
            "agent thread summary refresh failed after answering"
        );
    }
}

async fn refresh_thread_summary(
    state: &State<AppState>,
    agent: &Agent,
    caller: &Caller,
    ai: &apiplant_ai::Ai,
    history: &ThreadHistory,
    pending_messages: &[Message],
) -> Result<ThreadHistory, HttpResponse> {
    let owner_id = caller
        .principal
        .as_ref()
        .map(|principal| principal.user_id)
        .ok_or_else(|| error(500, "stored agents require an authenticated owner"))?;
    if history.recent_messages.is_empty() {
        return Ok(history.clone());
    }
    let summarize_count = summary_prefix_length(
        agent,
        &history.recent_messages,
        history.summary.is_some(),
        pending_messages,
    );
    if summarize_count == 0 {
        return Ok(history.clone());
    }
    let retained = history.recent_messages[summarize_count..].to_vec();

    let (verdict, _reply) = summary::summarize(
        ai,
        history.summary.as_deref(),
        &history.recent_messages[..summarize_count],
        summary_limits(agent),
    )
    .await
    .map_err(refused)?;
    let summary = match &verdict {
        Summary::Text(summary) => summary.as_str(),
        // Keeping the previous summary is always better than poisoning the
        // thread with a thinking trace the next turn would have to reason
        // about — or with nothing at all.
        Summary::Reasoning(_) => {
            tracing::warn!(
                agent = %agent.meta.name,
                thread_id = %history.thread_id,
                "agent thread summary looked like a reasoning trace and was discarded"
            );
            return Ok(history.clone());
        }
        Summary::Empty => {
            tracing::warn!(
                agent = %agent.meta.name,
                thread_id = %history.thread_id,
                "agent thread summary came back empty"
            );
            return Ok(history.clone());
        }
    };
    update_thread_summary(
        state,
        agent,
        caller.active_org,
        owner_id,
        history.thread_id,
        summary,
        history.summary_message_count + summarize_count,
    )
    .await?;

    Ok(ThreadHistory {
        thread_id: history.thread_id,
        summary: Some(summary.to_string()),
        summary_message_count: history.summary_message_count + summarize_count,
        recent_messages: retained,
    })
}

async fn refresh_thread_summary_until_fit(
    state: &State<AppState>,
    agent: &Agent,
    caller: &Caller,
    ai: &apiplant_ai::Ai,
    history: &ThreadHistory,
    pending_messages: &[Message],
) -> Result<ThreadHistory, HttpResponse> {
    let mut history = history.clone();
    while needs_summary_refresh(
        agent,
        &summary::with_pending_messages(&history.recent_messages, pending_messages),
    ) {
        let updated =
            refresh_thread_summary(state, agent, caller, ai, &history, pending_messages).await?;
        if updated.summary_message_count == history.summary_message_count {
            break;
        }
        history = updated;
    }
    Ok(history)
}

async fn update_thread_summary(
    state: &State<AppState>,
    agent: &Agent,
    active_org: Option<Uuid>,
    owner_id: Uuid,
    thread_id: Uuid,
    summary: &str,
    summary_message_count: usize,
) -> Result<(), HttpResponse> {
    let resource_name = agent.thread_resource_name();
    let Some(resource) = state.app.resources.get(&resource_name) else {
        return Err(error(500, "agent history resource is missing"));
    };
    let mut data = Map::new();
    let summary = summary.trim();
    data.insert("summary".into(), json!(summary));
    data.insert(
        "summary_message_count".into(),
        json!(summary_message_count as i64),
    );
    data.insert(
        "summary_characters".into(),
        json!(summary::text_length(summary) as i64),
    );
    data.insert("summary_updated_at".into(), json!(Utc::now().to_rfc3339()));

    let mut filters = vec![Filter::eq("owner_id", owner_id)];
    if agent.meta.scope == Scope::Organization {
        let Some(org) = active_org else {
            return Err(error(
                403,
                "select an organisation with the X-Organization header",
            ));
        };
        filters.push(Filter::eq("organization_id", org));
    }
    state
        .db
        .update(resource, thread_id, &data, &filters)
        .await
        .map_err(db_error)?
        .ok_or_else(|| error(404, "unknown thread"))?;
    Ok(())
}

/// The character budgets this agent's summaries work to.
fn summary_limits(agent: &Agent) -> SummaryLimits {
    SummaryLimits::new(
        agent
            .meta
            .storage
            .summary_after_characters(summary::DEFAULT_TRIGGER_CHARACTERS),
    )
}

fn needs_summary_refresh(agent: &Agent, messages: &[Message]) -> bool {
    summary::needs_refresh(messages, summary_limits(agent))
}

fn summary_prefix_length(
    agent: &Agent,
    recent_messages: &[Message],
    has_summary: bool,
    pending_messages: &[Message],
) -> usize {
    summary::prefix_length(
        recent_messages,
        has_summary,
        pending_messages,
        summary_limits(agent),
    )
}

fn owner_principal(user_id: Uuid) -> Principal {
    Principal::new(user_id, Vec::new())
}

async fn persist_tool_calls(
    state: &State<AppState>,
    prepared: &Prepared,
    calls: &[ToolCall],
) -> Result<(), HttpResponse> {
    for call in calls {
        insert_tool_event(state, prepared, "tool_call", call, None).await?;
    }
    Ok(())
}

async fn persist_tool_result(
    state: &State<AppState>,
    prepared: &Prepared,
    call: &ToolCall,
    output: &str,
) -> Result<(), HttpResponse> {
    insert_tool_event(state, prepared, "tool_result", call, Some(output)).await
}

async fn insert_tool_event(
    state: &State<AppState>,
    prepared: &Prepared,
    role: &str,
    call: &ToolCall,
    output: Option<&str>,
) -> Result<(), HttpResponse> {
    let Some(thread_id) = prepared.thread_id else {
        return Ok(());
    };
    let Some(owner_id) = prepared.owner_id else {
        return Ok(());
    };
    let resource_name = prepared.agent.message_resource_name();
    let Some(resource) = state.app.resources.get(&resource_name) else {
        return Err(error(500, "agent history resource is missing"));
    };
    let mut data = Map::new();
    data.insert("thread_id".into(), json!(thread_id));
    data.insert("owner_id".into(), json!(owner_id));
    data.insert("role".into(), json!(role));
    data.insert(
        "content".into(),
        json!(output
            .map(str::to_string)
            .unwrap_or_else(|| call.input.to_string())),
    );
    data.insert("tool_call_id".into(), json!(call.id));
    data.insert("tool_name".into(), json!(call.name));
    data.insert("tool_input".into(), call.input.clone());
    if let Some(output) = output {
        data.insert(
            "tool_output".into(),
            serde_json::from_str(output).unwrap_or_else(|_| json!(output)),
        );
    }
    if prepared.agent.meta.scope == Scope::Organization {
        let Some(org) = prepared.active_org else {
            return Err(error(
                403,
                "select an organisation with the X-Organization header",
            ));
        };
        data.insert("organization_id".into(), json!(org));
    }
    state.db.create(resource, &data).await.map_err(db_error)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_message(
    state: &State<AppState>,
    agent: &Agent,
    active_org: Option<Uuid>,
    owner_id: Uuid,
    thread_id: Uuid,
    role: &str,
    content: &str,
    reply: Option<&ChatReply>,
) -> Result<(), HttpResponse> {
    let resource_name = agent.message_resource_name();
    let Some(resource) = state.app.resources.get(&resource_name) else {
        return Err(error(500, "agent history resource is missing"));
    };
    let mut data = Map::new();
    data.insert("thread_id".into(), json!(thread_id));
    data.insert("owner_id".into(), json!(owner_id));
    data.insert("role".into(), json!(role));
    data.insert("content".into(), json!(content));
    if let Some(reply) = reply.filter(|reply| !reply.reasoning.trim().is_empty()) {
        data.insert("reasoning".into(), json!(reply.reasoning));
    }
    if agent.meta.scope == Scope::Organization {
        let Some(org) = active_org else {
            return Err(error(
                403,
                "select an organisation with the X-Organization header",
            ));
        };
        data.insert("organization_id".into(), json!(org));
    }
    if let Some(reply) = reply {
        if !reply.provider.is_empty() {
            data.insert("provider".into(), json!(reply.provider));
        }
        if !reply.model.is_empty() {
            data.insert("model".into(), json!(reply.model));
        }
        if !reply.done.finish_reason.is_empty() {
            data.insert("finish_reason".into(), json!(reply.done.finish_reason));
        }
        if let Some(tokens) = reply.done.input_tokens {
            data.insert("input_tokens".into(), json!(tokens as i64));
        }
        if let Some(tokens) = reply.done.output_tokens {
            data.insert("output_tokens".into(), json!(tokens as i64));
        }
    }
    state.db.create(resource, &data).await.map_err(db_error)?;
    Ok(())
}

fn title_from(message: &str) -> String {
    let single = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let short = single.chars().take(80).collect::<String>();
    if single.chars().count() > 80 {
        format!("{short}…")
    } else {
        short
    }
}

fn reply_json(reply: ChatReply, thread_id: Option<Uuid>) -> Value {
    let mut value = serde_json::to_value(reply).unwrap_or_else(|_| json!({}));
    if let Some(map) = value.as_object_mut() {
        if let Some(thread_id) = thread_id {
            map.insert("thread_id".into(), json!(thread_id));
        }
    }
    value
}

/// A model that spends its whole budget thinking answers with empty text and a
/// full reasoning trace. That is a real turn — the thinking is on the message
/// and the toggle reveals it — but the caller is owed an explanation for the
/// missing answer.
///
/// The trace is never promoted to *be* the answer. Doing that is what made
/// every truncated turn read as though the model had replied with its own
/// thinking, which is the one thing the reasoning/answer split exists to
/// prevent.
fn truncated_answer(reply: &ChatReply) -> Option<&'static str> {
    (reply.text.trim().is_empty() && !reply.reasoning.trim().is_empty()).then_some(TRUNCATED_ANSWER)
}

fn done_json(done: Done, thread_id: Option<Uuid>) -> Value {
    let mut value = serde_json::to_value(done).unwrap_or_else(|_| json!({}));
    if let Some(map) = value.as_object_mut() {
        if let Some(thread_id) = thread_id {
            map.insert("thread_id".into(), json!(thread_id));
        }
    }
    value
}

fn refused(e: apiplant_ai::AiError) -> HttpResponse {
    match e {
        apiplant_ai::AiError::Request(message) => error(400, message),
        apiplant_ai::AiError::Provider {
            provider,
            status,
            body,
        } => {
            tracing::warn!(provider, status, body = %body, "the ai provider refused a request");
            HttpResponse::BadGateway().json(&json!({
                "error": format!("the ai provider refused this request: {body}"),
                "provider": provider,
                "provider_status": status,
            }))
        }
        other => {
            tracing::error!(error = %other, "ai request failed");
            HttpResponse::BadGateway().json(&json!({ "error": other.to_string() }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apiplant_core::Agent as CoreAgent;

    /// The bug this guards: a model that spends its budget thinking has a full
    /// trace and no answer, and copying the trace into the answer made every
    /// truncated turn read as though the model replied with its own thinking.
    #[test]
    fn a_turn_that_is_all_thinking_stays_thinking() {
        let reply = ChatReply {
            text: "   ".into(),
            reasoning: "  I should tell them about the sea.  ".into(),
            ..ChatReply::default()
        };
        assert_eq!(truncated_answer(&reply), Some(TRUNCATED_ANSWER));
        // The trace is untouched: it reaches the caller as reasoning, behind
        // the toggle, and never as the answer.
        assert_eq!(reply.text, "   ");
    }

    #[test]
    fn a_real_answer_needs_no_explaining() {
        let reply = ChatReply {
            text: "The sea is cold.".into(),
            reasoning: "thinking".into(),
            ..ChatReply::default()
        };
        assert_eq!(truncated_answer(&reply), None);

        // Nothing at all: an empty answer with no thinking behind it is a
        // failed turn, not a truncated one.
        assert_eq!(truncated_answer(&ChatReply::default()), None);
    }

    fn test_agent(storage: &str) -> CoreAgent {
        toml::from_str(&format!(
            r#"
[agent]
name = "coach"
storage.enabled = true
{storage}
"#
        ))
        .unwrap()
    }

    #[test]
    fn summary_refresh_uses_character_length() {
        let agent = test_agent("");
        let short = vec![Message::user("short")];
        assert!(!needs_summary_refresh(&agent, &short));

        let long = vec![Message::user(
            "x".repeat(summary::DEFAULT_TRIGGER_CHARACTERS),
        )];
        assert!(needs_summary_refresh(&agent, &long));
    }

    #[test]
    fn summary_refresh_uses_agent_specific_thresholds() {
        let agent = test_agent("storage.summary_after_characters = 20");
        let messages = vec![Message::user(
            "this is comfortably longer than twenty chars",
        )];
        assert!(needs_summary_refresh(&agent, &messages));
    }

    #[test]
    fn summary_keeps_a_recent_tail_once_a_summary_exists() {
        let agent = test_agent("storage.summary_after_characters = 400");
        let messages = (0..10)
            .map(|index| Message::user(format!("message {index}")))
            .collect::<Vec<_>>();
        assert_eq!(
            summary_prefix_length(&agent, &messages, true, &[]),
            messages.len() - summary::RECENT_MESSAGE_COUNT
        );
    }

    #[test]
    fn pending_user_turn_can_trigger_a_pre_request_summary() {
        let agent = test_agent("storage.summary_after_characters = 40");
        let recent = vec![Message::assistant("A fairly long assistant update.")];
        let pending = [Message::user(
            "And here is the next long follow-up question.",
        )];
        assert!(needs_summary_refresh(
            &agent,
            &summary::with_pending_messages(&recent, &pending)
        ));
        assert_eq!(summary_prefix_length(&agent, &recent, false, &pending), 1);
    }

    #[test]
    fn an_agent_maps_onto_the_shared_summary_budgets() {
        let agent = test_agent("");
        assert_eq!(
            summary_limits(&agent).character_limit(),
            summary::MAX_CHARACTERS
        );

        let small = test_agent("storage.summary_after_characters = 1000");
        assert_eq!(summary_limits(&small).character_limit(), 500);
    }
}
