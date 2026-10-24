//! The on-disk half of the client-side Config option store (bead kuluu-uos3.4).
//!
//! Retail law (vault main_menu.md §59 addendum 47): every Config page writes into one
//! client-side table — an array of `0x2C`-byte records hung off the option singleton at
//! `[0x10662DCC]`, where record `+0x04` is the type, `+0x08` the live value and `+0x0C` the
//! persistence class. Records whose persistence class is 4 round-trip through the user file
//! `cnf.dat`: FFXiMain.dll retail-2026-09 collects them at `.text 0x195900` (walk of the whole
//! record array, only class-4 values pushed into a fixed buffer) and writes that buffer through
//! the user-file helper `.text 0x1C4920`; boot reads it back at `.text 0x1959A0` (header magic +
//! minimum-length gate, then each value re-applied record by record through `.text 0x196040`)
//! from the lifecycle arm `.text 0x195350`, and a failed read restores every default via
//! `.text 0x196260`. The matching shutdown arm `.text 0x1953F0` writes the file, but only once
//! boot has marked the store ready. Retail composes the path as `%s/%s` when no per-user id
//! exists and `%s/%x/%s` otherwise (`.text 0x1C3D00`) — a single bucket for the whole install is
//! retail's own no-id branch, which is what kuluu persists here.
//!
//! None of it rides the wire: LSB's config packet carries only the named flag bits
//! (vendor/server/src/map/packets/c2s/0x0dc_config.cpp), never store options, so kuluu keeps this
//! file law locally in `config_options.json` beside the other client stores. Unlike retail, kuluu
//! writes through on change instead of at shutdown — same content set, but it also survives a
//! killed process, which is what "the option still answers after logout" requires.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::*;
use kuluu_render::hud::menu::DynamicMenu;

/// File name for the option bucket, beside `graphics.json` and friends.
pub const CONFIG_OPTIONS_FILE_NAME: &str = "config_options.json";

/// One snapshot of the client-side option store: retail store id → value. Keys are decimal
/// strings in JSON so a hand-edited file stays readable.
pub type ConfigOptionMap = BTreeMap<u32, i32>;

#[derive(Debug, Clone)]
pub struct ConfigOptionsStore {
    path: PathBuf,
}

impl ConfigOptionsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> Result<PathBuf> {
        kuluu_session::config_dir::config_file(CONFIG_OPTIONS_FILE_NAME)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `Ok(None)` when the file does not exist yet — a fresh install has no persisted options,
    /// which is exactly retail's "read failed → keep the defaults" branch.
    pub fn load(&self) -> Result<Option<ConfigOptionMap>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let values = parse_config_options(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                Ok(Some(values))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    pub fn save(&self, values: &ConfigOptionMap) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(values).context("serialize config options")?;
        // Same write-then-rename discipline as the sibling stores: a crash mid-write must not
        // eat the previous file.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

/// Parse the option file entry by entry: one malformed pair (a hand-edited typo) drops just that
/// option instead of losing every other stored value, which the write-through system would then
/// overwrite and destroy. A syntactically broken document still fails outright — there is nothing
/// to salvage from it, and treating it as empty would blank the lot on the next change.
fn parse_config_options(bytes: &[u8]) -> Result<ConfigOptionMap> {
    let root: serde_json::Value = serde_json::from_slice(bytes).context("config options JSON")?;
    let mut values = ConfigOptionMap::new();
    if let Some(object) = root.as_object() {
        for (key, value) in object {
            let Ok(store_id) = key.parse::<u32>() else {
                continue;
            };
            // Option stores hold dwords: only whole numbers are meaningful.
            let Some(raw) = value.as_i64() else { continue };
            let Ok(stored) = i32::try_from(raw) else {
                continue;
            };
            values.insert(store_id, stored);
        }
    }
    Ok(values)
}

/// Loaded file content plus the store to write it back to. `store` is `None` when no user config
/// dir resolves at all — then nothing is persisted and every option stays session-only.
#[derive(Resource)]
pub struct ConfigOptionsStateRes {
    pub store: Option<ConfigOptionsStore>,
    pub persisted: Option<ConfigOptionMap>,
}

/// Read the file before the app builds anything, mirroring `graphics_store::load_or_default`. An
/// unreadable file warns and starts empty; it must never kill a session.
pub fn load_or_default() -> ConfigOptionsStateRes {
    let store = match ConfigOptionsStore::default_path() {
        Ok(path) => Some(ConfigOptionsStore::new(path)),
        Err(e) => {
            tracing::warn!(error = %e, "no config dir for the client-local config option store");
            None
        }
    };
    let persisted = match &store {
        Some(store) => match store.load() {
            Ok(values) => values,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %store.path().display(),
                    "config options unreadable — starting from the boot defaults"
                );
                None
            }
        },
        None => None,
    };
    ConfigOptionsStateRes { store, persisted }
}

/// Bring stored options back and keep them there.
///
/// Retail applies its file over the option table once, at store init (vault main_menu.md §59
/// addendum 47), so a row never touched in this run still answers as the player left it after a
/// logout. kuluu's menu manager boots empty, which lets the same rule be expressed incrementally:
/// every stored key missing from the live table is inserted (a live value always wins — the file
/// only ever fills gaps), and any content difference between the live table and what was last
/// written to disk becomes one write. Comparison is by content rather than change detection: menu
/// key handling deref-muts the option resource on every press even when a row ignores it, the trap
/// `persist_graphics_on_change` documents for graphics.
pub fn persist_config_options_on_change(
    mut menu: ResMut<DynamicMenu>,
    state: Res<ConfigOptionsStateRes>,
    mut last_written: Local<Option<ConfigOptionMap>>,
    mut last_attempt: Local<Option<ConfigOptionMap>>,
) {
    let Some(store) = state.store.as_ref() else {
        return;
    };

    if let Some(persisted) = state.persisted.as_ref() {
        for (key, value) in persisted {
            menu.config_option_values.entry(*key).or_insert(*value);
        }
    }

    let current: ConfigOptionMap = menu.config_option_values.clone();
    if last_written.as_ref() == Some(&current) || last_attempt.as_ref() == Some(&current) {
        // Nothing to do, or this exact content already failed to land — retrying it every frame
        // would just hammer the disk. Any further change re-arms the write.
        return;
    }
    // First look at the live table: when it still matches the file (or holds nothing), there is
    // no difference worth a write, so take it as the baseline instead.
    let first_look = last_written.is_none() && last_attempt.is_none();
    if first_look && (current.is_empty() || state.persisted.as_ref() == Some(&current)) {
        *last_written = Some(current);
        return;
    }

    match store.save(&current) {
        Ok(()) => {
            *last_written = Some(current);
            *last_attempt = None;
        }
        Err(e) => {
            tracing::warn!(
                path = %store.path().display(),
                error = %e,
                "config options: failed to persist",
            );
            *last_attempt = Some(current);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(label: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!(
            "ffxi-config-options-{}-{label}-{stamp}.json",
            std::process::id(),
        ));
        p
    }

    #[test]
    fn options_round_trip_through_the_store() {
        let path = tmp_path("round-trip");
        let store = ConfigOptionsStore::new(path.clone());
        let mut values = ConfigOptionMap::new();
        values.insert(0x18, 1);
        values.insert(0x9C, -1);
        values.insert(0xCE, 206);
        store.save(&values).unwrap();

        let reloaded = ConfigOptionsStore::new(path.clone());
        assert_eq!(reloaded.load().unwrap().as_ref(), Some(&values));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_file_means_no_persisted_options() {
        let store = ConfigOptionsStore::new(tmp_path("absent"));
        assert_eq!(store.load().unwrap(), None);
    }

    /// One bad pair costs only that option: the rest of the file must survive, because the
    /// write-through system would otherwise store the blanks it just parsed.
    #[test]
    fn a_malformed_entry_drops_only_itself() {
        let path = tmp_path("malformed");
        std::fs::write(
            &path,
            br#"{"18": 1, "20": null, "not-a-store-id": 7, "24": 3.5}"#,
        )
        .unwrap();
        let values = ConfigOptionsStore::new(path.clone())
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(values.get(&18), Some(&1));
        assert_eq!(values.get(&20), None);
        assert_eq!(values.len(), 1);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_broken_document_is_an_error_not_an_empty_store() {
        let path = tmp_path("broken");
        std::fs::write(&path, br#"{"18": 1"#).unwrap();
        assert!(ConfigOptionsStore::new(path.clone()).load().is_err());
        std::fs::remove_file(&path).ok();
    }

    /// A fresh menu opens with the file's values (retail applies them at store init), and that
    /// seeding alone does not rewrite the file.
    #[test]
    fn persisted_options_seed_a_fresh_menu_without_rewriting_the_file() {
        let path = tmp_path("seed");
        let store = ConfigOptionsStore::new(path.clone());
        let mut persisted = ConfigOptionMap::new();
        persisted.insert(0x32, 4);
        persisted.insert(0xBF, 0);
        store.save(&persisted).unwrap();
        let before = std::fs::read(&path).unwrap();

        let mut app = App::new();
        app.init_resource::<DynamicMenu>()
            .insert_resource(ConfigOptionsStateRes {
                store: Some(store.clone()),
                persisted: Some(persisted),
            })
            .add_systems(Update, persist_config_options_on_change);
        app.update();

        let menu = app.world().resource::<DynamicMenu>();
        assert_eq!(menu.config_option_values.get(&0x32), Some(&4));
        assert_eq!(menu.config_option_values.get(&0xBF), Some(&0));
        // Seeding fills gaps only; the file is left byte-identical until something changes.
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(&path).ok();
    }

    /// A change made in a Config page is on disk before any shutdown happens: logging out and back
    /// in must find it (retail writes at exit; kuluu writes through, so a killed client keeps it).
    #[test]
    fn an_option_change_is_written_through() {
        let path = tmp_path("write-through");
        let store = ConfigOptionsStore::new(path.clone());
        let mut app = App::new();
        app.init_resource::<DynamicMenu>()
            .insert_resource(ConfigOptionsStateRes {
                store: Some(store.clone()),
                persisted: None,
            })
            .add_systems(Update, persist_config_options_on_change);
        app.update();

        assert!(
            !path.exists(),
            "an unchanged option table was written to disk"
        );

        {
            let mut menu = app.world_mut().resource_mut::<DynamicMenu>();
            menu.config_option_values.insert(0x99, 64);
        }
        app.update();
        assert_eq!(
            store.load().unwrap().unwrap().get(&0x99),
            Some(&64),
            "a config option change never reached disk"
        );

        // A later frame whose content is identical writes nothing new.
        let before = std::fs::read(&path).unwrap();
        app.world_mut().resource_mut::<DynamicMenu>().set_changed();
        app.update();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_file(&path).ok();
    }

    /// The live table wins over the file: a value the player changed earlier in this run is never
    /// clobbered by an older stored copy.
    #[test]
    fn a_live_option_value_beats_the_stored_copy() {
        let path = tmp_path("live-wins");
        let store = ConfigOptionsStore::new(path.clone());
        let mut persisted = ConfigOptionMap::new();
        persisted.insert(0x41, 2);
        store.save(&persisted).unwrap();

        let mut app = App::new();
        let mut menu = DynamicMenu::default();
        menu.config_option_values.insert(0x41, 5);
        app.insert_resource(menu)
            .insert_resource(ConfigOptionsStateRes {
                store: Some(store.clone()),
                persisted: Some(persisted),
            })
            .add_systems(Update, persist_config_options_on_change);
        app.update();

        assert_eq!(
            app.world()
                .resource::<DynamicMenu>()
                .config_option_values
                .get(&0x41),
            Some(&5)
        );
        std::fs::remove_file(&path).ok();
    }
}
