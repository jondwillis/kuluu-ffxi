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

impl LauncherStore {
    /// The account to pre-select when `server_name` becomes the active
    /// server (e.g. the user picks it in ServerSelect). Returns the only
    /// saved account when a server has exactly one, otherwise the
    /// most-recently-used one. `None` means the server has no saved
    /// accounts and the user must type fresh credentials.
    ///
    /// Recency is encoded by list position: [`crate::view_native`]'s
    /// post-login `save_on_success` moves the just-used account to the
    /// front of `accounts`, so the first row matching `server_name` is
    /// both "the only one" (single-account case) and "the last one used"
    /// (multi-account case) — one rule covers both.
    pub fn preselect_account_for(&self, server_name: &str) -> Option<&SavedAccount> {
        self.accounts.iter().find(|a| a.server_name == server_name)
    }

    /// What the launcher should pre-fill on the first `OnEnter(Login)`:
    /// the [`SavedAccount`] named by `last_used` plus the matching
    /// [`ServerProfile`] (so the caller can restore the network endpoint,
    /// the window title, and the "Server:" chip — not just the username).
    ///
    /// `None` means "fall through to ServerSelect": either there's no
    /// `last_used` yet, or it names an account that's since been forgotten.
    /// `profile` can be `None` even when the account matches — that's the
    /// legacy raw-host CLI login whose `server_name` was the bare host with
    /// no saved profile; the caller keeps the CLI-default endpoint then.
    pub fn login_prefill(&self) -> Option<LoginPrefill<'_>> {
        let (server, user) = self.last_used.as_ref()?;
        let account = self
            .accounts
            .iter()
            .find(|a| &a.server_name == server && &a.username == user)?;
        let profile = self.servers.iter().find(|p| &p.name == server);
        Some(LoginPrefill { account, profile })
    }
}

/// Result of [`LauncherStore::login_prefill`] — the account to restore and
/// (when one exists) the server profile it was last used against. Borrows
/// the store so no cloning happens in the decision itself.
pub struct LoginPrefill<'a> {
    pub account: &'a SavedAccount,
    pub profile: Option<&'a ServerProfile>,
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

    fn acct(server: &str, user: &str) -> SavedAccount {
        SavedAccount {
            server_name: server.into(),
            username: user.into(),
            remember_password: false,
        }
    }

    #[test]
    fn preselect_single_account_regardless_of_order() {
        let mut store = LauncherStore::default();
        store.accounts = vec![acct("other", "x"), acct("local", "solo")];
        assert_eq!(
            store
                .preselect_account_for("local")
                .map(|a| a.username.as_str()),
            Some("solo"),
            "the sole account on a server is always pre-selected",
        );
    }

    #[test]
    fn preselect_multi_account_takes_most_recent_front() {
        let mut store = LauncherStore::default();
        // save_on_success inserts most-recent at the front, so `b` here
        // stands in for "logged in more recently than a".
        store.accounts = vec![acct("local", "b"), acct("local", "a")];
        assert_eq!(
            store
                .preselect_account_for("local")
                .map(|a| a.username.as_str()),
            Some("b"),
            "with several accounts the front (most-recent) one wins",
        );
    }

    #[test]
    fn preselect_none_when_server_has_no_accounts() {
        let store = LauncherStore::default();
        assert!(store.preselect_account_for("local").is_none());
    }

    fn profile(name: &str, host: &str) -> ServerProfile {
        ServerProfile {
            name: name.into(),
            host: host.into(),
            auth_port: 54231,
            data_port: 54230,
            view_port: 54001,
            flavor: AuthFlavorKind::Json,
            xiloader_version: None,
        }
    }

    #[test]
    fn login_prefill_restores_account_and_profile() {
        let mut store = LauncherStore::default();
        store.servers = vec![
            profile("HXI", "play.horizonxi.com"),
            profile("local", "127.0.0.1"),
        ];
        store.accounts = vec![acct("HXI", "batti"), acct("local", "claude")];
        store.last_used = Some(("HXI".into(), "batti".into()));

        let p = store
            .login_prefill()
            .expect("last_used account is restorable");
        assert_eq!(p.account.username, "batti");
        assert_eq!(p.account.server_name, "HXI");
        // The matched profile carries the host the launcher must point the
        // title / network endpoint at — not the CLI default.
        assert_eq!(
            p.profile.map(|p| p.host.as_str()),
            Some("play.horizonxi.com")
        );
    }

    #[test]
    fn login_prefill_none_when_last_used_account_was_forgotten() {
        let mut store = LauncherStore::default();
        store.servers = vec![profile("HXI", "play.horizonxi.com")];
        // last_used points at an account no longer in `accounts` (forgotten).
        store.accounts = vec![acct("HXI", "someone_else")];
        store.last_used = Some(("HXI".into(), "batti".into()));
        assert!(store.login_prefill().is_none());
    }

    #[test]
    fn login_prefill_none_without_last_used() {
        let mut store = LauncherStore::default();
        store.accounts = vec![acct("HXI", "batti")];
        assert!(store.login_prefill().is_none());
    }

    #[test]
    fn login_prefill_matches_account_without_a_saved_profile() {
        // Legacy raw-host login: account exists, but no ServerProfile is
        // named for it. We still restore the account; profile is None so
        // the caller keeps the CLI-default endpoint.
        let mut store = LauncherStore::default();
        store.accounts = vec![acct("127.0.0.1", "batti")];
        store.last_used = Some(("127.0.0.1".into(), "batti".into()));
        let p = store.login_prefill().expect("account still restorable");
        assert_eq!(p.account.username, "batti");
        assert!(p.profile.is_none());
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
