use std::fs;
use std::path::PathBuf;

use crate::archive::DatRoot;
use crate::{DatError, Result};

pub const NPC_LIST_FILE_ID_BASE: u32 = 6720;

pub const RECORD_SIZE: usize = 0x20;

pub const NAME_LEN: usize = 0x1C;

const ID_MARKER: u32 = 0x0100_0000;

const MAX_ZONE_ID: u16 = 0x0FFF;

#[derive(Debug)]
pub struct NpcNameTable {
    zone_id: u16,
    source: PathBuf,
    bytes: Box<[u8]>,
}

impl NpcNameTable {
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

    pub fn len(&self) -> usize {
        self.bytes.len() / RECORD_SIZE
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn lookup_by_slot(&self, slot: u16) -> Option<&str> {
        if slot == 0 {
            return None;
        }
        let offset = usize::from(slot) * RECORD_SIZE;
        let name_bytes = self.bytes.get(offset..offset + NAME_LEN)?;

        let end = name_bytes.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        let trimmed = &name_bytes[..end];
        if trimmed.is_empty() {
            return None;
        }

        if !trimmed.iter().all(|&b| (0x20..=0x7E).contains(&b)) {
            return None;
        }
        std::str::from_utf8(trimmed).ok()
    }

    pub fn lookup_by_id(&self, npc_id: u32) -> Option<&str> {
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

    fn synth_table(zone_id: u16) -> NpcNameTable {
        let mut buf = vec![0u8; 11 * RECORD_SIZE];

        write_record(&mut buf, 0, b"none", 0);

        write_record(
            &mut buf,
            1,
            b"Ceraule",
            0x0100_0000 | (u32::from(zone_id) << 12) | 1,
        );

        write_record(
            &mut buf,
            10,
            b"Apairemant",
            0x0100_0000 | (u32::from(zone_id) << 12) | 10,
        );
        NpcNameTable::from_bytes(zone_id, buf)
    }

    fn write_record(buf: &mut [u8], slot: usize, name: &[u8], id: u32) {
        let off = slot * RECORD_SIZE;
        buf[off..off + name.len()].copy_from_slice(name);

        buf[off + NAME_LEN..off + RECORD_SIZE].copy_from_slice(&id.to_le_bytes());
    }

    #[test]
    fn split_id_extracts_zone_and_slot() {
        assert_eq!(split_id(17_719_306), Some((230, 10)));

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
        let t = synth_table(230);
        assert_eq!(t.lookup_by_slot(0), None);
    }

    #[test]
    fn lookup_by_slot_returns_none_for_empty_record() {
        let t = synth_table(230);

        assert_eq!(t.lookup_by_slot(2), None);
    }

    #[test]
    fn lookup_by_slot_returns_none_for_out_of_range_slot() {
        let t = synth_table(230);
        assert_eq!(t.lookup_by_slot(999), None);
    }

    #[test]
    fn lookup_by_slot_rejects_non_ascii_name() {
        let mut buf = vec![0u8; 2 * RECORD_SIZE];
        write_record(&mut buf, 1, b"\x80valid?", 1);
        let t = NpcNameTable::from_bytes(230, buf);
        assert_eq!(t.lookup_by_slot(1), None);
    }

    #[test]
    fn lookup_by_slot_accepts_space_in_name() {
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
        let t = synth_table(230);

        let wrong_zone_id = 0x0100_0000 | (100u32 << 12) | 10;
        assert_eq!(t.lookup_by_id(wrong_zone_id), None);
    }

    #[test]
    fn lookup_by_id_rejects_ids_without_entity_marker() {
        let t = synth_table(230);

        assert_eq!(t.lookup_by_id(0x000E_600A), None);
    }
}
