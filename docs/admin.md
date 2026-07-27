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

The header reads *`<app name>` admin* beside the apiplant mark. Point `logo` at
an image of your own to replace the mark:

```toml
[admin]
logo = "/logo.png"   # served from the app's public/ directory
```

Want a different console entirely? Turn this one off and serve your own from
the app's [`public/`](configuration.md#public) directory:

```toml
[admin]
enabled = false
```

## A static copy, hosted elsewhere

`apiplant admin` bakes the same dashboard into a **directory of plain files**
for hosting away from the API — a CDN, a bucket, a different origin:

```bash
apiplant admin ./my-app --api https://api.example.com --out ./panel
# → ./panel/{index.html, app.js, app.css, apiplant-admin.json, …}
```

`--api` may be a bare domain (the app's `base_path` is appended) or a full base
URL (used as given); the panel makes cross-origin requests to it, so that origin
has to allow them. Nothing in the output is secret: the manifest describes the
same shape your [OpenAPI document](openapi.md) already publishes, and every
request it makes is authenticated as whoever signed in.

Unlike the built-in dashboard, this copy's manifest is **frozen at build time** —
re-run the command when models or functions change. The server never reads it
back; a directory left in the app is just files.

## What an operator sees

| Screen | What it's for |
|--------|---------------|
| Sign in / Create account | Getting in. Registration appears only when [`allow_registration`](configuration.md) is on. |
| Home | A greeting, the things they manage, the actions they can run. |
| A resource | A searchable, paginated table. Click a row to open it. |
| A record | One form with every field, its relationships, and the records attached to it. |
| An action | A form generated from a [function](functions.md)'s input type, and its result. |
| Team | Who is in the organisation and which roles each holds — added and revoked one at a time. |
| Organization | The workspace's details, and switching between workspaces. |
| Your account | Their own profile. |
| API keys | Issuing and revoking keys. |
| Create your organization | The first thing after signing in, when the account belongs to none. |
| Connect a terminal | Handing a key to [`apiplant cli`](cli.md) — reached only from the link that command opens. |

Everything is derived from your app. There is no dashboard code to write, and
the shipped JavaScript is byte-identical for every application — only
`apiplant-admin.json` differs.

### The first organization

Almost every resource is scoped to an organisation, so someone who belongs to
none sees empty tables and gets an error from every write — which reads as a
broken dashboard rather than as an account that is not finished. So a full-page
step stands in front of the interface until it is resolved.

What it offers is the app's decision, not the dashboard's. When `organization`
lets an authenticated caller `create` — the built-in default — it is a form
built from that resource's fields, and [the server makes the creator its
admin](multitenancy.md). When the app has narrowed that policy because it
provisions tenants itself, there is nothing useful to offer: the page says an
administrator has to add them, and offers to re-check. A `role:` policy counts
as narrowed, since nobody with no organisation can satisfy one.

The console handoff is the one route this step does not block — issuing an API
key needs no organisation, and blocking it would strand `apiplant cli` behind a
step it cannot complete.

### Connecting the console

`apiplant cli` opens `#/cli?callback=…` here, pointing at a one-request listener
on the operator's own machine. The screen names what is asking, and on a press
mints a key and posts it back, so no secret is copied between two windows. The
callback is refused unless it is plain HTTP on a loopback host — the address
arrives in a link, and a link is something anyone can send — and nothing is
minted without that press. See [the console](cli.md).

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

### Markup fields

A `text` (or `string`) field can say what its content *is*:

```toml
[fields.description]
type = "text"

[fields.description.admin]
format = "markdown"        # or "html"; "plain" is the default
```

Nothing changes server-side — the column, the API request and the API response
are the same characters they were. It only changes the editor: the dashboard
colours the markup and renders it live beside the input, or behind a
`Write`/`Preview` tab pair when the screen is too narrow for two columns. A
formatted field is always given a textarea, whatever `widget` would have picked.

The preview is sanitised (no scripts, no event handlers, no `javascript:` URLs),
because the text it renders is operator input and the dashboard session it
renders in can edit every record. What your own front end does with the stored
markup is your business — apiplant never renders it for you.

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

There is nothing to deploy for the built-in dashboard: it ships inside the
`apiplant` binary and its manifest is rebuilt on every boot, so a model or
function you changed is described correctly the moment the server restarts.
Compile your functions before starting, or the actions that need them won't be
listed:

```bash
apiplant build ./my-app --release
apiplant run ./my-app
```

Because it is served from the app's own origin, it needs no CORS setup and no
API base URL. For the static copy, re-run `apiplant admin` in the same breath as
a deploy — its manifest only changes when you rebuild it.
