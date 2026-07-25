# 02 · Resources

One `models/<name>.toml` file becomes **one Postgres table and five endpoints**.
No migrations to write, no handlers to implement.

```
02-resources/
├── main.toml
└── models/
    └── note.toml
```

## Run it

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_resources
cargo run -p apiplant -- run examples/02-resources
```

`models/note.toml` publishes:

| Method | Path | Action |
|--------|------|--------|
| GET | `/api/note` | list |
| POST | `/api/note` | create |
| GET | `/api/note/{id}` | read |
| PATCH / PUT | `/api/note/{id}` | update |
| DELETE | `/api/note/{id}` | delete |

```bash
# create
curl -s -XPOST localhost:8099/api/note -H 'content-type: application/json' \
  -d '{"title":"Buy milk","body":"semi-skimmed","slug":"buy-milk","priority":1}'

# list, filter, paginate
curl -s 'localhost:8099/api/note?priority=1&limit=10'

# update (PATCH is partial)
curl -s -XPATCH localhost:8099/api/note/<id> -H 'content-type: application/json' \
  -d '{"pinned":true}'

curl -s -XDELETE localhost:8099/api/note/<id> -i   # → 204
```

## What the file shows

* **Field types** — `string`, `text`, `integer`, `big_int`, `float`, `boolean`,
  `uuid`, `timestamp`, `json`, and `reference` (next example).
* **Options** — `required` (NOT NULL), `unique` (409 on conflict), `default`,
  `max_length`, and `hidden` (writable but never returned; how password hashes
  stay out of responses).
* **Automatic columns** — every row gets a `uuid` `id`, plus `created_at` and
  `updated_at` unless you set `timestamps = false`. You cannot declare `id`.

Try it: the `secret` field never comes back in a response, and posting a second
note with the same `slug` returns `409`.

## Migrations

There are none to write. Your models *are* the desired state, and each boot adds
missing tables, columns and foreign keys. Add a field to `note.toml`, restart,
and the column appears — existing rows keep working. Nothing is ever dropped or
retyped automatically; see [Resources](../../docs/resources.md#migrations).

**Next:** [03 · Relationships](../03-relationships) links tables together.
