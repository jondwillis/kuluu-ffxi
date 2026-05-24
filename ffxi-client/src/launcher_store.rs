//! Persistent launcher state — server profiles, saved accounts, and the
//! last-used (server, username) pair. Mirrors [`crate::graphics_store`]:
//! same XDG resolution, same atomic tmpfile + rename, same best-effort
//! load-with-warn-on-corrupt semantics.
//!
//! Passwords NEVER live in this file. When a [`SavedAccount`] has
//! `remember_password = true`, the password is stored in the OS keyring
//! under service [`KEYRING_SERVICE`] and account key
//! [`keyring_account_key`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Keyring service name used for every cached launcher password. The
/// per-account key is built by [`keyring_account_key`].
pub const KEYRING_SERVICE: &str = "ffxi-mcp";

/// Mirror of `auth_client::AuthFlavor` for on-disk serialization. Kept
/// separate so the on-disk schema doesn't drift if `AuthFlavor` ever
/// gains transport-only variants that have no meaning at rest.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthFlavorKind {
    Json,
    Binary,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ServerProfile {
    pub name: String,
    pub host: String,
    pub auth_port: u16,
    pub data_port: u16,
    pub view_port: u16,
    pub flavor: AuthFlavorKind,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedAccount {
    pub server_name: String,
    pub username: String,
    pub remember_password: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LauncherStore {
    #[serde(default)]
    pub servers: Vec<ServerProfile>,
    #[serde(default)]
    pub accounts: Vec<SavedAccount>,
    #[serde(default)]
    pub last_used: Option<(String, String)>,
}

/// Canonical keyring account key for `(server, user)`. Centralized so
/// the save path and the auto-fill path can never disagree on the
/// lookup string.
pub fn keyring_account_key(server_name: &str, username: &str) -> String {
    format!("{server_name}:{username}")
}

/// `$XDG_CONFIG_HOME/ffxi-mcp/launcher.json` or `$HOME/.config/...`.
fn default_path() -> Option<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("ffxi-mcp").join("launcher.json"))
}

/// Best-effort load. Missing file → defaults. Corrupt JSON → warn and
/// return defaults; startup must never block on a bad config.
pub fn load() -> LauncherStore {
    let Some(path) = default_path() else {
        tracing::warn!("launcher_store: no XDG_CONFIG_HOME / HOME; using empty defaults");
        return LauncherStore::default();
    };
    match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice::<LauncherStore>(&bytes) {
            Ok(store) => store,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "launcher_store: parse failed; using empty defaults",
                );
                LauncherStore::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LauncherStore::default(),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "launcher_store: read failed; using empty defaults",
            );
            LauncherStore::default()
        }
    }
}

/// Atomic save via tmpfile + rename. Creates parent dirs on demand.
pub fn save(store: &LauncherStore) -> std::io::Result<()> {
    let path = default_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "neither $XDG_CONFIG_HOME nor $HOME set",
        )
    })?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_key_format() {
        assert_eq!(keyring_account_key("local", "test1"), "local:test1");
    }

    #[test]
    fn flavor_serializes_lowercase() {
        let j = serde_json::to_string(&AuthFlavorKind::Json).unwrap();
        assert_eq!(j, "\"json\"");
        let b = serde_json::to_string(&AuthFlavorKind::Binary).unwrap();
        assert_eq!(b, "\"binary\"");
    }

    #[test]
    fn default_store_roundtrips() {
        let s = LauncherStore::default();
        let bytes = serde_json::to_vec(&s).unwrap();
        let back: LauncherStore = serde_json::from_slice(&bytes).unwrap();
        assert!(back.servers.is_empty());
        assert!(back.accounts.is_empty());
        assert!(back.last_used.is_none());
    }
}
