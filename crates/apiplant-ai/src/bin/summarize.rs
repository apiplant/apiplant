//! `summarize` — run the agent's rolling-summary logic against your own text.
//!
//! The summariser an agent uses is only as good as the model behind it, and
//! the only way to find that out is to watch a real model summarise real text.
//! This binary is exactly that: the same prompts, the same budgets and the
//! same sanitiser the server runs, pointed at whatever you paste in.
//!
//! ```text
//! summarize < transcript.txt
//! summarize --model qwen3:8b --limit 4000 notes.txt
//! summarize --endpoint https://api.openai.com --provider openai --model gpt-4o-mini < notes.txt
//! ```
//!
//! Defaults to `provider = custom` at `http://localhost:8080`, because a local
//! OpenAI-shaped server is the thing you are most likely to be evaluating.

use std::io::Read;
use std::process::ExitCode;

use apiplant_ai::summary::{self, Summary, SummaryLimits};
use apiplant_ai::{Ai, Message};
use apiplant_core::AiConfig;

const DEFAULT_ENDPOINT: &str = "http://localhost:8080";
const USAGE: &str = "\
usage: summarize [options] [file]

Summarise text with the same logic apiplant agents use for thread summaries.
Reads stdin when no file is given.

options:
  --provider <name>   custom (default), openai, anthropic
  --endpoint <url>    default http://localhost:8080 (custom only)
  --model <name>      model to ask for
  --api-key <key>     provider credential; also read from AI_API_KEY
  --limit <chars>     tail budget; the summary gets half of it, capped at 2000
  --max-tokens <n>    override the generation cap (reasoning is billed to it)
  --think <on|off>    ask the provider to think, or not, via its own switch
  --existing <text>   an existing summary to merge into
  --timeout <secs>    per-request timeout (default 300)
  --raw               print the model's untouched reply as well
  -h, --help          this text
";

struct Options {
    provider: String,
    endpoint: String,
    model: String,
    api_key: String,
    limits: SummaryLimits,
    max_tokens: Option<u32>,
    thinking: Option<bool>,
    existing: Option<String>,
    timeout_secs: u64,
    raw: bool,
    file: Option<String>,
}

fn main() -> ExitCode {
    let options = match parse_args() {
        Ok(Some(options)) => options,
        Ok(None) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(message) => return fail(&message),
    };

    let input = match read_input(options.file.as_deref()) {
        Ok(input) => input,
        Err(message) => return fail(&message),
    };
    if input.trim().is_empty() {
        return fail("nothing to summarise");
    }

    let config = AiConfig {
        provider: options.provider.clone(),
        endpoint: options.endpoint.clone(),
        model: options.model.clone(),
        api_key: options.api_key.clone(),
        timeout_secs: options.timeout_secs,
        thinking: options.thinking,
        ..AiConfig::default()
    };
    let ai = match Ai::from_config(&config) {
        Ok(Some(ai)) => ai,
        // Unreachable in practice: the provider defaults to `custom`, which is
        // enabled. Still worth saying rather than unwrapping.
        Ok(None) => return fail("provider `none` has nothing to ask"),
        Err(e) => return fail(&e.to_string()),
    };

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => return fail(&e.to_string()),
    };

    // A file of prose is one long user turn; a transcript pasted in is too.
    // Either way the summariser sees the same shape it sees in production.
    let messages = vec![Message::user(input)];
    let limits = options.limits;

    // Built from the same public pieces the server drives, rather than through
    // `summary::summarize`, so the two overrides below can reach the request.
    let transcript = summary::render_transcript(&messages);
    let budget = limits.budget(
        summary::text_length(&transcript)
            + options
                .existing
                .as_deref()
                .map(summary::text_length)
                .unwrap_or_default(),
    );
    let mut request = summary::request(options.existing.as_deref(), &transcript, budget, limits);
    if let Some(max_tokens) = options.max_tokens {
        request.max_tokens = Some(max_tokens);
    }
    eprintln!(
        "{} {} via {} — input {} chars, summary budget {} chars, {} max tokens",
        ai.provider().as_str(),
        if options.model.is_empty() {
            "(default model)"
        } else {
            options.model.as_str()
        },
        ai.url(),
        summary::text_length(&transcript),
        budget,
        request.max_tokens.unwrap_or_default(),
    );

    let reply = match runtime.block_on(ai.complete(&request)) {
        Ok(reply) => reply,
        Err(e) => return fail(&e.to_string()),
    };
    let verdict = summary::summary_from_reply(&reply, budget);

    if options.raw {
        eprintln!("--- raw text ---\n{}", reply.text);
        if !reply.reasoning.is_empty() {
            eprintln!("--- raw reasoning ---\n{}", reply.reasoning);
        }
        eprintln!("--- ---");
    }

    match verdict {
        Summary::Text(text) => {
            println!("{text}");
            eprintln!(
                "ok — {} chars of a {} budget",
                summary::text_length(&text),
                budget
            );
            ExitCode::SUCCESS
        }
        // The two ways a model fails here are worth telling apart: an agent
        // discards both, but they say different things about the model.
        Summary::Reasoning(text) => {
            eprintln!("rejected: the model narrated instead of summarising\n{text}");
            truncation_hint(&reply);
            ExitCode::FAILURE
        }
        Summary::Empty => {
            eprintln!("rejected: the model answered with nothing usable");
            truncation_hint(&reply);
            ExitCode::FAILURE
        }
    }
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut options = Options {
        provider: "custom".to_string(),
        endpoint: DEFAULT_ENDPOINT.to_string(),
        model: String::new(),
        api_key: std::env::var("AI_API_KEY").unwrap_or_default(),
        limits: SummaryLimits::default(),
        max_tokens: None,
        thinking: None,
        existing: None,
        timeout_secs: AiConfig::default().timeout_secs,
        raw: false,
        file: None,
    };

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .ok_or_else(|| format!("{arg} needs a value; see --help"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--raw" => options.raw = true,
            "--think" => {
                let raw = value()?;
                options.thinking = Some(match raw.as_str() {
                    "on" | "true" | "yes" => true,
                    "off" | "false" | "no" => false,
                    other => return Err(format!("--think wants on or off, not `{other}`")),
                });
            }
            "--max-tokens" => {
                let raw = value()?;
                options.max_tokens = Some(
                    raw.parse()
                        .map_err(|_| format!("--max-tokens wants a number, not `{raw}`"))?,
                );
            }
            "--provider" => options.provider = value()?,
            "--endpoint" => options.endpoint = value()?,
            "--model" => options.model = value()?,
            "--api-key" => options.api_key = value()?,
            "--existing" => options.existing = Some(value()?),
            "--limit" => {
                let raw = value()?;
                let characters = raw
                    .parse::<usize>()
                    .map_err(|_| format!("--limit wants a number, not `{raw}`"))?;
                options.limits = SummaryLimits::new(characters);
            }
            "--timeout" => {
                let raw = value()?;
                options.timeout_secs = raw
                    .parse()
                    .map_err(|_| format!("--timeout wants seconds, not `{raw}`"))?;
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unknown option `{other}`; see --help"))
            }
            other => {
                if options.file.is_some() {
                    return Err("only one input file, or stdin".to_string());
                }
                options.file = Some(other.to_string());
            }
        }
    }
    Ok(Some(options))
}

fn read_input(file: Option<&str>) -> Result<String, String> {
    match file {
        Some("-") | None => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .map_err(|e| format!("reading stdin: {e}"))?;
            Ok(buffer)
        }
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}")),
    }
}

/// A reply cut off by the cap means the model spent the budget thinking, which
/// looks like a bad summariser but is a budget problem.
fn truncation_hint(reply: &apiplant_ai::ChatReply) {
    if reply.done.finish_reason == "length" {
        eprintln!(
            "\n(the reply hit the token cap mid-thought — this model spends its budget \
planning. Try --think off, a larger --max-tokens, or a non-reasoning model.)"
        );
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("summarize: {message}");
    ExitCode::FAILURE
}
