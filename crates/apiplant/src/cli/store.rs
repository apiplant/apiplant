//! Where the console remembers its keys.
//!
//! One small JSON file in the user's config directory, keyed by server origin —
//! not the app directory, because the same checkout is routinely pointed at a
//! local server and a deployed one, and those are different accounts with
//! different keys. Keeping it out of the app directory also keeps a live
//! credential out of anything anyone might commit.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Saved {
    pub api_key: Option<String>,
    /// The organisation last chosen here, so the console comes back to the same
    /// tenant rather than whichever one happens to sort first.
    pub organization: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Store {
    servers: BTreeMap<String, Saved>,
    /// Where this came from. Absent when there is nowhere to write.
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Store {
    pub fn load() -> Store {
        let Some(path) = config_path() else {
            return Store::default();
        };
        let mut store = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Store>(&text).ok())
            // A file we cannot parse is a file from a future version or a
            // half-written one; starting empty asks for a sign-in, which is
            // recoverable, where refusing to start is not.
            .unwrap_or_default();
        store.path = Some(path);
        store
    }

    pub fn server(&self, origin: &str) -> Option<&Saved> {
        self.servers.get(origin)
    }

    /// Record what we know about a server, and write the file.
    ///
    /// Failing to save is not worth interrupting a session over — the console
    /// works fine, it just will not remember — so the error comes back for the
    /// status line rather than stopping anything.
    pub fn remember(&mut self, origin: &str, saved: Saved) -> Result<(), String> {
        self.servers.insert(origin.to_string(), saved);
        self.write()
    }

    pub fn forget(&mut self, origin: &str) -> Result<(), String> {
        self.servers.remove(origin);
        self.write()
    }

    fn write(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Err("there is no config directory to save to".into());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())?;
        restrict(path);
        Ok(())
    }
}

/// The file holds live API keys, so nobody else on the machine should be able
/// to read it.
#[cfg(unix)]
fn restrict(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &std::path::Path) {}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("APIPLANT_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("apiplant").join("cli.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_kept_per_server() {
        let mut store = Store::default();
        store.servers.insert(
            "http://a".into(),
            Saved {
                api_key: Some("one".into()),
                organization: None,
            },
        );
        store.servers.insert(
            "http://b".into(),
            Saved {
                api_key: Some("two".into()),
                organization: Some("org".into()),
            },
        );

        // The same checkout against two servers is two different accounts.
        assert_eq!(
            store.server("http://a").unwrap().api_key.as_deref(),
            Some("one")
        );
        assert_eq!(
            store.server("http://b").unwrap().organization.as_deref(),
            Some("org")
        );
        assert!(store.server("http://c").is_none());
    }

    #[test]
    fn a_store_with_nowhere_to_write_says_so_rather_than_pretending() {
        let mut store = Store::default();
        assert!(store.remember("http://a", Saved::default()).is_err());
        // …and still remembers it for this session.
        assert!(store.server("http://a").is_some());
    }

    #[test]
    fn an_unreadable_file_is_not_a_reason_to_refuse_to_start() {
        let text = "{ this is not json";
        assert!(serde_json::from_str::<Store>(text).is_err());
        let store: Store = serde_json::from_str(text).unwrap_or_default();
        assert!(store.servers.is_empty());
    }
}
