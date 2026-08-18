//! Talking to a running apiplant server over HTTP.
//!
//! The console is a second client for the same API the dashboard uses, so it
//! learns the app the same way the dashboard does — by fetching the admin
//! manifest the server builds on boot. Nothing here is generated from the app
//! directory: the directory only tells us *which server* to ask, and the server
//! is the authority on what it currently serves.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Map, Value};

/// The manifest file the server serves under its admin path.
pub const MANIFEST_FILE: &str = "apiplant-admin.json";

// --- the manifest ----------------------------------------------------------
//
// A deliberately forgiving mirror of `apiplant_server::admin`'s manifest: every
// field defaults, so a console built against one version still starts against a
// server that has since grown a field it does not know about.

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub title: String,
    pub app_name: String,
    pub api_base_url: String,
    pub docs_url: Option<String>,
    pub auth: AuthManifest,
    pub resources: Vec<ResourceManifest>,
    pub functions: Vec<FunctionManifest>,
    pub agents: Vec<AgentManifest>,
    /// Present only in an app that takes money. The `billing_*` tables are
    /// ordinary resources and already appear in the sidebar; this is the
    /// deployment facts around them that no resource carries — which provider,
    /// which currency, and whether anything is actually being recorded.
    #[serde(default)]
    pub billing: Option<BillingManifest>,
    /// The tenancy settings that belong to no single resource — chiefly who,
    /// if anybody, this deployment treats as its back office.
    pub organization: OrganizationManifest,
}

/// What an app's `[organization]` section amounts to, as the server reports it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct OrganizationManifest {
    /// `[organization] global_admin_role`, as a permission so it can be asked
    /// the same questions as any other. `private` — the default — means this
    /// deployment has no back office.
    pub global_admin_role: ActionPermission,
    pub known_classes: Vec<String>,
}

/// What an app's `[payments]` section amounts to, as the server reports it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BillingManifest {
    pub provider: String,
    pub publishable_key: String,
    pub currency: String,
    /// Whether the amounts in the price list are quoted before tax.
    pub automatic_tax: bool,
    pub tax_id_collection: bool,
    /// Whether the webhook can verify a delivery. `false` means checkouts
    /// complete and nothing is ever written down — the one billing
    /// misconfiguration that looks like silence rather than an error.
    pub webhooks_configured: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthManifest {
    pub identity_field: String,
    pub identity_label: String,
    pub allow_registration: bool,
    /// Whether the server can send email at all — the reason the three below
    /// are off when they are, and worth saying out loud rather than leaving an
    /// operator wondering where the invite button went.
    pub email_enabled: bool,
    /// New accounts must confirm their address before they can sign in.
    pub require_email_verification: bool,
    /// The Team screen may invite somebody who has no account yet.
    pub invitations_enabled: bool,
    /// `/auth/password/forgot` exists.
    pub password_reset_enabled: bool,
    /// Whether an organisation's admins may act as one of its members. A
    /// deployment with a back office can impersonate whatever this says — see
    /// [`OrganizationManifest::global_admin_role`] — so it is only half the
    /// answer to whether the route is there at all.
    pub allow_impersonation: bool,
    /// What the register form collects besides the identity and a password.
    pub signup_fields: Vec<FieldManifest>,
    /// What someone may change about themselves on the account screen.
    pub profile_fields: Vec<FieldManifest>,
    /// Every role this app names anywhere — in a permission, a function or a
    /// field's options. It is what the Team screen offers, because a role the
    /// app never mentions grants nothing to whoever is given it.
    pub known_roles: Vec<String>,
}

impl Default for AuthManifest {
    fn default() -> Self {
        AuthManifest {
            identity_field: "email".into(),
            identity_label: "Email".into(),
            allow_registration: false,
            // A console built against a newer server should assume the
            // *narrower* thing: offering a door that isn't there is worse than
            // omitting one that is.
            email_enabled: false,
            require_email_verification: false,
            invitations_enabled: false,
            password_reset_enabled: false,
            allow_impersonation: false,
            signup_fields: Vec::new(),
            profile_fields: Vec::new(),
            known_roles: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ResourceManifest {
    pub name: String,
    pub label: String,
    pub plural: String,
    pub group: Option<String>,
    pub order: i64,
    pub visible: bool,
    /// One of the auth/tenancy resources the dashboard manages with a
    /// purpose-built screen instead of a generic table.
    pub auth_resource: bool,
    pub scope: String,
    pub display_field: Option<String>,
    pub search_field: Option<String>,
    pub columns: Vec<String>,
    pub fields: Vec<FieldManifest>,
    /// What each reference field points at, and the key the expanded record
    /// arrives under when the API is asked to inline it.
    pub relations: Vec<RelationManifest>,
    /// The resources whose records point back at this one.
    pub children: Vec<ChildManifest>,
    pub permissions: ActionPermissions,
}

/// A reference from this resource to another.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RelationManifest {
    /// The field holding the id.
    pub field: String,
    /// The key `?expand=` puts the whole record under.
    pub relation: String,
    /// The resource it points at.
    pub target: String,
    pub label: String,
}

/// A resource that points back at this one — the records "underneath" it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChildManifest {
    /// The child resource's name.
    pub resource: String,
    /// The field on the child holding this record's id.
    pub field: String,
    pub label: String,
}

impl ResourceManifest {
    pub fn field(&self, name: &str) -> Option<&FieldManifest> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// The best human name for one record of this resource.
    pub fn title_of(&self, record: &Value) -> String {
        let display = self.display_field.as_deref().unwrap_or("id");
        for key in [display, "name", "title", "id"] {
            if let Some(value) = record.get(key) {
                let text = scalar(value);
                if !text.is_empty() {
                    return text;
                }
            }
        }
        String::new()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FieldManifest {
    pub name: String,
    pub label: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub widget: String,
    pub help: Option<String>,
    pub options: Vec<FieldOption>,
    pub required: bool,
    pub hidden: bool,
    pub admin_visible: bool,
    pub readonly: bool,
    pub references: Option<String>,
    pub default_value: Option<Value>,
    pub writable: bool,
}

impl FieldManifest {
    /// Whether an operator should be offered an input for this field.
    pub fn editable(&self) -> bool {
        self.writable && self.admin_visible && !self.hidden && !self.readonly
    }
}

/// Turn the text typed into a field's box into the JSON the API wants.
///
/// An empty box means "say nothing about this field" for everything but a
/// boolean, which has no empty state — so it is the caller, not this, that
/// decides whether to omit it.
pub fn parse_typed(ty: &str, label: &str, text: &str) -> Result<Value> {
    let text = text.trim();
    match ty {
        "boolean" => Ok(Value::Bool(matches!(
            text.to_ascii_lowercase().as_str(),
            "1" | "t" | "true" | "y" | "yes" | "on"
        ))),
        "integer" | "big_int" => text
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| anyhow!("`{label}` needs a whole number")),
        "float" | "number" => text
            .parse::<f64>()
            .map(Value::from)
            .map_err(|_| anyhow!("`{label}` needs a number")),
        "json" | "object" | "array" => {
            serde_json::from_str(text).map_err(|e| anyhow!("`{label}` needs valid JSON: {e}"))
        }
        _ => Ok(Value::String(text.to_string())),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ActionPermissions {
    pub list: ActionPermission,
    pub read: ActionPermission,
    pub create: ActionPermission,
    pub update: ActionPermission,
    pub delete: ActionPermission,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ActionPermission {
    pub value: String,
    pub role: Option<String>,
    pub note: String,
    pub requires_org: bool,
}

impl ActionPermission {
    /// Whether this action is offered to anybody at all. `private` means the
    /// server refuses it for everyone, so showing the key would only produce a
    /// 403 — the resource is written to by hooks, or by nothing.
    pub fn possible(&self) -> bool {
        self.value != "private"
    }

    /// Whether *this* caller may do it, as far as the manifest can say.
    ///
    /// The API remains the authority — `owner` narrows to your own rows and
    /// only the server knows which those are. This is for deciding what to put
    /// in front of somebody, which is exactly what the dashboard does with it.
    pub fn allowed(&self, signed_in: bool, organization: bool, roles: &[String]) -> bool {
        match self.value.as_str() {
            "public" => true,
            "private" => false,
            "authenticated" => signed_in,
            // Organisation-scoped work needs somewhere to do it.
            "member" | "owner" => signed_in && organization,
            _ => match &self.role {
                // `admin` holds every role, here as everywhere else.
                Some(role) => roles.iter().any(|held| held == role || held == "admin"),
                None => false,
            },
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FunctionManifest {
    pub name: String,
    pub label: String,
    pub description: String,
    pub group: Option<String>,
    pub order: i64,
    pub method: String,
    pub permission: String,
    pub requires_org: bool,
    pub visible: bool,
    pub confirm: Option<String>,
    pub run_label: String,
    pub input_schema: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AgentManifest {
    pub name: String,
    pub label: String,
    pub description: String,
    pub scope: String,
    pub storage: bool,
    pub thread_resource: Option<String>,
    pub message_resource: Option<String>,
    pub chat: ActionPermission,
    pub history: ActionPermission,
    pub delete_history: ActionPermission,
}

// --- the client ------------------------------------------------------------

/// Whatever the console has been given to prove who it is.
#[derive(Debug, Clone, Default)]
pub struct Credentials {
    /// A long-lived key, sent as `X-Api-Key`. This is what gets saved.
    pub api_key: Option<String>,
    /// A session JWT from `POST /auth/login`. Never saved: it expires.
    pub token: Option<String>,
}

impl Credentials {
    pub fn is_empty(&self) -> bool {
        self.api_key.is_none() && self.token.is_none()
    }
}

/// What `/auth/impersonate` and its `stop` hand back: a session, and enough
/// about it to say whose account it is.
#[derive(Debug, Clone)]
pub struct Borrowed {
    pub token: String,
    /// The account the token now acts as.
    pub user_id: String,
    /// Who is really behind it. `None` on the way back out.
    pub impersonator: Option<String>,
    /// The organisation the token is pinned to, when it is one. A pinned
    /// session ignores `X-Organization` entirely, which is what stops an
    /// admin from following a borrowed account into somebody else's tenant.
    pub organization: Option<String>,
}

impl Borrowed {
    fn parse(response: &Value) -> Result<Borrowed> {
        let text = |key: &str| {
            response
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|value| !value.is_empty())
        };
        Ok(Borrowed {
            token: text("token").ok_or_else(|| anyhow!("the server did not return a session"))?,
            user_id: text("user_id").unwrap_or_default(),
            impersonator: text("impersonator"),
            organization: text("organization_id"),
        })
    }
}

/// A configured connection to one apiplant server.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    /// `https://host:port`, with no trailing slash.
    pub origin: String,
    /// The API's mount point, e.g. `/api`. Empty when mounted at the root.
    pub base_path: String,
    /// Where the dashboard is served, e.g. `/admin`.
    pub admin_path: String,
    pub credentials: Credentials,
    /// Sent as `X-Organization` — org-scoped resources need it.
    pub organization: Option<String>,
}

impl Client {
    pub fn new(origin: String, base_path: String, admin_path: String) -> Result<Client> {
        let http = http_client(&origin)?;
        Ok(Client {
            http,
            origin: origin.trim_end_matches('/').to_string(),
            base_path: normalise_path(&base_path),
            admin_path: normalise_path(&admin_path),
            credentials: Credentials::default(),
            organization: None,
        })
    }

    /// Move to the API location the *server* reports, and say so if it moved.
    ///
    /// The app directory is a guess: it is whichever checkout the operator
    /// happened to point at, and it may have no `main.toml`, a stale one, or one
    /// belonging to an entirely different deployment than `--api` names. The
    /// manifest is not a guess — it is built by the process answering the
    /// requests — so `api_base_url` wins over anything read off disk. Getting
    /// this wrong sends every call to a prefix the server does not serve, which
    /// arrives as an unexplained 404 on the first thing you press.
    ///
    /// A statically hosted dashboard (`apiplant admin --api …`) writes a full
    /// URL there rather than a path, so that form is understood too.
    pub fn adopt_api_base(&mut self, api_base_url: &str) -> Result<Option<String>> {
        let trimmed = api_base_url.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        let (origin, path) = match trimmed.find("://") {
            Some(scheme_end) => {
                // Split after the authority: the first `/` past `scheme://`.
                let rest = &trimmed[scheme_end + 3..];
                match rest.find('/') {
                    Some(at) => (
                        trimmed[..scheme_end + 3 + at].to_string(),
                        rest[at..].to_string(),
                    ),
                    None => (trimmed.to_string(), String::new()),
                }
            }
            None => (self.origin.clone(), trimmed.to_string()),
        };

        let origin = origin.trim_end_matches('/').to_string();
        let path = normalise_path(&path);
        if origin == self.origin && path == self.base_path {
            return Ok(None);
        }

        let note = format!(
            "the server serves its API at {origin}{path}, not {}{} — using the server's answer",
            self.origin, self.base_path
        );
        if origin != self.origin {
            // The certificate policy was chosen for the old host, so the client
            // has to be rebuilt rather than merely repointed.
            self.http = http_client(&origin)?;
            self.origin = origin;
        }
        self.base_path = path;
        Ok(Some(note))
    }

    /// The dashboard's address — what we point a browser at.
    pub fn admin_url(&self) -> String {
        format!("{}{}/", self.origin, self.admin_path)
    }

    pub fn manifest_url(&self) -> String {
        format!("{}{}/{MANIFEST_FILE}", self.origin, self.admin_path)
    }

    /// Fetch the manifest. This doubles as the reachability check: if it fails,
    /// there is no point showing a sign-in screen for a server that is not
    /// there.
    pub async fn manifest(&self) -> Result<Manifest> {
        let url = self.manifest_url();
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("could not reach {url}"))?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "{url} answered {} — is the admin dashboard enabled?",
                response.status().as_u16()
            ));
        }
        response
            .json::<Manifest>()
            .await
            .with_context(|| format!("{url} did not return an admin manifest"))
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}{}", self.origin, self.base_path, path)
    }

    /// One API call, with whatever credentials we hold attached.
    ///
    /// Every failure comes back naming the request that produced it. A bare
    /// "not found" in a terminal is unactionable — the same three words cover a
    /// record that was deleted, an app that does not have that resource, and a
    /// console pointed at the wrong API prefix — and only the last one is worth
    /// panicking about.
    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let url = self.url(path);
        let mut request = self.http.request(method.clone(), &url);
        if let Some(key) = &self.credentials.api_key {
            request = request.header("X-Api-Key", key);
        }
        if let Some(token) = &self.credentials.token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        if let Some(org) = &self.organization {
            request = request.header("X-Organization", org.as_str());
        }
        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().await.map_err(|error| {
            // A transport failure never reached the app, so the app's own
            // vocabulary would be misleading here.
            let reason = if error.is_timeout() {
                "timed out".to_string()
            } else if error.is_connect() {
                "could not connect".to_string()
            } else {
                error.to_string()
            };
            anyhow!("{reason}\n{method} {url}")
        })?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let payload: Value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text.clone()))
        };

        if status.is_success() {
            return Ok(payload);
        }
        Err(anyhow!(
            "{}\n{method} {url} → {} {}",
            // The API reports failures as `{"error": "..."}`; anything else is a
            // proxy or a panic, and then the status is all we can honestly say.
            payload
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| explain(status.as_u16())),
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
        ))
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, path, None).await
    }

    pub async fn list(&self, resource: &str, query: &[(&str, String)]) -> Result<Vec<Value>> {
        let path = format!("/{}{}", encode(resource), query_string(query));
        Ok(as_records(self.get(&path).await?))
    }

    pub async fn read(&self, resource: &str, id: &str) -> Result<Value> {
        self.read_expanding(resource, id, &[]).await
    }

    /// Read a record with the named relations inlined, so a reference shows the
    /// name of what it points at rather than a uuid.
    pub async fn read_expanding(
        &self,
        resource: &str,
        id: &str,
        relations: &[String],
    ) -> Result<Value> {
        let query = expand_query(relations);
        self.get(&format!(
            "/{}/{}{}",
            encode(resource),
            encode(id),
            query_string(&query)
        ))
        .await
    }

    pub async fn create(&self, resource: &str, body: Value) -> Result<Value> {
        self.request(
            reqwest::Method::POST,
            &format!("/{}", encode(resource)),
            Some(body),
        )
        .await
    }

    pub async fn update(&self, resource: &str, id: &str, body: Value) -> Result<Value> {
        self.request(
            reqwest::Method::PATCH,
            &format!("/{}/{}", encode(resource), encode(id)),
            Some(body),
        )
        .await
    }

    pub async fn delete(&self, resource: &str, id: &str) -> Result<()> {
        self.request(
            reqwest::Method::DELETE,
            &format!("/{}/{}", encode(resource), encode(id)),
            None,
        )
        .await
        .map(|_| ())
    }

    pub async fn invoke(&self, function: &FunctionManifest, body: Value) -> Result<Value> {
        let path = format!("/functions/{}", encode(&function.name));
        let method = reqwest::Method::from_bytes(function.method.as_bytes())
            .unwrap_or(reqwest::Method::POST);
        // A GET carries no body; sending one is how you get a confusing 400.
        let body = if method == reqwest::Method::GET {
            None
        } else {
            Some(body)
        };
        self.request(method, &path, body).await
    }

    /// Exchange credentials for a session token.
    pub async fn login(
        &self,
        identity_field: &str,
        identity: &str,
        password: &str,
    ) -> Result<String> {
        let body = json!({ identity_field: identity, "password": password });
        self.authenticate("/auth/login", body).await
    }

    /// Create an account, on an app that allows it.
    ///
    /// Returns `None` when the app confirms addresses: there is no session yet
    /// and there is not supposed to be one, so this is an outcome rather than
    /// the failure [`authenticate`](Self::authenticate) would call it.
    pub async fn register(&self, body: Value) -> Result<Option<String>> {
        let response = self
            .request(reqwest::Method::POST, "/auth/register", Some(body))
            .await?;
        if let Some(token) = response.get("token").and_then(Value::as_str) {
            return Ok(Some(token.to_string()));
        }
        if response
            .get("verification_required")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Ok(None);
        }
        Err(anyhow!("the server did not return a session token"))
    }

    /// Email an invitation to join the active organisation.
    ///
    /// Only offered where `auth.invitations_enabled` says the route exists —
    /// see [`AuthManifest`]. Unlike `POST /membership`, the person invited need
    /// not have an account yet.
    pub async fn invite(&self, email: &str, role: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            "/auth/invitations",
            Some(json!({ "email": email, "role": role })),
        )
        .await
        .map(|_| ())
    }

    /// Ask for a password reset link.
    ///
    /// Succeeds whether or not the address has an account — the endpoint
    /// answers 202 either way, so that it cannot be used to find out which
    /// addresses are registered. The console repeats that hedge rather than
    /// claiming a message was sent.
    pub async fn forgot_password(&self, email: &str) -> Result<()> {
        self.request(
            reqwest::Method::POST,
            "/auth/password/forgot",
            Some(json!({ "email": email })),
        )
        .await
        .map(|_| ())
    }

    async fn authenticate(&self, path: &str, body: Value) -> Result<String> {
        let response = self
            .request(reqwest::Method::POST, path, Some(body))
            .await?;
        response
            .get("token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("the server did not return a session token"))
    }

    /// Take a session that acts as `user_id`.
    ///
    /// Who may is the server's business — an organisation's admin over one of
    /// its members, or the back office over anybody — and a refusal comes back
    /// saying which rule was missed, so the console asks rather than guessing.
    pub async fn impersonate(&self, user_id: &str) -> Result<Borrowed> {
        let response = self
            .request(
                reqwest::Method::POST,
                "/auth/impersonate",
                Some(json!({ "user_id": user_id })),
            )
            .await?;
        Borrowed::parse(&response)
    }

    /// Hand back a borrowed session and get one for whoever borrowed it.
    pub async fn stop_impersonating(&self) -> Result<Borrowed> {
        let response = self
            .request(reqwest::Method::POST, "/auth/impersonate/stop", None)
            .await?;
        Borrowed::parse(&response)
    }

    /// Mint a long-lived key for whoever the current credentials identify.
    ///
    /// Signing in gives us a token that expires; trading it for a key once is
    /// what lets the next `apiplant cli` start already signed in.
    pub async fn create_api_key(&self, name: &str) -> Result<String> {
        let response = self
            .request(
                reqwest::Method::POST,
                "/auth/apikeys",
                Some(json!({ "name": name })),
            )
            .await?;
        response
            .get("api_key")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow!("the server did not return an API key"))
    }

    /// The organisations the caller belongs to, as `(id, label)`.
    pub async fn organizations(&self) -> Result<Vec<(String, String)>> {
        let rows = self
            .list("organization", &[("limit", "100".into())])
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                let id = row.get("id").and_then(Value::as_str)?.to_string();
                let label = row
                    .get("name")
                    .or_else(|| row.get("slug"))
                    .map(scalar)
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| id.clone());
                Some((id, label))
            })
            .collect())
    }
}

/// One HTTP client, configured for the host it will talk to.
///
/// A development server almost always has a self-signed certificate, and
/// refusing to talk to the loopback address over one would make `cli` useless in
/// exactly the case it is most useful. Anywhere else, certificates are checked.
fn http_client(origin: &str) -> Result<reqwest::Client> {
    let loopback =
        origin.contains("127.0.0.1") || origin.contains("localhost") || origin.contains("[::1]");
    reqwest::Client::builder()
        .danger_accept_invalid_certs(loopback)
        .user_agent(concat!("apiplant-cli/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("could not start an HTTP client")
}

/// What a status code means here, when the server did not say.
fn explain(status: u16) -> String {
    match status {
        401 => "not signed in, or the credential was rejected".into(),
        403 => "not allowed — check your role in this organization".into(),
        404 => "the server has no endpoint there. If this happens on everything, \
                the console is pointed at the wrong API base path"
            .into(),
        405 => "that endpoint does not accept this method".into(),
        408 | 504 => "the server took too long".into(),
        502 | 503 => "the server is not answering — is it still running?".into(),
        code if code >= 500 => "the server hit an error handling that".into(),
        _ => "the request was refused".into(),
    }
}

// --- small helpers ---------------------------------------------------------

/// `/api`, `api/`, `` and `/` all mean the same two things; settle on one.
fn normalise_path(path: &str) -> String {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("/{trimmed}")
    }
}

/// The `?expand=` parameter for a set of relations, or nothing when there are
/// none to ask for.
pub fn expand_query(relations: &[String]) -> Vec<(&'static str, String)> {
    if relations.is_empty() {
        Vec::new()
    } else {
        vec![("expand", relations.join(","))]
    }
}

pub fn as_records(value: Value) -> Vec<Value> {
    match value {
        Value::Array(rows) => rows,
        // Some endpoints wrap the page; accept either shape rather than showing
        // an empty table for a response that plainly has rows in it.
        Value::Object(mut map) => match map.remove("data").or_else(|| map.remove("records")) {
            Some(Value::Array(rows)) => rows,
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

pub fn as_object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// A JSON value as one line of text, for a table cell.
pub fn scalar(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => (if *flag { "yes" } else { "no" }).to_string(),
        other => other.to_string(),
    }
}

fn query_string(query: &[(&str, String)]) -> String {
    if query.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = query
        .iter()
        .map(|(key, value)| format!("{}={}", encode(key), encode(value)))
        .collect();
    format!("?{}", pairs.join("&"))
}

/// Percent-encode one path or query component.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_borrowed_session_carries_the_pin_and_the_actor() {
        let borrowed = Borrowed::parse(&json!({
            "token": "jwt",
            "user_id": "u-sam",
            "impersonator": "u-me",
            "organization_id": "org-1",
        }))
        .expect("a session");
        assert_eq!(borrowed.token, "jwt");
        assert_eq!(borrowed.user_id, "u-sam");
        assert_eq!(borrowed.impersonator.as_deref(), Some("u-me"));
        assert_eq!(borrowed.organization.as_deref(), Some("org-1"));

        // An unpinned session — the back office's — reports `null` for the pin,
        // and stopping reports one for the actor. Both mean the same thing as
        // an absent key, and neither is a string to carry around.
        let unpinned = Borrowed::parse(&json!({
            "token": "jwt",
            "user_id": "u-me",
            "impersonator": Value::Null,
            "organization_id": Value::Null,
        }))
        .expect("a session");
        assert!(unpinned.impersonator.is_none());
        assert!(unpinned.organization.is_none());

        // No token is no session, whatever else came back.
        assert!(Borrowed::parse(&json!({ "user_id": "u-sam" })).is_err());
    }

    #[test]
    fn paths_normalise_to_one_spelling() {
        let client = Client::new("http://x:1/".into(), "api/".into(), "/admin/".into()).unwrap();
        assert_eq!(client.origin, "http://x:1");
        assert_eq!(client.base_path, "/api");
        assert_eq!(client.url("/user"), "http://x:1/api/user");
        assert_eq!(client.admin_url(), "http://x:1/admin/");

        // The API mounted at the root has no prefix at all — not a stray slash
        // that would turn `/user` into `//user`.
        let root = Client::new("http://x:1".into(), "/".into(), "/admin".into()).unwrap();
        assert_eq!(root.base_path, "");
        assert_eq!(root.url("/user"), "http://x:1/user");
    }

    #[test]
    fn the_server_decides_where_its_api_is() {
        // The directory said the API is at the root; the running server says
        // `/api`. Believing the directory sends every call to a prefix that is
        // not served, which arrives as an unexplained 404 on the first press.
        let mut client = Client::new("http://x:1".into(), "".into(), "/admin".into()).unwrap();
        let note = client.adopt_api_base("/api").unwrap();
        assert!(note.is_some(), "a move should be reported");
        assert_eq!(client.base_path, "/api");
        assert_eq!(client.url("/auth/login"), "http://x:1/api/auth/login");

        // Agreement is silent.
        assert!(client.adopt_api_base("/api").unwrap().is_none());

        // A statically hosted dashboard writes a full URL there, not a path.
        let mut client = Client::new("http://x:1".into(), "/old".into(), "/admin".into()).unwrap();
        client.adopt_api_base("https://api.example.com/v2").unwrap();
        assert_eq!(client.origin, "https://api.example.com");
        assert_eq!(client.base_path, "/v2");
        assert_eq!(client.url("/user"), "https://api.example.com/v2/user");

        // A full URL with no path at all still means "mounted at the root".
        let mut client = Client::new("http://x:1".into(), "/old".into(), "/admin".into()).unwrap();
        client.adopt_api_base("https://api.example.com").unwrap();
        assert_eq!(client.base_path, "");
        assert_eq!(client.url("/user"), "https://api.example.com/user");

        // An empty manifest field is not an instruction to move to the root.
        let mut client = Client::new("http://x:1".into(), "/api".into(), "/admin".into()).unwrap();
        assert!(client.adopt_api_base("  ").unwrap().is_none());
        assert_eq!(client.base_path, "/api");
    }

    #[test]
    fn failures_name_the_request_that_produced_them() {
        // "not found" alone covers a deleted record and a console pointed at the
        // wrong prefix, and only one of those is worth panicking about.
        assert!(explain(404).contains("API base path"));
        assert!(explain(401).contains("rejected"));
        assert!(explain(503).contains("still running"));
    }

    #[test]
    fn query_components_are_encoded() {
        assert_eq!(
            query_string(&[("limit", "50".into()), ("name", "a b&c".into())]),
            "?limit=50&name=a%20b%26c"
        );
        assert_eq!(query_string(&[]), "");
    }

    #[test]
    fn typed_fields_parse_what_was_typed() {
        assert_eq!(parse_typed("integer", "Count", " 12 ").unwrap(), json!(12));
        assert!(parse_typed("integer", "Count", "twelve").is_err());

        // A checkbox has no empty state: an untouched one is a `false`, not a
        // field the request leaves out.
        assert_eq!(parse_typed("boolean", "Live", "yes").unwrap(), json!(true));
        assert_eq!(parse_typed("boolean", "Live", "").unwrap(), json!(false));

        assert_eq!(
            parse_typed("json", "Meta", r#"{"a":1}"#).unwrap(),
            json!({"a": 1})
        );
        assert!(parse_typed("json", "Meta", "{").is_err());

        // An unknown type is text, which is what the API will coerce anyway.
        assert_eq!(parse_typed("uuid", "Id", "abc").unwrap(), json!("abc"));
    }

    #[test]
    fn a_list_response_is_read_from_either_shape() {
        assert_eq!(as_records(json!([{"id": 1}])).len(), 1);
        assert_eq!(as_records(json!({"data": [{"id": 1}, {"id": 2}]})).len(), 2);
        assert!(as_records(json!({"error": "no"})).is_empty());
    }
}
