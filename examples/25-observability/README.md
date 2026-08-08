# 25 · Observability

Logs, traces and metrics, over [OpenTelemetry] — and a Grafana stack in one
container to point them at.

```
25-observability/
├── main.toml          # the [observability] blocks; everything else is example 02
├── compose.yaml       # grafana/otel-lgtm: Loki, Grafana, Tempo, Prometheus
├── resources/note.toml   # an ordinary resource, here to be traffic
└── functions/work.ts  # one slow endpoint, one that throws, one that works
```

The app is deliberately boring. What this example is about is what the server
says about itself while it serves it.

## Run it

```bash
docker compose -f examples/25-observability/compose.yaml up -d   # Grafana on :3000
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_observability
cargo run -p apiplant -- build examples/25-observability
cargo run -p apiplant -- run examples/25-observability
```

`grafana/otel-lgtm` is Grafana's all-in-one image: an OpenTelemetry Collector on
`:4318` with Tempo (traces), Prometheus (metrics), Loki (logs) and Grafana behind
it, already wired to each other. It is a demo image — storage lives in the
container and goes with it.

Then make something happen:

```bash
B=localhost:8099/api

curl -s -XPOST $B/note -H 'content-type: application/json' \
  -d '{"title":"Watched","slug":"watched-1","priority":1}'
curl -s $B/functions/count
curl -s -XPOST $B/functions/slow -H 'content-type: application/json' -d '{"ms":750}'
curl -s -XPOST $B/functions/boom            # → 500
```

Open <http://localhost:3000> → **Explore** → **Tempo**, and search
`{resource.service.name="apiplant-observability-demo"}`. Four traces, one of them
red.

## The two switches

`[observability] enabled` and `[observability.otlp] endpoint` are separate, and
that is the design:

| | spans built | exported |
|---|---|---|
| neither | no | no |
| `enabled` only | **yes** | no |
| both | yes | **yes** |

`enabled` alone still gives every request a span, and every log line written
during that request inherits the span's fields — route, method, trace id. That
is most of the value of observability for none of the infrastructure: no
collector, no container, no bill. Comment the `[observability.otlp]` block out of
`main.toml` and the app still logs like this — note the `span` object, which
`logs.span_fields` puts there:

```json
{"level":"ERROR","fields":{"message":"function faulted","function":"boom"},
 "target":"apiplant_server::function_routes",
 "span":{"http.route":"/api/functions/boom","http.request.method":"POST",
          "otel.status_code":"ERROR","error.type":"function_fault",
          "trace_id":"3a7fcca366fda908dde8b6e00a8dd914"}}
```

"Show me every line from the request that failed" is then a filter on
`trace_id`, not an archaeology project.

One gap worth knowing about: `log.info` **called from inside a function** is
handed to the host from the runtime's own thread, so it is written without the
request's span —

```json
{"level":"INFO","fields":{"message":"count: 1 notes"},"target":"apiplant::function"}
```

no `span`, no `trace_id`. Lines the server writes around your function are
correlated; lines your function writes itself are not yet. Put anything you
intend to search by trace id in the response or let the error propagate, which
does land on the span.

## What a span carries

Fetch any trace and the attributes are the OpenTelemetry HTTP semantic
conventions, not names invented here — which is why a stock Grafana dashboard
works on the first try:

```
SPAN: POST /api/functions/boom     status = STATUS_CODE_ERROR
    http.request.method       = POST
    http.route                = /api/functions/boom
    url.path                  = /api/functions/boom
    http.response.status_code = 500
    error.type                = function_fault
    exception.message         = the thing that was supposed to work did not
    trace_id                  = e0821b9e723e6b7b77ff1d64329018f4
```

Note what the *client* got from that request:

```json
{"error":"internal function error"}
```

The message stays out of the response and goes to your trace instead. The caller
learns nothing; you learn what threw.

## Five things to try

**1. The route template, not the path.** `GET /api/note/<some-uuid>` records
`http.route = /api/note/{id}`. Ids are replaced before anything is labelled with
them, because a metric labelled per row grows a time series per row — the
standard way a monitoring bill goes from tens of dollars to thousands. Names are
left alone, so `/api/functions/slow` stays itself.

**2. `X-Trace-Id` on every response.** Turned on by `traces.response_header`:

```bash
curl -si -XPOST $B/functions/boom | grep -i x-trace-id
```

Paste that into Tempo's search box. A bug report becomes a lookup instead of a
reconstruction from timestamps.

**3. Someone else's trace is continued, not restarted.** Send a `traceparent`
and the response comes back inside *that* trace:

```bash
curl -si $B/note -H 'traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01' \
  | grep -i x-trace-id
# x-trace-id: 4bf92f3577b34da6a3ce929d0e0e4736
```

Without this the service is an island and every trace stops at your gateway.

**4. Credentials are refused even when you ask for them.** `main.toml` lists
`authorization` in `capture_headers` on purpose:

```bash
curl -s $B/note -H 'x-request-id: probe' -H 'authorization: Bearer SUPERSECRET'
```

The span gets `http.request.header.x-request-id = probe`. It does **not** get the
authorization header — `authorization`, `proxy-authorization`, `cookie`,
`set-cookie` and `x-api-key` are dropped whatever the config says, because a
captured bearer token is a credential sitting in your log aggregator.

**5. Health checks cost nothing.** `exclude_paths = ["/_health"]`, so
`GET /api/_health` produces no span *and* no metric. Curl it ten times and it
appears in neither backend. A liveness probe every second is otherwise a span
every second, forever, telling you nothing.

## The metrics

Two instruments, both named as the HTTP conventions define them:
`http.server.request.duration` (a histogram, seconds) and
`http.server.active_requests`. In Prometheus, that is:

```promql
sum by (http_route, http_response_status_code) (
  rate(http_server_request_duration_seconds_count{service_name="apiplant-observability-demo"}[5m])
)

# p95 latency per route
histogram_quantile(0.95, sum by (le, http_route) (
  rate(http_server_request_duration_seconds_bucket[5m])
))
```

Request rate, error rate and latency per route — the three you actually page on
— fall out of those two without any code in a handler.

## Somewhere other than localhost

OTLP/HTTP is the only export format, because it is the one everything speaks.
Nothing about this example is Grafana-specific: point `endpoint` at an
OpenTelemetry Collector, Jaeger, Honeycomb, Datadog, Grafana Cloud or New Relic
and it works unchanged. For a hosted backend, add the key as a header:

```toml
[observability.otlp]
endpoint = "https://otlp.example.com"
headers  = { "x-api-key" = "$OTLP_KEY" }
```

The standard `OTEL_*` environment variables are read when the matching key is
unset — `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`,
`OTEL_SERVICE_NAME`, `OTEL_TRACES_SAMPLER_ARG` — so a container can be pointed at
a collector without editing the file. Sampling is parent-based: drop
`sample_ratio` to `0.05` in production and traces are kept or dropped whole,
never left with holes in them.

Misconfiguration is never fatal. An unreachable collector, a bad endpoint, a
malformed header: each disables the part of the pipeline it belongs to and says
so on stderr. A server that refuses to boot because its monitoring is wrong has
turned an observability problem into an outage.

Full reference: [`[observability]`](../../docs/configuration.md#observability).

[OpenTelemetry]: https://opentelemetry.io
