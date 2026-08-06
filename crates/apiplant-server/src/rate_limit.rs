//! How often one client may call, and what happens when they call more often
//! than that.
//!
//! Three levels decide a limit, narrowest last: `main.toml`'s `[rate_limit]
//! default`, a resource's `[rate_limit]` (per action, or `all`), and a
//! function's `rate_limit` key. Every level is resolved here, once, at boot —
//! so a request costs one path split and one token-bucket check, not a walk
//! back up the configuration.
//!
//! ## Why this is one middleware and not one per route
//!
//! The route table is generic: `/{resource}` and `/{resource}/{id}` answer for
//! every resource an app declares, so there is no per-resource service to wrap.
//! The middleware therefore does the same match the router is about to do —
//! path shape plus method — and looks the answer up in a map built at boot. A
//! path that matches nothing in that map (`/auth/login`, `/billing/webhook`,
//! the docs) is limited by the global rule and nothing else.
//!
//! ## Who "one client" is
//!
//! The peer socket address, which the caller cannot forge. `X-Forwarded-For`
//! and `X-Real-IP` are read only when `trust_proxy_headers = true`, because a
//! header anybody can write is a rate limit anybody can leave. Behind a proxy
//! the flip side bites instead: every request arrives from the proxy's address,
//! so all callers share one bucket until that flag is set.
//!
//! The token buckets themselves come from [`ntex_ratelimiter`]; what lives here
//! is which bucket a request belongs in.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use apiplant_core::{App, CrudAction, RateLimitRule};
use ntex::http::header::{HeaderName, HeaderValue};
use ntex::http::Method;
use ntex::service::{Middleware, Service, ServiceCtx};
use ntex::web;
use ntex_ratelimiter::{RateLimitResult, RateLimiter, RateLimiterConfig};

use crate::functions::FunctionRegistry;
use crate::response::error;

const HEADER_LIMIT: &str = "x-ratelimit-limit";
const HEADER_REMAINING: &str = "x-ratelimit-remaining";
const HEADER_RESET: &str = "x-ratelimit-reset";
const HEADER_RETRY_AFTER: &str = "retry-after";

/// An endpoint that can carry its own limit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RouteKey {
    /// One action on one resource — `POST /products` is `("product", Create)`.
    Resource(String, CrudAction),
    /// One function, both its plain and its `/stream` endpoint: they are the
    /// same function, so they share a bucket.
    Function(String),
}

/// Every limit an app declared, resolved into the buckets that enforce them.
///
/// Each distinct limit gets its own [`RateLimiter`], which is what keeps the
/// counts separate: a caller who has used up a resource's `create` allowance
/// can still read, because those are different buckets, and a global limit
/// still counts every request either way.
pub struct RateLimitPolicy {
    /// Applies to any request no narrower rule claims. `None` = unlimited.
    global: Option<Arc<RateLimiter>>,
    /// Per-endpoint overrides. A present `None` is an explicit `off`, which is
    /// why this is not simply a map of limiters.
    routes: HashMap<RouteKey, Option<Arc<RateLimiter>>>,
    /// Prefix the API is mounted under, stripped before a path is matched.
    base_path: String,
    trust_proxy_headers: bool,
}

impl RateLimitPolicy {
    /// Resolve an app's configuration into buckets.
    ///
    /// Must be called from inside the async runtime: each limiter spawns a task
    /// that sweeps its own stale entries.
    pub fn build(app: &App, functions: &FunctionRegistry) -> RateLimitPolicy {
        let config = &app.config.rate_limit;
        let mut policy = RateLimitPolicy {
            global: None,
            routes: HashMap::new(),
            base_path: app.config.server.base_path.clone(),
            trust_proxy_headers: config.trust_proxy_headers,
        };
        // One switch turns the whole thing off, resources and functions
        // included — an empty policy limits nothing and costs one comparison
        // per request.
        if !config.enabled {
            return policy;
        }

        let make = |rule: RateLimitRule| -> Option<Arc<RateLimiter>> {
            let (requests, window_secs) = rule.limit()?;
            Some(RateLimiter::with_config(RateLimiterConfig {
                capacity: requests as usize,
                window: window_secs,
                cleanup_interval: Duration::from_secs(config.cleanup_interval_secs),
                stale_threshold: Duration::from_secs(config.stale_after_secs),
            }))
        };

        policy.global = make(config.default);

        for (name, resource) in &app.resources {
            for action in CrudAction::ALL {
                let rule = resource.rate_limit.for_action(action);
                if rule == RateLimitRule::Inherit {
                    continue;
                }
                policy
                    .routes
                    .insert(RouteKey::Resource(name.clone(), action), make(rule));
            }
        }

        for function in functions.iter() {
            let rule = function.rate_limit;
            if rule == RateLimitRule::Inherit {
                continue;
            }
            policy.routes.insert(
                RouteKey::Function(function.manifest.name.to_string()),
                make(rule),
            );
        }

        policy
    }

    /// A policy that limits nothing, for a host that assembles its own state.
    pub fn none() -> RateLimitPolicy {
        RateLimitPolicy {
            global: None,
            routes: HashMap::new(),
            base_path: String::new(),
            trust_proxy_headers: false,
        }
    }

    /// Whether any request could be refused. `false` means the middleware has
    /// nothing to do and skips the address lookup entirely.
    pub fn is_active(&self) -> bool {
        self.global.is_some() || self.routes.values().any(Option::is_some)
    }

    /// How many endpoints carry a limit of their own, for the boot log.
    pub fn overrides(&self) -> usize {
        self.routes.len()
    }

    /// The bucket a request belongs in, or `None` when it is not limited.
    fn limiter(&self, path: &str, method: &Method) -> Option<&Arc<RateLimiter>> {
        // Nothing overrides anything: the global rule is the whole answer, and
        // there is no reason to work out which endpoint this is (which costs an
        // allocation) to be told so.
        if self.routes.is_empty() {
            return self.global.as_ref();
        }
        match self.key(path, method) {
            // An endpoint that named a rule decides for itself, `off` included.
            Some(key) => match self.routes.get(&key) {
                Some(limiter) => limiter.as_ref(),
                None => self.global.as_ref(),
            },
            None => self.global.as_ref(),
        }
    }

    /// Which endpoint a path and method name — the router's own match, made
    /// early. `None` for everything else the server answers (auth, billing,
    /// docs, uploads), which the global rule covers.
    fn key(&self, path: &str, method: &Method) -> Option<RouteKey> {
        let path = path.strip_prefix(&self.base_path).unwrap_or(path);
        let mut segments = path.split('/').filter(|s| !s.is_empty());
        let first = segments.next()?;

        if first == "functions" {
            let name = segments.next()?;
            // `/functions/{name}` and `/functions/{name}/stream`, and nothing
            // deeper — the router has no such route either.
            return match segments.next() {
                None => Some(RouteKey::Function(name.to_string())),
                Some("stream") if segments.next().is_none() => {
                    Some(RouteKey::Function(name.to_string()))
                }
                _ => None,
            };
        }

        let action = match (segments.next(), segments.next(), segments.next()) {
            // `/{resource}`
            (None, _, _) => match *method {
                Method::GET => CrudAction::List,
                Method::POST => CrudAction::Create,
                _ => return None,
            },
            // `/{resource}/{id}`
            (Some(_), None, _) => match *method {
                Method::GET => CrudAction::Read,
                Method::PATCH | Method::PUT => CrudAction::Update,
                Method::DELETE => CrudAction::Delete,
                _ => return None,
            },
            // `/{parent}/{id}/{child}` — a listing of the child, so it is the
            // child's `list` limit that applies, not the parent's.
            (Some(_), Some(child), None) if method == Method::GET => {
                return Some(RouteKey::Resource(child.to_string(), CrudAction::List))
            }
            _ => return None,
        };
        Some(RouteKey::Resource(first.to_string(), action))
    }
}

impl std::fmt::Debug for RateLimitPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RateLimitPolicy")
            .field("global", &self.global.is_some())
            .field("overrides", &self.routes.len())
            .field("trust_proxy_headers", &self.trust_proxy_headers)
            .finish()
    }
}

/// The middleware `build_app!` wraps the API scope with.
pub struct RateLimit {
    policy: Arc<RateLimitPolicy>,
}

impl RateLimit {
    pub fn new(policy: Arc<RateLimitPolicy>) -> RateLimit {
        RateLimit { policy }
    }
}

impl<S> Middleware<S> for RateLimit {
    type Service = RateLimitService<S>;

    fn create(&self, service: S) -> Self::Service {
        RateLimitService {
            service,
            policy: Arc::clone(&self.policy),
        }
    }
}

pub struct RateLimitService<S> {
    service: S,
    policy: Arc<RateLimitPolicy>,
}

impl<S, Err> Service<web::WebRequest<Err>> for RateLimitService<S>
where
    S: Service<web::WebRequest<Err>, Response = web::WebResponse, Error = web::Error>,
    Err: web::ErrorRenderer,
{
    type Response = web::WebResponse;
    type Error = web::Error;

    ntex::forward_ready!(service);

    async fn call(
        &self,
        req: web::WebRequest<Err>,
        ctx: ServiceCtx<'_, Self>,
    ) -> Result<Self::Response, Self::Error> {
        let Some(limiter) = self.policy.limiter(req.path(), req.method()) else {
            return ctx.call(&self.service, req).await;
        };

        let result = limiter.check_rate_limit(client_ip(&req, self.policy.trust_proxy_headers));
        if !result.allowed {
            let mut response = error(
                429,
                "rate limit exceeded — too many requests, try again shortly",
            );
            // `Retry-After` is the one a client library acts on without being
            // taught anything; the `X-RateLimit-*` trio is what a human reads.
            let after = result.reset.saturating_sub(now_secs()).max(1);
            set(response.headers_mut(), HEADER_RETRY_AFTER, after);
            headers(response.headers_mut(), &result);
            return Ok(req.into_response(response));
        }

        let mut response = ctx.call(&self.service, req).await?;
        headers(response.headers_mut(), &result);
        Ok(response)
    }
}

/// The address the limit is counted against.
///
/// The peer socket by default, because it is the one thing in a request the
/// caller cannot choose. The proxy headers are read only where the deployment
/// says something in front of the server writes them; a forged `0.0.0.0` is
/// refused so a caller cannot land on the loopback fallback on purpose.
fn client_ip<Err>(req: &web::WebRequest<Err>, trust_proxy_headers: bool) -> IpAddr {
    if trust_proxy_headers {
        let forwarded = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            // The client is the first entry; everything after it is the chain
            // of proxies that handled the request.
            .and_then(|v| v.split(',').next())
            .or_else(|| req.headers().get("x-real-ip").and_then(|v| v.to_str().ok()));
        if let Some(ip) = forwarded
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
            .filter(|ip| !ip.is_unspecified())
        {
            return ip;
        }
    }
    req.peer_addr()
        .map(|peer| peer.ip())
        // No peer address at all is a test transport or a unix socket, not a
        // request off the network; count them together.
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn headers(map: &mut ntex::http::HeaderMap, result: &RateLimitResult) {
    set(map, HEADER_LIMIT, result.limit as u64);
    set(map, HEADER_REMAINING, result.remaining as u64);
    set(map, HEADER_RESET, result.reset);
}

fn set(map: &mut ntex::http::HeaderMap, name: &'static str, value: u64) {
    if let Ok(value) = HeaderValue::from_str(&value.to_string()) {
        map.insert(HeaderName::from_static(name), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(base_path: &str, routes: &[(RouteKey, bool)]) -> RateLimitPolicy {
        RateLimitPolicy {
            global: Some(RateLimiter::new(10, 60)),
            routes: routes
                .iter()
                .map(|(key, limited)| (key.clone(), limited.then(|| RateLimiter::new(1, 60))))
                .collect(),
            base_path: base_path.to_string(),
            trust_proxy_headers: false,
        }
    }

    fn key(path: &str, method: Method) -> Option<RouteKey> {
        policy("", &[]).key(path, &method)
    }

    #[ntex::test]
    async fn a_path_and_a_method_name_the_action_the_router_will_run() {
        use CrudAction::*;
        let resource = |name: &str, action| Some(RouteKey::Resource(name.to_string(), action));

        assert_eq!(key("/products", Method::GET), resource("products", List));
        assert_eq!(key("/products", Method::POST), resource("products", Create));
        assert_eq!(key("/products/7", Method::GET), resource("products", Read));
        assert_eq!(
            key("/products/7", Method::PATCH),
            resource("products", Update)
        );
        assert_eq!(
            key("/products/7", Method::PUT),
            resource("products", Update)
        );
        assert_eq!(
            key("/products/7", Method::DELETE),
            resource("products", Delete)
        );
        // A nested listing is the *child's* list endpoint.
        assert_eq!(key("/orders/7/lines", Method::GET), resource("lines", List));
    }

    #[ntex::test]
    async fn both_endpoints_of_a_function_share_one_key() {
        let expected = Some(RouteKey::Function("summarise".to_string()));
        assert_eq!(key("/functions/summarise", Method::POST), expected);
        assert_eq!(key("/functions/summarise/stream", Method::POST), expected);
        // Nothing is mounted deeper than `/stream`.
        assert_eq!(key("/functions/summarise/stream/more", Method::POST), None);
    }

    #[ntex::test]
    async fn the_base_path_is_stripped_before_a_path_is_matched() {
        let policy = policy("/api", &[]);
        assert_eq!(
            policy.key("/api/products", &Method::GET),
            Some(RouteKey::Resource("products".to_string(), CrudAction::List))
        );
    }

    #[ntex::test]
    async fn a_path_no_crud_route_answers_falls_to_the_global_rule() {
        // Not a resource route: the method has no CRUD meaning here.
        assert_eq!(key("/products", Method::HEAD), None);
        assert_eq!(key("/", Method::GET), None);
        // Deeper than any route the router registers.
        assert_eq!(key("/a/b/c/d", Method::GET), None);
    }

    #[ntex::test]
    async fn an_override_decides_for_its_own_endpoint_and_leaves_the_rest_global() {
        let limited = RouteKey::Resource("products".to_string(), CrudAction::Create);
        let exempt = RouteKey::Resource("products".to_string(), CrudAction::List);
        let policy = policy("", &[(limited, true), (exempt, false)]);

        // Its own bucket, not the global one.
        let own = policy.limiter("/products", &Method::POST).unwrap();
        assert_eq!(own.stats().capacity, 1);
        // `off` means no bucket at all, even though a global rule exists.
        assert!(policy.limiter("/products", &Method::GET).is_none());
        // Anything else still counts against the global rule.
        assert_eq!(
            policy
                .limiter("/products/7", &Method::GET)
                .unwrap()
                .stats()
                .capacity,
            10
        );
    }

    #[ntex::test]
    async fn an_empty_policy_is_inactive_and_a_global_rule_makes_it_active() {
        assert!(!RateLimitPolicy::none().is_active());
        assert!(policy("", &[]).is_active());

        let off_everywhere = RateLimitPolicy {
            global: None,
            routes: [(
                RouteKey::Resource("products".to_string(), CrudAction::List),
                None,
            )]
            .into_iter()
            .collect(),
            base_path: String::new(),
            trust_proxy_headers: false,
        };
        assert!(!off_everywhere.is_active());
    }
}
