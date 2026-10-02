use std::collections::BTreeMap;
use std::ops::Range;
use std::path::Path;

use crate::pol1::{self, SECTION_NAME_LEN};
use crate::{DatError, Result};

// research/xim MainDll.kt — table offsets are located by scanning FFXiMain.dll for a
// known big-endian marker word. The marker bytes ARE the first entries of the
// table, so the matched position is used directly as the table base; per-race
// entries are little-endian u16 at base + race_index * 2.
//
// Every table lives in the `.data` section: `.text` ships packed (its raw size
// is 0 in both KNOWN_CLIENTS horizonxi-2023 and retail-2026-09) and is only
// unpacked at load, so the file bytes of the code section hold no marker. The
// scan window is therefore the `.data` raw span from the PE section table; the
// fixed window below is the fallback for a file whose header does not parse.
pub const SCAN_START: usize = 0x30000;
pub const SCAN_WORDS: usize = 0xC000;
/// Every marker is 4-byte aligned in both builds, so the scan steps by a word.
const SCAN_STRIDE: usize = 4;

const DATA_SECTION_NAME: [u8; SECTION_NAME_LEN] = pol1::section_name(b".data");

pub const WEAPON_SKILL_HINT: u32 = 0xCB81_CB81;
pub const DANCE_SKILL_HINT: u32 = 0xB9E2_B9E2;
// research/xim MainDll.kt emoteAnimationOffsetHint.
pub const EMOTE_HINT: u32 = 0x4827_4827;
// research/xim MainDll.kt raceConfigLookupTableOffsetHint / actionAnimationFileTableOffsetHint.
pub const RACE_CONFIG_HINT: u32 = 0xA01B_A01B;
pub const ACTION_ANIM_HINT: u32 = 0xCB96_CB96;
// research/xim MainDll.kt battleAnimationFileTableOffsetHint.
pub const BATTLE_ANIM_HINT: u32 = 0xC825_C825;
// research/xim MainDll.kt equipmentLookupTableOffsetHint. Unlike the per-race u16
// tables the marker is the table's own first `(file_id, count)` pair rather than a
// repeated word: 0x1BA8 = 7080 is HumeM's face base, and the count's high half is 0.
pub const EQUIPMENT_HINT: u32 = 0xA81B_0000;

/// Per-race stride of the equipment lookup table, and the per-slot stride within
/// one race's block. research/xim resource/table/EquipmentModelTable.kt,
/// parseRaceGenderTable.
pub const EQUIPMENT_RACE_STRIDE: usize = 0x1B0;
pub const EQUIPMENT_SLOT_STRIDE: usize = 0x30;
/// Each slot row is six `(first_file_id, entry_count)` pairs; a zero file id ends it.
pub const EQUIPMENT_SLOT_BANDS: usize = 6;

/// The mount pose/movement clips a rider needs (`chi?`, `{n}un?`, …) live this far
/// past the race's action-animation base; the same distance in KNOWN_CLIENTS
/// horizonxi-2023 and retail-2026-09.
/// research/xim poc/Model.kt, PcModel.getMountAnimationResource.
pub const ACTION_ANIM_MOUNT_OFFSET: u16 = 0x05;

/// The fishing DAT, in the same block. It holds the `fsh0`..`fsh9` *routines*
/// (each naming the `fh0?`..`fhd?` motion clips that live alongside them) plus
/// the `hits`/`hitl` sweat routines s2c 0x038 SCHEDULOR triggers.
///
/// Measured identical on KNOWN_CLIENTS horizonxi-2023 and retail-2026-09 (race 1
/// action base 38603, fishing DAT 38604): 38604 is the only DAT in `base..base+8`
/// carrying `fsh*`. Pinned by `kuluu-render/tests/fishing_pose_clips.rs`.
pub const ACTION_ANIM_FISHING_OFFSET: u16 = 0x01;

/// The slash-command table: one 24-byte row per name the client accepts on the
/// input line. `char name[20]` (leading `/`, NUL-padded) then the command id and
/// a flag word. The run ends at the first row with an empty name, past which
/// sits a 12-byte `(handler, zero, command_id)` array — so the id, not the name,
/// is what the client dispatches on, and every row sharing an id is an alias.
///
/// KNOWN_CLIENTS retail-2026-09 RVA 0x0035_5408, horizonxi-2023 RVA
/// 0x0035_12b8; both in `.data`, whose raw bytes ship unpacked, so reading it
/// needs no POL1 decode. Located by the row pattern rather than by an address:
/// the table is a property of whichever client the user installed.
const COMMAND_STRIDE: usize = 24;
const COMMAND_NAME_LEN: usize = 20;
const COMMAND_ID_OFFSET: usize = 0x14;
const COMMAND_FLAGS_OFFSET: usize = 0x16;
const COMMAND_PREFIX: u8 = b'/';
/// Consecutive well-formed rows that identify the table. A shorter run risks a
/// stretch of unrelated `.data` that happens to hold one slash-led string.
const COMMAND_TABLE_MIN_RUN: usize = 8;

/// The motion-emote family. Nothing reads `CommandFlags::Flag4` in
/// research/XIClient, so this is a distributional decode and
/// `real_dll_emote_flag_partitions_the_table` is its whole justification.
const COMMAND_FLAG_EMOTE: u16 = 0x0004;
/// research/XIClient/src/XIClient/source/Game/Commands/CommandManager.cpp
/// CommandManager::CommandCalc suppresses the chat echo for any line starting
/// with a slash, then re-prints the expanded line when this bit is set.
const COMMAND_FLAG_ECHO_INPUT: u16 = 0x2000;

/// One row of the client's slash-command table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCommand {
    /// The word after the slash; the row stores it with the leading `/`.
    pub name: String,
    pub id: u16,
    /// A packed command class in the low byte, not independent predicates.
    /// Only the bits with an accessor below are decoded; read the raw word for
    /// the rest rather than expecting a name for them.
    pub flags: u16,
}

impl ClientCommand {
    /// A motion emote: `/wave`, `/dance2`, `/bell`. All 48 share one grammar,
    /// an optional `motion`/`text` keyword and an optional target.
    pub fn is_emote(&self) -> bool {
        self.flags & COMMAND_FLAG_EMOTE != 0
    }

    /// The client re-prints the typed line into the chat log before running the
    /// command. Only `/?` does.
    pub fn echoes_input(&self) -> bool {
        self.flags & COMMAND_FLAG_ECHO_INPUT != 0
    }
}

/// The client's slash-command table in row order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandTable {
    entries: Vec<ClientCommand>,
}

impl CommandTable {
    /// A table from rows a caller supplies rather than from an install — a
    /// server policy's extra commands, or a test that needs alias resolution
    /// without one.
    pub fn from_entries(entries: Vec<ClientCommand>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[ClientCommand] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Case-insensitive, matching how the client accepts `/Say`.
    pub fn id_for(&self, name: &str) -> Option<u16> {
        self.entries
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
            .map(|c| c.id)
    }

    /// The long form of a command: its first row. `/attack` precedes `/a`,
    /// `/shoot` precedes `/range` `/ra` `/throw`.
    pub fn canonical(&self, id: u16) -> Option<&str> {
        self.entries
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.name.as_str())
    }

    /// Every name the client accepts for one command, long form first.
    pub fn names_for(&self, id: u16) -> impl Iterator<Item = &str> {
        self.entries
            .iter()
            .filter(move |c| c.id == id)
            .map(|c| c.name.as_str())
    }

    pub fn ids(&self) -> impl Iterator<Item = u16> + '_ {
        let mut seen = std::collections::BTreeSet::new();
        self.entries
            .iter()
            .filter(move |c| seen.insert(c.id))
            .map(|c| c.id)
    }
}

// research/xim ZoneMapTable.kt
const ZONE_MAP_HINT: u64 = 0x6400_0001_0001_0100;
const ZONE_MAP_STRIDE: usize = 0x0E;
const ZONE_MAP_NEXT_DIVISOR: usize = 0x13;
const ZONE_MAP_SIZE_NUMERATOR: u16 = 2560;

/// The record's low nibble at byte 4 picks which file-table base its
/// `file_table_offset` counts from, per FFXiMain.dll's switch on
/// `byte4 & 0x0F` (KNOWN_CLIENTS retail-2026-09 RVA 0x1f85f0, horizonxi-2023
/// RVA 0x1f4d20): three inline immediates and no arm for any other nibble.
/// research/xim ZoneMapTable.kt getFileTableOffset reads index 2 as 0xD147;
/// only 0xD417 resolves all 148 index-2 rows through both installs' VTABLE
/// (0xD147 resolves 84 and aliases them onto the index-1 `m_2*` maps, while
/// 0xD417 lands on the disjoint `em_*`/`s?_*` set).
const ZONE_MAP_FILE_TABLE_BASES: [u32; 3] = [0x14C0, 0xD02F, 0xD417];
/// What that switch answers for a nibble without an arm: a whole file id,
/// `file_table_offset` not added. The map loader pre-seeds the same id as its
/// no-record default (KNOWN_CLIENTS retail-2026-09 RVA 0x1f838a, horizonxi-2023
/// RVA 0x1f4aba; research/xim ZoneMapTable.kt getFileTableOffset). No row of
/// either build takes this arm; the id resolves to ROM/18/105.DAT on both
/// installs.
const ZONE_MAP_FALLBACK_FILE_ID: u32 = 0x1592;
const ZONE_MAP_FILE_TABLE_BASE_MASK: u8 = 0x0F;
/// Byte 4's high nibble picks which key-item base signed byte 6 counts from,
/// naming the map key item the zone's map is gated on (KNOWN_CLIENTS
/// retail-2026-09 RVA 0x1f7480, horizonxi-2023 RVA 0x1f3bb0). Byte 6 == 0 or a
/// nibble without an arm answers "no key item"; a negative byte 6 answers
/// `ZONE_MAP_NEGATIVE_KEY_ITEM` whatever the nibble. Every row naming one lands
/// on a MAP_OF_* id of vendor/server/scripts/enum/key_item.lua on both builds.
const ZONE_MAP_KEY_ITEM_BASES: [u16; 3] = [0x180, 0x73F, 0x8FD];
const ZONE_MAP_KEY_ITEM_BASE_SHIFT: u8 = 4;
const ZONE_MAP_NEGATIVE_KEY_ITEM: u16 = 0x17F;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneMapRecord {
    pub zone_id: u16,
    pub sub_zone_id: u8,
    /// The map image's own DAT file id. Carrying it here is what lets a caller
    /// take the image and the calibration below from one row, instead of
    /// cross-referencing a table keyed on a different index.
    pub file_id: u32,
    /// The map key item this map is gated on, when the row names one.
    pub key_item: Option<u16>,
    pub size: u16,
    pub x_offset: i16,
    pub y_offset: i16,
}

pub struct MainDll {
    bytes: Vec<u8>,
    weapon_skill_base: usize,
    dance_skill_base: usize,
    emote_base: Option<usize>,
    zone_map_base: Option<usize>,
    race_config_base: Option<usize>,
    action_anim_base: Option<usize>,
    battle_anim_base: Option<usize>,
    equipment_base: Option<usize>,
    command_table_base: Option<usize>,
}

impl MainDll {
    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join("FFXiMain.dll");
        let bytes = std::fs::read(&path).map_err(|source| DatError::Io {
            path: path.clone(),
            source,
        })?;
        let window = scan_window(&bytes);
        let weapon_skill_base =
            find_offset(&bytes, &window, WEAPON_SKILL_HINT).ok_or(DatError::DllMarkerNotFound {
                hint: WEAPON_SKILL_HINT,
            })?;
        let dance_skill_base =
            find_offset(&bytes, &window, DANCE_SKILL_HINT).ok_or(DatError::DllMarkerNotFound {
                hint: DANCE_SKILL_HINT,
            })?;
        let emote_base = find_offset(&bytes, &window, EMOTE_HINT);
        let zone_map_base = find_offset_u64(&bytes, &window, ZONE_MAP_HINT);
        let race_config_base = find_offset(&bytes, &window, RACE_CONFIG_HINT);
        let action_anim_base = find_offset(&bytes, &window, ACTION_ANIM_HINT);
        let battle_anim_base = find_offset(&bytes, &window, BATTLE_ANIM_HINT);
        let equipment_base = find_offset(&bytes, &window, EQUIPMENT_HINT);
        let command_table_base = find_command_table(&bytes, &window);
        Ok(Self {
            bytes,
            weapon_skill_base,
            dance_skill_base,
            emote_base,
            zone_map_base,
            race_config_base,
            action_anim_base,
            battle_anim_base,
            equipment_base,
            command_table_base,
        })
    }

    /// Every slash command this client accepts. Empty when the table could not
    /// be located, which leaves a caller to fall back on the long forms it
    /// registers itself rather than losing the surface outright.
    pub fn commands(&self) -> CommandTable {
        let mut entries = Vec::new();
        let mut off = match self.command_table_base {
            Some(base) => base,
            None => return CommandTable::default(),
        };
        while let Some(entry) = command_record(&self.bytes, off) {
            entries.push(entry);
            off += COMMAND_STRIDE;
        }
        CommandTable { entries }
    }

    pub fn zone_map(&self, zone_id: u16, sub_zone_id: u8) -> Option<ZoneMapRecord> {
        self.zone_maps(zone_id)
            .into_iter()
            .find(|rec| rec.sub_zone_id == sub_zone_id)
    }

    /// Every map the zone ships, in table order. A quarter of the zones number
    /// their maps from 1, so callers must enumerate rather than assume a
    /// sub-zone 0 exists.
    pub fn zone_maps(&self, zone_id: u16) -> Vec<ZoneMapRecord> {
        let mut out = Vec::new();
        self.for_each_zone_map(|rec| {
            if rec.zone_id == zone_id {
                out.push(rec);
            }
        });
        out
    }

    /// How many maps each zone ships, ascending by zone id. One walk, so a
    /// caller that needs every zone's count (the Change Map list) does not
    /// re-walk the table per zone.
    ///
    /// Rows whose key is negative are skipped: the field is signed (research/xim
    /// ZoneMapTable.kt reads it with `next16Signed`) and xim only reaches those
    /// rows through a zone's `customDefinition.zoneMapId`, never through a zone
    /// id the server sends. Identical on KNOWN_CLIENTS horizonxi-2023 and
    /// retail-2026-09: they are the 0xFF07..0xFFFF band, 153 keys, none of
    /// them a zone.
    pub fn zone_map_counts(&self) -> BTreeMap<u16, usize> {
        let mut counts: BTreeMap<u16, usize> = BTreeMap::new();
        self.for_each_zone_map(|rec| {
            if rec.zone_id as i16 >= 0 {
                *counts.entry(rec.zone_id).or_default() += 1;
            }
        });
        counts
    }

    /// The zone-map table is a flat run of records ending at the first zero
    /// `has_next` byte, so every read of it costs the same walk (research/xim
    /// src/jsMain/kotlin/xim/resource/table/ZoneMapTable.kt, `parse`).
    fn for_each_zone_map(&self, mut f: impl FnMut(ZoneMapRecord)) {
        let Some(mut base) = self.zone_map_base else {
            return;
        };
        loop {
            let Some(rec) = self.bytes.get(base..base + ZONE_MAP_STRIDE) else {
                return;
            };
            if let Some(parsed) = parse_zone_map(rec) {
                f(parsed);
            }
            match self.bytes.get(base + ZONE_MAP_NEXT_DIVISOR) {
                Some(0) | None => return,
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

    /// First emote-animation file id for a race (the look race byte, HumeM=1);
    /// research/xim MainDll.kt getBaseEmoteAnimationIndex.
    pub fn base_emote_index(&self, race_index: u8) -> Option<u16> {
        self.read16(self.emote_base? + race_index as usize * 2)
    }

    /// The race's config DAT — skeleton plus the shared idle/walk/run clips. The
    /// two companion motion DATs sit at fixed offsets past it. `race_index` is the
    /// look race byte (HumeM=1), which also reaches the non-playable configs the
    /// look byte never carries: 32..=36 are the ridden chocobo, one per colour
    /// (research/xim poc/Model.kt, RaceGenderConfig and PcModelLoader.preload).
    pub fn base_race_config_index(&self, race_index: u8) -> Option<u16> {
        self.read16(self.race_config_base? + race_index as usize * 2)
    }

    /// First file of the race's action-animation block; see
    /// [`ACTION_ANIM_MOUNT_OFFSET`]. research/xim MainDll.kt,
    /// getActionAnimationIndex.
    pub fn base_action_animation_index(&self, race_index: u8) -> Option<u16> {
        self.read16(self.action_anim_base? + race_index as usize * 2)
    }

    /// First file of the race's battle-animation block (the per-weapon-type
    /// engaged stances and attack motions). research/xim MainDll.kt,
    /// getBaseBattleAnimationIndex.
    pub fn base_battle_animation_index(&self, race_index: u8) -> Option<u16> {
        self.read16(self.battle_anim_base? + race_index as usize * 2)
    }

    /// Model DAT for one equipment slot of one race. `table_index` is the race's
    /// *equipment* table row, which is not the race index for the non-playable
    /// configs (the chocobo's race 32 uses row 12; research/xim poc/Model.kt,
    /// RaceGenderConfig).
    /// `slot` is the retail slot number — 0 face, 1 head, 2 body, 3 hands,
    /// 4 legs, 5 feet, 6 main, 7 sub, 8 ranged.
    ///
    /// The row is a run of `(first_file_id, entry_count)` bands that partition the
    /// model id space in order, so a model id is located by walking bands and
    /// subtracting the counts already passed.
    /// research/xim resource/table/EquipmentModelTable.kt, getItemModelPath and
    /// parseRaceGenderTable.
    pub fn equipment_model_index(&self, table_index: u8, slot: u8, model_id: u16) -> Option<u32> {
        let row = self.equipment_base?
            + EQUIPMENT_RACE_STRIDE * (table_index.checked_sub(1)? as usize)
            + EQUIPMENT_SLOT_STRIDE * slot as usize;
        let mut passed = 0u32;
        for band in 0..EQUIPMENT_SLOT_BANDS {
            let first = self.read32(row + band * 8)?;
            let count = self.read32(row + band * 8 + 4)?;
            if first == 0 {
                continue;
            }
            if u32::from(model_id) < passed + count {
                return Some(first + u32::from(model_id) - passed);
            }
            passed += count;
        }
        None
    }

    fn read16(&self, off: usize) -> Option<u16> {
        let b = self.bytes.get(off..off + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read32(&self, off: usize) -> Option<u32> {
        let b = self.bytes.get(off..off + 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// The `.data` section's raw file span, clipped to the file, from the PE
/// section table; `None` when the image has no parseable section table or
/// no `.data` section.
fn data_section_span(bytes: &[u8]) -> Option<Range<usize>> {
    let data = pol1::find_section(bytes, &DATA_SECTION_NAME)?;
    let ptr = data.pointer_to_raw_data as usize;
    let end = ptr
        .checked_add(data.size_of_raw_data as usize)?
        .min(bytes.len());
    (ptr < end).then_some(ptr..end)
}

fn scan_window(bytes: &[u8]) -> Range<usize> {
    data_section_span(bytes).unwrap_or(SCAN_START..SCAN_START + SCAN_WORDS * SCAN_STRIDE)
}

fn find_offset(bytes: &[u8], window: &Range<usize>, hint: u32) -> Option<usize> {
    window.clone().step_by(SCAN_STRIDE).find(|&pos| {
        bytes
            .get(pos..pos + 4)
            .is_some_and(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) == hint)
    })
}

fn find_offset_u64(bytes: &[u8], window: &Range<usize>, hint: u64) -> Option<usize> {
    window.clone().step_by(SCAN_STRIDE).find(|&pos| {
        bytes.get(pos..pos + 8).is_some_and(|b| {
            u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) == hint
        })
    })
}

/// One `COMMAND_STRIDE`-byte row. `None` for anything that is not a command —
/// the empty name that ends the run, and any unrelated `.data` the locator
/// stepped over.
fn command_record(bytes: &[u8], off: usize) -> Option<ClientCommand> {
    let rec = bytes.get(off..off + COMMAND_STRIDE)?;
    let name = rec.get(..COMMAND_NAME_LEN)?;
    if name[0] != COMMAND_PREFIX {
        return None;
    }
    let len = name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(COMMAND_NAME_LEN);
    let word = name.get(1..len)?;
    if word.is_empty() || !word.iter().all(u8::is_ascii_graphic) {
        return None;
    }
    if name[len..].iter().any(|&b| b != 0) {
        return None;
    }
    Some(ClientCommand {
        name: std::str::from_utf8(word).ok()?.to_owned(),
        id: u16::from_le_bytes([rec[COMMAND_ID_OFFSET], rec[COMMAND_ID_OFFSET + 1]]),
        flags: u16::from_le_bytes([rec[COMMAND_FLAGS_OFFSET], rec[COMMAND_FLAGS_OFFSET + 1]]),
    })
}

/// First row of the command table, found by the only thing that survives a
/// client patch: a long run of rows in its shape. Rows are 4-byte aligned in
/// both known builds, and a position off by less than a stride lands mid-name,
/// where the leading-slash test fails.
fn find_command_table(bytes: &[u8], window: &Range<usize>) -> Option<usize> {
    let run = window.clone().step_by(SCAN_STRIDE).find(|&pos| {
        (0..COMMAND_TABLE_MIN_RUN)
            .all(|i| command_record(bytes, pos + i * COMMAND_STRIDE).is_some())
    })?;
    let mut base = run;
    while base >= window.start + COMMAND_STRIDE
        && command_record(bytes, base - COMMAND_STRIDE).is_some()
    {
        base -= COMMAND_STRIDE;
    }
    Some(base)
}

/// One `ZONE_MAP_STRIDE`-byte row. `None` when the divisor is 0, which is how
/// the table marks a zone that ships no drawable map.
fn parse_zone_map(rec: &[u8]) -> Option<ZoneMapRecord> {
    let divisor = rec[5];
    if divisor == 0 {
        return None;
    }
    let table_index = usize::from(rec[4] & ZONE_MAP_FILE_TABLE_BASE_MASK);
    let file_table_offset = i16::from_le_bytes([rec[8], rec[9]]);
    let file_id = match ZONE_MAP_FILE_TABLE_BASES.get(table_index) {
        Some(base) => base.wrapping_add_signed(i32::from(file_table_offset)),
        None => ZONE_MAP_FALLBACK_FILE_ID,
    };
    Some(ZoneMapRecord {
        zone_id: u16::from_le_bytes([rec[0], rec[1]]),
        sub_zone_id: rec[2],
        file_id,
        key_item: zone_map_key_item(rec[4] >> ZONE_MAP_KEY_ITEM_BASE_SHIFT, rec[6] as i8),
        size: ZONE_MAP_SIZE_NUMERATOR / u16::from(divisor),
        x_offset: i16::from_le_bytes([rec[10], rec[11]]),
        y_offset: i16::from_le_bytes([rec[12], rec[13]]),
    })
}

fn zone_map_key_item(high_nibble: u8, byte6: i8) -> Option<u16> {
    if byte6 < 0 {
        return Some(ZONE_MAP_NEGATIVE_KEY_ITEM);
    }
    if byte6 == 0 {
        return None;
    }
    let base = ZONE_MAP_KEY_ITEM_BASES.get(usize::from(high_nibble))?;
    Some(base + u16::from(byte6.unsigned_abs()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pol1::{
        PE_E_LFANEW_OFFSET, PE_NUMBER_OF_SECTIONS_OFFSET, PE_OPTIONAL_HEADER_OFFSET, PE_SIGNATURE,
        SECTION_HEADER_SIZE, SECTION_POINTER_TO_RAW_DATA_OFFSET, SECTION_SIZE_OF_RAW_DATA_OFFSET,
    };

    const FALLBACK_WINDOW: Range<usize> = SCAN_START..SCAN_START + SCAN_WORDS * SCAN_STRIDE;

    #[test]
    fn find_offset_matches_big_endian_marker_word_aligned() {
        let mut bytes = vec![0u8; SCAN_START + 0x40];
        let at = SCAN_START + 0x20;
        bytes[at..at + 4].copy_from_slice(&0xCB81_CB81u32.to_be_bytes());
        assert_eq!(
            find_offset(&bytes, &FALLBACK_WINDOW, WEAPON_SKILL_HINT),
            Some(at)
        );
    }

    #[test]
    fn find_offset_none_when_absent() {
        let bytes = vec![0u8; SCAN_START + 0x40];
        assert_eq!(
            find_offset(&bytes, &FALLBACK_WINDOW, WEAPON_SKILL_HINT),
            None
        );
    }

    #[test]
    fn scan_window_falls_back_when_there_is_no_pe_header() {
        assert_eq!(scan_window(&[0u8; 0x100]), FALLBACK_WINDOW);
        assert_eq!(scan_window(&[]), FALLBACK_WINDOW);
        let mut bytes = vec![0u8; 0x200];
        bytes[PE_E_LFANEW_OFFSET..PE_E_LFANEW_OFFSET + 4].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"NE\0\0");
        assert_eq!(scan_window(&bytes), FALLBACK_WINDOW);
    }

    /// A minimal image: DOS stub pointer, `PE\0\0`, a COFF header naming two
    /// sections with an empty optional header, then `.text` and `.data` rows.
    fn synthetic_pe(data_ptr: u32, data_raw: u32, total: usize) -> Vec<u8> {
        let pe = 0x80usize;
        let mut bytes = vec![0u8; total];
        bytes[PE_E_LFANEW_OFFSET..PE_E_LFANEW_OFFSET + 4]
            .copy_from_slice(&(pe as u32).to_le_bytes());
        bytes[pe..pe + PE_SIGNATURE.len()].copy_from_slice(PE_SIGNATURE);
        bytes[pe + PE_NUMBER_OF_SECTIONS_OFFSET..pe + PE_NUMBER_OF_SECTIONS_OFFSET + 2]
            .copy_from_slice(&2u16.to_le_bytes());
        let table = pe + PE_OPTIONAL_HEADER_OFFSET;
        bytes[table..table + SECTION_NAME_LEN].copy_from_slice(&pol1::section_name(b".text"));
        bytes[table + SECTION_POINTER_TO_RAW_DATA_OFFSET
            ..table + SECTION_POINTER_TO_RAW_DATA_OFFSET + 4]
            .copy_from_slice(&0x400u32.to_le_bytes());
        let data = table + SECTION_HEADER_SIZE;
        bytes[data..data + SECTION_NAME_LEN].copy_from_slice(&DATA_SECTION_NAME);
        bytes[data + SECTION_SIZE_OF_RAW_DATA_OFFSET..data + SECTION_SIZE_OF_RAW_DATA_OFFSET + 4]
            .copy_from_slice(&data_raw.to_le_bytes());
        bytes[data + SECTION_POINTER_TO_RAW_DATA_OFFSET
            ..data + SECTION_POINTER_TO_RAW_DATA_OFFSET + 4]
            .copy_from_slice(&data_ptr.to_le_bytes());
        bytes
    }

    #[test]
    fn scan_window_is_the_data_section_from_the_section_table() {
        let bytes = synthetic_pe(0x1000, 0x800, 0x2000);
        assert_eq!(scan_window(&bytes), 0x1000..0x1800);
    }

    #[test]
    fn scan_window_clips_a_data_section_that_overruns_the_file() {
        let bytes = synthetic_pe(0x1000, 0x8000, 0x1400);
        assert_eq!(scan_window(&bytes), 0x1000..0x1400);
        let bytes = synthetic_pe(0x4000, 0x100, 0x1400);
        assert_eq!(
            scan_window(&bytes),
            FALLBACK_WINDOW,
            "a section past EOF is no window"
        );
    }

    #[test]
    fn find_offset_ignores_a_marker_outside_the_data_section() {
        let mut bytes = synthetic_pe(0x1000, 0x800, 0x2000);
        let outside = 0x800;
        let inside = 0x1400;
        bytes[outside..outside + 4].copy_from_slice(&WEAPON_SKILL_HINT.to_be_bytes());
        bytes[inside..inside + 4].copy_from_slice(&WEAPON_SKILL_HINT.to_be_bytes());
        let window = scan_window(&bytes);
        assert_eq!(
            find_offset(&bytes, &window, WEAPON_SKILL_HINT),
            Some(inside)
        );
        let past = 0x1900;
        bytes[inside..inside + 4].fill(0);
        bytes[past..past + 4].copy_from_slice(&WEAPON_SKILL_HINT.to_be_bytes());
        assert_eq!(find_offset(&bytes, &window, WEAPON_SKILL_HINT), None);
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
            weapon_skill_base: base,
            dance_skill_base: base,
            emote_base: Some(base),
            race_config_base: Some(base),
            action_anim_base: Some(base),
            battle_anim_base: Some(base),
            ..blank(bytes)
        };
        assert_eq!(dll.base_weapon_skill_index(1), Some(0x1234));
        assert_eq!(dll.base_emote_index(1), Some(0x1234));
        assert_eq!(dll.base_race_config_index(1), Some(0x1234));
        assert_eq!(dll.base_action_animation_index(1), Some(0x1234));
        assert_eq!(dll.base_battle_animation_index(1), Some(0x1234));
    }

    #[test]
    fn missing_emote_marker_yields_none() {
        let dll = blank(vec![0u8; 4]);
        assert_eq!(dll.base_emote_index(1), None);
        assert_eq!(dll.base_race_config_index(1), None);
        assert_eq!(dll.base_action_animation_index(1), None);
        assert_eq!(dll.base_battle_animation_index(1), None);
        assert_eq!(dll.equipment_model_index(1, 0, 0), None);
    }

    /// One equipment slot row: `bands` written as the six `(first, count)` pairs
    /// the table stores, zero-padded.
    fn equipment_dll(table_index: u8, slot: u8, bands: &[(u32, u32)]) -> MainDll {
        let row = EQUIPMENT_RACE_STRIDE * usize::from(table_index - 1)
            + EQUIPMENT_SLOT_STRIDE * usize::from(slot);
        let mut bytes = vec![0u8; row + EQUIPMENT_SLOT_STRIDE + EQUIPMENT_RACE_STRIDE];
        for (i, &(first, count)) in bands.iter().enumerate() {
            let at = row + i * 8;
            bytes[at..at + 4].copy_from_slice(&first.to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&count.to_le_bytes());
        }
        MainDll {
            equipment_base: Some(0),
            ..blank(bytes)
        }
    }

    #[test]
    fn equipment_bands_partition_the_model_id_space_in_order() {
        // HumeM head, the first three bands of the retail table.
        let dll = equipment_dll(1, 1, &[(7112, 256), (63323, 48), (63371, 16)]);
        assert_eq!(dll.equipment_model_index(1, 1, 0), Some(7112));
        assert_eq!(dll.equipment_model_index(1, 1, 255), Some(7367));
        assert_eq!(dll.equipment_model_index(1, 1, 256), Some(63323));
        assert_eq!(dll.equipment_model_index(1, 1, 303), Some(63370));
        assert_eq!(dll.equipment_model_index(1, 1, 304), Some(63371));
        assert_eq!(dll.equipment_model_index(1, 1, 320), None);
    }

    #[test]
    fn equipment_zero_band_is_skipped_without_consuming_model_ids() {
        let dll = equipment_dll(1, 1, &[(7112, 4), (0, 99), (63323, 4)]);
        assert_eq!(dll.equipment_model_index(1, 1, 4), Some(63323));
    }

    fn blank(bytes: Vec<u8>) -> MainDll {
        MainDll {
            bytes,
            weapon_skill_base: 0,
            dance_skill_base: 0,
            emote_base: None,
            zone_map_base: None,
            race_config_base: None,
            action_anim_base: None,
            battle_anim_base: None,
            equipment_base: None,
            command_table_base: None,
        }
    }

    /// A synthetic command table: `rows` laid out exactly as the client stores
    /// them, then the empty-name row that ends the run.
    fn command_bytes(rows: &[(&str, u16, u16)]) -> Vec<u8> {
        let mut bytes = vec![0u8; (rows.len() + 1) * COMMAND_STRIDE];
        for (i, &(name, id, flags)) in rows.iter().enumerate() {
            let at = i * COMMAND_STRIDE;
            let slashed = format!("/{name}");
            bytes[at..at + slashed.len()].copy_from_slice(slashed.as_bytes());
            bytes[at + COMMAND_ID_OFFSET..at + COMMAND_ID_OFFSET + 2]
                .copy_from_slice(&id.to_le_bytes());
            bytes[at + COMMAND_FLAGS_OFFSET..at + COMMAND_FLAGS_OFFSET + 2]
                .copy_from_slice(&flags.to_le_bytes());
        }
        bytes
    }

    fn command_dll(rows: &[(&str, u16, u16)]) -> MainDll {
        MainDll {
            command_table_base: Some(0),
            ..blank(command_bytes(rows))
        }
    }

    #[test]
    fn command_rows_parse_and_stop_at_the_empty_name() {
        let dll = command_dll(&[("attack", 0x1d, 0x0141), ("a", 0x1d, 0x0141)]);
        let table = dll.commands();
        assert_eq!(table.len(), 2);
        assert_eq!(table.id_for("attack"), Some(0x1d));
        assert_eq!(
            table.id_for("ATTACK"),
            Some(0x1d),
            "the client accepts /Say"
        );
        assert_eq!(table.id_for("nosuchcommand"), None);
        assert_eq!(table.canonical(0x1d), Some("attack"));
        assert_eq!(
            table.names_for(0x1d).collect::<Vec<_>>(),
            vec!["attack", "a"],
            "long form first, aliases after"
        );
        assert_eq!(table.entries()[0].flags, 0x0141);
    }

    #[test]
    fn command_table_is_empty_when_it_was_not_located() {
        assert!(blank(vec![0u8; 0x40]).commands().is_empty());
    }

    #[test]
    fn a_name_filling_the_field_with_no_terminator_still_parses() {
        let full = "x".repeat(COMMAND_NAME_LEN - 1);
        let dll = command_dll(&[(full.as_str(), 0x8a, 0x0141)]);
        assert_eq!(dll.commands().id_for(&full), Some(0x8a));
    }

    #[test]
    fn a_row_is_rejected_unless_it_has_the_shape_of_a_command() {
        let good = command_bytes(&[("attack", 0x1d, 0x0141)]);
        assert!(command_record(&good, 0).is_some());

        let mut no_slash = good.clone();
        no_slash[0] = b'x';
        assert_eq!(command_record(&no_slash, 0), None);

        let mut trailing_junk = good.clone();
        trailing_junk[COMMAND_NAME_LEN - 1] = b'x';
        assert_eq!(
            command_record(&trailing_junk, 0),
            None,
            "the pad after the name is NUL, not arbitrary bytes"
        );

        let mut non_ascii = good.clone();
        non_ascii[2] = 0x80;
        assert_eq!(command_record(&non_ascii, 0), None);

        let bare_slash = command_bytes(&[("", 0x1d, 0)]);
        assert_eq!(
            command_record(&bare_slash, 0),
            None,
            "a slash is not a name"
        );
    }

    #[test]
    fn the_table_is_located_by_its_run_of_rows_and_walked_back_to_the_first() {
        let rows: Vec<(String, u16, u16)> = (0..COMMAND_TABLE_MIN_RUN + 4)
            .map(|i| (format!("cmd{i}"), i as u16, 0u16))
            .collect();
        let borrowed: Vec<(&str, u16, u16)> =
            rows.iter().map(|(n, i, f)| (n.as_str(), *i, *f)).collect();
        let table = command_bytes(&borrowed);

        let lead = 0x40usize;
        let mut bytes = vec![0u8; lead + table.len()];
        bytes[lead..lead + table.len()].copy_from_slice(&table);
        let window = 0..bytes.len();

        assert_eq!(
            find_command_table(&bytes, &window),
            Some(lead),
            "the walk-back reaches the first row, not the run it matched at"
        );
        let dll = MainDll {
            command_table_base: find_command_table(&bytes, &window),
            ..blank(bytes)
        };
        assert_eq!(dll.commands().len(), borrowed.len());
    }

    #[test]
    fn a_lone_slash_led_string_is_not_mistaken_for_the_table() {
        let short = command_bytes(&[("attack", 0x1d, 0), ("a", 0x1d, 0)]);
        let window = 0..short.len();
        assert_eq!(
            find_command_table(&short, &window),
            None,
            "two rows are not a table"
        );
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
            zone_map_base: Some(base),
            ..blank(bytes)
        };
        let rec = dll.zone_map(100, 0).expect("zone 100 record");
        assert_eq!(rec.size, 512);
        assert_eq!((rec.x_offset, rec.y_offset), (10, -20));
        assert_eq!(dll.zone_map(230, 0).map(|r| r.size), Some(320));
        assert_eq!(dll.zone_map(999, 0), None);
    }

    #[test]
    fn zone_map_file_table_base_follows_the_low_nibble() {
        for (nibble, base) in ZONE_MAP_FILE_TABLE_BASES.iter().enumerate() {
            let mut rec = [0u8; ZONE_MAP_STRIDE];
            rec[4] = nibble as u8;
            rec[5] = 4;
            rec[8..10].copy_from_slice(&7i16.to_le_bytes());
            assert_eq!(parse_zone_map(&rec).map(|r| r.file_id), Some(base + 7));
        }
        for nibble in ZONE_MAP_FILE_TABLE_BASES.len() as u8..=ZONE_MAP_FILE_TABLE_BASE_MASK {
            let mut rec = [0u8; ZONE_MAP_STRIDE];
            rec[4] = nibble;
            rec[5] = 4;
            rec[8..10].copy_from_slice(&7i16.to_le_bytes());
            assert_eq!(
                parse_zone_map(&rec).map(|r| r.file_id),
                Some(ZONE_MAP_FALLBACK_FILE_ID),
                "nibble {nibble} takes the fallback whole file id, offset ignored"
            );
        }
    }

    #[test]
    fn zone_map_key_item_follows_the_high_nibble_and_byte_6() {
        let key_item = |high: u8, byte6: u8| {
            let mut rec = [0u8; ZONE_MAP_STRIDE];
            rec[4] = high << ZONE_MAP_KEY_ITEM_BASE_SHIFT;
            rec[5] = 4;
            rec[6] = byte6;
            parse_zone_map(&rec).unwrap().key_item
        };
        for (high, base) in ZONE_MAP_KEY_ITEM_BASES.iter().enumerate() {
            assert_eq!(key_item(high as u8, 1), Some(base + 1));
        }
        assert_eq!(key_item(0, 0), None, "byte 6 == 0 names no key item");
        assert_eq!(
            key_item(3, 1),
            None,
            "a nibble without an arm names no key item"
        );
        assert_eq!(key_item(1, 0xFF), Some(ZONE_MAP_NEGATIVE_KEY_ITEM));
        assert_eq!(key_item(0, 0xFE), Some(ZONE_MAP_NEGATIVE_KEY_ITEM));
    }

    #[test]
    fn zone_map_counts_tallies_every_zone_and_skips_the_client_only_keys() {
        let mut bytes = vec![0u8; ZONE_MAP_STRIDE * 5];
        for (slot, zone, sub) in [
            (0usize, 238u16, 1u8),
            (1, 238, 2),
            (2, 100, 0),
            (3, 0xFF07, 0),
        ] {
            let at = slot * ZONE_MAP_STRIDE;
            bytes[at..at + 2].copy_from_slice(&zone.to_le_bytes());
            bytes[at + 2] = sub;
            bytes[at + 5] = 4;
            bytes[at + ZONE_MAP_NEXT_DIVISOR] = 1;
        }
        bytes[3 * ZONE_MAP_STRIDE + ZONE_MAP_NEXT_DIVISOR] = 0;
        let dll = MainDll {
            zone_map_base: Some(0),
            ..blank(bytes)
        };

        assert_eq!(
            dll.zone_map_counts(),
            BTreeMap::from([(238, 2), (100, 1)]),
            "one pass tallies each zone's rows, and the negative key is not a zone"
        );
        assert_eq!(dll.zone_map_counts().get(&999), None);
    }

    /// The install under test plus its DLL; `None` (after saying so) when no
    /// install is reachable.
    fn open_test_dll() -> Option<(crate::archive::DatRoot, MainDll)> {
        let Some(root) = crate::archive::open_test_install() else {
            eprintln!("skipping: no FFXI install");
            return None;
        };
        let dll = MainDll::load(root.root()).expect("FFXiMain.dll beside the install's VTABLE");
        Some((root, dll))
    }

    /// Gated on an install (self-skips). The Change Map list is built from
    /// `zone_map_counts`, so a count that disagrees with the per-zone walk the
    /// loader indexes would put a row on screen that previews blank.
    #[test]
    fn real_dll_zone_map_counts_agree_with_the_per_zone_walk() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };

        let counts = dll.zone_map_counts();
        assert!(!counts.is_empty(), "the retail table names zones");
        for (&zone, &count) in counts.iter() {
            assert_eq!(dll.zone_maps(zone).len(), count, "zone {zone}");
        }
        assert_eq!(counts.get(&238).copied(), Some(2), "Windurst Waters");
        assert!(
            counts.keys().all(|&zone| zone as i16 >= 0),
            "the 0xFF07.. band is keyed by client map ids, not zones"
        );
        assert_eq!(
            counts.get(&157).copied(),
            Some(6),
            "Middle Delkfutt's Tower, a zone POLUtils' map table omits"
        );
    }

    #[test]
    fn zone_maps_enumerates_a_zone_that_numbers_its_maps_from_one() {
        let mut bytes = vec![0u8; ZONE_MAP_STRIDE * 3];
        for (slot, sub) in [(0usize, 1u8), (1, 2)] {
            let at = slot * ZONE_MAP_STRIDE;
            bytes[at..at + 2].copy_from_slice(&238u16.to_le_bytes());
            bytes[at + 2] = sub;
            bytes[at + 5] = 4;
            bytes[at + 8..at + 10].copy_from_slice(&i16::from(sub).to_le_bytes());
            bytes[at + ZONE_MAP_NEXT_DIVISOR] = 1;
        }
        let dll = MainDll {
            zone_map_base: Some(0),
            ..blank(bytes)
        };

        assert_eq!(dll.zone_map(238, 0), None, "this zone has no sub-zone 0");
        let maps = dll.zone_maps(238);
        assert_eq!(
            maps.iter().map(|r| r.sub_zone_id).collect::<Vec<_>>(),
            vec![1, 2],
            "enumerating still finds both maps"
        );
        assert_eq!(
            maps.iter().map(|r| r.file_id).collect::<Vec<_>>(),
            vec![
                ZONE_MAP_FILE_TABLE_BASES[0] + 1,
                ZONE_MAP_FILE_TABLE_BASES[0] + 2
            ],
            "each record names its own map DAT"
        );
    }

    /// Gated on an install (self-skips). The defect this guards is a
    /// zone whose maps are numbered from 1 being looked up at sub-zone 0 and
    /// silently coming back empty.
    #[test]
    fn real_dll_zone_maps_cover_every_zone_that_ships_one() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };

        // Windurst Waters numbers its two maps 1 and 2.
        let waters = dll.zone_maps(238);
        assert_eq!(waters.len(), 2, "zone 238 ships two maps");
        assert!(
            dll.zone_map(238, 0).is_none(),
            "and none of them is sub-zone 0"
        );
        assert!(
            waters.iter().all(|r| r.file_id != 0 && r.size > 0),
            "each carries a usable file id and span"
        );

        // Across the whole table, enumerating never loses a zone that a
        // sub-zone-0 lookup would have found.
        let mut from_zero = 0usize;
        let mut enumerated = 0usize;
        for zone in 0..=u16::MAX {
            let maps = dll.zone_maps(zone);
            if !maps.is_empty() {
                enumerated += 1;
            }
            if dll.zone_map(zone, 0).is_some() {
                from_zero += 1;
                assert!(!maps.is_empty(), "zone {zone} regressed");
            }
        }
        assert!(
            enumerated > from_zero,
            "enumerating reaches more zones than a sub-zone-0 lookup ({enumerated} vs {from_zero})"
        );
    }

    /// Per-race table contents, identical on KNOWN_CLIENTS horizonxi-2023 and
    /// retail-2026-09. Each vector is races 1..=8 (HumeM..GalkaM); race 6 is
    /// the Tarutaru female slot that repeats the male's tables.
    const RACE_RANGE: std::ops::RangeInclusive<u8> = 1..=8;
    const WEAPON_SKILL_BY_RACE: [u16; 8] = [33227, 33995, 34763, 35531, 36299, 36299, 37067, 37835];
    const DANCE_SKILL_BY_RACE: [u16; 8] = [58041, 58425, 58809, 59193, 59577, 59577, 59961, 60345];
    const EMOTE_BY_RACE: [u16; 8] = [10056, 13232, 16408, 19584, 22760, 22984, 26160, 29336];
    const RACE_CONFIG_BY_RACE: [u16; 8] = [7072, 10248, 13424, 16600, 19776, 19776, 23176, 26352];
    const ACTION_ANIM_BY_RACE: [u16; 8] = [38603, 38699, 38795, 38891, 38987, 38987, 39083, 39179];
    const BATTLE_ANIM_BY_RACE: [u16; 8] = [9672, 12848, 16024, 19200, 22376, 22376, 25776, 28952];
    /// The ridden-chocobo configs, one per colour, at race indices 32..=36.
    const CHOCOBO_RACE_RANGE: std::ops::RangeInclusive<u8> = 32..=36;
    const RACE_CONFIG_BY_CHOCOBO: [u16; 5] = [55295, 55329, 55363, 55397, 55431];

    fn per_race(f: impl Fn(u8) -> Option<u16>, races: std::ops::RangeInclusive<u8>) -> Vec<u16> {
        races.map(|r| f(r).expect("race in table")).collect()
    }

    /// Gated on an install (self-skips).
    #[test]
    fn real_dll_per_race_tables_match_both_known_clients() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        assert_eq!(
            per_race(|r| dll.base_weapon_skill_index(r), RACE_RANGE),
            WEAPON_SKILL_BY_RACE
        );
        assert_eq!(
            per_race(|r| dll.base_dance_skill_index(r), RACE_RANGE),
            DANCE_SKILL_BY_RACE
        );
        assert_eq!(
            per_race(|r| dll.base_emote_index(r), RACE_RANGE),
            EMOTE_BY_RACE
        );
        assert_eq!(
            per_race(|r| dll.base_race_config_index(r), RACE_RANGE),
            RACE_CONFIG_BY_RACE
        );
        assert_eq!(
            per_race(|r| dll.base_race_config_index(r), CHOCOBO_RACE_RANGE),
            RACE_CONFIG_BY_CHOCOBO
        );
        assert_eq!(
            per_race(|r| dll.base_action_animation_index(r), RACE_RANGE),
            ACTION_ANIM_BY_RACE
        );
        assert_eq!(
            per_race(|r| dll.base_battle_animation_index(r), RACE_RANGE),
            BATTLE_ANIM_BY_RACE
        );
        assert_eq!(
            dll.base_action_animation_index(1)
                .map(|b| b + ACTION_ANIM_FISHING_OFFSET),
            Some(38604),
            "the fishing DAT the pose-clip test opens"
        );
    }

    /// Row and id totals of the slash-command table, identical on KNOWN_CLIENTS
    /// horizonxi-2023 and retail-2026-09: HorizonXI ships the table unpatched
    /// and puts its own commands elsewhere (server commands on `!` in chat
    /// text, client commands in Ashita addons that intercept the input line
    /// before the client sees it).
    const COMMAND_ROWS: usize = 277;
    const COMMAND_IDS: usize = 209;

    /// Gated on an install (self-skips).
    #[test]
    fn real_dll_command_table_matches_both_known_clients() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        let table = dll.commands();
        assert_eq!(table.len(), COMMAND_ROWS);
        assert_eq!(table.ids().count(), COMMAND_IDS);

        let mut names: Vec<&str> = table.entries().iter().map(|c| c.name.as_str()).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "no name is listed twice");
        assert!(
            table.entries().iter().all(|c| c
                .name
                .bytes()
                .all(|b| b.is_ascii_graphic() && !b.is_ascii_uppercase())),
            "every stored name is lowercase ASCII, so a lowercasing parser loses nothing"
        );
    }

    /// The complement of [`COMMAND_FLAG_EMOTE`]; every row carries one or the
    /// other, which is the whole evidence for what the emote bit means.
    const COMMAND_FLAG_STATEMENT: u16 = 0x0001;

    /// Gated on an install (self-skips). Carries the decode of
    /// [`COMMAND_FLAG_EMOTE`], which no reconstructed client code reads: the
    /// bit is trusted because it partitions the table against
    /// `COMMAND_FLAG_STATEMENT` and its side is exactly the emotes. A client
    /// build where that stops holding must fail loudly rather than mislabel.
    #[test]
    fn real_dll_emote_flag_partitions_the_table() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        let table = dll.commands();
        for entry in table.entries() {
            assert_ne!(
                entry.flags & COMMAND_FLAG_EMOTE != 0,
                entry.flags & COMMAND_FLAG_STATEMENT != 0,
                "/{} carries both or neither class bit (flags 0x{:04x})",
                entry.name,
                entry.flags,
            );
        }
        for word in ["wave", "bow", "dance2", "toss", "bell", "jobemote", "aim"] {
            let id = table.id_for(word).unwrap_or_else(|| panic!("/{word}"));
            let entry = table.entries().iter().find(|c| c.id == id).unwrap();
            assert!(entry.is_emote(), "/{word} is a motion emote");
        }
        for word in ["emote", "say", "magic", "equip", "logout"] {
            let id = table.id_for(word).unwrap_or_else(|| panic!("/{word}"));
            let entry = table.entries().iter().find(|c| c.id == id).unwrap();
            assert!(!entry.is_emote(), "/{word} is not a motion emote");
        }
        let echoes: Vec<&str> = table
            .entries()
            .iter()
            .filter(|c| c.echoes_input())
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(echoes, ["?"], "only /? re-prints its own line");
    }

    /// Gated on an install (self-skips). What makes the low byte a class field
    /// rather than independent predicates, held as asserts so a client build
    /// that breaks the structure says so instead of being quietly mis-read.
    #[test]
    fn real_dll_flag_bits_are_a_packed_class_not_predicates() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        let table = dll.commands();
        let carriers = |bit: u16| -> Vec<&str> {
            table
                .ids()
                .filter_map(|id| table.entries().iter().find(|c| c.id == id))
                .filter(|c| c.flags & bit != 0)
                .map(|c| c.name.as_str())
                .collect()
        };
        let disjoint = |a: u16, b: u16| carriers(a).iter().all(|n| !carriers(b).contains(n));
        let subset_of = |a: u16, b: u16| carriers(a).iter().all(|n| carriers(b).contains(n));

        const SUBTARGET_GUARD: u16 = 0x0002;
        const NAME_ARG: u16 = 0x0008;
        const MENU: u16 = 0x0010;
        const SETTING: u16 = 0x0020;
        const CLASS_HIGH: u16 = 0x0040;
        const FREE_TEXT: u16 = 0x0080;

        assert!(disjoint(COMMAND_FLAG_EMOTE, COMMAND_FLAG_STATEMENT));
        assert_eq!(
            carriers(COMMAND_FLAG_EMOTE).len() + carriers(COMMAND_FLAG_STATEMENT).len(),
            table.ids().count(),
            "the two class bits are exhaustive as well as exclusive"
        );
        assert!(disjoint(MENU, SETTING));
        for bit in [SUBTARGET_GUARD, COMMAND_FLAG_EMOTE, NAME_ARG, FREE_TEXT] {
            assert!(
                subset_of(bit, CLASS_HIGH),
                "0x{bit:04x} is not a subset of 0x{CLASS_HIGH:04x}"
            );
        }
        assert!(subset_of(NAME_ARG, MENU));
        for bit in [0x0200u16, 0x0400, 0x0800, 0x1000, 0x4000, 0x8000] {
            assert!(
                carriers(bit).is_empty(),
                "0x{bit:04x} is unused across the table"
            );
        }
    }

    /// Gated on an install (self-skips). The flags word is a property of the
    /// command, not of the typed name: no id's alias rows disagree. A caller
    /// may therefore read flags off any row of a group.
    #[test]
    fn real_dll_aliases_of_one_command_agree_on_flags() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        let table = dll.commands();
        for id in table.ids() {
            let mut rows = table.entries().iter().filter(|c| c.id == id);
            let first = rows.next().expect("an id came from a row");
            for other in rows {
                assert_eq!(
                    first.flags, other.flags,
                    "/{} and /{} share id 0x{id:04x} but not flags",
                    first.name, other.name,
                );
            }
        }
    }

    /// Gated on an install (self-skips). Alias groupings a name-keyed table
    /// gets wrong: LSB's emote enum has no Nod/Farewell/Upset at all, and
    /// FFXIclopedia lists /range and /throw as separate commands.
    #[test]
    fn real_dll_command_aliases_share_one_id() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        let table = dll.commands();
        for group in [
            &["attack", "a"][..],
            &["shoot", "range", "ra", "throw"],
            &["nod", "yes"],
            &["goodbye", "farewell"],
            &["disgusted", "upset"],
            &["makelinkshell", "makelinkpearl", "makeli"],
            &["supportdesk", "sd", "helpdesk"],
        ] {
            let canonical = group[0];
            let id = table
                .id_for(canonical)
                .unwrap_or_else(|| panic!("/{canonical} is a command"));
            assert_eq!(table.canonical(id), Some(canonical));
            assert_eq!(table.names_for(id).collect::<Vec<_>>(), group);
        }
        assert_eq!(
            table.id_for("ls"),
            None,
            "/l and /linkshell are the linkshell names; /ls is not one"
        );
        assert_ne!(
            table.id_for("help"),
            table.id_for("?"),
            "/help and /? are separate commands with separate handlers"
        );
    }

    /// Gated on an install (self-skips). `(table_index, slot, model_id)` cells
    /// of the equipment table, identical on both known clients.
    #[test]
    fn real_dll_equipment_cells_match_both_known_clients() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        for (table_index, slot, model_id, expected) in [
            (1u8, 0u8, 0u16, Some(7080u32)),
            (1, 1, 304, Some(63371)),
            (1, 6, 928, Some(107333)),
            (1, 6, 1196, None),
            (12, 2, 0, Some(55297)),
            (6, 0, 0, Some(22952)),
        ] {
            assert_eq!(
                dll.equipment_model_index(table_index, slot, model_id),
                expected,
                "row {table_index} slot {slot} model {model_id}"
            );
        }
    }

    /// Gated on an install (self-skips). Row and key totals of the zone-map
    /// table, identical on both known clients.
    #[test]
    fn real_dll_zone_map_totals_match_both_known_clients() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        let mut rows = 0usize;
        let mut negative = std::collections::BTreeSet::new();
        let mut nibbles = std::collections::BTreeSet::new();
        dll.for_each_zone_map(|rec| {
            rows += 1;
            if (rec.zone_id as i16) < 0 {
                negative.insert(rec.zone_id);
            }
        });
        for rec in zone_map_raw_rows(&dll) {
            if rec[5] != 0 && (u16::from_le_bytes([rec[0], rec[1]]) as i16) >= 0 {
                nibbles.insert(rec[4] & ZONE_MAP_FILE_TABLE_BASE_MASK);
            }
        }
        assert_eq!(rows, 829, "rows with a drawable map");
        assert_eq!(negative.len(), 153, "distinct client-only keys");
        let counts = dll.zone_map_counts();
        assert_eq!(counts.len(), 231, "zones with at least one map");
        assert_eq!(counts.get(&238).copied(), Some(2));
        assert_eq!(counts.get(&157).copied(), Some(6));
        assert_eq!(
            nibbles,
            std::collections::BTreeSet::from([0u8, 1]),
            "zone-keyed rows only ever pick the first two file-table bases"
        );
    }

    /// Gated on an install (self-skips). The key items the high nibble and
    /// byte 6 name are LSB's MAP_OF_THE_SAN_DORIA_AREA, MAP_OF_AL_ZAHBI and
    /// MAP_OF_RALA_WATERWAYS_U (vendor/server/scripts/enum/key_item.lua), one
    /// per key-item base; 655 rows name one on both known clients.
    #[test]
    fn real_dll_zone_map_key_items_are_the_lsb_map_key_items() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        for (zone, key_item) in [(100u16, 385u16), (48, 1856), (259, 2302)] {
            let maps = dll.zone_maps(zone);
            assert!(!maps.is_empty(), "zone {zone} ships a map");
            assert!(
                maps.iter().all(|r| r.key_item == Some(key_item)),
                "zone {zone}: {maps:?}"
            );
        }
        let mut named = 0usize;
        dll.for_each_zone_map(|rec| named += usize::from(rec.key_item.is_some()));
        assert_eq!(named, 655, "rows naming a map key item");
    }

    /// Raw rows of the zone-map table, walked with the same terminator as
    /// `for_each_zone_map`, for tests that need the file-table nibble the
    /// parsed record does not carry.
    fn zone_map_raw_rows(dll: &MainDll) -> Vec<&[u8]> {
        let mut rows = Vec::new();
        let Some(mut base) = dll.zone_map_base else {
            return rows;
        };
        while let Some(rec) = dll.bytes.get(base..base + ZONE_MAP_STRIDE) {
            rows.push(rec);
            match dll.bytes.get(base + ZONE_MAP_NEXT_DIVISOR) {
                Some(0) | None => break,
                Some(_) => base += ZONE_MAP_STRIDE,
            }
        }
        rows
    }

    /// Gated on an install (self-skips). Every record's file id, zone-keyed or
    /// client-only, is a DAT the install's VTABLE knows and ships; the tally
    /// pins the per-base split, including the 148 client-only rows on base
    /// index 2 — the set that separates the index-2 base from its digit
    /// transposition (see `ZONE_MAP_FILE_TABLE_BASES`).
    #[test]
    fn real_dll_every_zone_map_resolves_through_the_install() {
        let Some((root, dll)) = open_test_dll() else {
            return;
        };
        let mut tally: BTreeMap<(bool, u8), (usize, usize)> = BTreeMap::new();
        let mut unresolved_zone_keyed = Vec::new();
        let mut unresolved_client_only = std::collections::BTreeSet::new();
        for raw in zone_map_raw_rows(&dll) {
            let Some(rec) = parse_zone_map(raw) else {
                continue;
            };
            let zone_keyed = rec.zone_id as i16 >= 0;
            let nibble = raw[4] & ZONE_MAP_FILE_TABLE_BASE_MASK;
            let entry = tally.entry((zone_keyed, nibble)).or_default();
            entry.0 += 1;
            if root
                .resolve(rec.file_id)
                .is_ok_and(|loc| loc.path_under(&root).is_file())
            {
                entry.1 += 1;
            } else if zone_keyed {
                unresolved_zone_keyed.push(rec);
            } else {
                unresolved_client_only.insert(rec.file_id);
            }
        }
        assert!(
            unresolved_zone_keyed.is_empty(),
            "unresolved zone-keyed map DATs: {unresolved_zone_keyed:?}"
        );
        assert_eq!(
            tally,
            BTreeMap::from([
                ((true, 0u8), (284usize, 284usize)),
                ((true, 1), (377, 377)),
                ((false, 0), (20, 20)),
                ((false, 2), (148, 148)),
            ]),
            "(zone-keyed, base nibble) -> (rows, resolved)"
        );
        assert!(
            unresolved_client_only.is_empty(),
            "unresolved client-only map DATs: {unresolved_client_only:?}"
        );
    }

    fn first_occurrence(bytes: &[u8], needle: &[u8]) -> Option<usize> {
        bytes.windows(needle.len()).position(|w| w == needle)
    }

    /// Gated on an install (self-skips). Each marker's table base is the
    /// file's first occurrence of its hint at any alignment, inside `.data`.
    #[test]
    fn real_dll_every_marker_is_first_hit_inside_the_data_section() {
        let Some((_, dll)) = open_test_dll() else {
            return;
        };
        let data = data_section_span(&dll.bytes).expect("a PE section table naming .data");
        let markers: [(&str, Option<usize>, Vec<u8>); 8] = [
            (
                "weapon",
                Some(dll.weapon_skill_base),
                WEAPON_SKILL_HINT.to_be_bytes().to_vec(),
            ),
            (
                "dance",
                Some(dll.dance_skill_base),
                DANCE_SKILL_HINT.to_be_bytes().to_vec(),
            ),
            ("emote", dll.emote_base, EMOTE_HINT.to_be_bytes().to_vec()),
            (
                "race_config",
                dll.race_config_base,
                RACE_CONFIG_HINT.to_be_bytes().to_vec(),
            ),
            (
                "action_anim",
                dll.action_anim_base,
                ACTION_ANIM_HINT.to_be_bytes().to_vec(),
            ),
            (
                "battle_anim",
                dll.battle_anim_base,
                BATTLE_ANIM_HINT.to_be_bytes().to_vec(),
            ),
            (
                "equipment",
                dll.equipment_base,
                EQUIPMENT_HINT.to_be_bytes().to_vec(),
            ),
            (
                "zone_map",
                dll.zone_map_base,
                ZONE_MAP_HINT.to_be_bytes().to_vec(),
            ),
        ];
        for (name, base, needle) in markers {
            let base = base.unwrap_or_else(|| panic!("{name} marker found"));
            assert!(
                data.contains(&base),
                "{name} base {base:#x} inside .data {data:#x?}"
            );
            assert!(
                base.is_multiple_of(SCAN_STRIDE),
                "{name} base is word-aligned"
            );
            assert_eq!(
                first_occurrence(&dll.bytes, &needle),
                Some(base),
                "{name} base is the file's first occurrence of its hint"
            );
        }
    }
}
