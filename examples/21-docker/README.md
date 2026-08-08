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

docker compose --profile dev up dev   # the same app, rebuilt on every save
open http://localhost:8081/api/docs
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

## Uploads, and the one volume they need

`[storage]` decides where an uploaded file goes, and the `file` field type is
how a resource asks for one — `note.toml` has an `attachment`. In the dashboard
that field is an upload button *and* a URL box: the first stores the file and
writes back the link, the second takes a link to something already hosted.

```bash
curl -X POST 'localhost:8080/api/uploads?filename=receipt.pdf' \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/pdf' \
  --data-binary @receipt.pdf
# {"url":"/files/2026/08/1a2b3c4d5e6f-receipt.pdf", ...}

curl localhost:8080/files/2026/08/1a2b3c4d5e6f-receipt.pdf
```

The link stored in the row is **relative**. That is the whole design: the
column names a path this server answers, never a bucket, so the backend behind
it is a configuration decision and not a data migration.

In a container that means one volume:

```yaml
    environment:
      STORAGE_DIR: /data/uploads
    volumes:
      - uploads:/data/uploads
```

Without it every upload lives inside the container's writable layer and is
discarded on the next `up --build` — the same mistake as running Postgres with
no `pgdata`.

### Or no volume at all

`backend = "s3"` puts the files in block storage instead, and the same block
covers S3, Cloudflare R2, MinIO and anything else with an S3 front door; they
differ only in `endpoint`:

```toml
[storage]
backend           = "s3"
bucket            = "${STORAGE_BUCKET}"
region            = "auto"
endpoint          = "https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
access_key_id     = "${R2_ACCESS_KEY_ID}"
secret_access_key = "${R2_SECRET_ACCESS_KEY}"
```

Delete the volume, set those five variables, redeploy. Existing rows keep
working, because they still say `/files/…` and the server now answers that from
the bucket. Files already on the volume have to be copied across once — that is
an `aws s3 sync`, not a schema change.

The bucket stays **private**: reads are proxied through this server, which is
why nothing needs a public-read policy or a signed-URL scheme. An app that
would rather serve from a CDN sets `base_url` and stores absolute links
instead, trading the portability for the hop.

## Migrations, and why there is no migration step

`auto_migrate = true`, so the container reconciles the database with `resources/`
on boot. There is no release job and no separate migration container: start the
image, and the schema is what the resources say. The `seed` service in
`compose.yaml` is a one-shot `apiplant seed /app`, kept behind a profile because
fixtures are a first-run thing — it is idempotent, so running it again never
overwrites a row you changed.

The compose file waits on Postgres' healthcheck rather than hoping. Without it
the first `up` races the database's own initialisation and the app exits.

## Developing inside the container

The image above is the deployed shape: the app is copied in and never changes.
The `dev` service is the other one — this directory mounted live, with the
server rebuilding and restarting on every save:

```bash
docker compose --profile dev up dev
open http://localhost:8081/api/docs
```

Edit `resources/note.toml` or `functions/version.ts` on the host and watch the
container restart with the change. It is `apiplant run /app --watch`, which
polls for changes rather than waiting for inotify — a host editor's writes do
not deliver filesystem events across a bind mount, so a watcher that subscribed
to them would sit there doing nothing.

Two things about it are worth knowing:

* It builds from the **`functions` stage**, not the runtime image. Rebuilding
  `status.rs` needs cargo, and the runtime image deliberately has no toolchain.
  An app whose functions are all TypeScript can point `dev` straight at
  `ghcr.io/apiplant/apiplant` — `.ts` is transpiled in-process.
* It writes into the mounted directory: `functions/libstatus.so` and a
  `.apiplant-build/` cache appear on the host, owned by the container's user.
  Both are build output, and both are already in `.gitignore`.

It publishes 8081 so it can run alongside the deployed shape on 8080, against
the same database — which is the quickest way to see that the container you
deploy and the container you develop in differ only in where the app came from.

## Scheduled work reuses this image

The entrypoint is the `apiplant` binary, so anything the CLI can do is this
image with different arguments — including running one of the app's functions:

```bash
docker compose run --rm api call status /app
# → {"release":"local","env":"compose","notes":0}
```

That is the whole story for a Kubernetes CronJob too: same image, same
`envFrom`, `args: ["call", "status", "/app"]`. No second image to
build, no HTTP call to schedule against a server that might be mid-deploy, and
the function is the same one the API exposes. See
[Functions](../../docs/functions.md#from-the-command-line-and-from-a-scheduler).

## Before this is production

* **`JWT_SECRET` from a secret store, never the image.** Anyone holding it can
  mint a session for any user. The compose value is a demo value.
* **Pin the tag.** `:latest` is right for reading an example and wrong for
  anything you deploy — a pinned release is what makes a rollback a one-word
  change.
* **The `note` resource is `public` on all five actions**, so that the curl
  commands above need no login. Real resources are not.
* **Uploads are `authenticated`, and reads are not.** A stored link is
  unguessable but not secret: anyone holding it can fetch the file, because it
  has to work in an `<img>` tag and in an email. Files that need an access check
  do not belong in a `file` field.
* **`allowed_types` is set.** Left empty it accepts anything, and an upload
  endpoint that accepts anything is a file host.
* **TLS terminates somewhere.** Usually at the load balancer in front of this;
  see [Configuration](../../docs/configuration.md) for serving it directly from
  an `https/` directory instead.

[Security model](../../docs/security.md) is the full list.
