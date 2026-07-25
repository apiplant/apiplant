# 08 · Hooks (the full app)

Everything from the previous examples, plus **functions**: custom Rust compiled
into shared libraries, mounted as endpoints and attached to the resource
lifecycle.

```
08-hooks/
├── main.toml
├── models/
│   ├── post.toml               # org-scoped, with a [hooks] section
│   └── comment.toml            # post_id → post, owner_id → user
└── functions/
    ├── post_hooks.rs           # five lifecycle hooks in one library
    ├── post_before_create.toml #   …per-function config
    └── post_before_update.toml
```

The [previous example](../07-functions) mounted functions as their own
endpoints. The same functions can instead be attached to a resource's lifecycle,
so they run around the generated CRUD endpoints — that's a **hook**.

## Run it

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_hooks
cargo run -p apiplant -- build examples/08-hooks   # needs cargo on PATH
cargo run -p apiplant -- run examples/08-hooks
```

The build step is required: `models/post.toml` declares hooks, and a hook whose
function isn't loaded fails its requests closed with a `500` rather than silently
skipping validation. The boot log lists what resolved:

```
INFO apiplant_server:   hook post.before_create -> post_before_create
INFO apiplant_server:   hook post.after_list    -> post_after_list
…
```

All five are declared `Private`, so they have no HTTP endpoint of their own —
hooks run whatever a function's visibility.

## The hooks

`post_hooks.rs` exports five independent functions — one per event, no
dispatcher — and `models/post.toml` points each event at one:

| Event | Function | Behaviour |
|-------|----------|-----------|
| `before_create` | `post_before_create` | trims the title; rejects blank, over-long or banned titles |
| `before_update` | `post_before_update` | same rules; partial `PATCH` bodies pass through |
| `after_create` | `post_after_create` | logs the new post, its org and its author |
| `after_list` | `post_after_list` | members see published posts plus their own drafts; admins see all |
| `before_delete` | `post_before_delete` | published posts refuse to be deleted without `?force=1` |

## Try them

```bash
TOKEN=$(curl -s -XPOST localhost:8099/api/auth/register -H 'content-type: application/json' \
  -d '{"email":"ana@example.com","password":"pw"}' | jq -r .token)
curl -s -XPOST localhost:8099/api/organization -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' -d '{"name":"Acme","slug":"acme"}' > /dev/null

# before_create trims the title
curl -s -XPOST localhost:8099/api/post -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' -d '{"title":"   My first post   ","published":true}'
# → {"title":"My first post", …}

# …and rejects what the config forbids
curl -s -XPOST localhost:8099/api/post -H "authorization: Bearer $TOKEN" \
  -H 'content-type: application/json' -d '{"title":"lorem ipsum dolor"}'
# → 422 {"error":"title may not mention `lorem ipsum`"}

# before_delete protects published posts
curl -s -XDELETE localhost:8099/api/post/<id> -H "authorization: Bearer $TOKEN"
# → 409 {"error":"published posts are protected; retry with ?force=1"}
```

## Per-function config

Each function reads its own `functions/<name>.toml`. Here
`post_before_create.toml` caps titles at 80 characters while
`post_before_update.toml` allows 200 — same `Rules` type, independent settings.
Change a value, restart (no rebuild needed — config is read at boot), and the
rules change.

## Editing a function

```bash
$EDITOR examples/08-hooks/functions/post_hooks.rs
cargo run -p apiplant -- build examples/08-hooks
```

Only changed sources recompile. Run the server without rebuilding and it warns
that a source is newer than its library; `apiplant run --build` does both in one
step.

Details in [Functions](../../docs/functions.md) and
[Lifecycle hooks](../../docs/hooks.md).
