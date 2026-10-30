//! Synthesis wire law: c2s 0x096 GP_CLI_COMMAND_COMBINE_ASK and s2c 0x06F
//! GP_SERV_COMMAND_COMBINE_ANS. Authority is the server source
//! (vendor/server/src/map/packets/c2s/0x096_combine_ask.h, its .cpp process(),
//! and vendor/server/src/map/packets/s2c/0x06f_combine_ans.{h,cpp}); the client
//! window family behind it is vault main_menu.md §22 (the `cmbmenu` launcher's
//! row-2 craft list).

/// Body length of `GP_CLI_COMMAND_COMBINE_ASK::PacketData`: HashNo, pad,
/// Crystal u16, CrystalIdx, Items, ItemNo[8] (u16), TableNo[8], pad u16.
pub const ASK_BODY_LEN: usize = 32;
/// Body length of `GP_SERV_COMMAND_COMBINE_ANS::PacketData` (the trailing u32
/// padding included).
pub const ANS_BODY_LEN: usize = 52;

const ASK_OFF_HASH: usize = 0;
const ASK_OFF_CRYSTAL: usize = 2;
const ASK_OFF_CRYSTAL_IDX: usize = 4;
const ASK_OFF_ITEMS: usize = 5;
const ASK_OFF_ITEM_NO: usize = 6;
const ASK_OFF_TABLE_NO: usize = 22;

/// `SynthesisResult` (vendor/server/src/map/enums/synthesis_result.h). The
/// answer packet's first byte is one of these codes.
pub mod result {
    pub const SUCCESS: u8 = 0x00;
    pub const FAILED: u8 = 0x01;
    #[allow(dead_code)]
    pub const INTERRUPTED: u8 = 0x02;
    pub const CANCEL_BAD_RECIPE: u8 = 0x03;
    #[allow(dead_code)]
    pub const CANCEL: u8 = 0x04;
    pub const CANCEL_SKILL_TOO_LOW: u8 = 0x06;
    pub const CANCEL_RARE_ITEM: u8 = 0x07;
    pub const SUCCESS_DESYNTH: u8 = 0x0C;
    pub const MUST_WAIT_LONGER: u8 = 0x0D;
    #[allow(dead_code)]
    pub const INTERRUPTED_CRITICAL: u8 = 0x0E;

    /// Human label for the code, wording from synthesis_result.h's comments.
    pub fn name(code: u8) -> &'static str {
        match code {
            SUCCESS => "success",
            FAILED => "failed",
            INTERRUPTED => "interrupted",
            CANCEL_BAD_RECIPE => "bad recipe",
            CANCEL => "cancelled",
            CANCEL_SKILL_TOO_LOW => "skill too low",
            CANCEL_RARE_ITEM => "rare item",
            SUCCESS_DESYNTH => "desynthesis success",
            MUST_WAIT_LONGER => "must wait longer",
            INTERRUPTED_CRITICAL => "interrupted (critical)",
            _ => "unknown result",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SynthError {
    TooShort { want: usize, got: usize },
}

impl std::fmt::Display for SynthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { want, got } => {
                write!(f, "synthesis body too short: need {want} bytes, got {got}")
            }
        }
    }
}

impl std::error::Error for SynthError {}

/// One ingredient cell of the ask: the item id and the inventory slot it comes
/// from (`ItemNo[i]` / `TableNo[i]`, checked together by the server's process()).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SynthIngredient {
    pub item_id: u16,
    pub inv_slot: u8,
}

/// Encode a c2s 0x096 body. `entries` must be non-empty and at most 8 (LSB's
/// validate(): Items in 1..=8); entries keep the recipe row's own ingredient
/// order because `resolveRecipe` keys on that positional list.
pub fn encode_combine_ask(
    hash_no: u8,
    crystal_id: u16,
    crystal_slot: u8,
    entries: &[SynthIngredient],
) -> [u8; ASK_BODY_LEN] {
    debug_assert!((1..=8).contains(&entries.len()));
    let mut body = [0u8; ASK_BODY_LEN];
    body[ASK_OFF_HASH] = hash_no;
    body[ASK_OFF_CRYSTAL..ASK_OFF_CRYSTAL + 2].copy_from_slice(&crystal_id.to_le_bytes());
    body[ASK_OFF_CRYSTAL_IDX] = crystal_slot;
    let n = entries.len().min(8);
    body[ASK_OFF_ITEMS] = n as u8;
    for (i, e) in entries.iter().take(n).enumerate() {
        body[ASK_OFF_ITEM_NO + i * 2..ASK_OFF_ITEM_NO + i * 2 + 2]
            .copy_from_slice(&e.item_id.to_le_bytes());
        body[ASK_OFF_TABLE_NO + i] = e.inv_slot;
    }
    body
}

/// One s2c 0x06F answer. LSB never writes `grade` (it stays zero on the wire);
/// retail's client uses it to tier the success message, so kuluu carries it
/// through unchanged. The crystal/material echo fields are additive LSB
/// extensions over the original PS2 layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CombineAns {
    pub result: u8,
    pub grade: i8,
    pub count: u8,
    pub item_no: u16,
    /// Ingredient ids that broke (per-stack, at their recipe position; zeros elsewhere).
    pub break_nos: [u16; 8],
    pub up_kind: [i8; 4],
    pub up_level: [i8; 4],
    pub crystal_no: u16,
    pub material_nos: [u16; 8],
}

/// Decode an s2c 0x06F body (declaration order of `PacketData`).
pub fn parse_combine_ans(body: &[u8]) -> Result<CombineAns, SynthError> {
    if body.len() < ANS_BODY_LEN {
        return Err(SynthError::TooShort {
            want: ANS_BODY_LEN,
            got: body.len(),
        });
    }
    let mut ans = CombineAns {
        result: body[0],
        grade: body[1] as i8,
        count: body[2],
        item_no: 0,
        break_nos: [0; 8],
        up_kind: [0; 4],
        up_level: [0; 4],
        crystal_no: 0,
        material_nos: [0; 8],
    };
    ans.item_no = u16::from_le_bytes([body[4], body[5]]);
    for i in 0..8 {
        ans.break_nos[i] = u16::from_le_bytes([body[6 + i * 2], body[7 + i * 2]]);
    }
    for i in 0..4 {
        ans.up_kind[i] = body[22 + i] as i8;
        ans.up_level[i] = body[26 + i] as i8;
    }
    ans.crystal_no = u16::from_le_bytes([body[30], body[31]]);
    for i in 0..8 {
        ans.material_nos[i] = u16::from_le_bytes([body[32 + i * 2], body[33 + i * 2]]);
    }
    Ok(ans)
}

/// Pick the inventory slot for each ingredient entry of one ask, walking `bag`
/// in slot order (each item `(slot, item_id, quantity)`). A stack may serve
/// several entries up to its quantity — LSB counts uses per slot against that
/// stack (`GP_CLI_COMMAND_COMBINE_ASK::process`, the slotQty law) — and the
/// crystal's own slot is never offered. Returns None when no assignment covers
/// every entry, i.e. not craftable from this bag.
pub fn assign_ingredient_slots(
    needed: &[u16],
    bag: &[(u8, u16, u32)],
    crystal_slot: u8,
) -> Option<Vec<SynthIngredient>> {
    let mut remaining: Vec<(u8, u16, u32)> = bag
        .iter()
        .filter(|(slot, _, _)| *slot != crystal_slot)
        .cloned()
        .collect();
    let mut out = Vec::with_capacity(needed.len());
    for &id in needed {
        if id == 0 {
            continue; // empty recipe column
        }
        let hit = remaining
            .iter_mut()
            .find(|(_, item, left)| *item == id && *left > 0)?;
        out.push(SynthIngredient {
            item_id: id,
            inv_slot: hit.0,
        });
        hit.2 -= 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_body_matches_the_lsb_field_layout() {
        // Banquet Table (synth_recipes row 1): earth crystal in slot 3, three
        // ingredients from slots 9/10/2.
        let entries = [
            SynthIngredient {
                item_id: 3553,
                inv_slot: 9,
            },
            SynthIngredient {
                item_id: 3554,
                inv_slot: 10,
            },
            SynthIngredient {
                item_id: 3555,
                inv_slot: 2,
            },
        ];
        let body = encode_combine_ask(0x5A, 4099, 3, &entries);
        assert_eq!(body[0], 0x5A); // HashNo round-trips untouched
        assert_eq!(&body[2..4], &4099u16.to_le_bytes());
        assert_eq!(body[4], 3);
        assert_eq!(body[5], 3);
        let expect_items: Vec<u8> = [3553u16, 3554, 3555]
            .iter()
            .flat_map(|v| v.to_le_bytes().to_vec())
            .collect();
        assert_eq!(&body[6..12], &expect_items[..]);
        assert_eq!(&body[22..25], &[9, 10, 2]);
        // Slots past Items stay zero.
        assert!(body[25..32].iter().all(|&b| b == 0));
    }

    #[test]
    fn ans_parses_a_full_lsb_answer() {
        let mut body = [0u8; ANS_BODY_LEN];
        body[0] = result::SUCCESS;
        body[2] = 33; // count from ResultQty (Bone Arrow)
        body[4..6].copy_from_slice(&17319u16.to_le_bytes());
        body[8..10].copy_from_slice(&705u16.to_le_bytes()); // broke slot's item id at idx 1
        body[22] = 51; // UpKind: Goldsmithing (xi.SkillType range)
        body[30..32].copy_from_slice(&4099u16.to_le_bytes());
        body[32..34].copy_from_slice(&705u16.to_le_bytes());
        let ans = parse_combine_ans(&body).expect("full body");
        assert_eq!(ans.result, result::SUCCESS);
        assert_eq!((ans.count, ans.item_no), (33, 17319));
        assert_eq!(ans.break_nos[1], 705);
        assert_eq!(ans.up_kind[0], 51);
        assert_eq!((ans.crystal_no, ans.material_nos[0]), (4099, 705));
    }

    #[test]
    fn short_ans_bodies_are_rejected() {
        let body = [0u8; ANS_BODY_LEN - 1];
        assert_eq!(
            parse_combine_ans(&body),
            Err(SynthError::TooShort {
                want: ANS_BODY_LEN,
                got: ANS_BODY_LEN - 1
            })
        );
    }

    #[test]
    fn slot_assignment_shares_stacks_and_skips_the_crystal() {
        // Bone Arrow (row 25): bone blocks x? — model as item 705 needing three
        // slots; one stack of 2 in slot 4 plus singles in 6 and 8.
        let bag = [(3u8, 4099u16, 1), (4, 705, 2), (6, 705, 1), (8, 706, 4)];
        let needed = [705, 705, 705];
        let got = assign_ingredient_slots(&needed, &bag, 3).expect("assignable");
        assert_eq!(
            got,
            vec![
                SynthIngredient {
                    item_id: 705,
                    inv_slot: 4
                },
                SynthIngredient {
                    item_id: 705,
                    inv_slot: 4
                },
                SynthIngredient {
                    item_id: 705,
                    inv_slot: 6
                },
            ]
        );
        // The crystal slot itself is never an ingredient source.
        let only_crystal = [(3u8, 705u16, 9)];
        assert!(assign_ingredient_slots(&needed, &only_crystal, 3).is_none());
        // A missing id fails the whole ask.
        assert!(assign_ingredient_slots(&[999], &bag, 3).is_none());
    }
}
