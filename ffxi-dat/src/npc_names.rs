//! NPC-name DAT decoder. Server-agnostic: derives names from the FFXI
//! client install, never from emulator (LSB/Phoenix) SQL.
//!
//! Format (POLUtils `PlayOnline.FFXI.Utils.NPCRenamer`, Apache-2.0):
//!   - Each zone's NPC name list lives at `file_id = 6720 + zone_id`.
//!   - Files are flat sequential 32-byte records, no chunk header:
//!       [0x00..0x1C]  28 bytes ASCII name, NUL-padded, NUL-terminated
//!       [0x1C..0x20]  u32 LE npc id
//!   - Slot 0 is reserved (name "none", id 0).
//!
//! Static (database-resident) NPC ids follow the same encoding the
//! DAT stores: `0x01000000 | (zone << 12) | slot`. The low 12 bits
//! index directly into the DAT; the next 12 bits are the zone, which
//! is also implicit in the chosen `file_id`. We verify the zone
//! match in `lookup_by_id` so a caller can't get a wrong-zone hit by
//! accident.
//!
//! Dynamic entities (trusts, pets, fellows, etc.) are NOT in this table —
//! LSB carves them out via `targid + 0x100` in `zone_entities.cpp:629` so
//! their ids' low 12 bits land above the static slot range. Their names
//! arrive over the wire via `s2c::ENTITY_UPDATE1/2` (0x67/0x68).

use std::fs;
use std::path::PathBuf;

use crate::archive::DatRoot;
use crate::{DatError, Result};

/// Base file_id for zone NPC-name lists. POLUtils:
/// `string DATFileName = FFXI.GetFilePath(6720 + this.ID);`
pub const NPC_LIST_FILE_ID_BASE: u32 = 6720;

/// Bytes per record. 28-byte name + 4-byte id.
pub const RECORD_SIZE: usize = 0x20;

/// Length of the name slot inside a record.
pub const NAME_LEN: usize = 0x1C;

/// LSB/retail NPC id high marker: `id & 0xFF000000 == 0x01000000`.
const ID_MARKER: u32 = 0x0100_0000;

/// Maximum reasonable zone id. LSB's zone enum tops out around 300;
/// retail tops out lower. Anything above this is a malformed id.
const MAX_ZONE_ID: u16 = 0x0FFF;

/// One zone's worth of NPC names, kept as the raw DAT bytes so lookups
/// can return `&str` slices borrowed directly from the buffer.
#[derive(Debug)]
pub struct NpcNameTable {
    zone_id: u16,
    source: PathBuf,
    bytes: Box<[u8]>,
}

impl NpcNameTable {
    /// Open the NPC-name DAT for `zone_id` via the supplied `DatRoot`.
    /// Returns `DatError::FileNotPresent` if no APPID claims the
    /// file_id (the zone has no NPC list — e.g. a battlefield-only
    /// instance), `DatError::Io` for filesystem errors.
    pub fn open(root: &DatRoot, zone_id: u16) -> Result<Self> {
        let file_id = NPC_LIST_FILE_ID_BASE + u32::from(zone_id);
        let location = root.resolve(file_id)?;
        let path = location.path_under(root.root());
        let bytes = fs::read(&path).map_err(|source| DatError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self {
            zone_id,
            source: path,
            bytes: bytes.into_boxed_slice(),
        })
    }

    /// Build a table directly from bytes. Used for tests; production
    /// callers should use `open`.
    pub fn from_bytes(zone_id: u16, bytes: impl Into<Box<[u8]>>) -> Self {
        Self {
            zone_id,
            source: PathBuf::from("<in-memory>"),
            bytes: bytes.into(),
        }
    }

    pub fn zone_id(&self) -> u16 {
        self.zone_id
    }

    pub fn source(&self) -> &std::path::Path {
        &self.source
    }

    /// Number of complete 32-byte records in the file. Truncated trailing
    /// bytes are ignored.
    pub fn len(&self) -> usize {
        self.bytes.len() / RECORD_SIZE
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the name at `slot`, or `None` if slot is out of range,
    /// the name field is empty (record present but unused), or the
    /// name contains non-ASCII / non-printable bytes.
    ///
    /// "slot 0 is reserved ('none')" is enforced by returning `None`
    /// for slot 0 — callers can short-circuit before incurring the read.
    pub fn lookup_by_slot(&self, slot: u16) -> Option<&str> {
        if slot == 0 {
            return None;
        }
        let offset = usize::from(slot) * RECORD_SIZE;
        let name_bytes = self.bytes.get(offset..offset + NAME_LEN)?;
        // Trim at first NUL.
        let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        let trimmed = &name_bytes[..end];
        if trimmed.is_empty() {
            return None;
        }
        // ASCII-printable validation: rejects mojibake / re-purposed slots.
        // Space (0x20) is allowed — names like "Synergy Engineer" contain it.
        if !trimmed.iter().all(|&b| (0x20..=0x7E).contains(&b)) {
            return None;
        }
        std::str::from_utf8(trimmed).ok()
    }

    /// Convenience: extract the slot from an LSB/retail npc id and
    /// look it up. Returns `None` if the id is malformed, doesn't
    /// belong to this zone, or the slot is empty/unused.
    pub fn lookup_by_id(&self, npc_id: u32) -> Option<&str> {
        // Reject ids without the entity marker byte.
        if (npc_id & 0xFF00_0000) != ID_MARKER {
            return None;
        }
        let zone_bits = ((npc_id >> 12) & 0xFFF) as u16;
        if zone_bits != self.zone_id {
            return None;
        }
        let slot = (npc_id & 0xFFF) as u16;
        self.lookup_by_slot(slot)
    }
}

/// Decompose a static-NPC id into (zone, slot). Returns `None` for ids
/// missing the entity marker or with an out-of-range zone. Useful for
/// callers that want to pick a `NpcNameTable` (per-zone cache) without
/// instantiating one first.
pub fn split_id(npc_id: u32) -> Option<(u16, u16)> {
    if (npc_id & 0xFF00_0000) != ID_MARKER {
        return None;
    }
    let zone = ((npc_id >> 12) & 0xFFF) as u16;
    if zone > MAX_ZONE_ID {
        return None;
    }
    let slot = (npc_id & 0xFFF) as u16;
    Some((zone, slot))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal in-memory NpcNameTable with three slots:
    /// 0 = "none", 1 = "Ceraule", 10 = "Apairemant". Other slots empty.
    fn synth_table(zone_id: u16) -> NpcNameTable {
        let mut buf = vec![0u8; 11 * RECORD_SIZE];
        // Slot 0: "none"
        write_record(&mut buf, 0, b"none", 0);
        // Slot 1: Ceraule, id = 0x010E6001 for zone 230
        write_record(&mut buf, 1, b"Ceraule", 0x0100_0000 | (u32::from(zone_id) << 12) | 1);
        // Slot 10: Apairemant, id = 0x010E600A for zone 230
        write_record(&mut buf, 10, b"Apairemant", 0x0100_0000 | (u32::from(zone_id) << 12) | 10);
        NpcNameTable::from_bytes(zone_id, buf)
    }

    fn write_record(buf: &mut [u8], slot: usize, name: &[u8], id: u32) {
        let off = slot * RECORD_SIZE;
        buf[off..off + name.len()].copy_from_slice(name);
        // Name slot already NUL-filled by vec![0; ...].
        buf[off + NAME_LEN..off + RECORD_SIZE].copy_from_slice(&id.to_le_bytes());
    }

    #[test]
    fn split_id_extracts_zone_and_slot() {
        // Apairemant from a live LSB session.
        assert_eq!(split_id(17_719_306), Some((230, 10)));
        // Non-entity ids rejected.
        assert_eq!(split_id(0x0000_0000), None);
        assert_eq!(split_id(0x0200_0000), None);
    }

    #[test]
    fn lookup_by_slot_returns_name_at_slot() {
        let t = synth_table(230);
        assert_eq!(t.lookup_by_slot(1), Some("Ceraule"));
        assert_eq!(t.lookup_by_slot(10), Some("Apairemant"));
    }

    #[test]
    fn lookup_by_slot_zero_is_always_none() {
        // POLUtils stores "none" in slot 0 as a sentinel; we treat slot 0
        // as "no NPC" even though the byte slot has a printable string.
        let t = synth_table(230);
        assert_eq!(t.lookup_by_slot(0), None);
    }

    #[test]
    fn lookup_by_slot_returns_none_for_empty_record() {
        let t = synth_table(230);
        // Slot 2 was never written → all zeros → empty after NUL trim.
        assert_eq!(t.lookup_by_slot(2), None);
    }

    #[test]
    fn lookup_by_slot_returns_none_for_out_of_range_slot() {
        let t = synth_table(230);
        assert_eq!(t.lookup_by_slot(999), None);
    }

    #[test]
    fn lookup_by_slot_rejects_non_ascii_name() {
        // Synthesize a record where the name slot has a high-bit byte
        // (could be DAT-rot or a slot repurposed as binary data).
        let mut buf = vec![0u8; 2 * RECORD_SIZE];
        write_record(&mut buf, 1, b"\x80valid?", 1);
        let t = NpcNameTable::from_bytes(230, buf);
        assert_eq!(t.lookup_by_slot(1), None);
    }

    #[test]
    fn lookup_by_slot_accepts_space_in_name() {
        // POLUtils-style display names with spaces — e.g. "Synergy Engineer".
        let mut buf = vec![0u8; 2 * RECORD_SIZE];
        write_record(&mut buf, 1, b"Synergy Engineer", 1);
        let t = NpcNameTable::from_bytes(230, buf);
        assert_eq!(t.lookup_by_slot(1), Some("Synergy Engineer"));
    }

    #[test]
    fn lookup_by_id_returns_name_for_matching_zone() {
        let t = synth_table(230);
        assert_eq!(t.lookup_by_id(17_719_306), Some("Apairemant"));
    }

    #[test]
    fn lookup_by_id_rejects_wrong_zone() {
        // Same numeric slot, but the id's zone bits don't match the
        // table's zone_id. Returning Some here would be a wrong-zone hit.
        let t = synth_table(230);
        // Zone 100 << 12 | 10 = 0x0006_400A | 0x0100_0000 = 0x0106_400A
        let wrong_zone_id = 0x0100_0000 | (100u32 << 12) | 10;
        assert_eq!(t.lookup_by_id(wrong_zone_id), None);
    }

    #[test]
    fn lookup_by_id_rejects_ids_without_entity_marker() {
        let t = synth_table(230);
        // Strip the marker byte off a valid id.
        assert_eq!(t.lookup_by_id(0x000E_600A), None);
    }
}
