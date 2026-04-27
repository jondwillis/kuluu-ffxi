use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use bevy::prelude::Resource;
use ffxi_viewer_core::{Action, Bindings, KeyBind, Preset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PersistedKeybinds {
    #[serde(default)]
    pub preset: Preset,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<Action, KeyBind>,
}

impl PersistedKeybinds {
    pub fn into_bindings(self) -> Bindings {
        let mut bindings = self.preset.bindings();
        for (action, bind) in self.overrides {
            bindings.insert(action, bind);
        }
        bindings
    }

    pub fn from_preset(preset: Preset) -> Self {
        Self {
            preset,
            overrides: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct KeybindsStore {
    path: PathBuf,
}

#[derive(Resource, Debug, Clone)]
pub struct KeybindsStateRes {
    pub store: KeybindsStore,
    pub persisted: PersistedKeybinds,
}

impl KeybindsStateRes {
    pub fn apply_preset(&mut self, preset: Preset) -> (Bindings, std::io::Result<()>) {
        self.persisted = PersistedKeybinds::from_preset(preset);
        let new_bindings = self.persisted.clone().into_bindings();
        let save_result = self
            .store
            .save(&self.persisted)
            .map_err(|e| std::io::Error::other(format!("save keybinds: {e}")));
        (new_bindings, save_result)
    }

    pub fn apply_reset(&mut self) -> (Bindings, std::io::Result<()>) {
        let preset = self.persisted.preset;
        self.apply_preset(preset)
    }
}

impl KeybindsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

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
        Ok(base.join("ffxi-mcp").join("keybinds.json"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Option<PersistedKeybinds>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let kb: PersistedKeybinds = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                Ok(Some(kb))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("read {}", self.path.display())),
        }
    }

    pub fn save(&self, kb: &PersistedKeybinds) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(kb).context("serialize keybinds")?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename {} → {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

pub fn load_or_default() -> (Bindings, PersistedKeybinds) {
    let path = match KeybindsStore::default_path() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "keybinds: no XDG/HOME path; using default Compact 2");
            let kb = PersistedKeybinds::from_preset(Preset::default());
            return (kb.clone().into_bindings(), kb);
        }
    };
    let store = KeybindsStore::new(path);
    match store.load() {
        Ok(Some(kb)) => (kb.clone().into_bindings(), kb),
        Ok(None) => {
            let kb = PersistedKeybinds::from_preset(Preset::default());
            (kb.clone().into_bindings(), kb)
        }
        Err(e) => {
            tracing::warn!(
                path = %store.path().display(),
                error = %e,
                "keybinds: parse failed; falling back to default Compact 2",
            );
            let kb = PersistedKeybinds::from_preset(Preset::default());
            (kb.clone().into_bindings(), kb)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::keyboard::KeyCode;
    use ffxi_viewer_core::{KeyBind, Modifiers};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp_path() -> PathBuf {
        let mut p = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        p.push(format!(
            "ffxi-keybinds-store-{}-{stamp}.json",
            std::process::id()
        ));
        p
    }

    #[test]
    fn load_missing_returns_none() {
        let store = KeybindsStore::new(tmp_path());
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn save_and_load_roundtrip_with_overrides() {
        let store = KeybindsStore::new(tmp_path());
        let mut overrides = BTreeMap::new();
        overrides.insert(Action::MoveForward, KeyBind::new(KeyCode::ArrowUp));
        overrides.insert(
            Action::TargetParty2,
            KeyBind::with(KeyCode::Digit2, Modifiers::CTRL),
        );
        let kb = PersistedKeybinds {
            preset: Preset::Custom,
            overrides,
        };
        store.save(&kb).unwrap();

        let loaded = store.load().unwrap().expect("present after save");
        assert_eq!(loaded, kb);
        std::fs::remove_file(store.path()).ok();
    }

    #[test]
    fn save_and_load_preset_only() {
        let store = KeybindsStore::new(tmp_path());
        let kb = PersistedKeybinds::from_preset(Preset::Standard);
        store.save(&kb).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.preset, Preset::Standard);
        assert!(loaded.overrides.is_empty());
        std::fs::remove_file(store.path()).ok();
    }

    #[test]
    fn into_bindings_layers_overrides_on_preset() {
        let mut overrides = BTreeMap::new();
        overrides.insert(Action::MoveForward, KeyBind::new(KeyCode::ArrowUp));
        let kb = PersistedKeybinds {
            preset: Preset::Compact2,
            overrides,
        };
        let bindings = kb.into_bindings();

        assert_eq!(
            bindings.get(Action::MoveForward),
            Some(KeyBind::new(KeyCode::ArrowUp))
        );

        assert_eq!(
            bindings.get(Action::RotateLeft),
            Some(KeyBind::new(KeyCode::KeyQ))
        );
    }

    #[test]
    fn empty_overrides_omitted_from_json() {
        let kb = PersistedKeybinds::from_preset(Preset::Compact2);
        let json = serde_json::to_string(&kb).unwrap();

        assert!(!json.contains("overrides"), "got: {json}");
    }
}
