//! The client-local macro store (bead `kuluu-o9qt7.19.2`).
//!
//! Law from the DLL: macros never ride the wire — LSB ships no macro packets,
//! and the retail client keeps them in its own user files through the same
//! per-user-file framework that persists `tig.dat`/`cnf.dat` (the section
//! helper family at FFXiMain.dll retail-2026-09 RVA 0x1C4920; vault
//! main_menu.md §53). The store itself is the singleton `[0x10662DA0]`, whose
//! DAT-grounded capacity law is two blocks × twenty keytops (chunks 'mcrs' and
//! 'mcr7', each a grid of exactly twenty widgets — vault main_menu.md §41).
//! kuluu keeps that identity model in a JSON file per install, keyed by
//! character name (globally unique in FFXI), mirroring the sibling stores here
//! (`goal_store.rs`).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use kuluu_snapshot::MacroSlot;
use serde::{Deserialize, Serialize};

/// Macro blocks grounded by the two DAT grids ('mcrs' @ROM/0/1.DAT 0x2A3CC0 and
/// 'mcr7' @0x2A8560, twenty keytop widgets each — vault main_menu.md §41).
pub const MACRO_BLOCKS: u8 = 2;

/// Keytops per block (the same DAT law; the mcres20 ten-page pager scrolls a
/// window over these, it does not extend them — vault main_menu.md §41
/// correction).
pub const MACRO_KEYTOPS_PER_BLOCK: u8 = 20;

/// The whole user file: every character's slots in one map so the (tiny) store
/// round-trips atomically. Keys are lowercased character names.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PersistedMacros {
    #[serde(default)]
    pub by_character: BTreeMap<String, Vec<MacroSlot>>,
}

impl PersistedMacros {
    /// The store key for a character name (FFXI names are unique across all worlds).
    pub fn key(name: &str) -> String {
        name.to_lowercase()
    }

    pub fn slots_for(&self, name: &str) -> Vec<MacroSlot> {
        self.by_character
            .get(&Self::key(name))
            .cloned()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct MacroStore {
    path: PathBuf,
}

impl MacroStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn default_path() -> Result<PathBuf> {
        crate::config_dir::config_file("macros.json")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<PersistedMacros> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let macros: PersistedMacros = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", self.path.display()))?;
                Ok(macros)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PersistedMacros::default()),
            Err(e) => Err(anyhow::Error::new(e).context(format!("read {}", self.path.display()))),
        }
    }

    pub fn save(&self, macros: &PersistedMacros) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        let bytes = serde_json::to_vec_pretty(macros).context("serialize macros")?;
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

/// True when (block, index) names a real keytop under the DAT capacity law.
pub fn macro_slot_in_range(block: u8, index: u8) -> bool {
    block < MACRO_BLOCKS && index < MACRO_KEYTOPS_PER_BLOCK
}

/// Applies one retail-style edit to `slots` in place: any text replaces (or
/// creates) the slot; whitespace-only text clears it — matching retail's blank
/// keytops for unassigned cells. Out-of-capacity edits are refused (false).
pub fn apply_macro_edit(slots: &mut Vec<MacroSlot>, block: u8, index: u8, text: &str) -> bool {
    if !macro_slot_in_range(block, index) {
        return false;
    }
    slots.retain(|s| !(s.block == block && s.index == index));
    if !text.trim().is_empty() {
        slots.push(MacroSlot {
            block,
            index,
            text: text.to_string(),
        });
    }
    true
}

/// Canonical form for the file and the folded state: sorted by (block, index),
/// last write wins on duplicates, blanks dropped, anything outside the DAT
/// capacity law discarded. The session applies commits through this so the
/// runtime copy and the user file agree by construction.
pub fn normalize_macro_slots(slots: &[MacroSlot]) -> Vec<MacroSlot> {
    let mut seen = HashSet::new();
    let mut out: Vec<&MacroSlot> = slots
        .iter()
        .filter(|s| macro_slot_in_range(s.block, s.index) && !s.text.trim().is_empty())
        // Reverse iteration + reverse insertion makes the LAST duplicate win.
        .rev()
        .filter(|s| seen.insert((s.block, s.index)))
        .collect();
    out.reverse();
    out.sort_by_key(|s| (s.block, s.index));
    out.into_iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(block: u8, index: u8, text: &str) -> MacroSlot {
        MacroSlot {
            block,
            index,
            text: text.to_string(),
        }
    }

    #[test]
    fn edits_insert_replace_and_clear() {
        let mut slots = Vec::new();
        assert!(apply_macro_edit(&mut slots, 0, 3, "/ra"));
        assert_eq!(slots.len(), 1);
        // Replace keeps exactly one entry for the keytop.
        apply_macro_edit(&mut slots, 0, 3, "/hurtme");
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].text, "/hurtme");
        // Whitespace-only text clears (retail's blank keytops).
        assert!(apply_macro_edit(&mut slots, 0, 3, "   "));
        assert!(slots.is_empty());
    }

    #[test]
    fn capacity_law_refuses_beyond_two_blocks_of_twenty() {
        let mut slots = Vec::new();
        assert!(
            !apply_macro_edit(&mut slots, 2, 0, "/ra"),
            "block 2 is over capacity"
        );
        assert!(
            !apply_macro_edit(&mut slots, 0, 20, "/ra"),
            "keytop 20 is over capacity"
        );
        assert!(
            apply_macro_edit(&mut slots, 1, 19, "/ra"),
            "the far corner is valid"
        );
    }

    #[test]
    fn normalization_sorts_dedups_last_wins_and_discards_strays() {
        let raw = vec![
            slot(1, 2, "/b"),
            slot(0, 5, "/first"),
            slot(0, 5, "/last wins"),
            slot(0, 9, "  "),
            slot(7, 0, "/out of range block"),
        ];
        let got = normalize_macro_slots(&raw);
        assert_eq!(got.len(), 2);
        assert_eq!((got[0].block, got[0].index), (0, 5));
        assert_eq!(got[0].text, "/last wins");
        assert_eq!((got[1].block, got[1].index), (1, 2));
    }

    #[test]
    fn file_roundtrips_per_character() {
        let dir = std::env::temp_dir().join(format!("kuluu-macro-store-test-{}", file_id()));
        let store = MacroStore::new(dir.join("macros.json"));
        assert!(
            store.load().unwrap().by_character.is_empty(),
            "missing file reads empty"
        );

        let mut persisted = PersistedMacros::default();
        persisted
            .by_character
            .insert(PersistedMacros::key("Bionic"), vec![slot(0, 0, "/ra")]);
        persisted.by_character.insert(
            PersistedMacros::key("Mothurio"),
            vec![slot(1, 4, "/lastshout")],
        );
        store.save(&persisted).unwrap();

        let back = store.load().unwrap();
        assert_eq!(
            back.slots_for("bionic"),
            vec![slot(0, 0, "/ra")],
            "lookup case-folded"
        );
        assert_eq!(back.slots_for("Mothurio").len(), 1);
        assert!(back.slots_for("nobody").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_path_lands_in_the_kuluu_config_dir() {
        let p = MacroStore::default_path().unwrap();
        assert!(
            p.ends_with("kuluu/macros.json") || p.display().to_string().ends_with("macros.json"),
            "got {}",
            p.display()
        );
    }

    fn file_id() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(1);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        SEQ.fetch_add(1, Ordering::Relaxed) << 32 | nanos
    }
}
