# The console

```bash
apiplant cli [SERVER|APP_DIR]
```

`apiplant cli` provides the [admin dashboard](admin.md)'s functionality in a
terminal: browse resources, page through records, create, edit and delete them,
and run the app's callable functions over SSH, in a tmux pane, without a
browser.

It is a *client*. Point it at a server (`apiplant cli api.example.com`) and that
address is all it needs. Everything else (which resources exist, which fields
they have, which functions are callable, and what you are permitted to do) comes
from the running server, via the same manifest the dashboard loads. A model
change is visible as soon as the server restarts, and the console cannot
describe an app other than the one actually running.

## Where it connects

The argument is interpreted as a server first and an app directory second. A
URL, a `host:port` or a domain names the server directly; anything that exists
on disk, or looks like a path, is treated as an app directory whose `main.toml`
is read solely for the server address. With no argument, the current directory
is used, which is the common case when working locally.

```bash
apiplant cli https://api.example.com   # a server
apiplant cli box.local:8080            # a server, scheme assumed
apiplant cli ./my-app                  # an app directory
apiplant cli                           # this directory
```

The scheme may be left off; it is assumed to be `https`. A self-signed
certificate is accepted on loopback and nowhere else. A named server is dialled
as given, and its manifest is looked up at the default `/admin` path unless the
manifest itself specifies another.

From a directory, the address comes from `main.toml`:

| From | Used for |
|------|----------|
| `[server] host`, `port` | the address (a `0.0.0.0` bind is dialled as `127.0.0.1`) |
| `https/` | present ⇒ `https://`, absent ⇒ `http://` |
| `[admin] path` | where the manifest is fetched from |
| `[server] base_path` | a starting guess at the API prefix, until the manifest says otherwise |

The directory is only a means of locating the server. The API's actual location
comes from the manifest's `api_base_url`, since the process serving the requests
is the authoritative source. A missing `main.toml`, a stale one, or one
belonging to a different deployment would otherwise direct every call to an
unserved prefix, surfacing as an unexplained 404 on the first action. When the
two disagree, the console follows the server and reports this in the status
line. A statically hosted dashboard publishes a full URL in that field, so
pointing at one is enough to reach the API it was built for.

In any other case (an app behind a proxy, on another host, or in a container),
name the server rather than the directory.

`[admin] enabled = false` publishes no manifest, so the console has nothing to
read and reports this before starting.

## Signing in

The first screen offers three options, plus two that depend on the app:
**Create an account**, shown where registration is open, which collects whatever
fields an account requires and asks for the password twice; and **Forgot your
password?**, shown where the server
[can send email](authentication.md#reaching-people-by-email). Neither is offered
otherwise, since an option that always fails is worse than none.

Registering with an app that confirms addresses does not sign you in, as the
account is not yet usable: the console reports which address to check and
returns to the password form. The confirmation link opens in a browser, since a
terminal has no mail client.

**Open the dashboard in a browser.** The console opens a one-request web server
on `127.0.0.1`, sends you to the dashboard with its address attached, and waits.
The dashboard displays the request; pressing **Connect** issues a key for the
account signed in there and posts it back. Nothing is copied by hand. If no
browser can be opened, which is usually the case over SSH, the console prints
the address so it can be opened elsewhere.

Two rules make this safe: the dashboard rejects any callback that is not plain
HTTP on a loopback host, so a crafted link cannot send your key to a third
party, and it never issues a key without an explicit confirmation.

**Sign in with an email and password.** The console calls `POST /auth/login`,
then exchanges the session for a long-lived key so the next run starts
connected. If the app has no `api_key` resource, or the account may not create
one, the session still works, but ends when you quit.

**Paste an API key.** For a key you already hold. The console asks the terminal
to deliver pastes as a single unit, so a key cannot lose characters to a slow
redraw or have its characters interpreted as form shortcuts. It is validated
against an endpoint that requires it, so an invalid key is rejected here rather
than on the first list. Only an explicit *rejection* disqualifies a key: any
other failure indicates a problem with the app rather than the credential, so it
is reported and the key is retained. A rejected key is inspected and the reason
described (wrong length, wrong prefix, or non-hexadecimal characters), since a
masked field otherwise gives no indication of the problem. The key itself is
never echoed.

Keys are saved per server origin in `~/.config/apiplant/cli.json`
(`$XDG_CONFIG_HOME` or `$APIPLANT_CONFIG_DIR` if set), mode `0600`. Per *server*,
not per directory: the same checkout pointed at a local server and at a deployed
one uses two separate credentials. `x` on the Session screen signs out and
discards the key.

## Organizations

Every account is created with a **personal organisation** it administers, so a
session always has somewhere to work: signing in drops you straight into the
sidebar, with that organisation active.

`O` switches between the organisations you belong to, and the choice is stored
with the key. `N` on the Session screen creates another one, using a form built
from the `organization` resource's own fields as the app defines them, and the
console switches to it once created. An app that provisions tenants itself will
have restricted that resource's `create` policy, in which case `N` reports this
rather than offering a form the server would reject.

Renaming the one you were given is an ordinary edit on the `organization`
resource, like any other record.

## Using it

A sidebar of every resource and function you can reach, grouped exactly as the
dashboard groups them; a pane showing whichever you picked.

Only accessible entries are listed: the console applies the same rules the
dashboard does, using the manifest and your session. A `private` action is
available to nobody. An `authenticated` action requires a session. Anything
scoped to an organization requires an active one, so a session with no
organization is not shown tables that would list nothing or actions that could
only return 403. A `role:` policy requires that role, or `admin`, which
satisfies every role check. The sidebar is rebuilt whenever your roles or
organization change.

Hiding an entry asserts that it is unavailable, so it is only done when that can
be established. The console derives your roles from the API rather than being
told them, and an app that does not permit listing your own `membership_role`
rows leaves it unable to determine them. In that case role-gated entries remain
visible and the server rejects them if the assumption was wrong, which is
preferable to hiding an entry from someone who is in fact permitted to use it.
Two cases cannot be predicted at all: `owner` narrows a list to your own rows
rather than rejecting the request, and ownership of a particular record is only
determinable per record, so those still surface as a server 403 on the specific
attempt.

Below the app's own resources is a **Console** group containing the same
purpose-built screens the dashboard provides in place of raw tables: your
account, your team, the organizations you belong to, your API keys, and the
current session. The underlying tenancy tables (`user`, `membership` and
`membership_role`) are not listed in the sidebar. They describe how membership
is *stored*, and a table of rows with a `user_id` column is a poor way to answer
"who works here". An app that enables one explicitly with `[admin] visible =
true` gets it as an ordinary resource, without a duplicate entry for the same
rows.

| Key | |
|-----|--|
| `tab` | move between the sidebar and the pane |
| `↑ ↓` / `k j` | move |
| `enter` | open a record, edit a field, pick a reference, or submit |
| `n` `e` `d` | new, edit, delete |
| `c` | the records belonging to this one (on a record) |
| `/` | search: matches any part of the resource's designated search field, case-insensitively |
| `[` `]` | previous / next page of 50 |
| `r` | reload |
| `space` | toggle a switch in a form |
| `D` | clear a field |
| `esc` | back |
| `g` `t` | give / take away a role (on the Team screen) |
| `O` | switch organization |
| `N` | start another organization (on the Session screen) |
| `?` | the full list |
| `q` / `ctrl-c` | quit |

Forms are built from the manifest, so they contain the fields the dashboard
would show and omit anything hidden, read-only or not writable by you. A
reference field opens a picker listing records by name rather than requiring a
UUID, and displays them the same way: lists and records ask the API to inline
referenced rows, so a column shows `Beta Foods` rather than a UUID. Where that
is refused, for a relation whose target you may not read, the request is retried
without expansion so the table is still shown, with ids in place of names. An
edit sends only the fields you changed.

The dashboard renders a record's children beneath it. A terminal has no room for
that, so `c` on a record lists them (its order lines, its payments) and opens
the ordinary list screen filtered to that parent. A function's form is generated
from its input schema, with one field per property including its description,
defaults and `enum` values; a function that declares a confirmation prompts for
one here as well.

Org-scoped resources need an active organization, sent as `X-Organization`;
`O` switches it, and the choice is remembered with the key.

## The team

The **Team** screen is everyone in the active organization, one row each, with
every role they hold. It appears whenever memberships can be listed at all.

`n` adds someone by the identity they registered with, usually an email address.
The console does not look the account up first, since you may only read users
you already share an organization with, which by definition excludes the person
being added. The server resolves the address and reports clearly when no account
matches it. `d` removes them again; this revokes their access to the
organization and does not delete their account.

Where the app can [send email](authentication.md#invitations), `n` **invites**
instead: the form indicates this, and the address receives a link that works
whether or not the recipient has an account. Without a mail provider, an
unrecognised address is a dead end; with one, the case does not arise.

Roles form a set drawn from two places, the membership's own `role` column and
its `membership_role` rows, so the screen combines them exactly as the server
does when checking a permission. `g` grants the highlighted person a role and
`t` revokes one. See [permissions](permissions.md) for what a role means.

The pickers offer only actions that will succeed. A role someone already holds
is not offered again, because the server rejects duplicates and a duplicate
would make revoking the first grant appear to have no effect. `admin` is shown
as holding every other role, which is how it is evaluated. Your own `admin` is
never offered for removal: an organization can only lose its last administrator
if that administrator removes themselves, so the console does not permit it,
though another admin can do so from their own Team screen. An app whose
`membership_role` you may not create reports this rather than offering a picker
whose actions would be rejected.

You cannot remove your own access while you are an admin, for the same reason
you cannot drop your own `admin` role: it is the other way an organization could
end up with no administrator.

The role carried by the *membership* itself, the primary role the server reports
as `role`, has no row to delete, so revoking it clears the column instead. It
behaves identically otherwise.

## The other Console screens

**Account** is your own record, built from the fields the app says are yours to
change, saved with `enter` on the button like any other form.

**Organizations** and **API keys** are ordinary tables, so they use the ordinary
list screen with the same keys, paging and detail view. Key creation is the
exception: the plaintext exists only in the response that issues it, and the
table stores only a hash, so `n` prompts for a name and the key is then
displayed once for copying. Editing a key is not offered, since a hash has
nothing editable.

**Session** shows the account you are signed in as and the credential in use
(the console resolves the account from the session token's subject, or from the
owner of the API key), along with the server, the API and dashboard addresses,
the active organization and the roles you hold in it. It can also issue a key
(`g`), create another organization (`N`), and sign out (`x`).

## When something goes wrong

Errors appear in a bordered row above the status line, wrapped rather than
truncated, and persist until the next successful action. Within a form they are
also shown beneath the button that produced them, so the message appears where
the action was taken.

Each error names the request beneath the message: the method, the full URL and
the status. "Not found" alone could mean a deleted record, a resource this app
does not define, or a console using the wrong prefix, so the line below
identifies which.

## What it is not

The console is an operator tool rather than a development tool. It does not edit
models, compile functions or run migrations; those are handled by
`apiplant build`, `apiplant run` and [studio](../README.md#studio). It only
calls the public API, using your credentials and permissions, so it can do
exactly what the same requests via `curl` would allow.
