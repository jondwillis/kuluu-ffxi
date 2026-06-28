use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const KEYRING_SERVICE: &str = "ffxi-client";

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

    #[serde(default)]
    pub xiloader_version: Option<String>,

    #[serde(default)]
    pub version_check_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedAccount {
    pub server_name: String,
    pub username: String,
    pub remember_password: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvOverride {
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub override_env: bool,
}

impl EnvOverride {
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

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    #[serde(default)]
    pub dat_path: EnvOverride,

    #[serde(default)]
    pub navmesh_dir: EnvOverride,

    #[serde(default)]
    pub mac: EnvOverride,
}

impl Settings {
    pub fn entries(&self) -> [(&'static str, &EnvOverride); 3] {
        [
            ("FFXI_DAT_PATH", &self.dat_path),
            ("FFXI_NAVMESH_DIR", &self.navmesh_dir),
            ("FFXI_MAC", &self.mac),
        ]
    }

    pub fn apply_to_env(&self) {
        for (var, ov) in self.entries() {
            if let Some(v) = ov.resolved(var) {
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
    pub fn preselect_account_for(&self, server_name: &str) -> Option<&SavedAccount> {
        self.accounts.iter().find(|a| a.server_name == server_name)
    }

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

pub struct LoginPrefill<'a> {
    pub account: &'a SavedAccount,
    pub profile: Option<&'a ServerProfile>,
}

pub fn keyring_account_key(server_name: &str, username: &str) -> String {
    format!("{server_name}:{username}")
}

fn default_path() -> Option<PathBuf> {
    crate::config_dir::config_file("launcher.json").ok()
}

fn parse_store(path: &std::path::Path) -> Option<LauncherStore> {
    match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<LauncherStore>(&bytes) {
            Ok(store) => Some(store),
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "launcher_store: parse failed; using empty defaults",
                );
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "launcher_store: read failed; using empty defaults",
            );
            None
        }
    }
}

fn load_from(path: &std::path::Path) -> LauncherStore {
    parse_store(path).unwrap_or_default()
}

pub fn load() -> LauncherStore {
    let Some(path) = default_path() else {
        tracing::warn!("launcher_store: no config dir; using empty defaults");
        return LauncherStore::default();
    };
    load_from(&path)
}

fn write_store(path: &std::path::Path, store: &LauncherStore) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn save(store: &LauncherStore) -> std::io::Result<()> {
    let path = default_path().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "could not resolve a user config directory",
        )
    })?;
    write_store(&path, store)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_key_format() {
        assert_eq!(keyring_account_key("local", "test1"), "local:test1");
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kuluu-launcher-store-{tag}-{n}"))
    }

    #[test]
    fn config_path_uses_player_facing_dir() {
        let p = default_path().expect("config dir resolves");
        assert!(p.ends_with("kuluu/launcher.json"), "got {}", p.display());
    }

    #[test]
    fn load_from_reads_existing_file() {
        let dir = unique_dir("read");
        let path = dir.join("kuluu").join("launcher.json");

        let store = LauncherStore {
            accounts: vec![acct("HXI", "batti")],
            last_used: Some(("HXI".into(), "batti".into())),
            ..Default::default()
        };
        write_store(&path, &store).unwrap();

        let loaded = load_from(&path);
        assert_eq!(loaded.accounts.len(), 1);
        assert_eq!(loaded.accounts[0].username, "batti");
        assert_eq!(loaded.last_used, Some(("HXI".into(), "batti".into())));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_from_defaults_when_absent() {
        let dir = unique_dir("empty");
        let path = dir.join("kuluu").join("launcher.json");
        let loaded = load_from(&path);
        assert!(loaded.accounts.is_empty());
        assert!(loaded.last_used.is_none());
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
        let store = LauncherStore {
            accounts: vec![acct("other", "x"), acct("local", "solo")],
            ..Default::default()
        };
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
        let store = LauncherStore {
            accounts: vec![acct("local", "b"), acct("local", "a")],
            ..Default::default()
        };
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
            version_check_url: None,
        }
    }

    #[test]
    fn login_prefill_restores_account_and_profile() {
        let store = LauncherStore {
            servers: vec![
                profile("HXI", "play.horizonxi.com"),
                profile("local", "127.0.0.1"),
            ],
            accounts: vec![acct("HXI", "batti"), acct("local", "claude")],
            last_used: Some(("HXI".into(), "batti".into())),
            ..Default::default()
        };

        let p = store
            .login_prefill()
            .expect("last_used account is restorable");
        assert_eq!(p.account.username, "batti");
        assert_eq!(p.account.server_name, "HXI");

        assert_eq!(
            p.profile.map(|p| p.host.as_str()),
            Some("play.horizonxi.com")
        );
    }

    #[test]
    fn login_prefill_none_when_last_used_account_was_forgotten() {
        let store = LauncherStore {
            servers: vec![profile("HXI", "play.horizonxi.com")],
            accounts: vec![acct("HXI", "someone_else")],
            last_used: Some(("HXI".into(), "batti".into())),
            ..Default::default()
        };
        assert!(store.login_prefill().is_none());
    }

    #[test]
    fn login_prefill_none_without_last_used() {
        let store = LauncherStore {
            accounts: vec![acct("HXI", "batti")],
            ..Default::default()
        };
        assert!(store.login_prefill().is_none());
    }

    #[test]
    fn login_prefill_matches_account_without_a_saved_profile() {
        let store = LauncherStore {
            accounts: vec![acct("127.0.0.1", "batti")],
            last_used: Some(("127.0.0.1".into(), "batti".into())),
            ..Default::default()
        };
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

        assert_eq!(ov.resolved("FFXI_DAT_PATH"), Some("/games/ffxi".into()));
    }

    #[test]
    fn env_override_compose_only_fills_a_gap() {
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
