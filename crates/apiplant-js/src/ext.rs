//! The `apiplant_js` extension: our one op, and the bootstrap that uses it.
//!
//! This module is compiled **twice**: once as part of the crate, and once by
//! `build.rs` through a `#[path]` attribute. That is deliberate. The V8 startup
//! snapshot has to be built from the same op declarations the runtime later
//! registers — if the two lists disagree, deno_core refuses the snapshot at
//! isolate creation, and it is much better to find that out at build time than
//! to keep two definitions in step by hand.
//!
//! Because `build.rs` compiles it standalone, everything here must depend only
//! on `deno_core` and `crossbeam-channel` — no other module of this crate.

use std::cell::RefCell;
use std::rc::Rc;

use crossbeam_channel::{bounded, Sender};
use deno_core::{extension, op2, ExtensionFileSource, OpState};

/// What the isolate's thread sends back over a job's channel.
// `build.rs` compiles this module without the code that reads these, so the
// standalone compilation sees them as dead.
#[allow(dead_code)]
pub(crate) enum Message {
    /// The function asked the host for something and is blocked until answered.
    Host {
        kind: String,
        payload: String,
        answer: Sender<String>,
    },
    /// The call finished. Always the last message of a job.
    Done(Result<String, String>),
}

/// The isolate-side end of the current job, read by the op.
pub(crate) type Current = Rc<RefCell<Option<Sender<Message>>>>;

#[op2]
#[string]
fn op_apiplant_host(state: &mut OpState, #[string] kind: &str, #[string] payload: &str) -> String {
    let current = state.borrow::<Current>().clone();
    let replies = current.borrow().clone();
    let Some(replies) = replies else {
        // Only reachable if user code stashed `ctx` and called it after the
        // invocation returned — from a timer, say. There is no host to ask.
        return r#"{"error":"no invocation in progress"}"#.to_string();
    };

    let (answer, wait) = bounded(1);
    let sent = replies.send(Message::Host {
        kind: kind.to_string(),
        payload: payload.to_string(),
        answer,
    });
    if sent.is_err() {
        return r#"{"error":"the host stopped listening"}"#.to_string();
    }
    wait.recv()
        .unwrap_or_else(|_| r#"{"error":"the host stopped listening"}"#.to_string())
}

/// One outbound HTTP request, as `fetch.js` describes it.
#[derive(serde::Deserialize)]
pub(crate) struct FetchRequest {
    method: String,
    url: String,
    /// Header name/value pairs, already lowercased and combined by `Headers`.
    headers: Vec<(String, String)>,
    /// `follow` or `manual`; `error` is turned into `manual` by the JS side.
    redirect: String,
}

/// What comes back, in the shape `Response` is built from.
#[derive(serde::Serialize)]
pub(crate) struct FetchResponse {
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    /// The URL the response finally came from, which differs from the request's
    /// after a redirect — `Response.url` is defined to report the last one.
    url: String,
    redirected: bool,
    body: deno_core::ToJsBuffer,
}

/// How long one outbound request may take before it is abandoned.
///
/// A function's own invocation is already bounded by the watchdog, but that
/// terminates the isolate; a request that merely hangs should fail as a
/// `TypeError` the function can catch, and sooner.
fn fetch_timeout() -> std::time::Duration {
    let ms = std::env::var("APIPLANT_FETCH_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30_000);
    std::time::Duration::from_millis(ms)
}

/// The egress allowlist, if one is configured.
///
/// `APIPLANT_FETCH_ALLOW` is a comma-separated list of hosts, each optionally
/// leading with `*.` to include subdomains. Unset means no restriction, which
/// is what keeps `fetch` a drop-in; setting it is how a deployment stops a
/// function reaching link-local metadata endpoints or the database's own
/// network. Matching is on host only — a port or scheme cannot widen it.
fn egress_allowed(url: &reqwest::Url) -> bool {
    match std::env::var("APIPLANT_FETCH_ALLOW") {
        Ok(rule) => host_matches(url.host_str(), &rule),
        Err(_) => true,
    }
}

/// The allowlist decision itself, with the rule passed in.
///
/// Split from [`egress_allowed`] so it can be tested: the rule lives in an
/// environment variable, and a test that set one would race every other test in
/// the process.
fn host_matches(host: Option<&str>, rule: &str) -> bool {
    let Some(host) = host else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    rule.split(',')
        .map(|pattern| pattern.trim().to_ascii_lowercase())
        .filter(|pattern| !pattern.is_empty())
        .any(|pattern| match pattern.strip_prefix("*.") {
            // `*.example.com` covers subdomains and the domain itself, which is
            // what people mean by it — and never `notexample.com`, which a
            // plain `ends_with` would have let through.
            Some(domain) => host == domain || host.ends_with(&format!(".{domain}")),
            None => host == pattern,
        })
}

/// The client every isolate shares.
///
/// One client, not one per request: it owns the connection pool, and building a
/// fresh one per call would mean a new TLS handshake for every outbound request.
/// Built on first use rather than at extension init, because `build.rs` also
/// initialises this extension to take the snapshot and has no use for a socket.
fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // Redirects are followed here only when the request asked for it;
            // `fetch.js` passes the mode straight through.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a reqwest client with no TLS backend configured")
    })
}

/// Perform one request. The redirect chain is walked here rather than by
/// reqwest so that `Response.redirected` and `Response.url` can be reported
/// truthfully, which a policy-driven client does not expose.
// `deferred` rather than the eager form: the promise settles on a later tick
// even when the future is already ready, which keeps `fetch` from resolving
// synchronously for a cached or refused request and letting a function observe
// an ordering the network could never produce.
#[op2(async(deferred))]
#[serde]
async fn op_apiplant_fetch(
    #[serde] request: FetchRequest,
    #[buffer(copy)] body: Option<Vec<u8>>,
) -> Result<FetchResponse, deno_error::JsErrorBox> {
    // `fetch` rejects with a TypeError for every network-level failure, and
    // deliberately says little about why: the spec treats the difference between
    // "no such host" and "connection refused" as information not worth leaking.
    // Here it is also what keeps an allowlist from being a probe.
    let fail = |message: String| deno_error::JsErrorBox::type_error(message);

    let mut url = reqwest::Url::parse(&request.url)
        .map_err(|e| fail(format!("cannot fetch `{}`: {e}", request.url)))?;
    let method = reqwest::Method::from_bytes(request.method.as_bytes())
        .map_err(|_| fail(format!("`{}` is not a valid HTTP method", request.method)))?;
    let follow = request.redirect == "follow";

    let mut redirected = false;
    // The same ceiling browsers use. A cycle is otherwise unbounded.
    for _ in 0..20 {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(fail(format!(
                "cannot fetch `{url}`: only http and https are supported"
            )));
        }
        if !egress_allowed(&url) {
            return Err(fail(format!(
                "cannot fetch `{url}`: the host is not in APIPLANT_FETCH_ALLOW"
            )));
        }

        let mut outgoing = client()
            .request(method.clone(), url.clone())
            .timeout(fetch_timeout());
        for (name, value) in &request.headers {
            outgoing = outgoing.header(name, value);
        }
        if let Some(body) = body.clone() {
            outgoing = outgoing.body(body);
        }

        // reqwest's own `Display` stops at "error sending request"; the reason
        // is always one link further down. Walking the chain is what turns an
        // unactionable message into "connection refused".
        let response = outgoing.send().await.map_err(|e| {
            let mut reason = e.to_string();
            let mut source = std::error::Error::source(&e);
            while let Some(cause) = source {
                reason = format!("{reason}: {cause}");
                source = cause.source();
            }
            fail(format!("cannot fetch `{url}`: {reason}"))
        })?;

        let status = response.status();
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        if follow {
            if let (true, Some(location)) = (status.is_redirection(), location) {
                url = url
                    .join(&location)
                    .map_err(|e| fail(format!("cannot follow a redirect to `{location}`: {e}")))?;
                redirected = true;
                continue;
            }
        }

        let final_url = response.url().to_string();
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| fail(format!("cannot read the response body: {e}")))?;

        return Ok(FetchResponse {
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or_default().to_string(),
            headers,
            url: final_url,
            redirected,
            body: bytes.to_vec().into(),
        });
    }

    Err(fail(format!(
        "too many redirects fetching `{}`",
        request.url
    )))
}

/// The specifier the bootstrap is registered and entered under.
pub(crate) const BOOTSTRAP: &str = "ext:apiplant_js/bootstrap.js";

/// The `fetch` half of the runtime, imported by the bootstrap.
pub(crate) const FETCH: &str = "ext:apiplant_js/fetch.js";

const FETCH_SOURCE: &str = include_str!("../assets/fetch.js");

/// The bootstrap itself, compiled into the binary.
///
/// It lives in `assets/` and arrives here through `include_str!` rather than
/// `extension!`'s `esm = [dir …]` form, which does *not* embed the file: that
/// expands to `ExtensionFileSource::loaded_during_snapshot`, storing the build
/// machine's absolute `CARGO_MANIFEST_DIR` path. The snapshot now consumes the
/// source either way, but keeping it embedded means the extension is also
/// correct for a runtime built without one.
pub(crate) const BOOTSTRAP_SOURCE: &str = include_str!("../assets/bootstrap.js");

extension!(
    apiplant_js,
    // `deno_web` installs the Web globals the bootstrap imports; naming it here
    // is what guarantees it is initialised first.
    deps = [deno_webidl, deno_web],
    ops = [op_apiplant_host, op_apiplant_fetch],
    esm_entry_point = BOOTSTRAP,
    options = { current: Current },
    state = |state, options| state.put::<Current>(options.current),
);

/// The extension, with the bootstrap supplied as source rather than a path.
pub(crate) fn extension(current: Current) -> deno_core::Extension {
    let mut extension = apiplant_js::init(current);
    extension.esm_files = std::borrow::Cow::Owned(vec![
        ExtensionFileSource::new_computed(FETCH, FETCH_SOURCE.into()),
        ExtensionFileSource::new_computed(BOOTSTRAP, BOOTSTRAP_SOURCE.into()),
    ]);
    extension
}

/// A `Current` with no job attached — what the snapshot build and the tests use.
pub(crate) fn detached() -> Current {
    Rc::new(RefCell::new(None))
}

#[cfg(test)]
mod tests {
    use super::host_matches;

    /// The allowlist is the difference between a function that can reach the
    /// cloud metadata endpoint and one that cannot, so its edges are worth
    /// pinning: a suffix rule must not match a lookalike domain, and an absent
    /// host must never pass.
    #[test]
    fn the_egress_allowlist_matches_hosts_not_substrings() {
        assert!(host_matches(Some("api.stripe.com"), "api.stripe.com"));
        assert!(host_matches(
            Some("api.stripe.com"),
            "example.com, api.stripe.com"
        ));
        assert!(!host_matches(Some("api.stripe.com"), "stripe.com"));

        // `*.` covers the domain itself and any subdomain.
        assert!(host_matches(Some("stripe.com"), "*.stripe.com"));
        assert!(host_matches(Some("api.stripe.com"), "*.stripe.com"));
        assert!(host_matches(Some("a.b.stripe.com"), "*.stripe.com"));

        // The lookalike a naive `ends_with` would have allowed.
        assert!(!host_matches(Some("evilstripe.com"), "*.stripe.com"));
        assert!(!host_matches(Some("stripe.com.evil.net"), "*.stripe.com"));

        // Case and spacing in the rule are not the operator's problem.
        assert!(host_matches(Some("API.Stripe.com"), " *.STRIPE.com , "));

        // An empty rule allows nothing, and a URL with no host never passes —
        // both fail closed, which is the only safe direction here.
        assert!(!host_matches(Some("api.stripe.com"), ""));
        assert!(!host_matches(None, "*.stripe.com"));
    }
}
