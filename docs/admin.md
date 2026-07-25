# The admin dashboard

A **dashboard for the people who run the business** — not for the developer who
wrote the models — so it shows names rather than ids, forms rather than JSON,
and only the things the person signed in is allowed to touch.

Every served app has one, with nothing to generate:

```bash
apiplant run ./my-app     # → http://localhost:8080/admin/
```

The interface is embedded in the `apiplant` binary. Its manifest — the resources,
permissions, auth model and callable functions described below — is derived from
the app on boot, so the dashboard talks to its own origin and can't fall out of
step with the models it's showing. Switch it off, or move it, in `main.toml`:

```toml
[admin]
enabled = true
path    = "/admin"
```

## A static copy

`apiplant admin` bakes the same dashboard into a **directory of plain files**
that talks to a deployed API over CORS and holds no secrets — for hosting the
console somewhere other than the API, or for customising it.

```bash
apiplant admin ./my-app --api https://api.example.com
# → ./my-app/admin/{index.html, app.js, app.css, apiplant-admin.json, …}
```

Drop that directory anywhere static files are served. Left at `APP_DIR/admin`,
`apiplant run` serves its files at `/admin/` in place of the embedded ones — the
manifest excepted, which is always the live one built from the running app.

## What an operator sees

| Screen | What it's for |
|--------|---------------|
| Sign in / Create account | Getting in. Registration appears only when [`allow_registration`](configuration.md) is on. |
| Home | A greeting, the things they manage, the actions they can run. |
| A resource | A searchable, paginated table. Click a row to open it. |
| A record | One form with every field, its relationships, and the records attached to it. |
| An action | A form generated from a [function](functions.md)'s input type, and its result. |
| Team | Who is in the organisation and what role each holds. |
| Organization | The workspace's details, and switching between workspaces. |
| Your account | Their own profile. |
| API keys | Issuing and revoking keys. |

Everything is derived from your app. There is no dashboard code to write, and
the shipped JavaScript is byte-identical for every application — only
`apiplant-admin.json` differs.

## Two kinds of "who can"

These are separate, and the distinction matters:

* **`[permissions]`** (and a function's `permission`) decide what the **API**
  allows. The server enforces them on every request.
* **`[admin]`** decides what an **operator is shown**. It is presentation.

Hiding a resource from the dashboard does not protect it — anyone with a token
can still call the endpoint. Use [`[permissions]`](permissions.md) for that.
`[admin]` exists to keep the dashboard focused, not to keep it secure.

The dashboard also hides what a person *cannot* use: a resource whose `list`
policy they fail never appears in the navigation, and neither does an action
whose `permission` they do not hold. A door that opens onto a `403` is worse
than no door.

## `[admin]` on a resource

Every key is optional; a resource with no `[admin]` section still appears, with
labels and columns inferred.

```toml
# models/product.toml
[resource]
name = "product"

[admin]
visible       = true                          # default: true (see below)
roles         = ["manager", "admin"]          # who sees it; default: anyone who may list it
label         = "Product"                     # singular; default: title-cased name
plural        = "Products"                    # default: label + "s"
group         = "Catalogue"                   # sidebar heading; ungrouped sorts last
order         = 1                             # position within the group
display_field = "name"                        # what names a record everywhere
search_field  = "name"                        # what the search box filters on
columns       = ["name", "status", "category_id"]   # the list table, in order
```

| Key | Default |
|-----|---------|
| `visible` | `true`, except the [auth resources](#the-auth-resources) |
| `roles` | empty — visible to anyone whose `list` permission passes |
| `label` | the resource name, title-cased (`purchase_order` → `Purchase order`) |
| `plural` | `label` pluralised |
| `group` | none; the resource sorts after every named group |
| `order` | `0` |
| `display_field` | the first of `name`, `title`, `label`, `slug`, `code`, `number`, `email`, else the first string field |
| `search_field` | `display_field` |
| `columns` | `display_field`, then up to four more fields, skipping `text` and `json` (they never read well in a cell) |

`columns`, `display_field` and `search_field` must name real fields; a typo
fails the app at load rather than silently doing nothing.

`display_field` earns its keep twice over: it is what a table row is titled
with, *and* what a reference to this resource shows elsewhere. Set it and a
`category_id` column stops reading as a UUID.

## `[admin]` on a field

```toml
[fields.status]
type    = "string"
default = "draft"

[fields.status.admin]
visible     = true                              # show it in the dashboard at all
readonly    = false                             # show it, but refuse edits
label       = "Lifecycle"
help        = "Only active products can be sold."
widget      = "select"
options     = ["draft", "active|Live", "discontinued"]
placeholder = "Pick a stage"
```

An option is `"value"` or `"value|Label"`, so the stored value and the word
someone reads need not be the same.

`widget` defaults to `auto`, which picks from the field's type:

| Field type | Input |
|---|---|
| `string`, `uuid` | single-line text |
| `text` | a textarea |
| `integer`, `big_int`, `float` | a number input |
| `boolean` | a switch |
| `timestamp` | a date-and-time picker |
| `json` | a JSON textarea |
| `reference` | a searchable picker showing the target's `display_field` |
| anything with `options` | a dropdown |

Override it with `text`, `textarea`, `select`, `email`, `url`, `password`,
`color`, `date`, `date_time`, `json` or `switch`.

Note the two different "hidden"s:

* `hidden = true` on the field strips it from **every API response** — that is
  what a password hash wants.
* `[fields.x.admin] visible = false` keeps it in the API and only takes it out
  of the dashboard.

Fields the framework stamps itself — the `owner_field` and `organization_id` —
are never offered as inputs, whatever you say here.

### Form order

Fields are laid out in the order they read best, not the order they are stored:
the display field first, then the `columns` you chose (that ordering is already
a statement about what matters), then everything else, with textareas and JSON
last so they do not separate the short fields from each other.

## The auth resources

`user`, `organization`, `membership`, `api_key` and `oauth_connection` are
ordinary resources to the API, but a table of `membership` rows with a
free-text `role` column and a `user_id` foreign key is a *developer's* view of a
team. So the dashboard gives them purpose-built screens instead — Team,
Organization, Your account, API keys — and leaves them out of the resource
navigation.

That is only a default. An app that genuinely wants the generic table can ask
for it:

```toml
# models/user.toml
[admin]
visible = true
group   = "Administration"
roles   = ["admin"]
```

Adding fields to `user` needs no opt-in: extra columns show up on the account
screen automatically, and any you mark `required` are collected on the sign-up
form as real inputs — nobody is ever asked to type JSON to make an account.

## Actions

A [function](functions.md) with a non-`private` permission becomes an action in
the sidebar. Functions bound to a resource's [lifecycle](hooks.md) never do:
they are machinery, not something a person triggers.

```rust
apiplant_function::function! {
    name: "reindex_catalogue",
    description: "Rebuilds the product search index.",
    method: Post,
    permission: "role:admin",
    admin: {
        visible: true,                              // default: true
        roles: ["admin"],                           // who sees it in the sidebar
        label: "Rebuild search index",
        group: "Maintenance",
        description: "Run this after a bulk import.",
        confirm: "Rebuild the index for every product?",
        run_label: "Rebuild index",
        order: 10,
    },
    handler: reindex,
}
```

`confirm` is the one to reach for on anything that writes: the dashboard asks
before running, and validates the form *first*, so nobody confirms and then
learns a field was wrong.

### Actions get real forms

The `function!` macro derives a JSON Schema from your handler's `Input` type,
and the dashboard renders it as labelled inputs — doc comments become the
help text under each one:

```rust
#[derive(Deserialize, JsonSchema)]
struct Input {
    /// How many days back to count. Defaults to the last 30.
    #[serde(default = "thirty")]
    days: i64,
}
```

Strings, numbers, booleans and enums each get the right input; anything nested
falls back to a JSON box, as does a function with no schema at all. This needs
the `schema` feature (on by default) and `JsonSchema` derives — see
[Typed OpenAPI](functions.md#typed-openapi).

Results are rendered as a small table when the output is a flat object, and as
JSON otherwise.

## Function permissions

A function's access uses the same grammar as a resource's `[permissions]`:

```rust
permission: "public",          // anyone
permission: "authenticated",   // any signed-in caller
permission: "member",          // anyone in the caller's active organisation
permission: "role:manager",    // a member holding that role
permission: "private",         // no endpoint at all (404, not 403)
```

`member` is why `permission` exists — it is the level most operator-facing
actions want, and the older `visibility:` field cannot express it. `visibility:
RoleGated` + `role: "admin"` still works and means exactly what it always did;
give one or the other, not both.

See [Functions § Visibility](functions.md#visibility) and
[Permissions](permissions.md) for the full model.

## Deploying it

The manifest is baked at build time, so rebuild the dashboard whenever your
models or functions change:

```bash
apiplant build ./my-app --release          # compile functions first
apiplant admin ./my-app --api https://api.example.com
```

The `--api` value may be a bare domain (the app's `base_path` is appended) or a
full base URL (used as given). The dashboard makes cross-origin requests to it,
so that origin has to allow them.

Nothing in the output is secret: the manifest describes the same shape your
[OpenAPI document](openapi.md) already publishes, and every request it makes is
authenticated as whoever signed in.
