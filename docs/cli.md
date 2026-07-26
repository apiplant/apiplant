# The console

```bash
apiplant cli [APP_DIR]
```

`apiplant cli` is the [admin dashboard](admin.md)'s job in a terminal: browse
resources, page through records, create and edit and delete them, and run the
app's callable functions — over SSH, in a tmux pane, without a browser.

It is a *client*. Point it at an app directory and it reads that directory for
one thing only: the address of the server. Everything else it knows — which
resources exist, which fields they have, which functions are callable, what you
are allowed to do — comes from the running server, as the same manifest the
dashboard loads. A model change is visible the moment the server restarts, and
the console can never describe an app different from the one actually running.

## Where it connects

By default, from `main.toml`:

| From | Used for |
|------|----------|
| `[server] host`, `port` | the address (a `0.0.0.0` bind is dialled as `127.0.0.1`) |
| `https/` | present ⇒ `https://`, absent ⇒ `http://` |
| `[admin] path` | where the manifest is fetched from |
| `[server] base_path` | a starting guess at the API prefix, until the manifest says otherwise |

The directory is only a way to find the server. Where the API actually *is* comes
from the manifest's `api_base_url`, because the process answering the requests is
the only thing that knows — a directory with no `main.toml`, a stale one, or one
belonging to a different deployment would otherwise send every call to a prefix
that is not served, and that arrives as an unexplained 404 on the first thing you
press. When the two disagree the console follows the server and says so in the
status line. A statically hosted dashboard puts a full URL in that field, so
pointing at one is enough to reach the API it was built for.

For anything else — an app behind a proxy, on another host, in a container —
name the server yourself:

```bash
apiplant cli ./my-app --api https://api.example.com
apiplant cli ./my-app --api box.local:8080
```

The scheme may be left off; it is assumed to be `https`. A self-signed
certificate is accepted on loopback and nowhere else.

`[admin] enabled = false` publishes no manifest, so the console has nothing to
read and says so before it starts.

## Signing in

Three doors, offered on the first screen:

**Open the dashboard in a browser.** The console opens a one-request web server
on `127.0.0.1`, sends you to the dashboard with its address attached, and waits.
The dashboard shows what is being asked for; press **Connect** and it mints a
key for whoever is signed in there and posts it straight back. Nothing is copied
by hand. If no browser can be opened — an SSH session, usually — the console
prints the address so you can open it wherever you are.

Two rules make that safe: the dashboard refuses any callback that is not plain
HTTP on a loopback host, so a crafted link cannot mail your key to a stranger,
and it never mints anything without a press.

**Sign in with an email and password.** The console calls `POST /auth/login`,
then trades the session for a long-lived key so the next run starts connected.
If the app has no `api_key` resource, or your account may not create one, the
session still works — it just ends when you quit.

**Paste an API key.** For a key you already have — the console asks the terminal
to deliver pastes whole, so a key cannot lose characters to a slow redraw or have
its own letters read as form shortcuts on the way in. It is checked against an
endpoint that actually needs it, so a bad key is refused here rather than on the
first list — but only a *rejection* disqualifies it. Any other failure says
something about the app, not about the credential, so it is reported and the key
is kept. A rejected key is examined and described — the wrong length, the wrong
prefix, characters that are not hexadecimal — because a box full of dots gives
you nothing to go on. The key itself is never echoed.

Keys are saved per server origin in `~/.config/apiplant/cli.json`
(`$XDG_CONFIG_HOME` or `$APIPLANT_CONFIG_DIR` if set), mode `0600`. Per *server*,
not per directory: the same checkout pointed at a local server and a deployed
one is two accounts. `x` on the Session screen signs out and forgets the key.

## Your first organization

Almost every resource is scoped to an organisation, so a session that belongs to
none lists nothing and fails every write. The console resolves that on the way
in rather than letting you wander around in it: the first thing after signing in
is a modal, and which one depends on what the app allows.

If the `organization` resource lets an authenticated caller create one — the
built-in default — it is a form built from that resource's own fields, whatever
the app has made those. Submit it and the server makes you its admin, the
console adopts it as active, and you are dropped into the sidebar.

The modal only appears when the server has actually *said* you belong to none.
A lookup that fails is a different fact, reported as an error rather than
treated as an empty answer — otherwise a hiccup would push someone who already
has an organisation towards making a second one.

If the app has narrowed that policy — it provisions tenants itself — there is
nothing useful to offer, so the modal says an administrator has to add you.
`r` re-checks (they may have, while it was on screen) and `x` signs out. A
`role:` policy counts as narrowed, because nobody with no organisation can
satisfy one.

Either way `esc` puts the modal away: your account, your API keys and any other
global resource work perfectly well without an organisation, and being trapped
in a modal in a terminal is worse than the problem. `N` on the Session screen
brings it back.

## Using it

A sidebar of every resource and function you can reach, grouped exactly as the
dashboard groups them; a pane showing whichever you picked. A resource the
server will not list for anyone, and a function with no endpoint, are left out —
they would only teach you the console is broken.

| Key | |
|-----|--|
| `tab` | move between the sidebar and the pane |
| `↑ ↓` / `k j` | move |
| `enter` | open a record, edit a field, pick a reference, or submit |
| `n` `e` `d` | new, edit, delete |
| `/` | search (on the field the resource names for it) |
| `[` `]` | previous / next page of 50 |
| `r` | reload |
| `space` | toggle a switch in a form |
| `D` | clear a field |
| `esc` | back |
| `O` | switch organization |
| `N` | create an organization (on the Session screen) |
| `?` | the full list |
| `q` / `ctrl-c` | quit |

Forms come from the manifest, so they carry the fields the dashboard would show
and nothing that is hidden, read-only or not yours to write. A reference field
opens a picker of records by name rather than asking you to type a UUID. An edit
sends only what you changed. A function's form is generated from its input
schema — one box per property, with its description, defaults and `enum` values
— and a function that declares a confirmation asks for one here too.

Org-scoped resources need an active organization, sent as `X-Organization`;
`O` switches it, and the choice is remembered with the key.

The Session screen shows who you are signed in as and what with — the console
resolves your account from the session token's subject, or from the owner of the
API key it is using — along with the server, the API and dashboard addresses, and
the active organization. It issues a fresh API key for pasting into a script
(`g`), starts an organization (`N`), and signs out (`x`).

## When something goes wrong

Errors get their own bordered row above the status line, wrapped rather than
truncated, and they stay until the next thing you do succeeds. Inside a form
they are also drawn under the button that produced them — a message on the far
side of the screen from what you just pressed is a message that reads as
"nothing happened".

Every one of them names the request underneath the message: the method, the full
URL and the status. "Not found" on its own covers a record someone deleted, a
resource this app does not have, and a console talking to the wrong prefix, and
only the last is worth panicking about — so the line below it says which.

## What it is not

The console is the operator's tool, not the developer's. It does not edit
models, compile functions or migrate anything — that is `apiplant build`,
`apiplant run` and [studio](../README.md#studio). It only ever calls the public
API, with your credentials and your permissions, so it can do exactly what you
could do with `curl` and nothing more.
