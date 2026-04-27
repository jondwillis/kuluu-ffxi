use ffxi_client::session::{build_subpacket_action, build_subpacket_item_use};
use ffxi_client::state::ActionKind;

#[test]
fn cast_magic_action_layout_matches_phoenix_struct() {
    let kind = ActionKind::CastMagic {
        spell_id: 0x101,
        pos_x: 1.5,
        pos_y: 0.0,
        pos_z: -2.5,
    };
    let buf = build_subpacket_action(0xCAFE, 0x1234_5678, 0x00FF, &kind);
    assert_eq!(buf.len(), 28, "header(4) + body(24)");

    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(id_and_size & 0x1FF, 0x01A, "opcode = 0x01A ACTION");
    assert_eq!(id_and_size >> 9, 7, "size_words = 7 (28 bytes)");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xCAFE, "sync");

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

#[test]
fn weaponskill_action_writes_skill_id_only() {
    let buf = build_subpacket_action(
        0,
        0xDEAD_BEEF,
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

#[test]
fn item_use_packet_layout_matches_phoenix_struct() {
    let buf = build_subpacket_item_use(0xABCD, 0xC0FFEE00, 0x0007, 0x00, 12);
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
