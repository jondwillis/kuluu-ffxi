use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const KEYRING_SERVICE: &str = "kuluu";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthFlavorKind {
    Json,
    Binary,
    /// No auth server: Kuluu signs in to the PlayOnline account itself
    /// (kuluu_session::pol_inhouse) and opens the lobby with the session
    /// Square Enix issues.
    PlayOnline,
}

impl AuthFlavorKind {
    pub fn uses_auth_server(self) -> bool {
        !matches!(self, AuthFlavorKind::PlayOnline)
    }

    pub fn label(self) -> &'static str {
        match self {
            AuthFlavorKind::Json => "JSON",
            AuthFlavorKind::Binary => "Binary",
            AuthFlavorKind::PlayOnline => "PlayOnline",
        }
    }
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

    /// The patch stamp the server's lobby admits (login.CLIENT_VER); unset
    /// means the vendored LSB pin, ffxi_proto::login::LSB_CLIENT_VER.
    #[serde(default)]
    pub client_ver: Option<String>,

    /// login.VER_LOCK as ffxi_proto::login::VerLock::from_setting reads it;
    /// unset means ffxi_proto::login::LSB_DEFAULT_VER_LOCK.
    #[serde(default)]
    pub ver_lock: Option<u8>,

    /// An ffxi_client::Install name this server should be played from.
    #[serde(default)]
    pub preferred_client: Option<String>,

    /// The player has read this profile's third-party-client terms notice.
    #[serde(default)]
    pub terms_acknowledged: bool,
}

pub const HORIZONXI_HOST: &str = "play.horizonxi.com";
/// HorizonXI's launcher ships a 2.0.0 xiloader; the JSON auth flavor is what
/// its server answers.
pub const HORIZONXI_XILOADER_VERSION: &str = "2.0.0";
const HORIZONXI_KNOWN_CLIENT: &str = "horizonxi-2023";
pub const LOCALHOST: &str = "127.0.0.1";

pub struct ServerTemplate {
    pub label: &'static str,
    pub profile: ServerProfile,
}

pub fn server_templates() -> Vec<ServerTemplate> {
    let horizonxi_patch = ffxi_dat::client_profile::KNOWN_CLIENTS
        .iter()
        .find(|k| k.name == HORIZONXI_KNOWN_CLIENT)
        .and_then(|k| k.patch_version)
        .map(str::to_string);
    vec![
        ServerTemplate {
            label: "HorizonXI",
            profile: ServerProfile {
                xiloader_version: Some(HORIZONXI_XILOADER_VERSION.to_string()),
                client_ver: horizonxi_patch,
                ..ServerProfile::lsb_defaults("HorizonXI", HORIZONXI_HOST)
            },
        },
        ServerTemplate {
            label: "Local LandSandBoat",
            profile: ServerProfile::lsb_defaults("local", LOCALHOST),
        },
        ServerTemplate {
            label: "PlayOnline",
            profile: ServerProfile::playonline_defaults("PlayOnline", ffxi_pol::hosts::LOBBY_HOST),
        },
    ]
}

impl ServerProfile {
    pub fn lsb_defaults(name: &str, host: &str) -> Self {
        Self {
            name: name.to_string(),
            host: host.to_string(),
            auth_port: ffxi_proto::login::LOGIN_AUTH_PORT,
            data_port: ffxi_proto::login::LOGIN_DATA_PORT,
            view_port: ffxi_proto::login::LOGIN_VIEW_PORT,
            flavor: AuthFlavorKind::Json,
            xiloader_version: None,
            version_check_url: None,
            client_ver: None,
            ver_lock: None,
            preferred_client: None,
            terms_acknowledged: false,
        }
    }

    /// A lobby reached through a PlayOnline session: `host` is the FFXI lobby
    /// server, on retail's port map, which LSB mirrors; the auth port is
    /// unused because the PlayOnline account services replace the auth server.
    pub fn playonline_defaults(name: &str, host: &str) -> Self {
        Self {
            flavor: AuthFlavorKind::PlayOnline,
            ..Self::lsb_defaults(name, host)
        }
    }

    pub fn is_playonline(&self) -> bool {
        self.flavor == AuthFlavorKind::PlayOnline
    }

    pub fn expected_client_ver(&self) -> &str {
        self.client_ver
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(ffxi_proto::login::LSB_CLIENT_VER)
    }

    pub fn ver_lock(&self) -> ffxi_proto::login::VerLock {
        ffxi_proto::login::VerLock::from_setting(
            self.ver_lock
                .unwrap_or(ffxi_proto::login::LSB_DEFAULT_VER_LOCK),
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SavedAccount {
    pub server_name: String,
    pub username: String,
    pub remember_password: bool,
    /// The Square Enix id this account signs in with, for the flavor that
    /// authenticates two identities. The account is keyed on its PlayOnline
    /// id, which is the one that always exists, so this holds the other.
    /// Empty for every other flavor, for an account with no Square Enix id,
    /// and for a profile saved before the flavor existed.
    #[serde(default)]
    pub square_enix_id: String,
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

/// The secret-store key the Square Enix password lives under. A PlayOnline
/// sign-in holds two passwords: the account key carries the PlayOnline one,
/// which every flavor has an equivalent of, and this carries the other.
pub fn keyring_square_enix_key(server_name: &str, username: &str) -> String {
    format!(
        "{}{SQUARE_ENIX_KEY_SUFFIX}",
        keyring_account_key(server_name, username)
    )
}

const SQUARE_ENIX_KEY_SUFFIX: &str = ":square-enix";

fn default_path() -> Option<PathBuf> {
    kuluu_session::config_dir::config_file("launcher.json").ok()
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
        let p = serde_json::to_string(&AuthFlavorKind::PlayOnline).unwrap();
        assert_eq!(p, "\"playonline\"");
        assert!(AuthFlavorKind::Json.uses_auth_server());
        assert!(!AuthFlavorKind::PlayOnline.uses_auth_server());
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
            square_enix_id: String::new(),
        }
    }

    #[test]
    fn the_two_secret_keys_of_one_account_are_distinct() {
        let account = keyring_account_key("Retail", "XAAA0000");
        let sqex = keyring_square_enix_key("Retail", "XAAA0000");
        assert_ne!(account, sqex);
        assert!(sqex.starts_with(&account));
    }

    #[test]
    fn an_account_saved_before_the_playonline_flavor_still_loads() {
        let json = r#"{"server_name":"Retail","username":"someone","remember_password":true}"#;
        let acct: SavedAccount = serde_json::from_str(json).unwrap();
        assert_eq!(acct.username, "someone");
        assert!(acct.remember_password);
        assert!(acct.square_enix_id.is_empty());
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
        ServerProfile::lsb_defaults(name, host)
    }

    #[test]
    fn templates_carry_the_horizonxi_client_era_and_lsb_ports() {
        let templates = server_templates();
        let hxi = templates
            .iter()
            .find(|t| t.label == "HorizonXI")
            .expect("HorizonXI template");
        assert_eq!(hxi.profile.host, HORIZONXI_HOST);
        assert_eq!(
            hxi.profile.xiloader_version.as_deref(),
            Some(HORIZONXI_XILOADER_VERSION)
        );
        assert_eq!(hxi.profile.client_ver.as_deref(), Some("30230905_0"));
        assert_eq!(hxi.profile.auth_port, ffxi_proto::login::LOGIN_AUTH_PORT);
        let local = templates
            .iter()
            .find(|t| t.label == "Local LandSandBoat")
            .expect("local template");
        assert_eq!(local.profile.host, LOCALHOST);
        assert_eq!(local.profile.client_ver, None);
        assert_eq!(
            local.profile.expected_client_ver(),
            ffxi_proto::login::LSB_CLIENT_VER
        );
        let pol = templates
            .iter()
            .find(|t| t.label == "PlayOnline")
            .expect("PlayOnline template");
        assert!(pol.profile.is_playonline());
        assert!(!pol.profile.flavor.uses_auth_server());
        assert_eq!(pol.profile.host, ffxi_pol::hosts::LOBBY_HOST);
        assert_eq!(pol.profile.view_port, ffxi_proto::login::LOGIN_VIEW_PORT);
        assert_eq!(pol.profile.data_port, ffxi_proto::login::LOGIN_DATA_PORT);
        assert!(!pol.profile.terms_acknowledged);
    }

    #[test]
    fn a_playonline_profile_round_trips_its_terms_flag() {
        let profile = ServerProfile {
            terms_acknowledged: true,
            ..ServerProfile::playonline_defaults("retail", "lobby.example")
        };
        let text = serde_json::to_string(&profile).unwrap();
        assert!(text.contains("\"playonline\""), "{text}");
        let back: ServerProfile = serde_json::from_str(&text).unwrap();
        assert!(back.terms_acknowledged);
        assert_eq!(back.flavor.label(), "PlayOnline");
    }

    #[test]
    fn a_profile_saved_with_the_retired_session_file_field_still_loads() {
        let text = format!(
            r#"{{"name":"retail","host":"lobby.example","auth_port":{},"data_port":{},
            "view_port":{},"flavor":"playonline","pol_session_file":"C:/sessions/pol.json"}}"#,
            ffxi_proto::login::LOGIN_AUTH_PORT,
            ffxi_proto::login::LOGIN_DATA_PORT,
            ffxi_proto::login::LOGIN_VIEW_PORT
        );
        let back: ServerProfile = serde_json::from_str(&text).unwrap();
        assert!(back.is_playonline());
        assert_eq!(back.host, "lobby.example");
    }

    #[test]
    fn profile_without_era_fields_parses_and_falls_back_to_the_lsb_pin() {
        let j = format!(
            r#"{{"name":"local","host":"127.0.0.1","auth_port":{},
            "data_port":{},"view_port":{},"flavor":"json"}}"#,
            ffxi_proto::login::LOGIN_AUTH_PORT,
            ffxi_proto::login::LOGIN_DATA_PORT,
            ffxi_proto::login::LOGIN_VIEW_PORT
        );
        let p: ServerProfile = serde_json::from_str(&j).unwrap();
        assert_eq!(p.client_ver, None);
        assert_eq!(p.ver_lock, None);
        assert_eq!(p.preferred_client, None);
        assert_eq!(p.expected_client_ver(), ffxi_proto::login::LSB_CLIENT_VER);
        assert_eq!(
            p.ver_lock(),
            ffxi_proto::login::VerLock::from_setting(ffxi_proto::login::LSB_DEFAULT_VER_LOCK)
        );
    }

    #[test]
    fn blank_client_ver_counts_as_unset() {
        let mut p = profile("local", "127.0.0.1");
        p.client_ver = Some("   ".into());
        assert_eq!(p.expected_client_ver(), ffxi_proto::login::LSB_CLIENT_VER);
        p.client_ver = Some("30230905_0".into());
        assert_eq!(p.expected_client_ver(), "30230905_0");
        p.ver_lock = Some(1);
        assert_eq!(p.ver_lock(), ffxi_proto::login::VerLock::Exact);
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
