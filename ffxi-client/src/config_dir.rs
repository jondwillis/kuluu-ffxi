use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

pub const APP_DIR: &str = "kuluu";
pub const LEGACY_APP_DIR: &str = "ffxi-mcp";

pub fn config_base() -> Option<PathBuf> {
    std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })
}

pub fn config_file(name: &str) -> Result<PathBuf> {
    let base = config_base().ok_or_else(|| anyhow!("neither $XDG_CONFIG_HOME nor $HOME set"))?;
    Ok(base.join(APP_DIR).join(name))
}

pub fn legacy_config_file(name: &str) -> Option<PathBuf> {
    Some(config_base()?.join(LEGACY_APP_DIR).join(name))
}

pub fn migrate_legacy_file(new: &Path, legacy: &Path) -> bool {
    if new.exists() || !legacy.exists() {
        return false;
    }
    if let Some(parent) = new.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::warn!(
                to = %new.display(),
                error = %e,
                "config_dir: migration mkdir failed; keeping legacy file",
            );
            return false;
        }
    }
    match std::fs::copy(legacy, new) {
        Ok(_) => {
            tracing::warn!(
                from = %legacy.display(),
                to = %new.display(),
                "config_dir: migrating config from legacy ffxi-mcp dir to kuluu",
            );
            true
        }
        Err(e) => {
            tracing::warn!(
                from = %legacy.display(),
                to = %new.display(),
                error = %e,
                "config_dir: migration copy failed; keeping legacy file",
            );
            false
        }
    }
}

pub fn migrate_then(name: &str) -> Result<PathBuf> {
    let new = config_file(name)?;
    if let Some(legacy) = legacy_config_file(name) {
        migrate_legacy_file(&new, &legacy);
    }
    Ok(new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_dir(tag: &str) -> PathBuf {
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kuluu-config-dir-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn config_file_uses_player_facing_dir() {
        let dir = unique_dir("paths");
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let new = config_file("x").unwrap();
        let legacy = legacy_config_file("x").unwrap();
        std::env::remove_var("XDG_CONFIG_HOME");

        assert!(new.ends_with("kuluu/x"), "got {}", new.display());
        assert!(legacy.ends_with("ffxi-mcp/x"), "got {}", legacy.display());
    }

    #[test]
    fn migrate_copies_when_new_absent_and_legacy_present() {
        let dir = unique_dir("copy");
        let new = dir.join("kuluu").join("graphics.json");
        let legacy = dir.join("ffxi-mcp").join("graphics.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, b"{}").unwrap();

        assert!(migrate_legacy_file(&new, &legacy));
        assert!(new.exists(), "new path written by migration");
        assert!(legacy.exists(), "legacy file preserved");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migrate_is_noop_when_new_already_exists() {
        let dir = unique_dir("exists");
        let new = dir.join("kuluu").join("graphics.json");
        let legacy = dir.join("ffxi-mcp").join("graphics.json");
        std::fs::create_dir_all(new.parent().unwrap()).unwrap();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&new, b"new").unwrap();
        std::fs::write(&legacy, b"legacy").unwrap();

        assert!(!migrate_legacy_file(&new, &legacy));
        assert_eq!(std::fs::read(&new).unwrap(), b"new");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn migrate_is_noop_when_legacy_absent() {
        let dir = unique_dir("absent");
        let new = dir.join("kuluu").join("graphics.json");
        let legacy = dir.join("ffxi-mcp").join("graphics.json");

        assert!(!migrate_legacy_file(&new, &legacy));
        assert!(!new.exists());
    }
}
