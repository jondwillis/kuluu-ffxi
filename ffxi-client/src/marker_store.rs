use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::*;
use ffxi_viewer_core::hud::map_screen::{MapMarker, MapMarkers};
use ffxi_viewer_core::snapshot::SceneState;

/// On-disk map markers, keyed by character id then zone id. Loaded into the
/// `MapMarkers` resource when a character logs in; saved whenever the player
/// places or removes a marker. Persistence is per character + zone (retail).
type MarkerFile = HashMap<u32, HashMap<u16, Vec<MapMarker>>>;

#[derive(Resource, Debug, Clone)]
pub struct MarkerStoreRes {
    pub store: MarkerStore,
    /// The character id whose markers are currently in `MapMarkers`, so a login
    /// (or character switch) reloads exactly once.
    pub loaded_for: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct MarkerStore {
    path: PathBuf,
}

impl MarkerStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> Result<PathBuf> {
        crate::config_dir::config_file("markers.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load_all(&self) -> Result<MarkerFile> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(MarkerFile::new()),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    /// Markers for one character, empty if the file or character is absent.
    pub fn load_for(&self, char_id: u32) -> Result<HashMap<u16, Vec<MapMarker>>> {
        Ok(self.load_all()?.remove(&char_id).unwrap_or_default())
    }

    /// Replace one character's section and rewrite the file atomically.
    pub fn save_for(&self, char_id: u32, by_zone: &HashMap<u16, Vec<MapMarker>>) -> Result<()> {
        let mut all = self.load_all().unwrap_or_default();
        all.insert(char_id, by_zone.clone());
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(&all).context("serialize markers")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

pub fn load_or_default() -> MarkerStoreRes {
    let path = MarkerStore::default_path().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "markers: no config dir; using temp file");
        std::env::temp_dir().join("ffxi-markers.json")
    });
    MarkerStoreRes {
        store: MarkerStore::new(path),
        loaded_for: None,
    }
}

/// Load a character's saved markers on login/switch, and persist the in-memory
/// `MapMarkers` whenever the player edits them. The load-before-save ordering
/// (a character change reloads and resets `loaded_for`) keeps a fresh login from
/// overwriting stored markers with an empty set.
pub fn sync_markers(
    scene_state: Res<SceneState>,
    mut markers: ResMut<MapMarkers>,
    mut store: ResMut<MarkerStoreRes>,
) {
    let char_id = scene_state.snapshot.self_char_id;

    if let Some(id) = char_id {
        if store.loaded_for != Some(id) {
            match store.store.load_for(id) {
                Ok(by_zone) => markers.by_zone = by_zone,
                Err(e) => {
                    tracing::warn!(path = %store.store.path().display(), error = %e, "markers: load failed");
                    markers.by_zone.clear();
                }
            }
            store.loaded_for = Some(id);
            // `bypass_change_detection` isn't needed: the very next frame's save
            // branch would re-persist the just-loaded set, which is idempotent.
            return;
        }
    }

    if markers.is_changed() {
        if let Some(id) = store.loaded_for {
            if let Err(e) = store.store.save_for(id, &markers.by_zone) {
                tracing::warn!(path = %store.store.path().display(), error = %e, "markers: save failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_viewer_wire::Vec3;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_path() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "ffxi-markers-{}-{:?}-{stamp}.json",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    fn marker(x: f32, z: f32, label: &str) -> MapMarker {
        MapMarker {
            world: Vec3 { x, y: 0.0, z },
            label: label.to_string(),
        }
    }

    #[test]
    fn default_path_uses_player_facing_dir() {
        let path = MarkerStore::default_path().unwrap();
        assert!(
            path.ends_with("kuluu/markers.json"),
            "got {}",
            path.display()
        );
    }

    #[test]
    fn load_missing_char_is_empty() {
        let store = MarkerStore::new(tmp_path());
        assert!(store.load_for(42).unwrap().is_empty());
    }

    #[test]
    fn save_and_load_roundtrips_per_char_and_zone() {
        let store = MarkerStore::new(tmp_path());
        let mut by_zone = HashMap::new();
        by_zone.insert(
            231u16,
            vec![marker(10.0, 20.0, "Home"), marker(-5.0, 3.0, "NM")],
        );
        store.save_for(7, &by_zone).unwrap();

        // A second character's markers coexist in the same file.
        let mut other = HashMap::new();
        other.insert(100u16, vec![marker(1.0, 1.0, "AH")]);
        store.save_for(9, &other).unwrap();

        let back = store.load_for(7).unwrap();
        assert_eq!(back.get(&231).map(|v| v.len()), Some(2));
        assert_eq!(store.load_for(9).unwrap().get(&100).unwrap()[0].label, "AH");
        std::fs::remove_file(store.path()).ok();
    }
}
