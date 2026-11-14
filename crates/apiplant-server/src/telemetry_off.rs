//! [`crate::telemetry`] in a build without the `telemetry` feature.
//!
//! Same names and signatures, minus OpenTelemetry: [`init`] still installs the
//! log subscriber (format and level from `[observability.logs]`), because a
//! server that stops logging when you drop a feature has dropped more than
//! the feature. Nothing is traced, measured or exported. An app that sets
//! `[observability] enabled = true` is refused at boot by
//! [`crate::optional::check`] rather than served unmeasured.

use std::sync::Arc;

use apiplant_core::{LogFormat, ObservabilityConfig};
use ntex::service::Middleware;
use tracing::Span;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Nothing to keep alive: there are no exporters to flush.
#[must_use = "kept for parity with the `telemetry` build, where dropping it shuts the exporters down"]
pub struct Guard;

/// Install the global log subscriber. `RUST_LOG` wins over the file, as in the
/// full build.
pub fn init(config: &ObservabilityConfig, _app_name: &str) -> Guard {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.logs.level.clone()));
    let stdout = match config.logs.format {
        LogFormat::Pretty => tracing_subscriber::fmt::layer().boxed(),
        LogFormat::Compact => tracing_subscriber::fmt::layer().compact().boxed(),
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .json()
            .with_current_span(config.logs.span_fields)
            .with_span_list(config.logs.span_fields)
            .boxed(),
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(stdout)
        .init();
    Guard
}

/// Record a failure onto the current span. Kept because the rest of the crate
/// calls it everywhere; with no OpenTelemetry layer the fields only reach a
/// log line written inside a span that declared them.
pub fn record_error(kind: &'static str, detail: impl std::fmt::Display) {
    let span = Span::current();
    span.record("error.type", kind);
    span.record("exception.type", kind);
    span.record("exception.message", tracing::field::display(&detail));
}

/// Always inactive.
pub struct TelemetryPolicy;

impl TelemetryPolicy {
    pub fn build(_config: &ObservabilityConfig, _base_path: &str) -> TelemetryPolicy {
        TelemetryPolicy
    }

    pub fn is_active(&self) -> bool {
        false
    }
}

/// A middleware that is not there: it hands back the service it wraps.
pub struct Telemetry;

impl Telemetry {
    pub fn new(_policy: Arc<TelemetryPolicy>) -> Telemetry {
        Telemetry
    }
}

impl<S, St> Middleware<S, St> for Telemetry {
    type Service = S;

    fn create(&self, _: &St, service: S) -> S {
        service
    }
}
