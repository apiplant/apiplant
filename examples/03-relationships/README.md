# 03 · Relationships

A `reference` field is a real foreign key — and it gives you navigation in both
directions for free.

```
03-relationships/
└── models/
    ├── author.toml     # name, country
    ├── book.toml       # author_id → author
    └── review.toml     # book_id → book, plus reviewer_id and endorsed_by_id → author
```

## Run it

```bash
createdb -h 127.0.0.1 -p 55432 -U postgres apiplant_relationships
cargo run -p apiplant -- examples/03-relationships
```

```bash
POST() { curl -s -XPOST "localhost:8099/api/$1" -H 'content-type: application/json' -d "$2"; }

URSULA=$(POST author '{"name":"Ursula K. Le Guin","country":"US"}' | jq -r .id)
BO=$(POST author '{"name":"Bo Reviewer","country":"SE"}' | jq -r .id)

BOOK=$(POST book "{\"title\":\"A Wizard of Earthsea\",\"author_id\":\"$URSULA\",\"published_year\":1968}" | jq -r .id)

POST review "{\"body\":\"Wonderful.\",\"stars\":5,\"book_id\":\"$BOOK\",\"reviewer_id\":\"$BO\"}"
```

## The three things you get

**1. Inline the parent — `?expand=`**

```bash
curl -s "localhost:8099/api/book?expand=author"
# → [{ "id":"…", "title":"…", "author_id":"…", "author": { "name":"Ursula K. Le Guin", … } }]
```

The relation name drops the `_id` suffix: `author_id` expands under `author`.
Works on single reads too, and takes a comma-separated list.

**2. Walk the other way — nested collections**

```bash
curl -s "localhost:8099/api/author/$URSULA/book"   # every book by this author
curl -s "localhost:8099/api/book/$BOOK/review"     # every review of this book
```

`review` points at `author` twice — `reviewer_id` and `endorsed_by_id` — so for
that pair the server can't guess which link you mean:

```bash
curl -s "localhost:8099/api/author/$BO/review"
# → 400 `review` references `author` more than once; add ?via=<field>

curl -s "localhost:8099/api/author/$BO/review?via=reviewer_id"   # → the review
```

**3. Referential integrity — `on_delete`**

Enforced by Postgres, not by application code:

| Value | Effect |
|-------|--------|
| `restrict` *(default)* | refuses to delete a parent that still has children (`400`) |
| `cascade` | deletes the children too — `book.author_id` here |
| `set_null` | clears the column — `review.reviewer_id` and `endorsed_by_id` |
| `no_action` | no referential action |

Deleting the *reviewer* keeps the review and forgets who wrote it:

```bash
curl -s -XDELETE localhost:8099/api/author/$BO -i     # → 204
curl -s localhost:8099/api/review
# → [{ …, "reviewer_id": null }]     the review survives
```

Deleting the *book's author* takes the whole chain with it — the author's books
cascade, and each book's reviews cascade in turn:

```bash
curl -s -XDELETE localhost:8099/api/author/$URSULA -i  # → 204
curl -s localhost:8099/api/book     # → []
curl -s localhost:8099/api/review   # → []
```

Creating a row that points at a missing parent returns `400`.

Details in [Relationships](../../docs/relationships.md).

**Next:** [04 · Multitenancy](../04-multitenancy) isolates data per organisation.
