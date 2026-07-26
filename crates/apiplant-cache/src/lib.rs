//! # apiplant-cache
//!
//! An optional Redis cache, for functions to use.
//!
//! Nothing in the framework caches through it. Resources, permissions, hooks
//! and the admin manifest behave identically whether or not a cache is
//! configured, and that is deliberate: a cache in front of CRUD would have to
//! guess when a row changes, and guessing wrong means serving stale data
//! through an API that never said it might. What a cache *is* good for is the
//! work a function does — a third-party response worth memoising, a rate-limit
//! counter, a one-time token with a natural expiry — and that is work only the
//! function author can invalidate correctly.
//!
//! So the cache is exposed, deliberately, only through a function's `Context`:
//!
//! ```toml
//! [cache]
//! url = "redis://127.0.0.1:6379"
//! prefix = "my-app:"
//! ```
//!
//! ```ignore
//! if let Some(hit) = ctx.cache_get("rates:eur")? { return Ok(hit); }
//! let rates = fetch_rates()?;
//! ctx.cache_set("rates:eur", &rates, Some(900))?;
//! ```
//!
//! ## Failing soft
//!
//! A cache that is down is not an outage. Every operation here returns a
//! [`Result`], and the host reports failures to the function rather than to the
//! caller — a function that treats a miss and an error alike keeps working when
//! Redis is restarted, which is the whole point of the data being disposable.

use std::time::Duration;

use apiplant_core::CacheConfig;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// What went wrong talking to the cache.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// The `[cache]` section can't produce a working client.
    #[error("cache configuration: {0}")]
    Config(String),

    /// The request a function made isn't a cache operation.
    #[error("invalid cache request: {0}")]
    Request(String),

    /// Redis was unreachable, or took too long.
    #[error("cache: {0}")]
    Backend(String),
}

/// A connected cache, shared by every worker.
///
/// Cloning is cheap: the underlying [`redis::aio::ConnectionManager`] is
/// reference-counted and reconnects on its own, so a clone shares one
/// connection rather than opening another.
#[derive(Clone)]
pub struct Cache {
    connection: redis::aio::ConnectionManager,
    prefix: String,
    default_ttl_secs: u64,
    timeout: Duration,
}

impl std::fmt::Debug for Cache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The URL can carry a password, so it isn't kept or printed.
        f.debug_struct("Cache")
            .field("prefix", &self.prefix)
            .field("default_ttl_secs", &self.default_ttl_secs)
            .finish()
    }
}

impl Cache {
    /// Connect the cache an app's `[cache]` section describes.
    ///
    /// `Ok(None)` means the app configured none, which is the default and not
    /// an error. `Err` means it asked for one that can't be reached — worth
    /// failing the boot over, because a function written against a cache and
    /// silently given none is a bug that only shows up as a load spike.
    pub async fn connect(config: &CacheConfig) -> Result<Option<Cache>, CacheError> {
        if !config.is_active() {
            return Ok(None);
        }
        let url = config.url.trim();
        let client =
            redis::Client::open(url).map_err(|e| CacheError::Config(format!("{url}: {e}")))?;
        let timeout = Duration::from_secs(config.timeout_secs.max(1));

        let connection = tokio::time::timeout(timeout, redis::aio::ConnectionManager::new(client))
            .await
            .map_err(|_| CacheError::Backend(format!("timed out connecting to {url}")))?
            .map_err(|e| CacheError::Backend(format!("{url}: {e}")))?;

        Ok(Some(Cache {
            connection,
            prefix: config.prefix.clone(),
            default_ttl_secs: config.default_ttl_secs,
            timeout,
        }))
    }

    /// The physical key for a logical one.
    fn key(&self, key: &str) -> Result<String, CacheError> {
        prefixed(&self.prefix, key)
    }

    /// Run one operation, given as the JSON a function sent across the ABI, and
    /// return the JSON reply.
    ///
    /// This is the single entry point the host bridge uses, which is what keeps
    /// the ABI to one `cache` call instead of one per verb — a new operation
    /// costs a variant of [`Op`], not a change to the contract every compiled
    /// function was built against.
    pub async fn execute(&self, request: &str) -> Result<Value, CacheError> {
        let op: Op = serde_json::from_str(request)
            .map_err(|e| CacheError::Request(format!("{e}; expected {}", Op::grammar())))?;
        match op {
            Op::Get { key } => {
                let value = self.get(&key).await?;
                Ok(json!({ "hit": value.is_some(), "value": value }))
            }
            Op::Set { key, value, ttl } => {
                self.set(&key, &value, ttl).await?;
                Ok(json!({ "ok": true }))
            }
            Op::Delete { key } => Ok(json!({ "deleted": self.delete(&key).await? })),
            Op::Exists { key } => Ok(json!({ "exists": self.exists(&key).await? })),
            Op::Incr { key, by, ttl } => {
                let value = self.incr(&key, by, ttl).await?;
                Ok(json!({ "value": value }))
            }
            Op::Ttl { key } => Ok(json!({ "ttl": self.ttl(&key).await? })),
        }
    }

    /// Read a value, or `None` when the key is absent or expired.
    ///
    /// A value that isn't valid JSON (something else wrote it) comes back as a
    /// JSON string rather than as an error — the caller asked for whatever is
    /// there.
    pub async fn get(&self, key: &str) -> Result<Option<Value>, CacheError> {
        let key = self.key(key)?;
        let stored: Option<String> = self.run("get", self.connection.clone().get(&key)).await?;
        Ok(stored.map(|text| serde_json::from_str(&text).unwrap_or(Value::String(text))))
    }

    /// Write a value. `ttl` of `None` uses `[cache] default_ttl_secs`; `0`
    /// means "no expiry" in both places.
    pub async fn set(&self, key: &str, value: &Value, ttl: Option<u64>) -> Result<(), CacheError> {
        let key = self.key(key)?;
        let payload =
            serde_json::to_string(value).map_err(|e| CacheError::Request(e.to_string()))?;
        let ttl = ttl.unwrap_or(self.default_ttl_secs);
        let mut connection = self.connection.clone();
        if ttl == 0 {
            self.run::<()>("set", connection.set(&key, payload)).await
        } else {
            self.run::<()>("setex", connection.set_ex(&key, payload, ttl))
                .await
        }
    }

    /// Remove a key. `true` when it was there.
    pub async fn delete(&self, key: &str) -> Result<bool, CacheError> {
        let key = self.key(key)?;
        let removed: i64 = self.run("del", self.connection.clone().del(&key)).await?;
        Ok(removed > 0)
    }

    /// Whether a key is present and unexpired.
    pub async fn exists(&self, key: &str) -> Result<bool, CacheError> {
        let key = self.key(key)?;
        self.run("exists", self.connection.clone().exists(&key))
            .await
    }

    /// Add to a counter and return its new value, creating it at zero first.
    ///
    /// Atomic on the server, which is what makes it usable for rate limiting
    /// from several workers (or several hosts) at once — a read-modify-write
    /// through [`get`](Self::get) and [`set`](Self::set) would not be.
    pub async fn incr(&self, key: &str, by: i64, ttl: Option<u64>) -> Result<i64, CacheError> {
        let key = self.key(key)?;
        let mut connection = self.connection.clone();
        let value: i64 = self.run("incrby", connection.incr(&key, by)).await?;

        // An expiry is only applied when the counter is new, so a window that
        // started 50 seconds ago doesn't get another full minute on every hit.
        let ttl = ttl.unwrap_or(self.default_ttl_secs);
        if ttl > 0 && value == by {
            self.run::<bool>("expire", connection.expire(&key, ttl as i64))
                .await?;
        }
        Ok(value)
    }

    /// Seconds until a key expires: `None` when it is absent or has no expiry.
    pub async fn ttl(&self, key: &str) -> Result<Option<i64>, CacheError> {
        let key = self.key(key)?;
        let seconds: i64 = self.run("ttl", self.connection.clone().ttl(&key)).await?;
        // Redis answers -2 for "no such key" and -1 for "no expiry"; both are
        // "there is no deadline to report".
        Ok((seconds >= 0).then_some(seconds))
    }

    /// Apply the configured timeout to one command and label its failure.
    ///
    /// Every operation goes through here so that a wedged Redis costs one
    /// request its `timeout_secs` and nothing more — without it, a function
    /// blocking on the cache would hold its worker indefinitely.
    async fn run<T>(
        &self,
        command: &str,
        future: impl std::future::Future<Output = redis::RedisResult<T>>,
    ) -> Result<T, CacheError> {
        match tokio::time::timeout(self.timeout, future).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) => Err(CacheError::Backend(format!("{command}: {e}"))),
            Err(_) => Err(CacheError::Backend(format!(
                "{command}: timed out after {:?}",
                self.timeout
            ))),
        }
    }
}

/// The physical key for a logical one: `prefix` keeps several apps from
/// colliding in one Redis.
///
/// An empty key is refused rather than addressed, because `""` under a prefix
/// is the prefix itself — a real key that a second app would happily overwrite.
fn prefixed(prefix: &str, key: &str) -> Result<String, CacheError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(CacheError::Request("a cache key cannot be empty".into()));
    }
    Ok(format!("{prefix}{key}"))
}

/// One cache operation, as a function sends it over the ABI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Op {
    Get {
        key: String,
    },
    Set {
        key: String,
        value: Value,
        #[serde(default)]
        ttl: Option<u64>,
    },
    #[serde(alias = "del")]
    Delete {
        key: String,
    },
    Exists {
        key: String,
    },
    #[serde(alias = "increment")]
    Incr {
        key: String,
        #[serde(default = "one")]
        by: i64,
        #[serde(default)]
        ttl: Option<u64>,
    },
    Ttl {
        key: String,
    },
}

fn one() -> i64 {
    1
}

impl Op {
    /// The accepted requests, for the error a malformed one produces.
    pub fn grammar() -> &'static str {
        r#"{"op":"get"|"set"|"delete"|"exists"|"incr"|"ttl","key":"…", …}"#
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_is_only_connected_when_one_is_configured() {
        // `connect` is async, but the "nothing configured" answer is decided
        // before any I/O — checked here through the same predicate it uses.
        assert!(!CacheConfig::default().is_active());
        assert!(CacheConfig {
            url: "redis://127.0.0.1:6379".into(),
            ..CacheConfig::default()
        }
        .is_active());
    }

    #[test]
    fn operations_parse_from_the_json_a_function_sends() {
        let get: Op = serde_json::from_str(r#"{"op":"get","key":"k"}"#).unwrap();
        assert!(matches!(get, Op::Get { key } if key == "k"));

        let set: Op =
            serde_json::from_str(r#"{"op":"set","key":"k","value":{"a":1},"ttl":60}"#).unwrap();
        match set {
            Op::Set { key, value, ttl } => {
                assert_eq!(key, "k");
                assert_eq!(value["a"], 1);
                assert_eq!(ttl, Some(60));
            }
            other => panic!("expected a set, got {other:?}"),
        }

        // No `ttl` means "use the configured default", which is not the same as
        // `ttl: 0` ("never expire").
        let no_ttl: Op = serde_json::from_str(r#"{"op":"set","key":"k","value":null}"#).unwrap();
        assert!(matches!(no_ttl, Op::Set { ttl: None, .. }));
        let never: Op =
            serde_json::from_str(r#"{"op":"set","key":"k","value":1,"ttl":0}"#).unwrap();
        assert!(matches!(never, Op::Set { ttl: Some(0), .. }));

        // `incr` defaults to one, and `del` is accepted for `delete`.
        let incr: Op = serde_json::from_str(r#"{"op":"incr","key":"hits"}"#).unwrap();
        assert!(matches!(incr, Op::Incr { by: 1, .. }));
        assert!(matches!(
            serde_json::from_str::<Op>(r#"{"op":"del","key":"k"}"#).unwrap(),
            Op::Delete { .. }
        ));
    }

    #[test]
    fn an_unknown_operation_is_rejected() {
        assert!(serde_json::from_str::<Op>(r#"{"op":"flushall"}"#).is_err());
        assert!(serde_json::from_str::<Op>(r#"{"op":"get"}"#).is_err());
        assert!(serde_json::from_str::<Op>("not json").is_err());
    }

    #[test]
    fn keys_are_prefixed_and_never_empty() {
        assert_eq!(prefixed("app:", "rates").unwrap(), "app:rates");
        assert_eq!(prefixed("", " rates ").unwrap(), "rates");
        // `""` under a prefix *is* the prefix — a real key another app owns.
        assert!(prefixed("app:", "").is_err());
        assert!(prefixed("app:", "   ").is_err());
    }
}
