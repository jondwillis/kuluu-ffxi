use std::path::Path;

use crate::{DatError, Result};

// research/xim MainDll.kt — table offsets are located by scanning FFXiMain.dll for a known
// big-endian marker word, starting at 0x30000. The marker bytes ARE the first entries of the
// table, so the matched position is used directly as the table base; per-race entries are
// little-endian u16 at base + race_index * 2.
const SCAN_START: usize = 0x30000;
const SCAN_WORDS: usize = 0xC000;

const WEAPON_SKILL_HINT: u32 = 0xCB81_CB81;
const DANCE_SKILL_HINT: u32 = 0xB9E2_B9E2;

// research/xim ZoneMapTable.kt
const ZONE_MAP_HINT: u64 = 0x6400_0001_0001_0100;
const ZONE_MAP_STRIDE: usize = 0x0E;
const ZONE_MAP_NEXT_DIVISOR: usize = 0x13;
const ZONE_MAP_SIZE_NUMERATOR: u16 = 2560;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneMapRecord {
    pub zone_id: u16,
    pub sub_zone_id: u8,
    pub size: u16,
    pub x_offset: i16,
    pub y_offset: i16,
}

pub struct MainDll {
    bytes: Vec<u8>,
    weapon_skill_base: usize,
    dance_skill_base: usize,
    zone_map_base: Option<usize>,
}

impl MainDll {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join("FFXiMain.dll");
        let bytes = std::fs::read(&path).map_err(|source| DatError::Io {
            path: path.clone(),
            source,
        })?;
        let weapon_skill_base =
            find_offset(&bytes, WEAPON_SKILL_HINT).ok_or(DatError::DllMarkerNotFound {
                hint: WEAPON_SKILL_HINT,
            })?;
        let dance_skill_base =
            find_offset(&bytes, DANCE_SKILL_HINT).ok_or(DatError::DllMarkerNotFound {
                hint: DANCE_SKILL_HINT,
            })?;
        let zone_map_base = find_offset_u64(&bytes, ZONE_MAP_HINT);
        Ok(Self {
            bytes,
            weapon_skill_base,
            dance_skill_base,
            zone_map_base,
        })
    }

    pub fn zone_map(&self, zone_id: u16, sub_zone_id: u8) -> Option<ZoneMapRecord> {
        let mut base = self.zone_map_base?;
        loop {
            let rec = self.bytes.get(base..base + ZONE_MAP_STRIDE)?;
            let zid = u16::from_le_bytes([rec[0], rec[1]]);
            let divisor = rec[5];
            if zid == zone_id && rec[2] == sub_zone_id {
                if divisor == 0 {
                    return None;
                }
                return Some(ZoneMapRecord {
                    zone_id: zid,
                    sub_zone_id: rec[2],
                    size: ZONE_MAP_SIZE_NUMERATOR / divisor as u16,
                    x_offset: i16::from_le_bytes([rec[10], rec[11]]),
                    y_offset: i16::from_le_bytes([rec[12], rec[13]]),
                });
            }
            match self.bytes.get(base + ZONE_MAP_NEXT_DIVISOR) {
                Some(0) | None => return None,
                Some(_) => base += ZONE_MAP_STRIDE,
            }
        }
    }

    pub fn base_weapon_skill_index(&self, race_index: u8) -> Option<u16> {
        self.read16(self.weapon_skill_base + race_index as usize * 2)
    }

    pub fn base_dance_skill_index(&self, race_index: u8) -> Option<u16> {
        self.read16(self.dance_skill_base + race_index as usize * 2)
    }

    fn read16(&self, off: usize) -> Option<u16> {
        let b = self.bytes.get(off..off + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }
}

fn find_offset(bytes: &[u8], hint: u32) -> Option<usize> {
    let mut pos = SCAN_START;
    for _ in 0..SCAN_WORDS {
        let b = bytes.get(pos..pos + 4)?;
        if u32::from_be_bytes([b[0], b[1], b[2], b[3]]) == hint {
            return Some(pos);
        }
        pos += 4;
    }
    None
}

fn find_offset_u64(bytes: &[u8], hint: u64) -> Option<usize> {
    let mut pos = SCAN_START;
    for _ in 0..SCAN_WORDS {
        let b = bytes.get(pos..pos + 8)?;
        let word = u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
        if word == hint {
            return Some(pos);
        }
        pos += 4;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_offset_matches_big_endian_marker_word_aligned() {
        let mut bytes = vec![0u8; SCAN_START + 0x40];
        let at = SCAN_START + 0x20;
        bytes[at..at + 4].copy_from_slice(&0xCB81_CB81u32.to_be_bytes());
        assert_eq!(find_offset(&bytes, WEAPON_SKILL_HINT), Some(at));
    }

    #[test]
    fn find_offset_none_when_absent() {
        let bytes = vec![0u8; SCAN_START + 0x40];
        assert_eq!(find_offset(&bytes, WEAPON_SKILL_HINT), None);
    }

    #[test]
    fn read16_is_little_endian_per_race() {
        let mut bytes = vec![0u8; SCAN_START + 0x40];
        let base = SCAN_START + 0x20;
        bytes[base..base + 4].copy_from_slice(&WEAPON_SKILL_HINT.to_be_bytes());
        // race_index 1 -> base + 2
        bytes[base + 2] = 0x34;
        bytes[base + 3] = 0x12;
        let dll = MainDll {
            bytes,
            weapon_skill_base: base,
            dance_skill_base: base,
            zone_map_base: None,
        };
        assert_eq!(dll.base_weapon_skill_index(1), Some(0x1234));
    }

    #[test]
    fn zone_map_parses_record_and_stops_at_zero_divisor() {
        let base = 0usize;
        let mut bytes = vec![0u8; 64];
        bytes[0..2].copy_from_slice(&100u16.to_le_bytes());
        bytes[2] = 0;
        bytes[5] = 5;
        bytes[10..12].copy_from_slice(&10i16.to_le_bytes());
        bytes[12..14].copy_from_slice(&(-20i16).to_le_bytes());
        bytes[base + ZONE_MAP_NEXT_DIVISOR] = 1;
        let r1 = ZONE_MAP_STRIDE;
        bytes[r1..r1 + 2].copy_from_slice(&230u16.to_le_bytes());
        bytes[r1 + 5] = 8;

        let dll = MainDll {
            bytes,
            weapon_skill_base: 0,
            dance_skill_base: 0,
            zone_map_base: Some(base),
        };
        let rec = dll.zone_map(100, 0).expect("zone 100 record");
        assert_eq!(rec.size, 512);
        assert_eq!((rec.x_offset, rec.y_offset), (10, -20));
        assert_eq!(dll.zone_map(230, 0).map(|r| r.size), Some(320));
        assert_eq!(dll.zone_map(999, 0), None);
    }
}
