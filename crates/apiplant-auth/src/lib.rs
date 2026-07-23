//! # apiplant-auth
//!
//! Authentication and authorization primitives, deliberately independent of the
//! HTTP layer so they can be unit-tested in isolation.
//!
//! * [`Authenticator`] — password hashing (argon2), JWT session tokens, and
//!   API-key generation/hashing.
//! * [`Principal`] — the resolved caller identity a request carries.
//! * [`evaluate`] — turns a resource's [`Access`] policy plus the caller into an
//!   allow / allow-if-owner / deny [`Decision`].
//!
//! API keys authenticate *as* their owning user: the server looks the key's
//! SHA-256 up in the `api_key` resource and adopts the owner's [`Principal`].

use apiplant_core::Access;
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

/// The authenticated caller behind a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub user_id: Uuid,
    /// Role name, when the user has one. Drives [`Access::Role`] checks.
    pub role: Option<String>,
}

/// Result of a permission check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Allowed unconditionally.
    Allow,
    /// Allowed, but the query must be scoped to rows the caller owns.
    AllowOwned,
    /// Rejected.
    Deny,
}

/// Evaluate an [`Access`] policy for a (possibly anonymous) caller.
pub fn evaluate(access: &Access, principal: Option<&Principal>) -> Decision {
    match access {
        Access::Public => Decision::Allow,
        Access::Private => Decision::Deny,
        Access::Authenticated => {
            if principal.is_some() {
                Decision::Allow
            } else {
                Decision::Deny
            }
        }
        Access::Owner => {
            if principal.is_some() {
                Decision::AllowOwned
            } else {
                Decision::Deny
            }
        }
        Access::Role(required) => match principal.and_then(|p| p.role.as_deref()) {
            Some(role) if role == required => Decision::Allow,
            _ => Decision::Deny,
        },
    }
}

/// JWT claims for a session token.
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    /// Subject: the user id.
    sub: String,
    /// Role, if any.
    role: Option<String>,
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

    /// Mint a signed session JWT for a principal.
    pub fn issue_token(&self, principal: &Principal) -> Result<String, Error> {
        use jsonwebtoken::{encode, EncodingKey, Header};
        let exp = chrono::Utc::now().timestamp() + self.session_ttl_secs;
        let claims = Claims {
            sub: principal.user_id.to_string(),
            role: principal.role.clone(),
            exp,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
        .map_err(|_| Error::Token)
    }

    /// Verify a session JWT and recover its principal.
    pub fn verify_token(&self, token: &str) -> Result<Principal, Error> {
        use jsonwebtoken::{decode, DecodingKey, Validation};
        let data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(&self.secret),
            &Validation::default(),
        )
        .map_err(|_| Error::Token)?;
        let user_id = Uuid::parse_str(&data.claims.sub).map_err(|_| Error::Token)?;
        Ok(Principal {
            user_id,
            role: data.claims.role,
        })
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
        let p = Principal {
            user_id: Uuid::new_v4(),
            role: Some("admin".into()),
        };
        let token = auth.issue_token(&p).unwrap();
        assert_eq!(auth.verify_token(&token).unwrap(), p);
    }

    #[test]
    fn api_key_hash_is_deterministic() {
        assert_eq!(
            Authenticator::hash_api_key("apik_abc"),
            Authenticator::hash_api_key("apik_abc")
        );
    }

    #[test]
    fn permission_matrix() {
        let admin = Principal {
            user_id: Uuid::new_v4(),
            role: Some("admin".into()),
        };
        assert_eq!(evaluate(&Access::Public, None), Decision::Allow);
        assert_eq!(evaluate(&Access::Authenticated, None), Decision::Deny);
        assert_eq!(evaluate(&Access::Owner, Some(&admin)), Decision::AllowOwned);
        assert_eq!(
            evaluate(&Access::Role("admin".into()), Some(&admin)),
            Decision::Allow
        );
        assert_eq!(
            evaluate(&Access::Role("root".into()), Some(&admin)),
            Decision::Deny
        );
    }
}
