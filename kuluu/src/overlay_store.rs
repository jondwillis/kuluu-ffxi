//! Persisted DAT overlay search path.
//!
//! Private servers ship client-side DAT changes as an overlay directory rather
//! than by editing the retail install, so [`ffxi_dat::discover_overlays`] already
//! picks up the server's own XI-Pivot config with no configuration from us. This
//! store exists for the cases discovery cannot cover: a layout Pivot does not
//! describe, an overlay the player wants disabled, or a different order.
//!
//! An absent file means "use discovery"; a present one is authoritative,
//! including when it is empty — that is how a player turns the server's overlays
//! off.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};

pub const OVERLAY_FILE_VERSION: u32 = 1;

/// `deny_unknown_fields` makes an unrecognized shape a hard parse error rather
/// than an empty list that the next save would persist over.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OverlayFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub overlays: Vec<PathBuf>,
}

#[derive(Resource, Debug, Clone)]
pub struct OverlayStoreRes {
    pub store: OverlayStore,
}

#[derive(Debug, Clone)]
pub struct OverlayStore {
    path: PathBuf,
}

impl OverlayStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> Result<PathBuf> {
        kuluu_session::config_dir::config_file("dat_overlays.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `None` when no file exists, which means discovery decides.
    pub fn load(&self) -> Result<Option<Vec<PathBuf>>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let file: OverlayFile = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                Ok(Some(file.overlays))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    pub fn save(&self, overlays: &[PathBuf]) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let file = OverlayFile {
            version: OVERLAY_FILE_VERSION,
            overlays: overlays.to_vec(),
        };
        let bytes = serde_json::to_vec_pretty(&file).context("serialize overlay list")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }

    /// Drop the override so discovery decides again.
    pub fn clear(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("remove {}", self.path.display())),
        }
    }
}

/// The store at the default config path; `None` when no config dir resolves, in
/// which case discovery is the only source and `//overlay` cannot persist.
pub fn default_store() -> Option<OverlayStore> {
    match OverlayStore::default_path() {
        Ok(p) => Some(OverlayStore::new(p)),
        Err(e) => {
            tracing::warn!(error = %e, "dat: no config dir; overlay override unavailable");
            None
        }
    }
}

/// Seed a freshly opened root from the saved override. Must be called at BOTH
/// DatRoot construction points — startup and the settings reload — or the
/// override silently applies on one path and not the other.
pub fn apply_saved(root: &ffxi_dat::DatRoot) {
    if let Some(store) = default_store() {
        apply_saved_overlays(root, &store);
    }
}

/// Apply the persisted override to a freshly opened root, if there is one.
/// `DatRoot::open` has already seeded discovery, so doing nothing is correct
/// when no file exists.
pub fn apply_saved_overlays(root: &ffxi_dat::DatRoot, store: &OverlayStore) {
    match store.load() {
        Ok(Some(overlays)) => {
            tracing::info!(count = overlays.len(), "dat: applying saved overlay list");
            root.set_overlays(overlays);
        }
        Ok(None) => {}
        // A malformed file must not silently fall back to discovery, or the
        // player's disable stays disabled-looking while the overlays are live.
        Err(e) => tracing::warn!(error = %e, "dat: overlay override unreadable; using discovery"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, OverlayStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = OverlayStore::new(dir.path().join("dat_overlays.json"));
        (dir, s)
    }

    #[test]
    fn absent_file_defers_to_discovery() {
        let (_d, s) = store();
        assert_eq!(s.load().unwrap(), None);
    }

    #[test]
    fn saved_list_round_trips_in_order() {
        let (_d, s) = store();
        let list = vec![PathBuf::from("/a/one"), PathBuf::from("/b/two")];
        s.save(&list).unwrap();
        assert_eq!(s.load().unwrap(), Some(list));
    }

    // An empty list is how a player turns the server's overlays off, so it must
    // survive the round trip as Some(empty), not collapse to None.
    #[test]
    fn an_empty_saved_list_is_not_absent() {
        let (_d, s) = store();
        s.save(&[]).unwrap();
        assert_eq!(s.load().unwrap(), Some(Vec::new()));
    }

    #[test]
    fn clear_restores_discovery() {
        let (_d, s) = store();
        s.save(&[PathBuf::from("/a")]).unwrap();
        s.clear().unwrap();
        assert_eq!(s.load().unwrap(), None);
        s.clear().expect("clearing an absent file is not an error");
    }

    #[test]
    fn a_malformed_file_is_an_error_not_an_empty_list() {
        let (_d, s) = store();
        std::fs::write(s.path(), b"{\"overlays\": \"not-a-list\"}").unwrap();
        assert!(s.load().is_err());
    }
}
