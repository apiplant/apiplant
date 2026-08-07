# 26 · File upload

Uploads, into a bucket you do not own the code for. This one points at
[Cloudflare R2]; the same block runs against S3, MinIO, Backblaze B2 or a
directory on disk, and the app cannot tell which.

```
26-file-upload/
├── main.toml           # the [storage] block; everything else is example 04
├── models/photo.toml   # a resource with two `file` fields
└── seed/               # the usual two organisations and three users
```

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_uploads

R2_ACCOUNT_ID=... \
R2_ACCESS_KEY_ID=... \
R2_SECRET_ACCESS_KEY=... \
R2_BUCKET=files \
cargo run -p apiplant -- run --seed examples/26-file-upload
```

There are no functions, so there is nothing to build. The boot banner names the
backend it resolved, which is the fastest way to find a typo in the block:

```
storage -> s3 (files at https://….r2.cloudflarestorage.com), served at /files/
```

Sign in to <http://127.0.0.1:8099/admin/> as `admin@example.com` / `password`,
open **Photos → New**, and drop a picture on the image field. It uploads while
you are still filling in the title, and the row you save holds the link.

## The two endpoints

`[storage]` adds these to every app that sets it, and nothing else changes.

**`POST /api/uploads`** takes the file as the **raw request body** — not
multipart. The name goes in `?filename=`, the type in `Content-Type`:

```bash
TOKEN=$(curl -s -X POST 127.0.0.1:8099/api/auth/login \
  -H 'content-type: application/json' \
  -d '{"email":"admin@example.com","password":"password"}' | jq -r .token)

curl -X POST '127.0.0.1:8099/api/uploads?filename=maas.png' \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: image/png' \
  --data-binary @maas.png
```

```json
{
  "url": "/files/apiplant-example-26/2026/08/c259711d2eb6-maas.png",
  "key": "apiplant-example-26/2026/08/c259711d2eb6-maas.png",
  "size": 20481,
  "content_type": "image/png"
}
```

A browser posts a `File` object as a body with no encoding step, so multipart
would buy a parser and nothing else. An upload is one file.

The endpoint is authenticated — an upload spends somebody's S3 bill — but it is
not organisation-scoped, because the two things every app uploads are a user's
avatar and an organisation's logo, and those sit on either side of that line.

**`GET /files/{key}`** reads it back, and is deliberately *un*authenticated. A
stored URL carries a UUID per object and ends up in `<img src>` tags, style
sheets and mail — places that cannot send a bearer token. The response is
`immutable` for a year (a key is minted per upload and never written twice) and
`nosniff` (an uploaded file must not become scriptable, whatever it claims to
be). **Files that must not be readable by anyone holding the link do not belong
in a `file` field.**

## The field

```toml
[fields.image]
type     = "file"
required = true
```

That is a string column. What it buys is the dashboard widget — drop a file, get
a link — and the guarantee underneath it: the column holds `/files/…`, a
**relative** URL, never a bucket address.

So put the link in yourself if you already have one:

```bash
ORG=$(curl -s 127.0.0.1:8099/api/organization -H "authorization: Bearer $TOKEN" \
  | jq -r '.[] | select(.slug=="acme") | .id')

curl -X POST 127.0.0.1:8099/api/photo \
  -H "authorization: Bearer $TOKEN" -H "X-Organization: $ORG" \
  -H 'content-type: application/json' \
  -d '{"title":"Rotterdam, 7am","image":"/files/apiplant-example-26/2026/08/c259711d2eb6-maas.png"}'
```

The API is none the wiser either way.

## Why the indirection

No row, no API response and no client ever names a bucket. That is what makes
this the entire difference between a volume and R2:

```toml
[storage]
backend = "local"
dir     = "storage"
```

```toml
[storage]
backend           = "s3"
bucket            = "files"
region            = "auto"
endpoint          = "https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
access_key_id     = "${R2_ACCESS_KEY_ID}"
secret_access_key = "${R2_SECRET_ACCESS_KEY}"
```

Start on a mounted volume, move to R2 when the second container appears, and
every stored link keeps working — because the server answers `/files/…` from
whichever backend is configured, and nothing was ever written down that says
which. There is no migration, and no row to rewrite.

`main.toml` ships the second block and carries the first commented out. Swap
them and re-run: same endpoints, same URLs, same rows.

## R2, specifically

R2 is S3 with an `endpoint`, which is also true of MinIO, B2 and Spaces — hence
one backend and not four. Three things differ from AWS:

* **`region = "auto"`.** R2 has no regions. The string still has to be there,
  because it is part of what a SigV4 signature is computed over.
* **`path_style = true`.** Objects are addressed `<endpoint>/<bucket>/<key>`
  rather than with the bucket in the hostname. apiplant defaults to path-style
  whenever an `endpoint` is set, so this line is optional — it is written out in
  `main.toml` only because it is what people go looking for when a non-AWS
  service answers 404.
* **The endpoint is per account, not per bucket:**
  `https://<account-id>.r2.cloudflarestorage.com`. Cloudflare also offers
  jurisdiction-specific hostnames (EU, FedRAMP) — use the one the dashboard
  shows you, since a token minted for one will not sign against another.

Create the bucket and an **Object Read & Write** API token in the Cloudflare
dashboard under R2. The token is scoped to buckets, so `access_key_id` here
generally cannot list your account — which is the right shape for an app.

## What else the block does

```toml
max_size_mb   = 10
allowed_types = ["image/*", "application/pdf"]
prefix        = "apiplant-example-26"
```

`max_size_mb` is enforced twice: as a payload limit the route is built with, so
an oversized body is refused *while it arrives*, and again after, in case the
two ever drift. `allowed_types` checks what the upload declares — exact types
and `type/*` wildcards both — and answers 415:

```
$ curl -X POST '…/uploads?filename=x.exe' -H 'content-type: application/x-msdownload' …
{"error":"this app does not accept application/x-msdownload uploads"}
```

`prefix` is a key prefix inside the bucket, so several apps can share one.

Keys are minted, never taken from the caller: `2026/08/<12 hex>-<name>`. Dated,
so a directory backend never grows one folder with a hundred thousand entries in
it; UUID-prefixed, so two people uploading `logo.png` a second apart do not
overwrite each other; and the sanitised original name is kept on the end,
because it is the one part a person reading a URL can use.

## The one setting that is a one-way door

```toml
# base_url = "https://cdn.example.com"
```

Left unset, links stay relative and reads are proxied through the server, which
is what lets the bucket stay private. Set it, and rows store absolute URLs
against that origin instead: faster, but the objects must be publicly readable
and the links stop being portable. It is the only line in the block that changes
what ends up in the database.

## Next

* **[21 · docker](../21-docker)** — the same `file` field on the local backend,
  with the volume that makes it survive a deploy.
* **[04 · multitenancy](../04-multitenancy)** — where `scope = "organization"`
  and the `X-Organization` header above come from.

[Cloudflare R2]: https://developers.cloudflare.com/r2/
