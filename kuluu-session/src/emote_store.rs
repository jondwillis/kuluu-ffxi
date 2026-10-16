//! The client-local emote display order (bead `kuluu-o9qt7.34`), in the same
//! user-file law family as [`crate::macro_store`]: retail keeps the player's
//! arranged emote list in its own per-user blob and never puts it on the wire —
//! the `emote` window reads that blob at fill time (FFXiMain.dll retail-2026-09
//! `.text 0x2259C6`, through the user-file helper family `.text 0x1C4740` over
//! the config singleton `[0x10662dcc]`), and the swap driven from `abisortw`
//! mutates a shared sort model (`.text 0x225918` calling `[0x1066715c]->+…`)
//! whose contents then re-feed the fill. LSB ships no emote-order packet at all
//! [V-negative], so like macros this is honest client-local state: kuluu keeps
//! one JSON file per install keyed by lowercased character name.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The whole user file: every character's arranged emote ids in one map so the
/// (tiny) store round-trips atomically. Keys are lowercased character names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PersistedEmoteOrder {
    #[serde(default)]
    pub by_character: BTreeMap<String, Vec<u8>>,
}

impl PersistedEmoteOrder {
    /// The store key for a character name (same law as `PersistedMacros::key`).
    pub fn key(name: &str) -> String {
        name.to_lowercase()
    }

    pub fn order_for(&self, name: &str) -> Vec<u8> {
        self.by_character
            .get(&Self::key(name))
            .cloned()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct EmoteStore {
    path: PathBuf,
}

impl EmoteStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> Result<PathBuf> {
        crate::config_dir::config_file("emote_order.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<PersistedEmoteOrder> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(PersistedEmoteOrder::default())
            }
            Err(e) => Err(anyhow::Error::new(e).context(format!("read {}", self.path.display()))),
        }
    }

    pub fn save(&self, order: &PersistedEmoteOrder) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(order).context("serialize emote order")?;
        // Same write-then-rename discipline as the sibling stores: a crash mid
        // write must not eat the previous file.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)
            .with_context(|| format!("write {}", tmp.display()))
            .and_then(|()| {
                std::fs::rename(&tmp, &self.path)
                    .with_context(|| format!("rename {} -> {}", tmp.display(), self.path.display()))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_round_trips_per_character() {
        let dir = std::env::temp_dir().join(format!(
            "kuluu-emote-store-{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let store = EmoteStore::new(dir.join("emote_order.json"));
        let mut persisted = PersistedEmoteOrder::default();
        persisted
            .by_character
            .insert(PersistedEmoteOrder::key("Bionic"), vec![6, 4, 12]);
        store.save(&persisted).unwrap();
        let back = store.load().unwrap();
        assert_eq!(
            back.order_for("bionic"),
            vec![6, 4, 12],
            "names key case-insensitively like the macro store"
        );
        assert_eq!(back.order_for("nobody"), Vec::<u8>::new());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A missing file is not an error: it means "never arranged", which the fold
    /// renders as the default order (empty `SceneSnapshot::emote_order`).
    #[test]
    fn a_missing_store_reads_as_never_arranged() {
        let store = EmoteStore::new(std::env::temp_dir().join("kuluu-emote-store-absent.json"));
        assert!(store.load().unwrap().by_character.is_empty());
    }
}
