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

/// Keyring service name used for every cached launcher password — this
/// is the label macOS surfaces in its Keychain-access prompt, so it
/// names the *app*, not an internal codename. (Was `"ffxi-mcp"`, a
/// historical leak from the MCP-only era; the on-disk config dir keeps
/// that name but the user-facing secret does not.) The per-account key
/// is built by [`keyring_account_key`].
pub const KEYRING_SERVICE: &str = "ffxi-client";

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
    /// Per-server xiloader version override (e.g. `"2.0.0"`). `None`
    /// inherits the global precedence chain handled by
    /// `auth_client::resolve_client_version` (CLI flag → env →
    /// hardcoded default). `#[serde(default)]` so launcher.json files
    /// written before this field existed still load.
    #[serde(default)]
    pub xiloader_version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedAccount {
    pub server_name: String,
    pub username: String,
    pub remember_password: bool,
}

/// One GUI-configurable value that composes with an environment variable.
///
/// Semantics (see [`EnvOverride::resolved`]):
///   * `override_env == false` (default): the live env var wins; `value`
///     only fills the gap when the env var is unset. This is the "compose"
///     case — a real `FFXI_DAT_PATH=…` in the environment is never
///     clobbered by a stale GUI entry.
///   * `override_env == true`: `value` takes precedence over the env var.
///
/// Empty `value` contributes nothing in either case.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvOverride {
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub override_env: bool,
}

impl EnvOverride {
    /// What this setting should set `var` to in the process environment, or
    /// `None` to leave the existing value untouched. Pure except for the
    /// read of the current env (no mutation here).
    pub fn resolved(&self, var: &str) -> Option<String> {
        let v = self.value.trim();
        if v.is_empty() {
            return None;
        }
        if self.override_env || std::env::var_os(var).is_none() {
            Some(v.to_string())
        } else {
            None
        }
    }
}

/// Global, non-server-specific launcher settings that mirror a handful of
/// runtime env vars so they can be set from the GUI instead of the shell.
/// Each field maps to exactly one env var via [`Settings::apply_to_env`].
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// `FFXI_DAT_PATH` — retail install root (the dir containing `VTABLE.DAT`).
    #[serde(default)]
    pub dat_path: EnvOverride,
    /// `FFXI_NAVMESH_DIR` — Detour `.nav` directory override.
    #[serde(default)]
    pub navmesh_dir: EnvOverride,
    /// `FFXI_MAC` — MAC override for binary / HorizonXI auth.
    #[serde(default)]
    pub mac: EnvOverride,
}

impl Settings {
    /// The (env var, field) pairs, in one place so `apply_to_env` and any
    /// future UI iteration can't disagree on the mapping.
    pub fn entries(&self) -> [(&'static str, &EnvOverride); 3] {
        [
            ("FFXI_DAT_PATH", &self.dat_path),
            ("FFXI_NAVMESH_DIR", &self.navmesh_dir),
            ("FFXI_MAC", &self.mac),
        ]
    }

    /// Apply the stored overrides to the process environment. MUST be called
    /// once at startup, before any `DatRoot::from_env_or_default` / other
    /// env-reading resolution and before worker threads spawn (env mutation
    /// is process-global). Idempotent.
    pub fn apply_to_env(&self) {
        for (var, ov) in self.entries() {
            if let Some(v) = ov.resolved(var) {
                // Safe on edition 2021; called single-threaded at startup.
                std::env::set_var(var, v);
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct LauncherStore {
    #[serde(default)]
    pub servers: Vec<ServerProfile>,
    #[serde(default)]
    pub accounts: Vec<SavedAccount>,
    #[serde(default)]
    pub last_used: Option<(String, String)>,
    #[serde(default)]
    pub settings: Settings,
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
        assert_eq!(back.settings, Settings::default());
    }

    #[test]
    fn env_override_empty_value_contributes_nothing() {
        let ov = EnvOverride {
            value: "   ".into(),
            override_env: true,
        };
        assert_eq!(ov.resolved("FFXI_DAT_PATH"), None);
    }

    #[test]
    fn env_override_true_wins_without_reading_env() {
        let ov = EnvOverride {
            value: "/games/ffxi".into(),
            override_env: true,
        };
        // override_env short-circuits before the env read, so this is
        // independent of whatever FFXI_DAT_PATH happens to be.
        assert_eq!(ov.resolved("FFXI_DAT_PATH"), Some("/games/ffxi".into()));
    }

    #[test]
    fn env_override_compose_only_fills_a_gap() {
        // A var name unique to this test to avoid clobbering real config or
        // racing sibling tests.
        let var = "FFXI_TEST_COMPOSE_8731";
        let ov = EnvOverride {
            value: "/gui/path".into(),
            override_env: false,
        };
        std::env::remove_var(var);
        assert_eq!(
            ov.resolved(var),
            Some("/gui/path".into()),
            "fills when unset"
        );
        std::env::set_var(var, "/env/path");
        assert_eq!(ov.resolved(var), None, "env wins when set");
        std::env::remove_var(var);
    }
}
