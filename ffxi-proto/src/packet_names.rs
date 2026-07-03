//! Canonical FFXI packet id ↔ name tables, scraped at build time from LSB's
//! authoritative `PacketS2C` / `PacketC2S` enums
//! (`vendor/server/src/map/enums/packet_{s2c,c2s}.h`). LSB has absorbed the
//! atom0s/XiPackets layouts; consult that repo for per-field semantics.
//! Names are the canonical suffix (e.g. `0x00D` → `"CHAR_PC"`).

include!(concat!(env!("OUT_DIR"), "/packet_names_s2c_table.rs"));
include!(concat!(env!("OUT_DIR"), "/packet_names_c2s_table.rs"));

fn lookup(table: &'static [(u16, &'static str)], id: u16) -> Option<&'static str> {
    table
        .binary_search_by_key(&id, |&(k, _)| k)
        .ok()
        .map(|i| table[i].1)
}

pub fn s2c_name(id: u16) -> Option<&'static str> {
    lookup(PACKET_NAMES_S2C, id)
}

pub fn c2s_name(id: u16) -> Option<&'static str> {
    lookup(PACKET_NAMES_C2S, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{c2s, s2c};

    #[test]
    fn scraped_tables_are_nonempty_and_sorted() {
        assert!(PACKET_NAMES_S2C.len() > 100);
        assert!(PACKET_NAMES_C2S.len() > 100);
        assert!(PACKET_NAMES_S2C.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(PACKET_NAMES_C2S.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn every_decoded_s2c_id_is_a_known_lsb_packet() {
        for (id, ours) in [
            (s2c::ENTERZONE, "ENTERZONE"),
            (s2c::MESSAGE, "MESSAGE"),
            (s2c::LOGIN, "LOGIN"),
            (s2c::LOGOUT, "LOGOUT"),
            (s2c::CHAR_PC, "CHAR_PC"),
            (s2c::CHAR_NPC, "CHAR_NPC"),
            (s2c::CHAR_STATUS, "CHAR_STATUS"),
            (s2c::CHAT, "CHAT"),
            (s2c::ITEM_MAX, "ITEM_MAX"),
            (s2c::ITEM_SAME, "ITEM_SAME"),
            (s2c::ITEM_NUM, "ITEM_NUM"),
            (s2c::ITEM_LIST, "ITEM_LIST"),
            (s2c::ITEM_ATTR, "ITEM_ATTR"),
            (s2c::BATTLE2, "BATTLE2"),
            (s2c::BATTLE_MESSAGE, "BATTLE_MESSAGE"),
            (s2c::EVENT, "EVENT"),
            (s2c::EVENTSTR, "EVENTSTR"),
            (s2c::EVENTNUM, "EVENTNUM"),
            (s2c::BATTLE_MESSAGE2, "BATTLE_MESSAGE2"),
            (s2c::SHOP_LIST, "SHOP_LIST"),
            (s2c::SHOP_OPEN, "SHOP_OPEN"),
            (s2c::MISCDATA, "MISCDATA"),
            (s2c::SYSTEMMES, "SYSTEMMES"),
            (s2c::WEATHER, "WEATHER"),
            (s2c::MUSIC, "MUSIC"),
            (s2c::MUSIC_VOLUME, "MUSIC_VOLUME"),
            (s2c::EQUIP_CLEAR, "EQUIP_CLEAR"),
            (s2c::EQUIP_LIST, "EQUIP_LIST"),
            (s2c::GRAP_LIST, "GRAP_LIST"),
            (s2c::WPOS, "WPOS"),
            (s2c::WPOS2, "WPOS2"),
            (s2c::MAGIC_DATA, "MAGIC_DATA"),
            (s2c::COMMAND_DATA, "COMMAND_DATA"),
            (s2c::ABIL_RECAST, "ABIL_RECAST"),
            (s2c::ENTITY_UPDATE1, "ENTITY_UPDATE1"),
            (s2c::ENTITY_UPDATE2, "ENTITY_UPDATE2"),
            (s2c::GROUP_LIST, "GROUP_LIST"),
            (s2c::GROUP_ATTR, "GROUP_ATTR"),
        ] {
            assert!(
                s2c_name(id).is_some(),
                "s2c 0x{id:03X} ({ours}) is not in LSB's PacketS2C enum — \
                 our id may be stale or the enum changed"
            );
        }
    }

    #[test]
    fn every_sent_c2s_id_is_a_known_lsb_packet() {
        for (id, ours) in [
            (c2s::LOGIN, "LOGIN"),
            (c2s::GAMEOK, "GAMEOK"),
            (c2s::NETEND, "NETEND"),
            (c2s::ZONE_TRANSITION, "ZONE_TRANSITION"),
            (c2s::POS, "POS"),
            (c2s::ACTION, "ACTION"),
            (c2s::EVENT_END, "EVENT_END"),
            (c2s::CHAT, "CHAT"),
            (c2s::SHOP_BUY, "SHOP_BUY"),
            (c2s::EQUIP_SET, "EQUIP_SET"),
            (c2s::ITEM_STACK, "ITEM_STACK"),
            (c2s::REQ_LOGOUT, "REQ_LOGOUT"),
        ] {
            assert!(
                c2s_name(id).is_some(),
                "c2s 0x{id:03X} ({ours}) is not in LSB's PacketC2S enum — \
                 our id may be stale or the enum changed"
            );
        }
    }
}
