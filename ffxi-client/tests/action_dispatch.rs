//! Wire-level fixture tests for the Stage-8 action surface builders.
//!
//! These don't touch the network — they assert that the byte layout of
//! the encrypted-payload precursor (the sub-packet body) matches the
//! Phoenix server's `GP_CLI_COMMAND_*` packed structs. If the server's
//! `PacketValidator` ever rejects a packet, the cause is *here*, not
//! deep in the session loop, so isolating the fixture tests pays off.
//!
//! Mirrors the inline `tell_packet_layout_matches_phoenix_struct` style
//! in `session.rs::tests` but at integration-test scope so the public
//! surface (`build_subpacket_action` / `build_subpacket_item_use`) gets
//! exercised through the same import path the rest of the crate uses.

use ffxi_client::session::{build_subpacket_action, build_subpacket_item_use};
use ffxi_client::state::ActionKind;

/// `GP_CLI_COMMAND_ACTION` body = 4 hdr + 4 UniqueNo + 2 ActIndex +
/// 2 ActionID + 16 ActionBuf = 28 bytes total. ActionID for CastMagic
/// is 0x03, and `ActionBuf` is `(spell_id, pos_x, pos_z, pos_y)` LE
/// f32s — note the wire order is (X, Z, Y), matching ACTIONBUF_CASTMAGIC,
/// not POS's (X, Z, Y). See `state::ActionKind::fill_action_buf`.
#[test]
fn cast_magic_action_layout_matches_phoenix_struct() {
    let kind = ActionKind::CastMagic {
        spell_id: 0x101, // Cure-ish placeholder
        pos_x: 1.5,
        pos_y: 0.0,
        pos_z: -2.5,
    };
    let buf = build_subpacket_action(0xCAFE, 0x1234_5678, 0x00FF, &kind);
    assert_eq!(buf.len(), 28, "header(4) + body(24)");

    // Header.
    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(id_and_size & 0x1FF, 0x01A, "opcode = 0x01A ACTION");
    assert_eq!(id_and_size >> 9, 7, "size_words = 7 (28 bytes)");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xCAFE, "sync");

    // UniqueNo, ActIndex, ActionID.
    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        0x1234_5678,
        "UniqueNo"
    );
    assert_eq!(
        u16::from_le_bytes(buf[8..10].try_into().unwrap()),
        0x00FF,
        "ActIndex"
    );
    assert_eq!(
        u16::from_le_bytes(buf[10..12].try_into().unwrap()),
        0x03,
        "ActionID for CastMagic"
    );

    // ActionBuf union — CastMagic layout: spell(u32) pos_x(f32) pos_z(f32) pos_y(f32).
    assert_eq!(
        u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        0x101,
        "spell_id"
    );
    assert_eq!(
        f32::from_le_bytes(buf[16..20].try_into().unwrap()),
        1.5,
        "pos_x"
    );
    assert_eq!(
        f32::from_le_bytes(buf[20..24].try_into().unwrap()),
        -2.5,
        "pos_z (wire order is X, Z, Y)"
    );
    assert_eq!(
        f32::from_le_bytes(buf[24..28].try_into().unwrap()),
        0.0,
        "pos_y"
    );
}

/// Weaponskill action: ActionID = 0x07, body[0..4] = skill_id, rest 0.
#[test]
fn weaponskill_action_writes_skill_id_only() {
    let buf = build_subpacket_action(
        0,
        0x0DEAD_BEEF,
        0x10,
        &ActionKind::Weaponskill { skill_id: 0xCAFE },
    );
    assert_eq!(buf.len(), 28);
    assert_eq!(
        u16::from_le_bytes(buf[10..12].try_into().unwrap()),
        0x07,
        "ActionID = 0x07 Weaponskill"
    );
    assert_eq!(
        u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        0xCAFE,
        "skill_id"
    );
    assert!(
        buf[16..28].iter().all(|&b| b == 0),
        "trailing ActionBuf bytes zeroed"
    );
}

/// JobAbility action: ActionID = 0x09, body[0..4] = ability_id.
#[test]
fn job_ability_action_writes_ability_id_only() {
    let buf = build_subpacket_action(0, 0, 0, &ActionKind::JobAbility { ability_id: 42 });
    assert_eq!(
        u16::from_le_bytes(buf[10..12].try_into().unwrap()),
        0x09,
        "ActionID = 0x09 JobAbility"
    );
    assert_eq!(
        u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        42,
        "ability_id"
    );
}

/// `GP_CLI_COMMAND_ITEM_USE` (0x037) body layout. The server-side
/// validator (`Phoenix/src/map/packets/c2s/0x037_item_use.cpp`)
/// enforces `ItemNum == 0` regardless of the actual item, so the
/// builder unconditionally writes 0 there.
#[test]
fn item_use_packet_layout_matches_phoenix_struct() {
    let buf = build_subpacket_item_use(
        0xABCD,     // sync
        0xC0FFEE00, // recipient UniqueNo
        0x0007,     // recipient ActIndex
        0x00,       // category = LOC_INVENTORY
        12,         // slot
    );
    assert_eq!(buf.len(), 20, "4 hdr + 16 body");

    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(id_and_size & 0x1FF, 0x037, "opcode = 0x037 ITEM_USE");
    assert_eq!(id_and_size >> 9, 5, "size_words = 5 (20 bytes)");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xABCD, "sync");

    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        0xC0FFEE00,
        "UniqueNo (recipient)"
    );
    assert_eq!(
        u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        0,
        "ItemNum = 0 (server-validated)"
    );
    assert_eq!(
        u16::from_le_bytes(buf[12..14].try_into().unwrap()),
        7,
        "ActIndex"
    );
    assert_eq!(buf[14], 12, "PropertyItemIndex (slot)");
    assert_eq!(buf[15], 0, "padding00");
    assert_eq!(
        u32::from_le_bytes(buf[16..20].try_into().unwrap()),
        0,
        "Category"
    );
}
