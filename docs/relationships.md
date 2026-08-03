# Relationships

Relationships in apiplant start from a single primitive: a **`reference`
field**. Declaring one produces a real Postgres foreign key, a navigable
`has_many` endpoint on the other side, on-demand inlining with `?expand=`, and
filtering by the foreign key, with no further configuration.

```toml
# models/comment.toml
[resource]
name = "comment"

[fields.body]
type = "text"
required = true

[fields.post_id]
type       = "reference"
references = "post"
required   = true
on_delete  = "cascade"      # delete a post's comments with the post

[fields.owner_id]
type       = "reference"
references = "user"
# on_delete defaults to "restrict"
```

## `belongs_to`: the reference field

A `reference` field is a `uuid` column plus a foreign-key constraint to the
target resource's `id`. It models the "many" side: a comment **belongs to** a
post.

| Option | Default | Meaning |
|--------|---------|---------|
| `references` | *(required)* | Target resource name. |
| `on_delete` | `restrict` | What happens to this row when the referenced row is deleted. |

### `on_delete` actions

| Value | Effect when the parent is deleted |
|-------|-----------------------------------|
| `restrict` | **Block** the delete while children exist (safe default). |
| `set_null` | Null this reference (needs a nullable column, i.e. not `required`). |
| `cascade` | Delete this row too. |
| `no_action` | No referential action (deferred check). |

Foreign keys are enforced by Postgres. Inserting a reference to a non-existent
row returns **400** (`references a record that does not exist`); deleting a
`restrict` parent that still has children returns an error mapped from the DB.

## `has_many`: the reverse direction

Because `comment.post_id` references `post`, apiplant automatically exposes the
reverse collection on the parent:

```
GET /api/post/{id}/comment      → comments whose post_id = {id}
```

This applies the **child's** `list` permission and supports the same
`limit`/`offset` paging as a normal list. It is derived entirely from the
reference graph; `has_many` is never declared explicitly.

### Disambiguating multiple references

If a child references the same parent through more than one field, for example a
`message` with both `sender_id` and `recipient_id` referencing `user`, add
`?via=<field>` to pick which one the nested endpoint follows:

```
GET /api/user/{id}/message?via=sender_id
GET /api/user/{id}/message?via=recipient_id
```

With a single reference, `?via=` is unnecessary.

## Expansion: inlining referenced records

Any `belongs_to` reference can be inlined into a response with `?expand=`, on
both list and read endpoints. The expand key is the field name **without a
trailing `_id`**: `owner_id` → `owner`, `post_id` → `post`.

```
GET /api/comment?expand=post,owner
```

```json
[
  {
    "id": "…",
    "body": "nice post",
    "post_id": "…",
    "owner_id": "…",
    "post":  { "id": "…", "title": "Relational post", … },
    "owner": { "id": "…", "email": "author@example.com", … }
  }
]
```

Notes:

* Multiple relations: comma-separated (`?expand=post,owner`).
* Expansion is **batched**: one `WHERE id IN (…)` query per relation, so a page
  of results does not cause an N+1 query pattern.
* Hidden fields on the referenced resource remain hidden, so a user's
  `password_hash` never appears.
* Expansion is a **read of the target resource** and is authorized as one: the
  target's `read` [permission](permissions.md) and, for an org-scoped target,
  its organisation filter both apply. A relation pointing at something the
  caller may not read expands to `null` rather than to a row the direct
  endpoint would have rejected.
* A dangling reference expands to `null`.
* Expansion is one level deep.

## Filtering by a reference (or any field)

List endpoints accept `?<field>=<value>` for exact matches on any column,
including foreign keys:

```
GET /api/comment?post_id=<uuid>       # comments on one post
GET /api/post?published=true          # published posts
GET /api/user?email=you@example.com
GET /api/post?title~=launch           # substring, case-insensitive (text columns)
```

Values are parsed according to the column's type; an invalid value returns
`400`. Unknown query keys are ignored, multiple filters combine with `AND`, and
owner scoping from an `owner` permission is always applied in addition.

> Note: `GET /api/post/{id}/comment` is equivalent to
> `GET /api/comment?post_id={id}`, but additionally verifies that the parent
> path names a real resource.

## Full example

```toml
# models/post.toml
[resource]
name = "post"
[permissions]
list = "public"
read = "public"
create = "authenticated"
update = "owner"
delete = "role:admin"
[fields.title]
type = "string"
required = true
[fields.owner_id]           # post belongs_to user; user has_many posts
type = "reference"
references = "user"
```

```toml
# models/comment.toml  (as above): comment belongs_to post AND user
```

Gives you:

```
GET  /api/user/{id}/post           # a user's posts (has_many)
GET  /api/post/{id}/comment        # a post's comments (has_many)
GET  /api/post?expand=owner        # posts with author inlined
GET  /api/comment?expand=post,owner
GET  /api/comment?post_id=<uuid>   # filter
```

All of these are backed by enforced foreign keys with the configured
`on_delete` behaviour.
