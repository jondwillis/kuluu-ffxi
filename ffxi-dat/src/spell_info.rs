use std::collections::BTreeMap;
use std::path::Path;

use crate::archive::DatRoot;
use crate::chunk;

// research/xim SpellListSection.kt + DatResource.kt: the retail client reads its own
// spell table from a DAT container whose spell list lives in a section of type
// 0x49 (S49_SpellList). Each spell is a 0x64-byte block obfuscated with the
// per-block rotate scheme in research/xim BlockDecoder.kt.

/// VTABLE/FTABLE file id of the spell table.
pub const SPELL_LIST_FILE_ID: u32 = 81;

/// Where [`SPELL_LIST_FILE_ID`] resolved on the horizonxi-2023 and
/// retail-2026-09 [`crate::client_profile::KNOWN_CLIENTS`] rows; the fallback
/// for a root whose VTABLE/FTABLE cannot place the id, pinned against every
/// install by `ffxi-dat/tests/fixed_dat_ids.rs`.
const SPELL_DAT_ERA_ROM_PATH: &str = "ROM/118/114.DAT";

pub const SPELL_LIST_SECTION_KIND: u8 = 0x49;

pub const SPELL_LIST_CHUNK_NAME: [u8; 4] = *b"mgc_";

pub const SPELL_BLOCK_SIZE: usize = 0x64;

// research/xim SpellListSection.kt SpellInfo.toFrames: castTime/recastDelay are stored
// in units of 0.25s.
const CAST_UNIT_MS: u32 = 250;

// The word at 0x00 is both the block's position and the LSB spell id the client's cast
// lookup keys on; the word at 0x3E (xim's "id") is a per-build menu ordinal that differs
// between horizonxi-2023 and retail-2026-09, so it is not the key.
const OFF_SPELL_ID: usize = 0x00;
const OFF_MAGIC_TYPE: usize = 0x02;
const OFF_CAST_TIME: usize = 0x0C;
const OFF_RECAST: usize = 0x0D;
const ROTATE_KEY_BYTES: [usize; 3] = [0x02, 0x0B, 0x0C];

// research/xim SpellListSection.kt MagicType — the client's own cast-animation class,
// distinct from the LSB magic *skill*. Enfeebling is split across White/Black here.
// Not LSB's SPELLGROUP order (vendor/server/src/map/spell.h SPELLGROUP): only Black,
// Ninjutsu, Geomancy and Trust share a value.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpellStatic {
    pub magic_type: MagicType,
    pub cast_time_ms: u32,
    pub recast_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpellTableError {
    #[error("no kind 0x{SPELL_LIST_SECTION_KIND:02X} chunk named {}", String::from_utf8_lossy(&SPELL_LIST_CHUNK_NAME))]
    SectionMissing,
    #[error("spell list data is {len} bytes, not a multiple of {SPELL_BLOCK_SIZE}")]
    UnalignedData { len: usize },
    #[error("block {position} decodes to spell id {spell_id}, not its own position")]
    IndexMismatch { position: usize, spell_id: u16 },
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
        if ROTATE_KEY_BYTES.contains(&i) {
            continue;
        }
        *b = b.rotate_right(rotate);
    }
}

fn read_u16_le(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(off)?, *b.get(off + 1)?]))
}

fn is_spell_list_chunk(c: &chunk::Chunk<'_>) -> bool {
    c.kind == SPELL_LIST_SECTION_KIND && c.name == SPELL_LIST_CHUNK_NAME
}

pub fn parse_spell_table(dat_bytes: &[u8]) -> Result<BTreeMap<u16, SpellStatic>, SpellTableError> {
    let section = chunk::walk(dat_bytes)
        .filter_map(|r| r.ok())
        .find(is_spell_list_chunk)
        .ok_or(SpellTableError::SectionMissing)?;
    if section.data.len() % SPELL_BLOCK_SIZE != 0 {
        return Err(SpellTableError::UnalignedData {
            len: section.data.len(),
        });
    }

    let mut table = BTreeMap::new();
    for (position, raw) in section.data.chunks_exact(SPELL_BLOCK_SIZE).enumerate() {
        let mut block = raw.to_vec();
        decode_block(&mut block);
        let spell_id = read_u16_le(&block, OFF_SPELL_ID).unwrap_or(0);
        if spell_id == 0 {
            continue;
        }
        if usize::from(spell_id) != position {
            return Err(SpellTableError::IndexMismatch { position, spell_id });
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
    Ok(table)
}

/// The retail spell table, keyed by spell id. Empty if the DAT is missing/malformed,
/// so a partial install degrades to the LSB-derived fallback at the call site.
#[derive(Default)]
pub struct SpellTable {
    spells: BTreeMap<u16, SpellStatic>,
}

impl SpellTable {
    /// Overlay-aware: resolves [`SPELL_LIST_FILE_ID`] through the install's
    /// VTABLE/FTABLE, falling back to the era ROM path only when the tables
    /// cannot place it.
    pub fn open_from_root(root: &DatRoot) -> SpellTable {
        let path = match root.resolve(SPELL_LIST_FILE_ID) {
            Ok(loc) => loc.path_under(root),
            Err(e) => {
                let fallback = root.root().join(SPELL_DAT_ERA_ROM_PATH);
                eprintln!(
                    "spell table: file id {SPELL_LIST_FILE_ID} unresolved under {} ({e}); using {}",
                    root.root().display(),
                    fallback.display()
                );
                fallback
            }
        };
        Self::open_path(&path)
    }

    /// For a caller holding only a path: the install's own tables place the id,
    /// and a directory carrying none (a synthetic fixture) falls back to the
    /// era ROM path.
    pub fn open(root_dir: &Path) -> SpellTable {
        match DatRoot::open(root_dir) {
            Ok(root) => Self::open_from_root(&root),
            Err(_) => Self::open_path(&root_dir.join(SPELL_DAT_ERA_ROM_PATH)),
        }
    }

    fn open_path(path: &Path) -> SpellTable {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("spell table: {} unreadable ({e})", path.display());
                return SpellTable::default();
            }
        };
        match parse_spell_table(&bytes) {
            Ok(spells) => SpellTable { spells },
            Err(e) => {
                eprintln!("spell table: {} rejected ({e})", path.display());
                SpellTable::default()
            }
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
    use crate::chunk::CHUNK_KIND_MASK;
    use crate::ftable::SubPath;

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
            if ROTATE_KEY_BYTES.contains(&i) {
                continue;
            }
            *b = b.rotate_left(rotate);
        }
        enc
    }

    /// Retail chunk walker (crate::chunk::ChunkWalker): a 16-byte header whose size
    /// field counts 16-byte units; only the name word and the kind/size word are used.
    const CHUNK_HEADER_BYTES: usize = 16;
    const CHUNK_HEADER_USED_BYTES: usize = SPELL_LIST_CHUNK_NAME.len() + size_of::<u32>();

    fn synth_chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = CHUNK_HEADER_BYTES + body.len();
        let padded_total = total.div_ceil(CHUNK_HEADER_BYTES) * CHUNK_HEADER_BYTES;
        let size_units = (padded_total / CHUNK_HEADER_BYTES) as u32;
        let value = (size_units << 7) | (kind as u32 & CHUNK_KIND_MASK);
        let mut out = Vec::with_capacity(padded_total);
        out.extend_from_slice(name);
        out.extend_from_slice(&value.to_le_bytes());
        out.extend(std::iter::repeat_n(
            0u8,
            CHUNK_HEADER_BYTES - CHUNK_HEADER_USED_BYTES,
        ));
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(0u8, padded_total - total));
        out
    }

    fn encode_spell(position: u16, magic_type: u8, cast: u8, recast: u8) -> Vec<u8> {
        let [lo, hi] = position.to_le_bytes();
        encode_block(&[
            (OFF_SPELL_ID, lo),
            (OFF_SPELL_ID + 1, hi),
            (OFF_MAGIC_TYPE, magic_type),
            (OFF_CAST_TIME, cast),
            (OFF_RECAST, recast),
        ])
    }

    /// The body is kept a whole number of header-sized units so chunk padding stays
    /// out of the block grid.
    const SYNTH_BLOCKS: usize = 4;
    const _: () = assert!((SYNTH_BLOCKS * SPELL_BLOCK_SIZE).is_multiple_of(CHUNK_HEADER_BYTES));

    fn synth_spell_list(blocks: &[Vec<u8>]) -> Vec<u8> {
        assert_eq!(blocks.len(), SYNTH_BLOCKS);
        let body: Vec<u8> = blocks.concat();
        synth_chunk(&SPELL_LIST_CHUNK_NAME, SPELL_LIST_SECTION_KIND, &body)
    }

    fn well_formed_blocks() -> Vec<Vec<u8>> {
        vec![
            vec![0u8; SPELL_BLOCK_SIZE],
            encode_spell(1, 1, 8, 20),
            encode_spell(2, 2, 2, 8),
            encode_spell(3, 8, 8, 40),
        ]
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

    #[test]
    fn parses_a_well_formed_synthetic_list() {
        let dat = synth_spell_list(&well_formed_blocks());
        let table = parse_spell_table(&dat).unwrap();
        assert_eq!(table.len(), 3);
        let s = table[&2];
        assert_eq!(s.magic_type, MagicType::BlackMagic);
        assert_eq!(s.cast_time_ms, 2 * CAST_UNIT_MS);
        assert_eq!(s.recast_ms, 8 * CAST_UNIT_MS);
        assert_eq!(table[&3].magic_type, MagicType::Trust);
    }

    #[test]
    fn rejects_a_block_whose_id_is_not_its_position() {
        let mut blocks = well_formed_blocks();
        blocks[2] = encode_spell(5, 2, 2, 8);
        let dat = synth_spell_list(&blocks);
        assert_eq!(
            parse_spell_table(&dat),
            Err(SpellTableError::IndexMismatch {
                position: 2,
                spell_id: 5
            })
        );
    }

    #[test]
    fn rejects_unaligned_section_data() {
        let body: Vec<u8> = well_formed_blocks().concat();
        let dat = synth_chunk(
            &SPELL_LIST_CHUNK_NAME,
            SPELL_LIST_SECTION_KIND,
            &body[..body.len() - CHUNK_HEADER_BYTES],
        );
        assert_eq!(
            parse_spell_table(&dat),
            Err(SpellTableError::UnalignedData {
                len: body.len() - CHUNK_HEADER_BYTES
            })
        );
    }

    #[test]
    fn requires_both_kind_and_name() {
        let body: Vec<u8> = well_formed_blocks().concat();
        let wrong_name = synth_chunk(b"xxxx", SPELL_LIST_SECTION_KIND, &body);
        assert_eq!(
            parse_spell_table(&wrong_name),
            Err(SpellTableError::SectionMissing)
        );
        let wrong_kind = synth_chunk(&SPELL_LIST_CHUNK_NAME, SPELL_LIST_SECTION_KIND + 1, &body);
        assert_eq!(
            parse_spell_table(&wrong_kind),
            Err(SpellTableError::SectionMissing)
        );
    }

    #[test]
    fn malformed_file_yields_an_empty_table() {
        let dir = tempfile::tempdir().unwrap();
        assert!(SpellTable::open(dir.path()).is_empty());
        let path = dir.path().join(SPELL_DAT_ERA_ROM_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut blocks = well_formed_blocks();
        blocks[1] = encode_spell(9, 1, 8, 20);
        std::fs::write(&path, synth_spell_list(&blocks)).unwrap();
        assert!(SpellTable::open(dir.path()).is_empty());
        std::fs::write(&path, synth_spell_list(&well_formed_blocks())).unwrap();
        assert!(!SpellTable::open(dir.path()).is_empty());
    }

    /// Measured identical on horizonxi-2023 and retail-2026-09: 1024 blocks, block 0 empty.
    const INSTALLED_BLOCK_COUNT: usize = 1024;

    // vendor/server/sql/spell_list.sql rows (spellid, castTime, recastTime); the magic
    // types are the client's own MagicType numbering, not the LSB group column.
    const CURE: u16 = 1;
    const FIRE: u16 = 144;
    const POISON: u16 = 220;
    const POISON_RECAST_MS: u32 = 5000;
    const UTSUSEMI_ICHI: u16 = 338;
    const GEO_REFRESH: u16 = 800;
    const SHANTOTTO: u16 = 896;
    // retail-2026-09 Fire is 2 units (500 ms), matching LSB castTime; horizonxi-2023
    // ships the era-accurate 8 units (2000 ms).
    const FIRE_CAST_MS_RETAIL: u32 = 500;
    const FIRE_CAST_MS_HORIZON: u32 = 2000;

    #[test]
    fn installed_spell_list_pins() {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let loc = root.resolve(SPELL_LIST_FILE_ID).unwrap();
        assert_eq!(loc.rom_dir, "ROM");
        assert_eq!(
            loc.sub_path,
            SubPath {
                dir: 118,
                file: 114
            }
        );

        let bytes = std::fs::read(loc.path_under(&root)).unwrap();
        let sections: Vec<_> = chunk::walk(&bytes)
            .filter_map(|r| r.ok())
            .filter(is_spell_list_chunk)
            .collect();
        assert_eq!(sections.len(), 1);
        assert_eq!(
            sections[0].data.len(),
            INSTALLED_BLOCK_COUNT * SPELL_BLOCK_SIZE
        );

        let table = SpellTable::open_from_root(&root);
        let poison = table.lookup(POISON).unwrap();
        assert_eq!(poison.magic_type, MagicType::BlackMagic);
        assert_eq!(poison.recast_ms, POISON_RECAST_MS);
        assert_eq!(
            table.lookup(CURE).unwrap().magic_type,
            MagicType::WhiteMagic
        );
        assert_eq!(
            table.lookup(SHANTOTTO).unwrap().magic_type,
            MagicType::Trust
        );
        assert_eq!(
            table.lookup(UTSUSEMI_ICHI).unwrap().magic_type,
            MagicType::Ninjutsu
        );
        assert_eq!(
            table.lookup(GEO_REFRESH).unwrap().magic_type,
            MagicType::Geomancy
        );

        let fire = table.lookup(FIRE).unwrap();
        match root.profile().name() {
            "horizonxi-2023" => assert_eq!(fire.cast_time_ms, FIRE_CAST_MS_HORIZON),
            "retail-2026-09" => assert_eq!(fire.cast_time_ms, FIRE_CAST_MS_RETAIL),
            other => eprintln!("skipping Fire cast-time pin: client profile {other} not measured"),
        }
    }
}
