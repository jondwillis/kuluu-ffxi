use std::collections::BTreeMap;
use std::path::Path;

use crate::chunk;

// research/xim SpellListSection.kt + DatResource.kt: the retail client reads its own
// spell table from ROM/118/114.DAT, a DAT container whose spell list lives in a
// section of type 0x49 (S49_SpellList). Each spell is a 0x64-byte block obfuscated
// with the per-block rotate scheme in research/xim BlockDecoder.kt.
pub const SPELL_DAT_ROM_PATH: &str = "ROM/118/114.DAT";

pub const SPELL_LIST_SECTION_KIND: u8 = 0x49;

pub const SPELL_BLOCK_SIZE: usize = 0x64;

// research/xim SpellInfo.toFrames: castTime/recastDelay are stored in units of 0.25s.
const CAST_UNIT_MS: u32 = 250;

const OFF_SPELL_ID: usize = 0x00;
const OFF_MAGIC_TYPE: usize = 0x02;
const OFF_CAST_TIME: usize = 0x0C;
const OFF_RECAST: usize = 0x0D;

// research/xim SpellListSection.kt MagicType — the client's own cast-animation class,
// distinct from the LSB magic *skill*. Enfeebling is split across White/Black here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagicType {
    None,
    WhiteMagic,
    BlackMagic,
    Summoning,
    Ninjutsu,
    Songs,
    BlueMagic,
    Geomancy,
    Trust,
}

impl MagicType {
    fn from_u16(v: u16) -> MagicType {
        match v {
            1 => MagicType::WhiteMagic,
            2 => MagicType::BlackMagic,
            3 => MagicType::Summoning,
            4 => MagicType::Ninjutsu,
            5 => MagicType::Songs,
            6 => MagicType::BlueMagic,
            7 => MagicType::Geomancy,
            8 => MagicType::Trust,
            _ => MagicType::None,
        }
    }

    // research/xim DatResource.kt::castSuffix — the cast-motion clip is "ca"+suffix.
    pub fn cast_suffix(self) -> Option<&'static str> {
        Some(match self {
            MagicType::None => return None,
            MagicType::WhiteMagic => "wh",
            MagicType::BlackMagic => "bk",
            MagicType::Summoning => "sm",
            MagicType::Ninjutsu => "nj",
            MagicType::Songs => "so",
            MagicType::BlueMagic => "bl",
            MagicType::Geomancy => "ge",
            MagicType::Trust => "fa",
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SpellStatic {
    pub magic_type: MagicType,
    pub cast_time_ms: u32,
    pub recast_ms: u32,
}

// research/xim BlockDecoder.kt: the rotate amount is chosen from the popcount of three
// key bytes (0x02, 0x0B, 0x0C), which are themselves left un-rotated.
fn decode_block(block: &mut [u8]) {
    let pop = |b: u8| b.count_ones() as i32;
    let factor = (pop(block[0x02]) - pop(block[0x0B]) + pop(block[0x0C])) % 5;
    let rotate = match factor {
        0 => 7,
        1 => 1,
        2 => 6,
        3 => 2,
        4 => 5,
        _ => 0,
    };
    for (i, b) in block.iter_mut().enumerate() {
        if i == 0x02 || i == 0x0B || i == 0x0C {
            continue;
        }
        *b = b.rotate_right(rotate);
    }
}

fn read_u16_le(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(off)?, *b.get(off + 1)?]))
}

pub fn parse_spell_table(dat_bytes: &[u8]) -> BTreeMap<u16, SpellStatic> {
    let mut table = BTreeMap::new();
    let Some(section) = chunk::walk(dat_bytes)
        .filter_map(|r| r.ok())
        .find(|c| c.kind == SPELL_LIST_SECTION_KIND)
    else {
        return table;
    };

    for raw in section.data.chunks_exact(SPELL_BLOCK_SIZE) {
        let mut block = raw.to_vec();
        decode_block(&mut block);
        let Some(spell_id) = read_u16_le(&block, OFF_SPELL_ID) else {
            continue;
        };
        if spell_id == 0 {
            continue;
        }
        let magic_type = MagicType::from_u16(read_u16_le(&block, OFF_MAGIC_TYPE).unwrap_or(0));
        let cast_time_ms = block[OFF_CAST_TIME] as u32 * CAST_UNIT_MS;
        let recast_ms = block[OFF_RECAST] as u32 * CAST_UNIT_MS;
        table.insert(
            spell_id,
            SpellStatic {
                magic_type,
                cast_time_ms,
                recast_ms,
            },
        );
    }
    table
}

/// The retail spell table, keyed by spell id. Empty if the DAT is missing/malformed,
/// so a partial install degrades to the LSB-derived fallback at the call site.
#[derive(Default)]
pub struct SpellTable {
    spells: BTreeMap<u16, SpellStatic>,
}

impl SpellTable {
    pub fn open(root_dir: &Path) -> SpellTable {
        let path = root_dir.join(SPELL_DAT_ROM_PATH);
        let Ok(bytes) = std::fs::read(&path) else {
            return SpellTable::default();
        };
        SpellTable {
            spells: parse_spell_table(&bytes),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.spells.is_empty()
    }

    pub fn lookup(&self, spell_id: u16) -> Option<SpellStatic> {
        self.spells.get(&spell_id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_type_suffixes_match_xim() {
        assert_eq!(MagicType::WhiteMagic.cast_suffix(), Some("wh"));
        assert_eq!(MagicType::BlackMagic.cast_suffix(), Some("bk"));
        assert_eq!(MagicType::Trust.cast_suffix(), Some("fa"));
        assert_eq!(MagicType::None.cast_suffix(), None);
    }

    // Round-trips a block through the exact rotate-left inverse of decode_block so the
    // decoder is pinned without needing the (asset-free) retail DAT.
    fn encode_block(fields: &[(usize, u8)]) -> Vec<u8> {
        let mut plain = vec![0u8; SPELL_BLOCK_SIZE];
        for &(off, v) in fields {
            plain[off] = v;
        }
        // Pick a rotate by seeding the three key bytes, then invert (rotate_left) the
        // non-key bytes so decode_block(rotate_right) reproduces `plain`.
        let pop = |b: u8| b.count_ones() as i32;
        let factor = (pop(plain[0x02]) - pop(plain[0x0B]) + pop(plain[0x0C])) % 5;
        let rotate = match factor {
            0 => 7,
            1 => 1,
            2 => 6,
            3 => 2,
            4 => 5,
            _ => 0,
        };
        let mut enc = plain.clone();
        for (i, b) in enc.iter_mut().enumerate() {
            if i == 0x02 || i == 0x0B || i == 0x0C {
                continue;
            }
            *b = b.rotate_left(rotate);
        }
        enc
    }

    #[test]
    fn decode_block_round_trips_fields() {
        // spell id 220 (poison), magicType Black(2), castTime 4 units (1000ms),
        // recast 20 units (5000ms).
        let enc = encode_block(&[
            (OFF_SPELL_ID, 220),
            (OFF_MAGIC_TYPE, 2),
            (OFF_CAST_TIME, 4),
            (OFF_RECAST, 20),
        ]);
        let mut block = enc.clone();
        decode_block(&mut block);
        assert_eq!(read_u16_le(&block, OFF_SPELL_ID), Some(220));
        assert_eq!(read_u16_le(&block, OFF_MAGIC_TYPE), Some(2));
        assert_eq!(block[OFF_CAST_TIME], 4);
        assert_eq!(block[OFF_RECAST], 20);
    }
}
