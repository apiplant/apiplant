# 21 · Docker

Every example so far ran from a checkout. This one ships: a `Dockerfile` that
puts the app inside `ghcr.io/apiplant/apiplant`, and a `compose.yaml` that runs
it next to a Postgres.

The whole idea is that the server is already built. The published image carries
the `apiplant` binary, so a deployment image is that image plus your app
directory — no cargo, no `pnpm build`, no compiled framework in your registry.
Three files' worth of app is the payload; the runtime is a `FROM` line.

```bash
docker compose up --build
docker compose run --rm seed          # optional: the usual fixture
open http://localhost:8080/api/docs
```

```bash
curl localhost:8080/api/note
curl localhost:8080/api/functions/status     # {"release":"local","env":"compose","notes":0}
curl localhost:8080/api/functions/version
curl -X POST localhost:8080/api/note \
  -H 'content-type: application/json' \
  -d '{"title":"Shipped","body":"From inside a container."}'
```

## What the Dockerfile does

Three stages, and only the last one ships:

| Stage | Base | For |
|---|---|---|
| `apiplant` | `ghcr.io/apiplant/apiplant:latest` | naming the runtime once, so every stage agrees |
| `functions` | `rust:1-bookworm` | compiling `functions/` — the only stage that needs a toolchain |
| runtime | the `apiplant` stage | the server, the TOML, and the built `.so` |

`apiplant build` shells out per language — cargo for `.rs`, `cc` for `.c`, `zig`
for `.zig`, `go` for `.go` — and the runtime image carries none of them, on
purpose: a compiler in production is weight and attack surface. So the build
stage supplies one, by copying the `apiplant` binary *out* of the runtime image
onto a base that has cargo. Swapping `rust:1-bookworm` for `golang:` builds Go
functions the same way.

TypeScript needs none of this. `version.ts` is transpiled in-process by the
`apiplant` binary itself, so an app whose functions are all TypeScript can
delete the middle stage and run `apiplant build /app` directly on the runtime
image. This example has one of each — `status.rs` and `version.ts` — so both
paths are visible; they are mounted, authenticated and documented identically,
which is the point.

## What the deployment decides

Nothing in the image knows which environment it is. Every value a deployment
owns is read from the environment, in `main.toml` and in the two function
configs:

```toml
url        = "${DATABASE_URL:-postgres://postgres:postgres@db:5432/apiplant_docker}"
jwt_secret = "${JWT_SECRET:-docker-example-secret-change-me}"
```

Expansion applies to *string* values, so `[server] port` is a literal 8080 —
a host that assigns its own port (Fly, Cloud Run, Railway) is handled by
publishing 8080 to it, not by reading `$PORT` here.

The `:-default` half is what keeps `docker compose up` working with no setup;
the variable half is what makes the same image run in production. Note
`[server] host = "0.0.0.0"` — the default in a container, since `127.0.0.1`
would only ever answer itself.

`APP_RELEASE` and `APP_ENV` come in as build args and reach the functions
through `status.toml` and `version.toml`, which is how `GET
/api/functions/status` can tell you which commit is serving you. That function
also runs one query, so a 200 from it means the database genuinely answers —
enough for a readiness probe.

## Migrations, and why there is no migration step

`auto_migrate = true`, so the container reconciles the database with `models/`
on boot. There is no release job and no separate migration container: start the
image, and the schema is what the models say. The `seed` service in
`compose.yaml` is a one-shot `apiplant seed /app`, kept behind a profile because
fixtures are a first-run thing — it is idempotent, so running it again never
overwrites a row you changed.

The compose file waits on Postgres' healthcheck rather than hoping. Without it
the first `up` races the database's own initialisation and the app exits.

## Before this is production

* **`JWT_SECRET` from a secret store, never the image.** Anyone holding it can
  mint a session for any user. The compose value is a demo value.
* **Pin the tag.** `:latest` is right for reading an example and wrong for
  anything you deploy — a pinned release is what makes a rollback a one-word
  change.
* **The `note` resource is `public` on all five actions**, so that the curl
  commands above need no login. Real resources are not.
* **TLS terminates somewhere.** Usually at the load balancer in front of this;
  see [Configuration](../../docs/configuration.md) for serving it directly from
  an `https/` directory instead.

[Security model](../../docs/security.md) is the full list.
