# 22 · OAuth

Signing in with GitHub, Google, LinkedIn and X — which is four blocks of two
lines.

```toml
[oauth.github]
client_id     = "${GITHUB_CLIENT_ID}"
client_secret = "${GITHUB_CLIENT_SECRET}"
```

That is the integration. apiplant knows each provider's authorize URL, token
URL, userinfo endpoint, scopes, whether it wants PKCE and whether it insists on
the client secret as HTTP Basic — the four things they disagree about, and the
reason a handshake written by hand is rarely written twice. Naming a provider
mounts the endpoints and adds the table a half-finished sign-in lives in.

```
22-oauth/
├── main.toml              # the four [oauth.…] blocks, and nothing else about OAuth
├── public/
│   ├── index.html         # a sign-in page made of <a> tags
│   └── oauth/gitlab.svg   # a logo for a provider apiplant draws no mark for
└── seed/
```

There is no `resources/` directory: not one field has to be added for any of this,
because `display_name`, `avatar_url` and `email_placeholder` are all in the
built-in `user`. There is no function either, no callback handler, no state
table to declare, and no code that knows what GitHub is.

## Run it

```bash
createdb -h 127.0.0.1 -p 5432 -U postgres apiplant_oauth

export GITHUB_CLIENT_ID=Ov23li… GITHUB_CLIENT_SECRET=…
cargo run -p apiplant -- run --seed examples/22-oauth
```

Open <http://localhost:8104/>. One provider is enough to see the whole thing;
with none configured the app still boots and serves a sign-in page with no
buttons on it, which is the app working.

On the way up it prints the URI to register for each provider:

```
INFO apiplant_server:   oauth GitHub -> /api/auth/oauth/github/start
                        (redirect URI: http://localhost:8104/api/auth/oauth/github/callback)
```

## Getting the credentials

Every provider works the same way: create an app in its console, tell it where
to send people back, and copy the two values it gives you into the environment.
The redirect URI is the same for all four but for the provider's name, and it is
printed at boot — it must match **byte for byte**, trailing slash included:

```
http://localhost:8104/api/auth/oauth/<provider>/callback
```

### GitHub

1. <https://github.com/settings/developers> → **OAuth Apps** → **New OAuth App**.
   (Not "GitHub App" — that is a different product for acting on repositories.)
2. *Application name* and *Homepage URL* are yours to choose;
   `http://localhost:8104` will do.
3. *Authorization callback URL*: `http://localhost:8104/api/auth/oauth/github/callback`
4. **Register application**, then **Generate a new client secret** — it is shown
   once.

```bash
export GITHUB_CLIENT_ID=Ov23li…
export GITHUB_CLIENT_SECRET=…
```

The fastest of the four, and the only one that never asks about a privacy
policy. Start here.

### Google

1. <https://console.cloud.google.com/apis/credentials> — create or pick a
   project.
2. **OAuth consent screen** first, if you have not configured one: *External*,
   an app name, a support email. While it is in *Testing* only the accounts you
   list under *Test users* can sign in, which is fine for this.
3. **Credentials** → **Create credentials** → **OAuth client ID** → *Web
   application*.
4. Under **Authorised redirect URIs**, add
   `http://localhost:8104/api/auth/oauth/google/callback`.

```bash
export GOOGLE_CLIENT_ID=…apps.googleusercontent.com
export GOOGLE_CLIENT_SECRET=GOCSPX-…
```

### LinkedIn

1. <https://www.linkedin.com/developers/apps> → **Create app**. It has to be
   attached to a LinkedIn *Page*; a page you make for the purpose is fine.
2. **Products** → request **Sign In with LinkedIn using OpenID Connect**. Do
   this first: until it is granted, the `openid profile email` scopes are not
   available to the app and the sign-in fails at the consent screen with an
   unhelpful message.
3. **Auth** → *Authorized redirect URLs for your app* →
   `http://localhost:8104/api/auth/oauth/linkedin/callback`.
4. The client id and secret are on the same **Auth** tab.

```bash
export LINKEDIN_CLIENT_ID=…
export LINKEDIN_CLIENT_SECRET=…
```

### X

1. <https://developer.x.com/en/portal/dashboard> → create a project and an app
   (the free tier is enough to sign people in).
2. **User authentication settings** → *Set up*:
   * **App permissions**: *Read*
   * **Type of App**: **Web App, Automated App or Bot** — this is the one that
     matters. It makes the app a *confidential client*, which is what can hold a
     secret; a "Native/Public" app cannot, and its token requests are refused.
   * **Callback URI**: `http://localhost:8104/api/auth/oauth/x/callback`
   * **Website URL**: anything real, e.g. `http://localhost:8104`
3. **Keys and tokens** → *OAuth 2.0 Client ID and Client Secret*. These are
   **not** the API Key/Secret above them, which belong to the older OAuth 1.0a
   flow and will not work here.

```bash
export X_CLIENT_ID=…
export X_CLIENT_SECRET=…
```

X releases no email address to an ordinary app, so an account created this way
gets a placeholder — see *Things worth trying* below.

### Not on localhost?

Set `PUBLIC_URL` to the origin the browser actually uses — a tunnel's
`https://…`, the deployment's own domain — and register the URI apiplant prints
for that origin instead. LinkedIn and X are the strict ones about https;
GitHub and Google both accept `http://localhost` for development.

## The client side, in full

```html
<a href="/api/auth/oauth/github/start">Sign in with GitHub</a>
```

`public/index.html` is that four times over, plus six lines that read the token
out of the URL fragment when the browser comes back. The endpoint answers with a
redirect to the provider; the provider redirects back into the API; the API
finishes the handshake and sends the browser to `/` with the session on it.

Which buttons to draw comes from the server, so adding a provider is a config
change and not a front-end change:

```bash
curl -s http://localhost:8104/api/auth/oauth | jq
```
```json
{ "providers": [
  { "provider": "github", "label": "GitHub", "provides_email": true,
    "start_url": "http://localhost:8104/api/auth/oauth/github/start" }
] }
```

## The dashboard picks them up too

<http://localhost:8104/admin/> gets the same buttons, with each provider's own
mark on them, above the password form — on both the sign-in and the create-account
tabs. Nothing was configured for that: the admin manifest carries whichever
providers `[oauth]` names, so the console draws exactly the buttons that work.

*Your account* then grows a **Linked accounts** card: connect a second provider,
or disconnect one. Connecting is the same endpoint the buttons use, called with
the session — which is the whole difference between "sign me in" and "add this
to my account".

## Watching it work

```bash
API=http://localhost:8104/api

# Where the browser would go. Follow this URL yourself and approve.
curl -si "$API/auth/oauth/github/start" | grep -i location
```

The `state` is now a row, and it is the only thing that will let a callback
through:

```bash
psql apiplant_oauth -c \
  "select provider, used_at is null as pending, expires_at > now() as live from apiplant_oauth_state"
```

After approving, the browser lands on `/#token=…`. To watch the JSON instead,
set `token_delivery = "json"` in `[oauth]` and the callback answers:

```json
{ "token": "eyJ0eXAiOiJKV1Qi…", "user": { "email": "octo@example.com", … },
  "provider": "github", "created": true, "linked": false }
```

```bash
curl -s $API/auth/me -H "authorization: Bearer eyJ0eXAiOiJKV1Qi…"
# {"user_id":"36bd3a1a-…"}
```

That last line is the point. The token came out of an OAuth callback, and
`/auth/me` — which knows nothing about OAuth — accepts it, because it is the
same HS256 JWT with the same claims that `POST /auth/login` issues, signed with
the same `[auth] jwt_secret`. Every permission, hook and `owner`-scoped query in
the app works unchanged.

## The decision that matters

Everything else is plumbing. This is not:

> Somebody arrives with a verified GitHub address that matches an account
> already in the database. Are they that account's owner?

apiplant answers in four steps — an existing connection, a session that started
the flow, a **verified** address match, otherwise a new account — and the third
is the one to read carefully. It is safe only because the provider says it
verified the address: with an unverified one, anybody could set their address at
a careless provider to somebody else's and sign in as them. Several real "sign in
with" compromises were exactly that.

So an unverified match is never made. It is refused, with an answer that says
what to do instead:

> an account already uses ann@example.com — sign in with it, then connect GitHub
> from your account settings

`link_by_verified_email = false` extends that refusal to verified addresses too,
for an app that would rather nothing happen automatically.

## Things worth trying

* **Sign in with X.** The account gets an `@oauth.invalid` address and
  `email_placeholder = true`, because X releases no addresses. That is a fact
  about X rather than an error: the address is at a domain RFC 2606 reserves so
  it can never resolve, the flag records that apiplant invented it, and the
  framework will not try to mail it. A real app shows those accounts a *tell us
  your email* prompt.
* **Move the profile somewhere else.** Add a `resources/users.toml` that calls the
  picture `picture`, point `[oauth] avatar_field` at it, and it lands there
  instead — or set either field to `""` and apiplant stops writing it.
* **Add a fifth provider with a logo.** Uncomment the `[oauth.gitlab]` block at
  the bottom of `main.toml`. `icon = "/oauth/gitlab.svg"` points at a file that
  is already in `public/`, from
  [Super Tiny Icons](https://github.com/edent/SuperTinyIcons); take the `icon`
  line out and the button falls back to a **G** on a plain tile, which is what
  apiplant shows for a provider it draws no mark for.
* **Connect a second provider.** Sign in, then press *Connect Google*. Same
  endpoint, sent with a session — that is the entire difference between "sign me
  in" and "link this too", and the choice is recorded server-side rather than
  trusted from the callback.
* **Unlink your only credential.** Refused: an account with no password and no
  other provider would become permanently unreachable.
* **Post the callback twice.** The second is refused — a `state` is spent once,
  and so is the code it authorised.
* **Set `allow_registration = false`.** A provider button stops creating
  accounts (403) and keeps signing in the people who already have one.
* **Break `public_url` on purpose** — `http://127.0.0.1:8104` instead of
  `http://localhost:8104`, same server, different string — and every provider
  refuses. This is the failure everyone hits once.

## Related

* [Authentication → Signing in with somebody else's
  account](../../docs/authentication.md#signing-in-with-somebody-elses-account)
  — the account-resolution rules, the endpoints, and what it deliberately does
  not do (refresh tokens, `id_token` verification, cookies).
* [Configuration → `[oauth]`](../../docs/configuration.md#oauth) — every setting.
* [`examples/05-auth`](../05-auth) — the password half of the same story.

**Next:** back to [the index](../README.md) — this is the last one.
