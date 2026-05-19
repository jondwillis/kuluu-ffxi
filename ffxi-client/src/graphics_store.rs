//! Persistent graphics settings — writes the user's quality knobs to
//! disk so a chosen preset (or hand-tuned custom config) survives a
//! restart.
//!
//! Mirror of [`crate::keybinds_store`]: same XDG path scheme, same
//! atomic tmpfile + rename, same `serde_json`. The on-disk shape is
//! intentionally human-editable so an operator can hand-edit
//! `graphics.json` outside the in-game menu.
//!
//! Default location is `$XDG_CONFIG_HOME/ffxi-mcp/graphics.json`
//! (falling back to `$HOME/.config/...`). Tests pass an explicit path.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use bevy::prelude::*;
use ffxi_viewer_core::GraphicsSettings;

/// Bevy resource that bundles the on-disk store with the persisted
/// settings. Mutating [`GraphicsSettings`] from the menu triggers
/// `persist_graphics_on_change` to ask `store` to write back. Holding
/// both as a single resource avoids the failure mode where one is
/// updated and the other isn't.
#[derive(Resource, Debug, Clone)]
pub struct GraphicsStateRes {
    pub store: GraphicsStore,
}

#[derive(Debug, Clone)]
pub struct GraphicsStore {
    path: PathBuf,
}

impl GraphicsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `$XDG_CONFIG_HOME/ffxi-mcp/graphics.json` or `$HOME/.config/...`.
    /// Errors if neither env var is set; callers in those environments
    /// should pass an explicit path.
    pub fn default_path() -> Result<PathBuf> {
        let base = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("HOME")
                    .ok()
                    .map(|h| PathBuf::from(h).join(".config"))
            })
            .ok_or_else(|| anyhow!("neither $XDG_CONFIG_HOME nor $HOME set"))?;
        Ok(base.join("ffxi-mcp").join("graphics.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the persisted settings. `Ok(None)` if the file does not
    /// exist (fresh install); errors only on I/O or parse failures so
    /// a corrupt file is visible to the operator rather than silently
    /// reverting to defaults.
    pub fn load(&self) -> Result<Option<GraphicsSettings>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let settings: GraphicsSettings = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                Ok(Some(settings))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    /// Persist the given settings. Creates parent directories on
    /// demand. Atomic via tmpfile + rename — a partial write never
    /// leaves a half-valid file on disk.
    pub fn save(&self, settings: &GraphicsSettings) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(settings).context("serialize graphics settings")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

/// Best-effort startup load: try the default path; on missing file, use
/// `GraphicsSettings::default()` (High preset). On parse error, log and
/// fall back to default — startup must never block on a bad config file.
///
/// Returns `(settings, store)` so the caller can stash both as
/// resources before adding `ViewerCorePlugin`.
pub fn load_or_default() -> (GraphicsSettings, GraphicsStore) {
    let path = match GraphicsStore::default_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "graphics: no XDG/HOME path; using High preset");
            return (
                GraphicsSettings::default(),
                GraphicsStore::new(std::env::temp_dir().join("ffxi-graphics.json")),
            );
        }
    };
    let store = GraphicsStore::new(path);
    match store.load() {
        Ok(Some(settings)) => (settings, store),
        Ok(None) => (GraphicsSettings::default(), store),
        Err(e) => {
            tracing::warn!(
                path = %store.path().display(),
                error = %e,
                "graphics: parse failed; falling back to High preset",
            );
            (GraphicsSettings::default(), store)
        }
    }
}

/// System: on every change to [`GraphicsSettings`], persist to disk.
/// Best-effort — a disk failure logs a warning but the in-memory
/// update succeeds anyway, so a transient I/O hiccup doesn't lock the
/// operator out of changing settings.
pub fn persist_graphics_on_change(
    settings: Res<GraphicsSettings>,
    state: Res<GraphicsStateRes>,
) {
    if !settings.is_changed() {
        return;
    }
    if let Err(e) = state.store.save(&settings) {
        tracing::warn!(
            path = %state.store.path().display(),
            error = %e,
            "graphics: failed to persist settings",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_core::{GraphicsField, QualityPreset};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_path() -> PathBuf {
        // pid + nanos can collide between parallel tests when the
        // platform clock resolution is coarse; threading in the
        // current thread id makes the path unique even if two tests
        // observe the same nano-timestamp.
        let mut p = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!(
            "ffxi-graphics-store-{}-{:?}-{stamp}.json",
            std::process::id(),
            std::thread::current().id(),
        ));
        p
    }

    #[test]
    fn load_missing_returns_none() {
        let store = GraphicsStore::new(tmp_path());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let store = GraphicsStore::new(tmp_path());
        let mut settings = GraphicsSettings::for_preset(QualityPreset::Low);
        // Tweak one field so the preset flips to Custom — exercises the
        // "user hand-tuned a knob and we must remember every value"
        // path, which is the real failure mode for a serialization bug.
        settings.cycle(GraphicsField::BloomIntensity, 1);
        assert_eq!(settings.preset, QualityPreset::Custom);

        store.save(&settings).unwrap();
        let loaded = store.load().unwrap().expect("present after save");
        assert_eq!(loaded, settings);
        std::fs::remove_file(store.path()).ok();
    }

    #[test]
    fn save_and_load_preset_only() {
        let store = GraphicsStore::new(tmp_path());
        let settings = GraphicsSettings::for_preset(QualityPreset::Ultra);
        store.save(&settings).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.preset, QualityPreset::Ultra);
        std::fs::remove_file(store.path()).ok();
    }
}
