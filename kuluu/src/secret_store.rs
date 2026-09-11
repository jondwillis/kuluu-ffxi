use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use keyring::Entry;

pub const FALLBACK_FILE: &str = "secrets.json";

// Steam Deck Game Mode (and any session without an org.freedesktop.secrets
// provider) has no keyring daemon, so the platform store is best-effort and a
// mode-0600 file under the config dir catches what it rejects.
pub struct SecretStore;

impl SecretStore {
    pub fn get(service: &str, account: &str) -> Option<String> {
        platform_get(service, account)
            .or_else(|| FileSecrets::default_path().and_then(|f| f.get(service, account)))
    }

    pub fn set(service: &str, account: &str, password: &str) -> bool {
        let Some(file) = FileSecrets::default_path() else {
            return platform_set(service, account, password);
        };
        if platform_set(service, account, password) {
            file.delete(service, account);
            return true;
        }
        tracing::info!(
            service,
            account,
            "keyring: unavailable, saving to fallback file"
        );
        file.set(service, account, password)
    }

    pub fn delete(service: &str, account: &str) -> bool {
        let platform_ok = platform_delete(service, account);
        let file_ok = FileSecrets::default_path().is_none_or(|f| f.delete(service, account));
        platform_ok && file_ok
    }
}

fn platform_get(service: &str, account: &str) -> Option<String> {
    match Entry::new(service, account) {
        Ok(entry) => match entry.get_password() {
            Ok(pw) => Some(pw),
            Err(keyring::Error::NoEntry) => None,
            Err(e) => {
                tracing::warn!(service, account, error = %e, "keyring: get failed");
                None
            }
        },
        Err(e) => {
            tracing::warn!(service, account, error = %e, "keyring: open failed");
            None
        }
    }
}

fn platform_set(service: &str, account: &str, password: &str) -> bool {
    match Entry::new(service, account) {
        Ok(entry) => match entry.set_password(password) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(service, account, error = %e, "keyring: set failed");
                false
            }
        },
        Err(e) => {
            tracing::warn!(service, account, error = %e, "keyring: open failed");
            false
        }
    }
}

fn platform_delete(service: &str, account: &str) -> bool {
    match Entry::new(service, account) {
        Ok(entry) => match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => true,
            Err(e) => {
                tracing::warn!(service, account, error = %e, "keyring: delete failed");
                false
            }
        },
        Err(e) => {
            tracing::warn!(service, account, error = %e, "keyring: open failed");
            false
        }
    }
}

type SecretMap = BTreeMap<String, BTreeMap<String, String>>;

pub struct FileSecrets {
    path: PathBuf,
}

impl FileSecrets {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn default_path() -> Option<Self> {
        kuluu_session::config_dir::config_file(FALLBACK_FILE)
            .ok()
            .map(Self::new)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, service: &str, account: &str) -> Option<String> {
        self.read()?.get(service)?.get(account).cloned()
    }

    pub fn set(&self, service: &str, account: &str, password: &str) -> bool {
        let mut map = self.read().unwrap_or_default();
        map.entry(service.to_string())
            .or_default()
            .insert(account.to_string(), password.to_string());
        self.write(&map)
    }

    pub fn delete(&self, service: &str, account: &str) -> bool {
        let Some(mut map) = self.read() else {
            return true;
        };
        let removed = map
            .get_mut(service)
            .is_some_and(|accounts| accounts.remove(account).is_some());
        if !removed {
            return true;
        }
        map.retain(|_, accounts| !accounts.is_empty());
        self.write(&map)
    }

    fn read(&self) -> Option<SecretMap> {
        match std::fs::read(&self.path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(map) => Some(map),
                Err(e) => {
                    tracing::warn!(path = %self.path.display(), error = %e, "secrets file: parse failed");
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!(path = %self.path.display(), error = %e, "secrets file: read failed");
                None
            }
        }
    }

    fn write(&self, map: &SecretMap) -> bool {
        match self.try_write(map) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(path = %self.path.display(), error = %e, "secrets file: write failed");
                false
            }
        }
    }

    fn try_write(&self, map: &SecretMap) -> std::io::Result<()> {
        use std::io::Write;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(map)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self.path.with_extension("json.tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(OWNER_ONLY_MODE);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(unix)]
const OWNER_ONLY_MODE: u32 = 0o600;

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_store(tag: &str) -> (PathBuf, FileSecrets) {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kuluu-secrets-{tag}-{n}"));
        let store = FileSecrets::new(dir.join("kuluu").join(FALLBACK_FILE));
        (dir, store)
    }

    #[test]
    fn roundtrips_and_deletes() {
        let (dir, store) = unique_store("roundtrip");
        assert_eq!(store.get("kuluu", "local:a"), None);
        assert!(store.set("kuluu", "local:a", "hunter2"));
        assert!(store.set("kuluu", "local:b", "swordfish"));
        assert_eq!(store.get("kuluu", "local:a").as_deref(), Some("hunter2"));
        assert_eq!(store.get("kuluu", "local:b").as_deref(), Some("swordfish"));

        assert!(store.delete("kuluu", "local:a"));
        assert_eq!(store.get("kuluu", "local:a"), None);
        assert_eq!(store.get("kuluu", "local:b").as_deref(), Some("swordfish"));

        assert!(store.delete("kuluu", "local:b"));
        let raw = std::fs::read_to_string(store.path()).unwrap();
        assert_eq!(raw.trim(), "{}", "emptied services are pruned");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_of_missing_entry_is_a_noop_success() {
        let (dir, store) = unique_store("noop");
        assert!(store.delete("kuluu", "nobody"));
        assert!(!store.path().exists(), "delete never creates the file");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, store) = unique_store("mode");
        assert!(store.set("kuluu", "local:a", "hunter2"));
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, OWNER_ONLY_MODE);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_path_is_beside_launcher_json() {
        let p = FileSecrets::default_path().expect("config dir resolves");
        assert!(
            p.path().ends_with("kuluu/secrets.json"),
            "got {}",
            p.path().display()
        );
    }
}
