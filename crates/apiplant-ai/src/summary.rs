//! Rolling conversation summaries.
//!
//! An agent with stored history cannot send its whole transcript forever, so
//! the oldest turns are replaced by a summary the next request carries in its
//! system prompt. Everything that decides *what* to summarise, *how* to ask
//! for it and *whether the answer is usable* lives here rather than in the
//! server, because none of it depends on storage: it is a conversation, a
//! character budget and a model.
//!
//! The server drives this against a persisted thread; the `summarize` binary
//! drives the same code against text on stdin, which is how you find out
//! whether a given local model summarises well before pointing an app at it.

use serde_json::Value;

use crate::{Ai, AiError, ChatReply, ChatRequest, Message, Role};

/// Default unsummarised-tail budget, in characters.
pub const DEFAULT_TRIGGER_CHARACTERS: usize = 12_000;
/// Rough characters-per-token, for turning a character budget into a token cap.
pub const CHARACTERS_PER_TOKEN: usize = 4;
/// Tokens allowed on top of the summary itself, for a model that thinks before
/// it writes. Generous on purpose: reasoning is billed against the same cap as
/// the answer, so a tight cap does not produce a shorter summary — it produces
/// no summary at all, the model having spent the budget planning one.
pub const REASONING_TOKEN_HEADROOM: usize = 8_192;
/// Ceiling on that, so a runaway thinker is still cut off eventually.
pub const MAX_TOKENS: usize = 16_384;
/// How many recent messages stay verbatim once a summary exists.
pub const RECENT_MESSAGE_COUNT: usize = 6;
/// Floor and ceiling for a summary's own character budget.
pub const MIN_CHARACTERS: usize = 200;
pub const MAX_CHARACTERS: usize = 2_000;
/// A summary shorter than this fraction of the budget is almost certainly a
/// stray fragment rather than a summary, so a candidate that short is ignored
/// in favour of the model's own text.
const MIN_CANDIDATE_CHARACTERS: usize = 40;

/// The character budgets one summarisation works to.
///
/// Built from a single number — how long the unsummarised tail may grow —
/// because every other budget here is a consequence of that one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryLimits {
    trigger_characters: usize,
}

impl Default for SummaryLimits {
    fn default() -> Self {
        SummaryLimits::new(DEFAULT_TRIGGER_CHARACTERS)
    }
}

impl SummaryLimits {
    pub fn new(trigger_characters: usize) -> SummaryLimits {
        SummaryLimits {
            trigger_characters: trigger_characters.max(1),
        }
    }

    /// How long the tail may grow before a refresh is due.
    pub fn trigger_characters(&self) -> usize {
        self.trigger_characters
    }

    /// How long a rolling summary may be. Half of the tail budget, so the
    /// summary is always meaningfully shorter than what it replaces while
    /// still having room to carry the conversation.
    pub fn character_limit(&self) -> usize {
        (self.trigger_characters / 2).clamp(MIN_CHARACTERS, MAX_CHARACTERS)
    }

    /// The budget for summarising a transcript of this length.
    ///
    /// A summary is never usefully longer than what it replaces, and a model
    /// given a budget far larger than its input will pad to fill it — inventing
    /// detail, or narrating its own plan until it runs out of tokens. So the
    /// configured ceiling is only a ceiling: a short transcript gets a short
    /// budget.
    ///
    /// The floor stops the clamp from becoming pathological: there is little to
    /// compress in a two-line exchange, so a summary of it is legitimately
    /// about as long as it is.
    pub fn budget(&self, transcript_characters: usize) -> usize {
        let ceiling = self.character_limit();
        transcript_characters.clamp(MIN_CHARACTERS.min(ceiling), ceiling)
    }

    /// Room for the summary itself plus whatever the model thinks first — a
    /// reasoning model that runs out mid-thought answers with nothing at all.
    pub fn token_limit(&self, budget: usize) -> u32 {
        budget
            .div_ceil(CHARACTERS_PER_TOKEN)
            .saturating_add(REASONING_TOKEN_HEADROOM)
            .min(MAX_TOKENS) as u32
    }
}

/// What came back from asking a model to summarise.
#[derive(Debug, Clone, PartialEq)]
pub enum Summary {
    /// A usable summary. Already sanitised and inside the budget.
    Text(String),
    /// The model narrated instead of answering. Keeping the previous summary
    /// is always better than poisoning the thread with a thinking trace the
    /// next turn would have to reason about.
    Reasoning(String),
    /// The model answered with nothing usable.
    Empty,
}

impl Summary {
    pub fn text(&self) -> Option<&str> {
        match self {
            Summary::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// Ask the model to merge `existing_summary` with `messages` into one summary.
///
/// Returns the raw reply alongside the verdict, so a caller debugging a model
/// can see what it actually said before the sanitiser got to it.
pub async fn summarize(
    ai: &Ai,
    existing_summary: Option<&str>,
    messages: &[Message],
    limits: SummaryLimits,
) -> Result<(Summary, ChatReply), AiError> {
    let transcript = render_transcript(messages);
    // Everything the source material must fit is measured against the source
    // material, existing summary included: merging into an old summary is
    // still summarising, and the pair is what the model is working from.
    let budget = limits
        .budget(text_length(&transcript) + existing_summary.map(text_length).unwrap_or_default());
    let reply = ai
        .complete(&request(existing_summary, &transcript, budget, limits))
        .await?;
    Ok((summary_from_reply(&reply, budget), reply))
}

/// The request `summarize` sends. Public so a caller can inspect or replay it.
pub fn request(
    existing_summary: Option<&str>,
    transcript: &str,
    budget: usize,
    limits: SummaryLimits,
) -> ChatRequest {
    ChatRequest {
        messages: vec![Message::user(prompt(existing_summary, transcript, budget))],
        system: Some(system_prompt(budget)),
        temperature: Some(0.1),
        max_tokens: Some(limits.token_limit(budget)),
        ..ChatRequest::default()
    }
}

/// Judge one reply: a model that answered only in its reasoning channel still
/// gets read, since some local servers put the whole answer there.
pub fn summary_from_reply(reply: &ChatReply, budget: usize) -> Summary {
    let raw = if reply.text.trim().is_empty() {
        &reply.reasoning
    } else {
        &reply.text
    };
    let cleaned = sanitize(raw, budget);
    let summary = cleaned.trim();
    if looks_like_reasoning(summary) {
        return Summary::Reasoning(summary.to_string());
    }
    if summary.is_empty() {
        return Summary::Empty;
    }
    Summary::Text(summary.to_string())
}

/// Has the unsummarised tail grown past its budget?
pub fn needs_refresh(messages: &[Message], limits: SummaryLimits) -> bool {
    if messages.is_empty() {
        return false;
    }
    transcript_length(messages) >= limits.trigger_characters()
}

/// How many of `recent_messages` to fold into the summary, leaving a tail that
/// fits the budget once `pending_messages` are appended.
pub fn prefix_length(
    recent_messages: &[Message],
    has_summary: bool,
    pending_messages: &[Message],
    limits: SummaryLimits,
) -> usize {
    if recent_messages.is_empty() {
        return 0;
    }
    let mut split = if has_summary {
        recent_messages.len().saturating_sub(RECENT_MESSAGE_COUNT)
    } else if recent_messages.len() <= RECENT_MESSAGE_COUNT {
        recent_messages.len()
    } else {
        recent_messages.len() - RECENT_MESSAGE_COUNT
    };

    while split < recent_messages.len() {
        let tail = with_pending_messages(&recent_messages[split..], pending_messages);
        if !needs_refresh(&tail, limits) || recent_messages.len() - split <= 1 {
            break;
        }
        split += 1;
    }
    tool_pair_safe_split(recent_messages, split)
}

/// A retained tail must never start with a tool result whose call was
/// summarised away — most providers reject that outright. Pull the orphaned
/// results into the summarised prefix instead.
pub fn tool_pair_safe_split(messages: &[Message], split: usize) -> usize {
    let mut split = split.min(messages.len());
    while split > 0 && split < messages.len() && messages[split].role == Role::Tool {
        split += 1;
    }
    split
}

/// Drop tool traffic that lost its other half — a tool result whose call is no
/// longer in the window, or a tool call whose result never arrived.
pub fn drop_orphan_tool_messages(messages: &[Message]) -> Vec<Message> {
    let answered = messages
        .iter()
        .filter(|message| message.role == Role::Tool)
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect::<std::collections::BTreeSet<_>>();
    let mut open = std::collections::BTreeSet::new();
    let mut kept = Vec::with_capacity(messages.len());
    for message in messages {
        if message.role == Role::Assistant && !message.tool_calls.is_empty() {
            let calls = message
                .tool_calls
                .iter()
                .filter(|call| answered.contains(call.id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if calls.is_empty() {
                if message.content.trim().is_empty() {
                    continue;
                }
                let mut plain = message.clone();
                plain.tool_calls.clear();
                kept.push(plain);
                continue;
            }
            for call in &calls {
                open.insert(call.id.clone());
            }
            let mut trimmed = message.clone();
            trimmed.tool_calls = calls;
            kept.push(trimmed);
            continue;
        }
        if message.role == Role::Tool {
            let paired = message
                .tool_call_id
                .as_deref()
                .is_some_and(|id| open.contains(id));
            if !paired {
                continue;
            }
            kept.push(message.clone());
            continue;
        }
        kept.push(message.clone());
    }
    kept
}

pub fn with_pending_messages(messages: &[Message], pending_messages: &[Message]) -> Vec<Message> {
    messages
        .iter()
        .cloned()
        .chain(pending_messages.iter().cloned())
        .collect()
}

pub fn transcript_length(messages: &[Message]) -> usize {
    text_length(&render_transcript(messages))
}

pub fn text_length(text: &str) -> usize {
    text.chars().count()
}

/// Flatten a conversation into the plain transcript the model is asked to
/// summarise, tool activity included: what a tool returned is often the only
/// place a durable fact appears.
pub fn render_transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for message in messages {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
            Role::Tool => "Tool",
        });
        if let Some(id) = message.tool_call_id.as_deref().filter(|id| !id.is_empty()) {
            out.push_str(" [");
            out.push_str(id);
            out.push(']');
        }
        out.push_str(": ");
        if !message.content.trim().is_empty() {
            out.push_str(message.content.trim());
        }
        if !message.tool_calls.is_empty() {
            if !message.content.trim().is_empty() {
                out.push('\n');
            }
            for call in &message.tool_calls {
                out.push_str("tool_call ");
                out.push_str(&call.name);
                out.push_str(" id=");
                out.push_str(&call.id);
                out.push_str(" input=");
                out.push_str(&Value::to_string(&call.input));
                out.push('\n');
            }
            out.pop();
        }
    }
    out
}

/// The instructions the summariser answers under.
///
/// Deliberately short and stated as prohibitions on *output*, not as a
/// checklist: a reasoning model handed a long list of requirements tends to
/// restate the list back, and the restating is what eats the token budget
/// before it writes a word. `budget` is a ceiling only — naming a floor makes
/// a model pad a two-line conversation into an essay.
pub fn system_prompt(budget: usize) -> String {
    format!(
        "You write backend-only rolling conversation summaries for an AI agent. \
Capture durable context: who the user is, what they want, decisions, facts, constraints, \
unresolved work and notable tool results. Cover the whole conversation, not just the last turn. \
Write only what the conversation actually establishes — never invent detail, and never pad to \
reach a length. A short conversation gets a short summary. Do not address the user, do not \
mention that this is a summary, do not quote the transcript, do not plan or explain your \
approach. Your entire reply must be <summary>the summary</summary> and nothing else, starting \
with the opening tag. At most {budget} characters."
    )
}

pub fn prompt(existing_summary: Option<&str>, transcript: &str, budget: usize) -> String {
    let mut prompt = String::new();
    if let Some(summary) = existing_summary
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
    {
        prompt.push_str("Existing summary:\n");
        prompt.push_str(summary);
        prompt.push_str("\n\n");
    }
    prompt.push_str("Recent conversation to merge:\n");
    prompt.push_str(transcript);
    prompt.push_str(&format!(
        "\n\nReply now with <summary>…</summary> and nothing else: plain prose, \
at most {budget} characters, shorter if the conversation is."
    ));
    prompt
}

/// Pull a usable summary out of whatever the model wrote around it.
pub fn sanitize(text: &str, character_limit: usize) -> String {
    // The summary prompt asks for <summary></summary> tags precisely so a model
    // that thinks out loud in plain text still has a machine-readable answer.
    if let Some(tagged) = extract_tagged_summary(text) {
        return truncate(strip_summary_label(tagged.trim()), character_limit);
    }
    // Inline `<think>` blocks are separated by the transport before a reply
    // gets here (see `ThinkingSplit`), so this is the model's visible answer.
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Some models answer with their whole reasoning trace and only label the
    // summary at the end. Take that labelled tail when it is long enough to be
    // a summary; anything shorter is a fragment, and the model's own text is a
    // better answer than a stray line.
    let candidate = extract_summary_candidate(trimmed)
        .filter(|candidate| {
            candidate.chars().count() >= MIN_CANDIDATE_CHARACTERS.min(character_limit)
        })
        .unwrap_or(trimmed);

    truncate(strip_summary_label(candidate.trim()), character_limit)
}

/// Does this read as a model thinking out loud rather than a summary?
pub fn looks_like_reasoning(text: &str) -> bool {
    let opening = text
        .chars()
        .take(160)
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "thinking process",
        "let me think",
        "let's think",
        "let me start by",
        "analyze user input",
        "step 1",
        "first, i need to",
        "i need to rewrite",
    ]
    .iter()
    .any(|marker| opening.contains(marker))
}

/// Find the summary a model tagged, preferring the last **closed** block.
///
/// A model that drafts out loud writes the same summary several times — and a
/// reply cut off by the token cap ends mid-way through the final copy. Taking
/// the last opening tag therefore picks the one truncated copy over the
/// complete ones that precede it, which is how a whole good summary turns into
/// half a sentence. An unterminated block is only worth having when there is
/// no closed one anywhere.
fn extract_tagged_summary(text: &str) -> Option<&str> {
    let mut last_closed = None;
    let mut rest = text;
    let mut offset = 0;
    while let Some(index) = rest.find("<summary>") {
        let body_at = offset + index + "<summary>".len();
        let body = &text[body_at..];
        match body.find("</summary>") {
            Some(end) => {
                let closed = body[..end].trim();
                if !closed.is_empty() {
                    last_closed = Some(closed);
                }
                offset = body_at + end + "</summary>".len();
            }
            None => break,
        }
        rest = &text[offset..];
    }
    if last_closed.is_some() {
        return last_closed;
    }
    let open = text.rfind("<summary>")? + "<summary>".len();
    let body = text[open..].trim();
    (!body.is_empty()).then_some(body)
}

/// Remove a leading `Summary:` style label the model may prepend.
fn strip_summary_label(text: &str) -> &str {
    for label in [
        "Final summary:",
        "Final Summary:",
        "Updated summary:",
        "Updated Summary:",
        "Summary:",
    ] {
        if let Some(rest) = text.strip_prefix(label) {
            return rest.trim();
        }
    }
    text
}

fn extract_summary_candidate(text: &str) -> Option<&str> {
    for marker in [
        "Final Output",
        "Final output",
        "Final Summary",
        "Final summary",
    ] {
        if let Some(index) = text.rfind(marker) {
            let tail = text[index + marker.len()..].trim_start();
            let tail = tail.trim_start_matches("Generation").trim_start();
            let tail = tail.trim_start_matches(':').trim();
            let normalized = tail.trim_matches('"').trim();
            if !normalized.is_empty() {
                return Some(normalized);
            }
        }
    }
    None
}

fn truncate(text: &str, character_limit: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = compact.chars().count();
    if count <= character_limit {
        return compact;
    }
    // Prefer cutting on a sentence boundary inside the budget.
    let mut upto = 0;
    let mut seen = 0;
    for (index, ch) in compact.char_indices() {
        seen += 1;
        if seen > character_limit {
            break;
        }
        if matches!(ch, '.' | '!' | '?') {
            upto = index + ch.len_utf8();
        }
    }
    if upto > 0 {
        return compact[..upto].to_string();
    }
    let keep = character_limit.saturating_sub(1);
    let short = compact.chars().take(keep).collect::<String>();
    format!("{short}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolCall;
    use serde_json::json;

    #[test]
    fn refresh_uses_character_length() {
        let limits = SummaryLimits::default();
        assert!(!needs_refresh(&[Message::user("short")], limits));
        assert!(needs_refresh(
            &[Message::user("x".repeat(DEFAULT_TRIGGER_CHARACTERS))],
            limits
        ));
        assert_eq!(text_length("abcdefgh"), 8);
    }

    #[test]
    fn refresh_uses_the_configured_threshold() {
        let messages = vec![Message::user(
            "this is comfortably longer than twenty chars",
        )];
        assert!(needs_refresh(&messages, SummaryLimits::new(20)));
    }

    #[test]
    fn a_recent_tail_survives_once_a_summary_exists() {
        let limits = SummaryLimits::new(400);
        let messages = (0..10)
            .map(|index| Message::user(format!("message {index}")))
            .collect::<Vec<_>>();
        assert_eq!(
            prefix_length(&messages, true, &[], limits),
            messages.len() - RECENT_MESSAGE_COUNT
        );
    }

    #[test]
    fn a_pending_turn_can_trigger_a_pre_request_summary() {
        let limits = SummaryLimits::new(40);
        let recent = vec![Message::assistant("A fairly long assistant update.")];
        let pending = [Message::user(
            "And here is the next long follow-up question.",
        )];
        assert!(needs_refresh(
            &with_pending_messages(&recent, &pending),
            limits
        ));
        assert_eq!(prefix_length(&recent, false, &pending, limits), 1);
    }

    #[test]
    fn the_limit_is_half_the_trigger_within_bounds() {
        assert_eq!(SummaryLimits::default().character_limit(), MAX_CHARACTERS);
        assert_eq!(SummaryLimits::new(1_000).character_limit(), 500);
        // Tiny triggers still get a usable summary: it lives in the system
        // prompt, not in the tail the trigger measures.
        assert_eq!(SummaryLimits::new(100).character_limit(), MIN_CHARACTERS);
    }

    #[test]
    fn the_budget_never_exceeds_the_text_it_replaces() {
        let limits = SummaryLimits::default();
        // A one-line conversation must not be handed a 2000-character budget:
        // a model asked for far more than its input invents the difference.
        assert_eq!(limits.budget(600), 600);
        assert_eq!(limits.budget(100_000), MAX_CHARACTERS);
        // …but a floor keeps a two-line exchange from being clamped to nothing,
        // since there is little in it to compress.
        assert_eq!(limits.budget(40), MIN_CHARACTERS);
        assert_eq!(limits.budget(0), MIN_CHARACTERS);

        // A tiny configured ceiling still wins over the floor.
        assert_eq!(SummaryLimits::new(100).budget(40), MIN_CHARACTERS);
    }

    #[test]
    fn the_token_budget_leaves_room_for_reasoning() {
        // A summary of 500 characters is ~125 tokens; a reasoning model needs
        // far more than that before it writes a word.
        // Reasoning is billed against this cap, so it must leave room for a
        // model that drafts the summary several times before answering.
        assert!(SummaryLimits::new(1_000).token_limit(500) > 8_000);
        assert!(SummaryLimits::new(40_000).token_limit(MAX_CHARACTERS) as usize <= MAX_TOKENS);
    }

    #[test]
    fn a_split_never_orphans_a_tool_result() {
        let messages = vec![
            Message::user("Need the notes."),
            Message::assistant_tool_calls(vec![ToolCall {
                id: "call_1".to_string(),
                name: "notes".to_string(),
                input: json!({}),
            }]),
            Message::tool_result("call_1", "{}"),
            Message::assistant("Here they are."),
        ];
        // Splitting between the call and its result pulls the result along.
        assert_eq!(tool_pair_safe_split(&messages, 2), 3);
        assert_eq!(tool_pair_safe_split(&messages, 1), 1);
    }

    #[test]
    fn orphan_tool_traffic_is_dropped_from_the_request() {
        let messages = vec![
            Message::tool_result("call_gone", "{}"),
            Message::user("Still here?"),
            Message::assistant_tool_calls(vec![ToolCall {
                id: "call_open".to_string(),
                name: "notes".to_string(),
                input: json!({}),
            }]),
        ];
        let kept = drop_orphan_tool_messages(&messages);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].role, Role::User);
    }

    #[test]
    fn paired_tool_traffic_survives() {
        let messages = vec![
            Message::assistant_tool_calls(vec![ToolCall {
                id: "call_1".to_string(),
                name: "notes".to_string(),
                input: json!({}),
            }]),
            Message::tool_result("call_1", "{}"),
            Message::assistant("Done."),
        ];
        assert_eq!(drop_orphan_tool_messages(&messages).len(), 3);
    }

    #[test]
    fn the_transcript_keeps_tool_activity() {
        let transcript = render_transcript(&[
            Message::user("Need the latest notes."),
            Message::assistant_tool_calls(vec![ToolCall {
                id: "call_1".to_string(),
                name: "current_user_notes".to_string(),
                input: json!({ "limit": 5 }),
            }]),
            Message::tool_result("call_1", r#"{"notes":[]}"#),
            Message::assistant("No notes yet."),
        ]);

        assert!(transcript.contains("User: Need the latest notes."));
        assert!(transcript.contains("tool_call current_user_notes id=call_1 input={\"limit\":5}"));
        assert!(transcript.contains("Tool [call_1]: {\"notes\":[]}"));
        assert!(transcript.contains("Assistant: No notes yet."));
    }

    #[test]
    fn the_prompt_merges_an_existing_summary_and_the_transcript() {
        let prompt = prompt(
            Some("User is planning a launch."),
            "User: give me a checklist",
            500,
        );
        assert!(prompt.contains("Existing summary:\nUser is planning a launch."));
        assert!(prompt.contains("Recent conversation to merge:\nUser: give me a checklist"));
        assert!(prompt.contains("at most 500 characters"));
        // No lower bound anywhere: a floor is what makes a model pad.
        assert!(!prompt.contains("roughly"));
    }

    #[test]
    fn the_sanitizer_keeps_the_whole_answer() {
        let answer = "The caller is planning a support workflow. Requirements so far: triage, \
ownership, SLA policies and an audit trail. Nothing is built yet.";
        assert_eq!(sanitize(answer, 500), answer);
    }

    #[test]
    fn the_sanitizer_ignores_short_quoted_fragments() {
        // The bug this guards: a stray quoted word from the transcript used to
        // win over the model's actual summary.
        let answer = "The caller asked \"wow\" and then moved on to Roman history, \
which is now the running topic of the conversation.";
        assert_eq!(sanitize(answer, 500), answer);
    }

    #[test]
    fn the_sanitizer_drops_labels() {
        let verbose = "Summary: The caller is preparing a product launch and wants a checklist \
covering marketing, engineering readiness and support staffing.";
        assert_eq!(
            sanitize(verbose, 500),
            "The caller is preparing a product launch and wants a checklist covering marketing, \
engineering readiness and support staffing."
        );
    }

    #[test]
    fn the_sanitizer_takes_a_long_labelled_tail() {
        let verbose = r#"Here's a thinking process:

1. Analyze.

Final Output:
SaaS support workflow design in progress. Core requirements include triage, ownership, SLA policies, org-scoped inboxes, admin escalations, routing rules, timers, and an audit trail."#;

        assert_eq!(
            sanitize(verbose, 500),
            "SaaS support workflow design in progress. Core requirements include triage, ownership, SLA policies, org-scoped inboxes, admin escalations, routing rules, timers, and an audit trail."
        );
    }

    #[test]
    fn a_reasoning_trace_is_not_a_summary() {
        assert!(looks_like_reasoning(
            "Here's a thinking process: 1. Analyze User Input: the caller wants a summary."
        ));
        assert!(!looks_like_reasoning(
            "The caller is planning a launch and wants a checklist."
        ));
    }

    #[test]
    fn the_sanitizer_prefers_the_tagged_block() {
        let verbose = "Here's a thinking process: 1. Analyze. 2. Write.\n\
<summary>The caller asked for two long essays, one on the sea and one on mountains.</summary>";
        assert_eq!(
            sanitize(verbose, 500),
            "The caller asked for two long essays, one on the sea and one on mountains."
        );
    }

    #[test]
    fn a_complete_tagged_block_beats_a_truncated_later_one() {
        // What a drafting model actually emits when the token cap cuts it off:
        // two good copies, then half of a third. The half is the last one.
        let drafted = "Draft:\n<summary>Han and Chewie deliver the archive.</summary>\n\
Refined:\n<summary>Han and Chewie deliver the Alderaanian archive to Chandrila.</summary>\n\
Generating:\n<summary>Han and Chewie deliver the Alderaanian archive to Chan";
        assert_eq!(
            sanitize(drafted, 500),
            "Han and Chewie deliver the Alderaanian archive to Chandrila."
        );
    }

    #[test]
    fn an_unterminated_block_is_used_when_it_is_all_there_is() {
        let cut_off = "<summary>Han and Chewie deliver the archive";
        assert_eq!(sanitize(cut_off, 500), "Han and Chewie deliver the archive");
    }

    #[test]
    fn truncation_uses_the_budget() {
        let long = "Sentence one is here. Sentence two is here. Sentence three is here.";
        let cut = sanitize(long, 45);
        assert!(cut.chars().count() <= 45);
        assert_eq!(cut, "Sentence one is here. Sentence two is here.");
    }

    #[test]
    fn a_reply_that_only_reasoned_is_still_read() {
        let reply = ChatReply {
            text: String::new(),
            reasoning: "<summary>The caller wants a summary utility.</summary>".to_string(),
            ..ChatReply::default()
        };
        assert_eq!(
            summary_from_reply(&reply, MAX_CHARACTERS),
            Summary::Text("The caller wants a summary utility.".to_string())
        );

        assert_eq!(
            summary_from_reply(&ChatReply::default(), MAX_CHARACTERS),
            Summary::Empty
        );
    }
}
