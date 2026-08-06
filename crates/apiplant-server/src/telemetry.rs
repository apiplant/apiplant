//! Logs, traces and metrics — what the server says about itself.
//!
//! Three things live here, in the order a request meets them:
//!
//! 1. [`init`], called once at startup, which decides what a log line looks
//!    like and whether an OTLP exporter is running behind it.
//! 2. [`Telemetry`], the middleware that gives every request a span, joins it
//!    to whatever trace the caller was already in, and records the two metrics
//!    a dashboard is built from.
//! 3. [`record_error`], which is how the rest of the crate reports a failure
//!    onto the span it is already inside.
//!
//! ## Why the span is the unit and not the log line
//!
//! A log line says one thing happened. A span says one thing happened, how
//! long it took, what it was part of, and what else happened inside it — and
//! the log lines written during it inherit its fields, so "show me every line
//! from the request that 500'd" is a filter rather than an archaeology
//! project. That is true even with no collector configured, which is why
//! spans are built whenever `[observability] enabled` is set and not only when
//! there is somewhere to send them.
//!
//! ## Semantic conventions
//!
//! Field names follow the OpenTelemetry HTTP semantic conventions
//! (`http.request.method`, `http.route`, `http.response.status_code`,
//! `error.type`) rather than anything of our own invention. It is the
//! difference between a Grafana dashboard that works on the first try and one
//! somebody has to be taught the app to write.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apiplant_core::{LogFormat, ObservabilityConfig, OtlpProtocol, TracesConfig};
use ntex::http::header::{HeaderName, HeaderValue};
use ntex::service::{Middleware, Service, ServiceCtx};
use ntex::web;
use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry::{global, KeyValue};
// The builder methods below live on traits, not on the builders themselves.
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use tracing::Span;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// The instrumentation scope every span and metric from this process carries.
const SCOPE: &str = "apiplant";

const HEADER_TRACE_ID: &str = "x-trace-id";

/// Keeps the exporters alive, and flushes them when the process ends.
///
/// Dropping this is the only thing that makes a batch exporter deliver what it
/// is still holding, so it has to outlive the server — hold it in `main` for
/// the length of the run. A process that exits without dropping it loses the
/// last batch, which is reliably the interesting one.
#[must_use = "dropping the guard immediately shuts the exporters down again"]
pub struct Guard {
    traces: Option<SdkTracerProvider>,
    metrics: Option<SdkMeterProvider>,
}

impl Drop for Guard {
    fn drop(&mut self) {
        // Both report their own failures and neither is worth failing an exit
        // over: the process is already on its way out, and a collector that
        // cannot be reached now could not be reached a moment ago either.
        if let Some(provider) = self.traces.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("apiplant: flushing traces on shutdown failed: {e}");
            }
        }
        if let Some(provider) = self.metrics.take() {
            if let Err(e) = provider.shutdown() {
                eprintln!("apiplant: flushing metrics on shutdown failed: {e}");
            }
        }
    }
}

/// Install the global subscriber, and the OTLP exporters if one is configured.
///
/// Call once, as early in `main` as the configuration allows — anything logged
/// before it is lost, and a second call is ignored by the `tracing` global.
///
/// Never fatal. An unreachable collector, a malformed endpoint, a header that
/// is not valid HTTP: each of those disables the part of the pipeline it
/// belongs to and says so on stderr. A server that refuses to boot because its
/// monitoring is misconfigured has turned an observability problem into an
/// outage, which is exactly backwards.
pub fn init(config: &ObservabilityConfig, app_name: &str) -> Guard {
    // `RUST_LOG` wins over the file: it is what someone reaches for while a
    // container is running, and a config file they cannot edit from there
    // should not override them.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.logs.level.clone()));

    let stdout = match config.logs.format {
        LogFormat::Pretty => tracing_subscriber::fmt::layer().boxed(),
        LogFormat::Compact => tracing_subscriber::fmt::layer().compact().boxed(),
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            // Without this a JSON line knows nothing about the request it was
            // written during, which throws away the main reason to emit JSON.
            .with_current_span(config.logs.span_fields)
            .with_span_list(config.logs.span_fields)
            .boxed(),
    };

    if !config.is_active() {
        tracing_subscriber::registry()
            .with(filter)
            .with(stdout)
            .init();
        return Guard {
            traces: None,
            metrics: None,
        };
    }

    // The exporter's TLS is built against whatever provider the process has
    // installed, and it may run before anything else has installed one — an
    // HTTPS collector with no provider is a panic on the first export, on a
    // background thread, at whatever moment the first batch fills.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let resource = resource(config, app_name);
    let endpoint = config.endpoint();
    let headers: HashMap<String, String> = config.export_headers().into_iter().collect();

    let traces = config
        .traces
        .enabled
        .then(|| tracer_provider(config, &endpoint, &headers, resource.clone()));
    let metrics = config
        .metrics
        .enabled
        .then(|| meter_provider(config, &endpoint, &headers, resource))
        .flatten();

    // `traceparent` in and out. Without a propagator installed, an incoming
    // trace context is ignored and this service starts a new trace for a
    // request that was already part of one — the classic "why does my trace
    // stop at the gateway" report.
    global::set_text_map_propagator(TraceContextPropagator::new());

    let otel = traces.as_ref().map(|provider| {
        global::set_tracer_provider(provider.clone());
        tracing_opentelemetry::layer()
            .with_tracer(provider.tracer(SCOPE))
            // Four attributes the layer adds by default that are worth
            // nothing here and are paid for on every exported span: the
            // source location (always this file), the worker thread's name
            // and id (an ntex worker number), and busy/idle nanoseconds
            // (a duration the span already has).
            .with_location(false)
            .with_threads(false)
            .with_tracked_inactivity(false)
    });
    if let Some(provider) = &metrics {
        global::set_meter_provider(provider.clone());
    }

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout)
        .with(otel)
        .init();

    match &endpoint {
        Some(endpoint) => tracing::info!(
            %endpoint,
            traces = config.traces.enabled,
            metrics = metrics.is_some(),
            sample_ratio = config.traces.sample_ratio,
            "observability: exporting over OTLP",
        ),
        // Worth a line: someone who set `enabled = true` and no endpoint is
        // one key away from what they wanted, and silence looks identical to
        // a collector that is quietly refusing the data.
        None => tracing::info!(
            "observability: tracing in-process only — set [observability.otlp] endpoint to export"
        ),
    }

    Guard { traces, metrics }
}

/// Who is reporting: the identity every span and metric is tagged with.
fn resource(config: &ObservabilityConfig, app_name: &str) -> Resource {
    use opentelemetry_semantic_conventions::resource;

    let mut attributes = vec![KeyValue::new(
        resource::SERVICE_VERSION,
        config
            .service_version
            .clone()
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string()),
    )];
    if let Some(environment) = &config.environment {
        attributes.push(KeyValue::new(
            resource::DEPLOYMENT_ENVIRONMENT_NAME,
            environment.clone(),
        ));
    }
    for (key, value) in &config.resource_attributes {
        attributes.push(KeyValue::new(key.clone(), value.clone()));
    }

    Resource::builder()
        .with_service_name(config.service_name(app_name))
        .with_attributes(attributes)
        .build()
}

/// A tracer provider, exporting if there is an endpoint and sampling either
/// way.
///
/// The no-endpoint case still builds a provider: the spans go nowhere, but
/// they exist, which is what gives the log lines their trace id and their
/// request fields.
fn tracer_provider(
    config: &ObservabilityConfig,
    endpoint: &Option<String>,
    headers: &HashMap<String, String>,
    resource: Resource,
) -> SdkTracerProvider {
    let builder = SdkTracerProvider::builder()
        .with_resource(resource)
        // Parent-based, so a trace is sampled as one piece: a request that
        // arrives already sampled is kept whatever the ratio says, and a child
        // span never contradicts its root. Sampling each span independently
        // produces traces with holes in them, which are worse than no traces.
        .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
            config.traces.sample_ratio,
        ))));

    let Some(endpoint) = endpoint else {
        return builder.build();
    };

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(format!("{endpoint}/v1/traces"))
        .with_protocol(protocol(config.otlp.protocol))
        .with_timeout(Duration::from_secs(config.otlp.timeout_secs))
        .with_headers(headers.clone())
        .build();

    match exporter {
        // Batched rather than one request per span: a span-per-request
        // exporter puts an HTTP round trip on the critical path of every
        // request it is measuring.
        Ok(exporter) => builder.with_batch_exporter(exporter).build(),
        Err(e) => {
            eprintln!("apiplant: OTLP trace exporter disabled: {e}");
            builder.build()
        }
    }
}

/// A meter provider, or `None` when there is nowhere to push to — metrics,
/// unlike spans, are worth nothing in-process.
fn meter_provider(
    config: &ObservabilityConfig,
    endpoint: &Option<String>,
    headers: &HashMap<String, String>,
    resource: Resource,
) -> Option<SdkMeterProvider> {
    let endpoint = endpoint.as_ref()?;
    let exporter = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(format!("{endpoint}/v1/metrics"))
        .with_protocol(protocol(config.otlp.protocol))
        .with_timeout(Duration::from_secs(config.otlp.timeout_secs))
        .with_headers(headers.clone())
        .build();

    match exporter {
        Ok(exporter) => Some(
            SdkMeterProvider::builder()
                .with_reader(
                    PeriodicReader::builder(exporter)
                        .with_interval(Duration::from_secs(config.metrics.interval_secs))
                        .build(),
                )
                .with_resource(resource)
                .build(),
        ),
        Err(e) => {
            eprintln!("apiplant: OTLP metric exporter disabled: {e}");
            None
        }
    }
}

fn protocol(protocol: OtlpProtocol) -> opentelemetry_otlp::Protocol {
    match protocol {
        OtlpProtocol::HttpProtobuf => opentelemetry_otlp::Protocol::HttpBinary,
        OtlpProtocol::HttpJson => opentelemetry_otlp::Protocol::HttpJson,
    }
}

/// Report a failure onto the span the current request is already inside.
///
/// The span is what a trace backend colours red and what an alert counts, so a
/// handler that returns a 500 without calling this produces a trace that says
/// the request succeeded slowly. `kind` is the low-cardinality label the error
/// is grouped by — `"database"`, `"function_panic"` — and `detail` is the
/// message a person reads.
///
/// Deliberately usable from anywhere: with no span active it is a no-op, so a
/// helper called from both a request and a background task does not need to
/// know which it is in.
pub fn record_error(kind: &'static str, detail: impl std::fmt::Display) {
    let span = Span::current();
    span.record("error.type", kind);
    span.record("otel.status_code", "ERROR");
    // The `exception.*` names are the convention a backend renders as an error
    // event on the span rather than as one more attribute nobody looks at.
    span.record("exception.type", kind);
    span.record("exception.message", tracing::field::display(&detail));
}

/// The two numbers every HTTP dashboard is built from.
///
/// Both are the names the OpenTelemetry HTTP conventions define, so a stock
/// dashboard finds them without configuration.
struct Instruments {
    duration: opentelemetry::metrics::Histogram<f64>,
    active: opentelemetry::metrics::UpDownCounter<i64>,
}

impl Instruments {
    fn new() -> Instruments {
        let meter = global::meter(SCOPE);
        Instruments {
            duration: meter
                .f64_histogram("http.server.request.duration")
                .with_unit("s")
                .with_description("Duration of inbound HTTP requests.")
                .build(),
            active: meter
                .i64_up_down_counter("http.server.active_requests")
                .with_unit("{request}")
                .with_description("Requests currently being handled.")
                .build(),
        }
    }
}

/// Everything the middleware needs, resolved once at boot.
pub struct TelemetryPolicy {
    config: TracesConfig,
    /// Stripped from a path before it is turned into a route template, so
    /// `/api/products/7` and `/products/7` are the same route.
    base_path: String,
    instruments: Option<Instruments>,
}

impl TelemetryPolicy {
    /// Build the policy for an app. Must be called after [`init`], because the
    /// instruments are created against the global meter that installs.
    pub fn build(config: &ObservabilityConfig, base_path: &str) -> TelemetryPolicy {
        TelemetryPolicy {
            config: if config.enabled {
                config.traces.clone()
            } else {
                TracesConfig {
                    enabled: false,
                    ..TracesConfig::default()
                }
            },
            base_path: base_path.to_string(),
            instruments: (config.enabled && config.metrics.enabled).then(Instruments::new),
        }
    }

    /// Nothing measured, nothing recorded — one comparison per request.
    pub fn is_active(&self) -> bool {
        self.config.enabled || self.instruments.is_some()
    }

    fn excluded(&self, path: &str) -> bool {
        let path = path.strip_prefix(&self.base_path).unwrap_or(path);
        self.config
            .exclude_paths
            .iter()
            .any(|excluded| path.starts_with(excluded.as_str()))
    }
}

/// The low-cardinality name of the endpoint a path belongs to.
///
/// `/products/9f2a…` becomes `/products/{id}`, because a metric labelled with
/// the id would grow a time series per row in the table — the standard way to
/// take a monitoring bill from tens of dollars to thousands. Only the segments
/// that are recognisably identifiers are replaced; a fixed path is left alone
/// so `/auth/login` still reads as itself.
fn route_template(path: &str) -> String {
    let mut route = String::with_capacity(path.len());
    for segment in path.split('/').skip(1) {
        route.push('/');
        if looks_like_an_id(segment) {
            route.push_str("{id}");
        } else {
            route.push_str(segment);
        }
    }
    if route.is_empty() {
        route.push('/');
    }
    route
}

/// Whether a path segment is a value rather than a name.
///
/// Digits and UUIDs cover every id this server issues. A slug like
/// `summarise-text` is left alone: it names a function, and a handful of those
/// is exactly the cardinality a dashboard wants.
fn looks_like_an_id(segment: &str) -> bool {
    if segment.is_empty() {
        return false;
    }
    if segment.bytes().all(|b| b.is_ascii_digit()) {
        return true;
    }
    // A UUID in any of the spellings serde will have accepted.
    let hex = segment.len() == 36
        && segment.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        });
    hex
}

/// The middleware `build_app!` wraps the API scope with.
pub struct Telemetry {
    policy: Arc<TelemetryPolicy>,
}

impl Telemetry {
    pub fn new(policy: Arc<TelemetryPolicy>) -> Telemetry {
        Telemetry { policy }
    }
}

impl<S> Middleware<S> for Telemetry {
    type Service = TelemetryService<S>;

    fn create(&self, service: S) -> Self::Service {
        TelemetryService {
            service,
            policy: Arc::clone(&self.policy),
        }
    }
}

pub struct TelemetryService<S> {
    service: S,
    policy: Arc<TelemetryPolicy>,
}

impl<S, Err> Service<web::WebRequest<Err>> for TelemetryService<S>
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
        if !self.policy.is_active() || self.policy.excluded(req.path()) {
            return ctx.call(&self.service, req).await;
        }

        let method = req.method().to_string();
        let route = route_template(req.path());
        let attributes = vec![
            KeyValue::new("http.request.method", method.clone()),
            KeyValue::new("http.route", route.clone()),
        ];

        if let Some(instruments) = &self.policy.instruments {
            instruments.active.add(1, &attributes);
        }
        let started = Instant::now();

        let span = self
            .policy
            .config
            .enabled
            .then(|| self.span(&req, &method, &route));
        let trace_id = span.as_ref().and_then(trace_id);
        // Recorded onto the span so every log line written inside the request
        // carries it. Without this the logs and the traces are two piles of
        // data about the same request with no way to join them — which is the
        // whole point of running both.
        if let (Some(span), Some(trace_id)) = (&span, &trace_id) {
            span.record("trace_id", trace_id.as_str());
        }

        let result = match &span {
            Some(span) => {
                use tracing::Instrument;
                ctx.call(&self.service, req).instrument(span.clone()).await
            }
            None => ctx.call(&self.service, req).await,
        };

        let elapsed = started.elapsed().as_secs_f64();
        // The status is the one label a dashboard splits by, and it is only
        // known now — hence recorded here rather than declared above.
        let status = match &result {
            Ok(response) => response.status().as_u16(),
            // An error that reached the middleware never became a response;
            // ntex will render it as a 500, so that is what was served.
            Err(_) => 500,
        };

        if let Some(span) = &span {
            span.record("http.response.status_code", status);
            match &result {
                Err(e) => record_error("unhandled", e),
                // A 5xx is a failure whether or not anything called
                // `record_error` on the way out — a handler that returns
                // `error(500, …)` by hand should still colour the trace red.
                Ok(_) if status >= 500 => record_error("http_server_error", status),
                Ok(_) => {}
            }
        }

        if let Some(instruments) = &self.policy.instruments {
            instruments.active.add(-1, &attributes);
            let mut attributes = attributes;
            attributes.push(KeyValue::new("http.response.status_code", status as i64));
            instruments.duration.record(elapsed, &attributes);
        }

        let mut response = result?;
        // Handing the trace id back is what lets a user's bug report be looked
        // up directly instead of reconstructed from a timestamp.
        if self.policy.config.response_header {
            if let Some(trace_id) = trace_id {
                if let Ok(value) = HeaderValue::from_str(&trace_id) {
                    response
                        .headers_mut()
                        .insert(HeaderName::from_static(HEADER_TRACE_ID), value);
                }
            }
        }
        Ok(response)
    }
}

impl<S> TelemetryService<S> {
    /// The span for one request: named for its route, parented to whatever
    /// trace the caller was already in.
    fn span<Err>(&self, req: &web::WebRequest<Err>, method: &str, route: &str) -> Span {
        // `otel.name` and `otel.kind` are how `tracing-opentelemetry` is told
        // what the exported span should be called and that it is a server
        // span; the `Empty` fields are recorded once the response exists.
        let span = tracing::info_span!(
            "http.request",
            otel.name = %format!("{method} {route}"),
            otel.kind = "server",
            otel.status_code = tracing::field::Empty,
            trace_id = tracing::field::Empty,
            "http.request.method" = %method,
            "http.route" = %route,
            "url.path" = %req.path(),
            "url.query" = tracing::field::Empty,
            "http.response.status_code" = tracing::field::Empty,
            "error.type" = tracing::field::Empty,
            "exception.type" = tracing::field::Empty,
            "exception.message" = tracing::field::Empty,
        );

        let query = req.query_string();
        if !query.is_empty() {
            span.record("url.query", query);
        }

        for name in &self.policy.config.capture_headers {
            // Refused rather than trusted to the operator: a captured
            // `authorization` header is a bearer token written to a log
            // aggregator, and that is a credential leak no config key should
            // be able to ask for.
            if is_sensitive(name) {
                continue;
            }
            if let Some(value) = req
                .headers()
                .get(name.as_str())
                .and_then(|v| v.to_str().ok())
            {
                span.set_attribute(format!("http.request.header.{name}"), value.to_string());
            }
        }

        // Continue the caller's trace when they sent one. Extracting from the
        // real headers is the whole of distributed tracing: without it this
        // service's spans form an island.
        let parent = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(req.headers()))
        });
        if parent.span().span_context().is_valid() {
            // Fails only when there is no OpenTelemetry layer to attach to,
            // which is the untraced configuration — nothing to report.
            let _ = span.set_parent(parent);
        }
        span
    }
}

/// Headers that must never be copied onto a span, whatever the config says.
fn is_sensitive(name: &str) -> bool {
    matches!(
        name,
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie" | "x-api-key"
    )
}

/// The trace id of a span, as the 32 hex digits a backend's search box wants.
fn trace_id(span: &Span) -> Option<String> {
    let context = span.context();
    let span_context = context.span().span_context().clone();
    span_context
        .is_valid()
        .then(|| span_context.trace_id().to_string())
}

/// Reads `traceparent` / `tracestate` out of ntex's header map for the
/// propagator, which is defined over a generic key-value getter.
struct HeaderExtractor<'a>(&'a ntex::http::HeaderMap);

impl opentelemetry::propagation::Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|name| name.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_segment_is_replaced_and_a_name_is_not() {
        assert_eq!(route_template("/products/7"), "/products/{id}");
        assert_eq!(
            route_template("/products/2f8a1c4e-1111-4222-8333-abcdefabcdef"),
            "/products/{id}"
        );
        assert_eq!(route_template("/auth/login"), "/auth/login");
        assert_eq!(
            route_template("/functions/summarise-text"),
            "/functions/summarise-text"
        );
        // A nested listing keeps both names and loses only the parent's id.
        assert_eq!(route_template("/orders/7/lines"), "/orders/{id}/lines");
        assert_eq!(route_template("/"), "/");
    }

    #[test]
    fn a_near_uuid_is_not_mistaken_for_one() {
        // Right length, wrong shape: dashes in the wrong places.
        assert!(!looks_like_an_id("2f8a1c4e11114222-8333-abcdefabcdefab"));
        // Right shape, not hex.
        assert!(!looks_like_an_id("zf8a1c4e-1111-4222-8333-abcdefabcdef"));
        assert!(!looks_like_an_id(""));
    }

    #[test]
    fn excluded_paths_are_matched_under_the_base_path() {
        let policy = TelemetryPolicy {
            config: TracesConfig {
                exclude_paths: vec!["/_health".to_string()],
                ..TracesConfig::default()
            },
            base_path: "/api".to_string(),
            instruments: None,
        };
        assert!(policy.excluded("/api/_health"));
        assert!(!policy.excluded("/api/products"));
    }

    #[test]
    fn credential_headers_are_never_captured() {
        assert!(is_sensitive("authorization"));
        assert!(is_sensitive("cookie"));
        assert!(!is_sensitive("x-request-id"));
    }
}
