//! # apiplant-auth
//!
//! Authentication primitives, independent of HTTP so they can be unit-tested in
//! isolation:
//!
//! * [`Authenticator`] — password hashing (argon2), JWT session tokens, and
//!   API-key generation/hashing.
//! * [`Principal`] — the resolved caller identity, including the organisations
//!   they belong to and their **role within each** (roles are per-organisation).
//!
//! Authorization itself (mapping a resource's [`Access`](apiplant_core::Access)
//! policy plus org context to a decision) lives in the server, where the active
//! organisation and resource schema are known.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Auth errors.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("password hashing failed: {0}")]
    Hash(String),
    #[error("invalid or expired token")]
    Token,
}

/// The role that satisfies every other.
///
/// An `admin` of an organisation holds, by definition, every role the app
/// defines there — so a `role:billing` permission passes for them without
/// anyone having to grant `billing` explicitly. This is a rule about *checks*,
/// not about data: it is never written into anyone's roles, so removing a role
/// from an admin cannot silently do nothing.
pub const ADMIN_ROLE: &str = "admin";

/// A user's membership in one organisation, and the roles they hold there.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OrgMembership {
    pub org_id: Uuid,
    /// The member's *primary* role, from `membership.role`. Kept as its own
    /// field because that is the column apps and hook contexts already read.
    pub role: Option<String>,
    /// Every role held here, primary included — one entry per role, in the
    /// order the database returned them.
    pub roles: Vec<String>,
}

impl OrgMembership {
    /// Build a membership from its primary role and any additional ones,
    /// keeping `roles` a set with the primary first.
    pub fn new(
        org_id: Uuid,
        role: Option<String>,
        extra: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut roles: Vec<String> = Vec::new();
        for candidate in role.clone().into_iter().chain(extra) {
            if !candidate.is_empty() && !roles.contains(&candidate) {
                roles.push(candidate);
            }
        }
        OrgMembership {
            org_id,
            role,
            roles,
        }
    }

    /// Whether this membership carries `role` — directly, or by being `admin`.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|held| held == role) || self.is_admin()
    }

    /// Whether this membership holds `admin` itself.
    pub fn is_admin(&self) -> bool {
        self.roles.iter().any(|held| held == ADMIN_ROLE)
    }
}

/// The authenticated caller behind a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user_id: Uuid,
    /// Organisations the caller belongs to (loaded per request). Drives all
    /// org-scoped access and `role:` checks.
    pub organizations: Vec<OrgMembership>,
}

impl Principal {
    /// Membership in a specific organisation, if any.
    pub fn membership(&self, org: Uuid) -> Option<&OrgMembership> {
        self.organizations.iter().find(|m| m.org_id == org)
    }

    /// Whether the caller belongs to `org`.
    pub fn is_member(&self, org: Uuid) -> bool {
        self.membership(org).is_some()
    }

    /// The caller's *primary* role within `org`, if any.
    ///
    /// This is what a hook context's `role` reports. Authorization asks
    /// [`has_role_in`](Self::has_role_in) instead, because holding a role is
    /// not the same as it being your headline one.
    pub fn role_in(&self, org: Uuid) -> Option<&str> {
        self.membership(org).and_then(|m| m.role.as_deref())
    }

    /// Every role the caller holds in `org`.
    pub fn roles_in(&self, org: Uuid) -> &[String] {
        self.membership(org)
            .map(|m| m.roles.as_slice())
            .unwrap_or(&[])
    }

    /// Whether the caller may act as `role` in `org`. The question every
    /// `role:` permission asks.
    pub fn has_role_in(&self, org: Uuid, role: &str) -> bool {
        self.membership(org).is_some_and(|m| m.has_role(role))
    }

    /// Whether the caller administers `org`.
    pub fn is_admin_of(&self, org: Uuid) -> bool {
        self.membership(org).is_some_and(OrgMembership::is_admin)
    }

    /// Every organisation id the caller belongs to.
    pub fn org_ids(&self) -> Vec<Uuid> {
        self.organizations.iter().map(|m| m.org_id).collect()
    }

    /// Organisations where the caller may act as `role` — including the ones
    /// they merely administer.
    pub fn org_ids_with_role(&self, role: &str) -> Vec<Uuid> {
        self.organizations
            .iter()
            .filter(|m| m.has_role(role))
            .map(|m| m.org_id)
            .collect()
    }
}

/// JWT claims for a session token. Org memberships are *not* baked in — they are
/// resolved fresh from the database each request so changes take effect at once.
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    /// Subject: the user id.
    sub: String,
    /// Expiry (unix seconds).
    exp: i64,
}

/// Issues and verifies credentials for one running server.
#[derive(Clone)]
pub struct Authenticator {
    secret: Vec<u8>,
    session_ttl_secs: i64,
}

impl Authenticator {
    pub fn new(secret: impl Into<Vec<u8>>, session_ttl_secs: u64) -> Self {
        Authenticator {
            secret: secret.into(),
            session_ttl_secs: session_ttl_secs as i64,
        }
    }

    // --- Passwords --------------------------------------------------------

    /// Hash a plaintext password with argon2id (random salt).
    pub fn hash_password(&self, plaintext: &str) -> Result<String, Error> {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        use argon2::Argon2;
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(plaintext.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| Error::Hash(e.to_string()))
    }

    /// Verify a plaintext password against a stored argon2 hash.
    pub fn verify_password(&self, plaintext: &str, hash: &str) -> bool {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        use argon2::Argon2;
        match PasswordHash::new(hash) {
            Ok(parsed) => Argon2::default()
                .verify_password(plaintext.as_bytes(), &parsed)
                .is_ok(),
            Err(_) => false,
        }
    }

    // --- Session tokens ---------------------------------------------------

    /// Mint a signed session JWT for a user id.
    pub fn issue_token(&self, user_id: Uuid) -> Result<String, Error> {
        use jsonwebtoken::{encode, EncodingKey, Header};
        let exp = chrono::Utc::now().timestamp() + self.session_ttl_secs;
        let claims = Claims {
            sub: user_id.to_string(),
            exp,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|_| Error::Token)
    }

    /// Verify a session JWT and recover the user id it was issued for.
    pub fn verify_token(&self, token: &str) -> Result<Uuid, Error> {
        use jsonwebtoken::{decode, DecodingKey, Validation};
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(&self.secret),
            &Validation::default(),
        )
        .map_err(|_| Error::Token)?;
        Uuid::parse_str(&data.claims.sub).map_err(|_| Error::Token)
    }

    // --- API keys ---------------------------------------------------------

    /// Generate a new API key: `(plaintext, sha256_hex)`. The plaintext is shown
    /// to the user once; only the hash is stored.
    pub fn generate_api_key(&self) -> (String, String) {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let plaintext = format!("apik_{}", hex::encode(bytes));
        let hash = Self::hash_api_key(&plaintext);
        (plaintext, hash)
    }

    /// Deterministic hash used to look an API key up. SHA-256 (not argon2) so a
    /// single indexed equality lookup resolves the key.
    pub fn hash_api_key(plaintext: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(plaintext.as_bytes());
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let auth = Authenticator::new(b"secret".to_vec(), 3600);
        let hash = auth.hash_password("hunter2").unwrap();
        assert!(auth.verify_password("hunter2", &hash));
        assert!(!auth.verify_password("wrong", &hash));
    }

    #[test]
    fn token_roundtrip() {
        let auth = Authenticator::new(b"secret".to_vec(), 3600);
        let id = Uuid::new_v4();
        let token = auth.issue_token(id).unwrap();
        assert_eq!(auth.verify_token(&token).unwrap(), id);
    }

    #[test]
    fn api_key_hash_is_deterministic() {
        assert_eq!(
            Authenticator::hash_api_key("apik_abc"),
            Authenticator::hash_api_key("apik_abc")
        );
    }

    #[test]
    fn membership_lookup() {
        let org = Uuid::new_v4();
        let p = Principal {
            user_id: Uuid::new_v4(),
            organizations: vec![OrgMembership::new(org, Some("support".into()), [])],
        };
        assert!(p.is_member(org));
        assert_eq!(p.role_in(org), Some("support"));
        assert!(!p.is_member(Uuid::new_v4()));
        assert_eq!(p.org_ids_with_role("support"), vec![org]);
        assert!(p.org_ids_with_role("billing").is_empty());
    }

    #[test]
    fn a_member_holds_every_role_granted_to_them() {
        let org = Uuid::new_v4();
        let p = Principal {
            user_id: Uuid::new_v4(),
            organizations: vec![OrgMembership::new(
                org,
                Some("support".into()),
                ["billing".to_string()],
            )],
        };

        // The primary role is one of the set, not a separate kind of thing.
        assert_eq!(p.roles_in(org), ["support", "billing"]);
        assert!(p.has_role_in(org, "support"));
        assert!(p.has_role_in(org, "billing"));
        assert!(!p.has_role_in(org, "admin"));

        // Holding a role is not the same as it being your headline one, which
        // is what a hook context reports.
        assert_eq!(p.role_in(org), Some("support"));
    }

    #[test]
    fn an_admin_holds_every_role_without_being_granted_them() {
        let org = Uuid::new_v4();
        let other = Uuid::new_v4();
        let p = Principal {
            user_id: Uuid::new_v4(),
            organizations: vec![
                OrgMembership::new(org, Some("admin".into()), []),
                OrgMembership::new(other, Some("support".into()), []),
            ],
        };

        assert!(p.is_admin_of(org));
        assert!(p.has_role_in(org, "billing"));
        assert!(p.has_role_in(org, "anything-at-all"));
        assert_eq!(p.org_ids_with_role("billing"), vec![org]);

        // …and only in the organisation where they are the admin.
        assert!(!p.has_role_in(other, "billing"));
        assert!(!p.is_admin_of(other));

        // The rule is about checks, never about stored data: an admin's roles
        // are still just the ones they were given, so taking one away is a
        // change that means something.
        assert_eq!(p.roles_in(org), ["admin"]);
    }

    #[test]
    fn a_role_held_twice_is_still_one_role() {
        let org = Uuid::new_v4();
        // The primary role appearing again among the extras is a duplicate, not
        // a second grant — otherwise removing one would appear to do nothing.
        let membership = OrgMembership::new(
            org,
            Some("admin".into()),
            ["admin".to_string(), "billing".to_string(), String::new()],
        );
        assert_eq!(membership.roles, ["admin", "billing"]);
    }

    #[test]
    fn a_member_with_no_role_holds_none() {
        let org = Uuid::new_v4();
        let membership = OrgMembership::new(org, None, []);
        assert!(membership.roles.is_empty());
        assert!(!membership.is_admin());
        assert!(!membership.has_role("member"));
    }
}
