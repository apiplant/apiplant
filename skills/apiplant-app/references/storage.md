# File storage

An app that needs to hold a picture, a logo or an attachment declares a `file`
field and points `[storage]` at somewhere to put it:

```toml
# main.toml
[storage]
backend = "local"
dir     = "storage"
```

```toml
# resources/product.toml
[fields.photo]
type = "file"
```

That is the whole configuration. `local` is the default backend, so an app that
writes no `[storage]` section at all already accepts uploads into a `storage/`
directory beside its resources.

## What a `file` field actually holds

A **relative URL**, as an ordinary string:

```json
{ "id": "…", "name": "Chair", "photo": "/files/2026/08/1a2b3c4d5e6f-chair.png" }
```

Not a bucket name, not an object key, not a signed link with an expiry. The
column is a `varchar(1024)` and the value is a path this server answers.

Everything else follows from that:

* **The backend is a configuration decision, not a data migration.** Moving from
  a mounted volume to S3 changes five lines of TOML. No row is rewritten,
  because no row ever named the volume.
* **The API is unchanged.** A `file` is a string to every client, and appears in
  the OpenAPI document as `{"type": "string", "format": "uri-reference"}`. An
  API client does not have to know the type exists.
* **A URL you did not store is a valid value.** The column takes any string, so
  a picture already hosted somewhere — an identity provider's avatar, a CDN
  asset — is written straight in. This is not a fallback for when uploads are
  off; it is the other half of what a `file` field means, and the dashboard
  offers both.

## The two endpoints

### `POST <base>/uploads`

Takes the file as the **raw request body**, with its name in `?filename=` and
its type in `Content-Type`. Not multipart: an upload is one file, and a browser
sends a `File` as a body with no encoding step.

```bash
curl -X POST 'localhost:8080/api/uploads?filename=chair.png' \
  -H "authorization: Bearer $TOKEN" \
  -H 'content-type: image/png' \
  --data-binary @chair.png
```

```json
{
  "url": "/files/2026/08/1a2b3c4d5e6f-chair.png",
  "key": "2026/08/1a2b3c4d5e6f-chair.png",
  "size": 20481,
  "content_type": "image/png"
}
```

`url` is what goes in the field. Keys are dated (so no directory grows to a
hundred thousand entries) and carry a UUID (so two people uploading `logo.png`
a second apart do not overwrite each other) with the original filename on the
end, reduced to something safe for a URL.

**Authenticated.** An upload spends disk or somebody's S3 bill, so the caller
has to be somebody. It is not organisation-scoped, because the two things every
app uploads — a user's avatar and an organisation's logo — are set from either
side of that line.

| Status | Meaning |
|---|---|
| `200` | Stored; the body carries the URL. |
| `400` | Empty body. |
| `401` | No credentials. |
| `413` | Larger than `max_size_mb`. |
| `415` | A content type outside `allowed_types`. |
| `500` | The backend refused the write. |

### `GET /files/<key>`

Reads the file back, from whichever backend is configured. Served with the
content type its extension implies, `X-Content-Type-Options: nosniff`, and a
one-year immutable cache header — a key is minted per upload and never written
twice, so nothing behind a `/files` URL can change.

**Unauthenticated, by design.** A stored link is unguessable but it is not
secret: it has to work in an `<img src>`, in a stylesheet and in an email, none
of which can send a bearer token. Files that need an access check on every read
do not belong in a `file` field.

## Backends

### `local` — a directory

```toml
[storage]
backend = "local"
dir     = "storage"      # relative to the app root, or absolute
```

Files are written under `dir`, which is created on boot — a directory that
cannot be written fails the boot rather than the first upload.

In a container `dir` must be a **mounted volume**, or every upload is discarded
with the container's writable layer on the next deploy:

```yaml
    environment:
      STORAGE_DIR: /data/uploads
    volumes:
      - uploads:/data/uploads
```

See [example 21](../examples/21-docker/) for the whole compose file.

### `s3` — block storage

One backend covers AWS S3, Cloudflare R2, MinIO, Backblaze B2 and anything else
with an S3 front door. They differ only in `endpoint`:

```toml
# AWS
[storage]
backend           = "s3"
bucket            = "app-uploads"
region            = "eu-west-1"
access_key_id     = "${AWS_ACCESS_KEY_ID}"
secret_access_key = "${AWS_SECRET_ACCESS_KEY}"

# Cloudflare R2
[storage]
backend           = "s3"
bucket            = "app-uploads"
region            = "auto"
endpoint          = "https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
access_key_id     = "${R2_ACCESS_KEY_ID}"
secret_access_key = "${R2_SECRET_ACCESS_KEY}"

# MinIO, on your own hardware
[storage]
backend           = "s3"
bucket            = "app-uploads"
region            = "us-east-1"
endpoint          = "http://minio:9000"
access_key_id     = "${MINIO_ROOT_USER}"
secret_access_key = "${MINIO_ROOT_PASSWORD}"
```

Requests are signed with AWS Signature Version 4, in about forty lines — there
is no AWS SDK in the dependency graph.

The bucket stays **private**. Reads are proxied through the server, which is why
nothing here needs a public-read policy, a bucket website, or a signed-URL
scheme. It costs one hop; it buys links that keep working when the bucket
changes.

### Switching between them

Change the `[storage]` block and redeploy. Stored links keep working, because
they still say `/files/…` and the server now answers that from the new place.
Files already written have to be copied across once — an `aws s3 sync`, not a
schema change.

## Serving from a CDN instead

`base_url` opts out of the proxy: links are stored **absolute**, under the
origin you name, and the server never sees a read.

```toml
[storage]
backend  = "s3"
bucket   = "app-uploads"
base_url = "https://cdn.example.com"
# … credentials …
```

Stored values become `https://cdn.example.com/files/2026/08/…`. This is a
deliberate trade: faster, but the objects must be publicly readable, and the
links stop being portable — moving the CDN now means rewriting rows. Leave it
empty unless the hop is measurably costing something.

## In the dashboard

A `file` field renders as an upload button and a URL box, together. Uploading
stores the file and writes the link back; typing a URL writes that instead. A
preview sits beside both, so a typo is visible immediately rather than when
something tries to load it.

`user.avatar_url` and `organization.avatar_url` are `file` fields, so **Your
account** and **Organization** both get a picture picker without any app
configuration. An avatar filled in by an [OAuth](authentication.md) sign-in
arrives as the provider's URL and is left exactly as it is; replacing it in the
dashboard uploads into `[storage]` and swaps the string.

## Configuration reference

| Key | Default | Notes |
|-----|---------|-------|
| `backend` | `local` | `local`, `s3`, or `none` to refuse uploads outright. |
| `dir` | `storage` | `local` only. Relative to the app root unless absolute. Created on boot. |
| `public_base` | `/files` | URL prefix the stored links carry and the server answers on. |
| `max_size_mb` | `10` | Largest upload accepted. Enforced while the body arrives, not after. |
| `allowed_types` | *(empty)* | Exact types (`image/png`) or wildcards (`image/*`). Empty accepts anything. |
| `bucket` | *(empty)* | `s3` only. Required. |
| `region` | `auto` | `s3` only. R2 and most S3-compatibles want `auto`. |
| `endpoint` | *(empty)* | `s3` only. Empty uses AWS's own. |
| `access_key_id` / `secret_access_key` | *(empty)* | `s3` only. Both required. |
| `path_style` | *(set when `endpoint` is)* | Address objects as `<endpoint>/<bucket>/<key>`. Required by MinIO and R2. |
| `prefix` | *(empty)* | Key prefix inside the bucket or directory, so several apps can share one. |
| `base_url` | *(empty)* | Store absolute links under this origin instead of proxying reads. |

A misconfigured backend — an `s3` with no bucket, a `dir` that cannot be
created — fails the boot rather than the first upload.

## Turning it off

```toml
[storage]
backend = "none"
```

No upload endpoint, no `/files`, and no directory created. A `file` field still
works as a column: it holds whatever URL is written to it, which is the right
behaviour for an app whose images all live somewhere else already.

## Before this is production

* **Set `allowed_types`.** Left empty the endpoint accepts anything, and an
  upload endpoint that accepts anything is a file host.
* **Nothing is served as HTML.** An uploaded `.html` comes back as
  `application/octet-stream`, and every response carries `nosniff`, so an upload
  cannot become a script running on your origin.
* **Uploads are authenticated; reads are not.** See above — a `file` field is
  for things that are fine to hand out to whoever holds the link.
* **`max_size_mb` is the only quota.** There is no per-user cap and no total
  cap; an app that needs one enforces it in a
  [hook](hooks.md) or in front of the endpoint.
* **Deleting a row does not delete its file.** The column is a string, and the
  same string may be in several rows. Cleaning up is an app's decision.
