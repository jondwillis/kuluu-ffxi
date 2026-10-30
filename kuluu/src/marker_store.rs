use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use bevy::prelude::{DetectChanges, ResMut, Resource};
use kuluu_render::hud::map_screen::{MapMarker, MapMarkers};
use kuluu_render::snapshot::{system_chat_line, SceneState};
use serde::{Deserialize, Serialize};

pub const MARKER_FILE_VERSION: u32 = 1;

/// Pre-versioning files carry no `version`; they are read as 0 and upgraded on
/// the next write.
const MARKER_FILE_VERSION_LEGACY: u32 = 0;

type MarkersByChar = HashMap<u32, HashMap<u16, Vec<MapMarker>>>;

/// On-disk map markers, keyed by character id then zone id. Loaded into the
/// `MapMarkers` resource when a character logs in; saved whenever the player
/// places or removes a marker. Persistence is per character + zone (retail).
///
/// `deny_unknown_fields` makes an unrecognized shape a hard parse error instead
/// of an empty file that the next save would persist over.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MarkerFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub chars: MarkersByChar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerLoad {
    Idle,
    Loaded(u32),
    Failed(u32),
}

impl MarkerLoad {
    fn char_id(self) -> Option<u32> {
        match self {
            MarkerLoad::Idle => None,
            MarkerLoad::Loaded(id) | MarkerLoad::Failed(id) => Some(id),
        }
    }

    /// Only a successful load may be written back; a `Failed` load leaves the
    /// on-disk file as the authority.
    fn saveable_char_id(self) -> Option<u32> {
        match self {
            MarkerLoad::Loaded(id) => Some(id),
            MarkerLoad::Idle | MarkerLoad::Failed(_) => None,
        }
    }
}

#[derive(Resource, Debug, Clone)]
pub struct MarkerStoreRes {
    pub store: MarkerStore,
    /// The character id whose markers are currently in `MapMarkers`, so a login
    /// (or character switch) reloads exactly once.
    pub loaded: MarkerLoad,
    /// No config dir: the store points at a temp path the OS may wipe, so
    /// saves are refused rather than silently persisted somewhere volatile.
    pub read_only: bool,
    save_issue_notified: bool,
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
        kuluu_session::config_dir::config_file("markers.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load_all(&self) -> Result<MarkerFile> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(MarkerFile {
                    version: MARKER_FILE_VERSION,
                    chars: MarkersByChar::new(),
                });
            }
            Err(e) => return Err(e).with_context(|| format!("read {}", self.path.display())),
        };
        let file = match serde_json::from_slice::<MarkerFile>(&bytes) {
            Ok(file) => file,
            Err(versioned_err) => serde_json::from_slice::<MarkersByChar>(&bytes)
                .map(|chars| MarkerFile {
                    version: MARKER_FILE_VERSION_LEGACY,
                    chars,
                })
                .map_err(|_| versioned_err)
                .with_context(|| format!("parse {}", self.path.display()))?,
        };
        if file.version > MARKER_FILE_VERSION {
            return Err(anyhow!(
                "{} has marker schema version {}, this build understands up to {MARKER_FILE_VERSION}",
                self.path.display(),
                file.version,
            ));
        }
        Ok(file)
    }

    /// Markers for one character, empty if the file or character is absent.
    pub fn load_for(&self, char_id: u32) -> Result<HashMap<u16, Vec<MapMarker>>> {
        Ok(self.load_all()?.chars.remove(&char_id).unwrap_or_default())
    }

    /// Replace one character's section and rewrite the file atomically. Refuses
    /// to write when the existing file can't be read, so an unreadable file
    /// never costs another character their markers.
    pub fn save_for(&self, char_id: u32, by_zone: &HashMap<u16, Vec<MapMarker>>) -> Result<()> {
        let mut all = self.load_all()?;
        all.version = MARKER_FILE_VERSION;
        all.chars.insert(char_id, by_zone.clone());
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
    match MarkerStore::default_path() {
        Ok(path) => MarkerStoreRes {
            store: MarkerStore::new(path),
            loaded: MarkerLoad::Idle,
            read_only: false,
            save_issue_notified: false,
        },
        Err(e) => {
            tracing::warn!(error = %e, "markers: no config dir; markers will not persist");
            MarkerStoreRes {
                store: MarkerStore::new(std::env::temp_dir().join("ffxi-markers.json")),
                loaded: MarkerLoad::Idle,
                read_only: true,
                save_issue_notified: false,
            }
        }
    }
}

/// Load a character's saved markers on login/switch, and persist the in-memory
/// `MapMarkers` whenever the player edits them. The load-before-save ordering
/// (a character change reloads and resets `loaded`) keeps a fresh login from
/// overwriting stored markers with an empty set; a failed load clears the map
/// display and parks in `MarkerLoad::Failed`, which never saves and never
/// retries, so a damaged file is left for the player to inspect rather than
/// overwritten. Any state that blocks or fails saving is surfaced once in the
/// chat log instead of discarding edits silently.
pub fn sync_markers(
    mut scene_state: ResMut<SceneState>,
    mut markers: ResMut<MapMarkers>,
    mut store: ResMut<MarkerStoreRes>,
) {
    let char_id = scene_state.snapshot.self_char_id;

    if let Some(id) = char_id {
        if store.loaded.char_id() != Some(id) {
            match store.store.load_for(id) {
                Ok(by_zone) => {
                    markers.by_zone = by_zone;
                    store.loaded = MarkerLoad::Loaded(id);
                    store.save_issue_notified = false;
                }
                Err(e) => {
                    tracing::warn!(path = %store.store.path().display(), error = %e, "markers: load failed");
                    markers.by_zone = HashMap::new();
                    store.loaded = MarkerLoad::Failed(id);
                }
            }
            // `bypass_change_detection` isn't needed: the very next frame's save
            // branch would re-persist the just-loaded set, which is idempotent.
            return;
        }
    }

    if !markers.is_changed() || char_id.is_none() {
        return;
    }

    let blocked = if store.read_only {
        Some(format!(
            "not saved this session (no config dir; using {})",
            store.store.path().display()
        ))
    } else if matches!(store.loaded, MarkerLoad::Failed(_)) {
        Some(format!(
            "read-only this session ({} failed to load)",
            store.store.path().display()
        ))
    } else {
        None
    };

    match blocked {
        Some(reason) => {
            if !store.save_issue_notified {
                scene_state.push_local_toast(system_chat_line(format!("Map markers: {reason}")));
                store.save_issue_notified = true;
            }
        }
        None => {
            if let Some(id) = store.loaded.saveable_char_id() {
                match store.store.save_for(id, &markers.by_zone) {
                    Ok(()) => store.save_issue_notified = false,
                    Err(e) => {
                        tracing::warn!(path = %store.store.path().display(), error = %e, "markers: save failed");
                        if !store.save_issue_notified {
                            scene_state.push_local_toast(system_chat_line(format!(
                                "Map markers: save failed ({e:#})"
                            )));
                            store.save_issue_notified = true;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use kuluu_snapshot::{SceneSnapshot, Vec3};
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

    #[test]
    fn saved_file_carries_current_schema_version() {
        let store = MarkerStore::new(tmp_path());
        store.save_for(7, &HashMap::new()).unwrap();
        let file: MarkerFile =
            serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(file.version, MARKER_FILE_VERSION);
        std::fs::remove_file(store.path()).ok();
    }

    #[test]
    fn legacy_unversioned_file_still_loads_and_upgrades() {
        let store = MarkerStore::new(tmp_path());
        std::fs::write(store.path(), br#"{"7":{"231":[]}}"#).unwrap();
        assert!(store.load_for(7).unwrap().contains_key(&231));

        let mut by_zone = HashMap::new();
        by_zone.insert(100u16, vec![marker(1.0, 1.0, "AH")]);
        store.save_for(9, &by_zone).unwrap();

        assert!(store.load_for(7).unwrap().contains_key(&231));
        assert_eq!(store.load_for(9).unwrap().get(&100).unwrap()[0].label, "AH");
        std::fs::remove_file(store.path()).ok();
    }

    #[test]
    fn future_schema_version_is_a_load_error() {
        let store = MarkerStore::new(tmp_path());
        let future = MARKER_FILE_VERSION + 1;
        std::fs::write(
            store.path(),
            format!(r#"{{"version":{future},"chars":{{}}}}"#),
        )
        .unwrap();
        assert!(store.load_for(7).is_err());
        std::fs::remove_file(store.path()).ok();
    }

    #[test]
    fn corrupt_file_is_left_untouched_by_save() {
        let store = MarkerStore::new(tmp_path());
        let corrupt = br#"{"7": {"231": [ truncated"#;
        std::fs::write(store.path(), corrupt).unwrap();

        let mut by_zone = HashMap::new();
        by_zone.insert(231u16, vec![marker(0.0, 0.0, "New")]);
        assert!(store.save_for(7, &by_zone).is_err());

        assert_eq!(std::fs::read(store.path()).unwrap(), corrupt);
        std::fs::remove_file(store.path()).ok();
    }

    #[test]
    fn save_does_not_drop_other_chars_when_load_fails() {
        let store = MarkerStore::new(tmp_path());
        let mut kept = HashMap::new();
        kept.insert(231u16, vec![marker(10.0, 20.0, "Home")]);
        store.save_for(7, &kept).unwrap();

        let mut file: MarkerFile =
            serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap();
        file.version = MARKER_FILE_VERSION + 1;
        std::fs::write(store.path(), serde_json::to_vec(&file).unwrap()).unwrap();
        let on_disk = std::fs::read(store.path()).unwrap();

        let mut other = HashMap::new();
        other.insert(100u16, vec![marker(1.0, 1.0, "AH")]);
        assert!(store.save_for(9, &other).is_err());

        assert_eq!(std::fs::read(store.path()).unwrap(), on_disk);
        let recovered: MarkerFile =
            serde_json::from_slice(&std::fs::read(store.path()).unwrap()).unwrap();
        assert_eq!(
            recovered.chars.get(&7).unwrap().get(&231).unwrap()[0].label,
            "Home"
        );
        std::fs::remove_file(store.path()).ok();
    }

    fn sync_app(path: PathBuf, char_id: u32) -> App {
        let mut app = App::new();
        app.insert_resource(SceneState {
            snapshot: SceneSnapshot {
                self_char_id: Some(char_id),
                ..default()
            },
            ..default()
        });
        app.insert_resource(MapMarkers::default());
        app.insert_resource(MarkerStoreRes {
            store: MarkerStore::new(path),
            loaded: MarkerLoad::Idle,
            read_only: false,
            save_issue_notified: false,
        });
        app.add_systems(Update, sync_markers);
        app
    }

    fn toasts(app: &App) -> Vec<String> {
        app.world()
            .resource::<SceneState>()
            .local_toasts
            .iter()
            .map(|l| l.text.clone())
            .collect()
    }

    #[test]
    fn failed_load_clears_stale_display_and_never_saves() {
        let path = tmp_path();
        let corrupt = br#"not json at all"#;
        std::fs::write(&path, corrupt).unwrap();

        let mut app = sync_app(path.clone(), 7);
        app.world_mut()
            .resource_mut::<MapMarkers>()
            .by_zone
            .insert(231, vec![marker(1.0, 2.0, "StaleFromPrevChar")]);

        app.update();
        assert_eq!(
            app.world().resource::<MarkerStoreRes>().loaded,
            MarkerLoad::Failed(7)
        );
        assert!(
            app.world().resource::<MapMarkers>().by_zone.is_empty(),
            "failed load must not display another character's markers"
        );

        app.update();
        app.world_mut()
            .resource_mut::<MapMarkers>()
            .by_zone
            .insert(100, vec![marker(3.0, 4.0, "Later")]);
        app.update();

        assert_eq!(std::fs::read(&path).unwrap(), corrupt);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn failed_load_edit_toasts_once_not_per_edit() {
        let path = tmp_path();
        std::fs::write(&path, br#"not json at all"#).unwrap();

        let mut app = sync_app(path.clone(), 7);
        app.update();
        app.world_mut()
            .resource_mut::<MapMarkers>()
            .by_zone
            .insert(100, vec![marker(3.0, 4.0, "Later")]);
        app.update();
        app.world_mut()
            .resource_mut::<MapMarkers>()
            .by_zone
            .insert(101, vec![marker(5.0, 6.0, "Again")]);
        app.update();

        let toasts = toasts(&app);
        assert_eq!(toasts.len(), 1, "{toasts:?}");
        assert!(toasts[0].contains("read-only this session"), "{toasts:?}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_only_store_toasts_once_and_never_writes() {
        let path = tmp_path();
        let mut app = sync_app(path.clone(), 7);
        app.world_mut().resource_mut::<MarkerStoreRes>().read_only = true;

        app.update();
        app.world_mut()
            .resource_mut::<MapMarkers>()
            .by_zone
            .insert(100, vec![marker(1.0, 1.0, "AH")]);
        app.update();
        app.world_mut()
            .resource_mut::<MapMarkers>()
            .by_zone
            .insert(101, vec![marker(2.0, 2.0, "Home")]);
        app.update();

        let toasts = toasts(&app);
        assert_eq!(toasts.len(), 1, "{toasts:?}");
        assert!(toasts[0].contains("not saved this session"), "{toasts:?}");
        assert!(!path.exists(), "read-only store must not write");
    }

    #[test]
    fn successful_save_emits_no_toast() {
        let path = tmp_path();
        let mut app = sync_app(path.clone(), 7);

        app.update();
        app.world_mut()
            .resource_mut::<MapMarkers>()
            .by_zone
            .insert(100, vec![marker(1.0, 1.0, "AH")]);
        app.update();

        assert!(toasts(&app).is_empty());
        assert!(path.exists());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn successful_load_permits_save() {
        let path = tmp_path();
        let store = MarkerStore::new(path.clone());
        let mut by_zone = HashMap::new();
        by_zone.insert(231u16, vec![marker(10.0, 20.0, "Home")]);
        store.save_for(7, &by_zone).unwrap();

        let mut app = sync_app(path.clone(), 7);
        app.update();
        assert_eq!(
            app.world().resource::<MarkerStoreRes>().loaded,
            MarkerLoad::Loaded(7)
        );

        app.world_mut()
            .resource_mut::<MapMarkers>()
            .by_zone
            .insert(100, vec![marker(1.0, 1.0, "AH")]);
        app.update();

        assert_eq!(store.load_for(7).unwrap().get(&100).unwrap()[0].label, "AH");
        std::fs::remove_file(&path).ok();
    }
}
