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

/// A user's membership in one organisation, with their role there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgMembership {
    pub org_id: Uuid,
    /// Role within this organisation (e.g. `"admin"`), if any.
    pub role: Option<String>,
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

    /// The caller's role within `org`, if any.
    pub fn role_in(&self, org: Uuid) -> Option<&str> {
        self.membership(org).and_then(|m| m.role.as_deref())
    }

    /// Every organisation id the caller belongs to.
    pub fn org_ids(&self) -> Vec<Uuid> {
        self.organizations.iter().map(|m| m.org_id).collect()
    }

    /// Organisations where the caller holds a specific role.
    pub fn org_ids_with_role(&self, role: &str) -> Vec<Uuid> {
        self.organizations
            .iter()
            .filter(|m| m.role.as_deref() == Some(role))
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
            organizations: vec![OrgMembership {
                org_id: org,
                role: Some("admin".into()),
            }],
        };
        assert!(p.is_member(org));
        assert_eq!(p.role_in(org), Some("admin"));
        assert!(!p.is_member(Uuid::new_v4()));
        assert_eq!(p.org_ids_with_role("admin"), vec![org]);
        assert!(p.org_ids_with_role("member").is_empty());
    }
}
