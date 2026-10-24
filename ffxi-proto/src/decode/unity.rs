use super::*;
use crate::s2c_layout::{
    alter_ego_points as alter_ego_lsb, miscdata_unity_members as members_lsb,
    miscdata_unity_points as points_lsb, unity as unity_lsb,
};

/// s2c 0x08E GP_SERV_PACKET_ALTER_EGO_POINTS: the alter ego (Trust) point balance, then the
/// current level and the next cost of each upgrade id
/// (vendor/server/src/map/packets/s2c/0x08e_alter_ego_points.h PacketData).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlterEgoPoints {
    pub points: u16,
    pub upgrades: [u8; Self::UPGRADE_SLOTS],
    pub costs: [u16; Self::UPGRADE_SLOTS],
}

impl AlterEgoPoints {
    /// `Upgrades[32]` / `Costs[32]`, indexed by upgrade id.
    pub const UPGRADE_SLOTS: usize = 32;
    pub(crate) const POINTS_OFFSET: usize = 0x0;
    pub(crate) const UPGRADES_OFFSET: usize = 0x4;
    pub(crate) const COSTS_OFFSET: usize = 0x24;
    pub(crate) const SIZE: usize = Self::COSTS_OFFSET + 2 * Self::UPGRADE_SLOTS;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let mut upgrades = [0u8; Self::UPGRADE_SLOTS];
        upgrades.copy_from_slice(
            &body[Self::UPGRADES_OFFSET..Self::UPGRADES_OFFSET + Self::UPGRADE_SLOTS],
        );
        let mut costs = [0u16; Self::UPGRADE_SLOTS];
        for (id, cost) in costs.iter_mut().enumerate() {
            *cost = rd16(Self::COSTS_OFFSET + 2 * id);
        }
        Ok(Self {
            points: rd16(Self::POINTS_OFFSET),
            upgrades,
            costs,
        })
    }
}

pin_s2c_offset!(
    AlterEgoPoints::POINTS_OFFSET,
    alter_ego_lsb::POINTS,
    "GP_SERV_PACKET_ALTER_EGO_POINTS.Points"
);
pin_s2c_offset!(
    AlterEgoPoints::UPGRADES_OFFSET,
    alter_ego_lsb::UPGRADES,
    "GP_SERV_PACKET_ALTER_EGO_POINTS.Upgrades"
);
pin_s2c_offset!(
    AlterEgoPoints::UPGRADE_SLOTS,
    alter_ego_lsb::UPGRADES_COUNT,
    "GP_SERV_PACKET_ALTER_EGO_POINTS.Upgrades length"
);
pin_s2c_offset!(
    AlterEgoPoints::COSTS_OFFSET,
    alter_ego_lsb::COSTS,
    "GP_SERV_PACKET_ALTER_EGO_POINTS.Costs"
);
pin_s2c_offset!(
    AlterEgoPoints::SIZE,
    alter_ego_lsb::SIZE,
    "GP_SERV_PACKET_ALTER_EGO_POINTS size"
);

/// s2c 0x110 GP_SERV_COMMAND_UNITY: the Unity quest block LSB answers c2s 0x116/0x117 with
/// (vendor/server/src/map/packets/s2c/0x110_unity.h PacketData). The Unity windows only note
/// its arrival (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnityQuest {
    pub sparks: u32,
    pub deeds: u16,
    pub plaudits: u16,
    pub roe_unity_shared: u8,
    pub roe_unity_leader: u8,
}

impl UnityQuest {
    pub(crate) const SPARKS_OFFSET: usize = 0x0;
    pub(crate) const SPARKS_MASK: u32 = 0x00FF_FFFF;
    pub(crate) const DEEDS_OFFSET: usize = 0x4;
    pub(crate) const PLAUDITS_OFFSET: usize = 0x6;
    pub(crate) const ROE_UNITY_SHARED_OFFSET: usize = 0x8;
    pub(crate) const ROE_UNITY_LEADER_OFFSET: usize = 0x9;
    pub(crate) const NEEDED: usize = Self::ROE_UNITY_LEADER_OFFSET + 1;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::NEEDED {
            return Err(DecodeError::Truncated(Self::NEEDED, body.len()));
        }
        let rd16 = |o: usize| u16::from_le_bytes([body[o], body[o + 1]]);
        let o = Self::SPARKS_OFFSET;
        let word = u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
        Ok(Self {
            sparks: word & Self::SPARKS_MASK,
            deeds: rd16(Self::DEEDS_OFFSET),
            plaudits: rd16(Self::PLAUDITS_OFFSET),
            roe_unity_shared: body[Self::ROE_UNITY_SHARED_OFFSET],
            roe_unity_leader: body[Self::ROE_UNITY_LEADER_OFFSET],
        })
    }
}

pin_s2c_offset!(
    UnityQuest::SPARKS_OFFSET,
    unity_lsb::SPARKS,
    "GP_SERV_COMMAND_UNITY.Sparks"
);
const _: () = assert!(
    UnityQuest::SPARKS_MASK == unity_lsb::SPARKS_MASK << unity_lsb::SPARKS_SHIFT,
    "LSB moved GP_SERV_COMMAND_UNITY.Sparks bits: the decoder mask is stale"
);
pin_s2c_offset!(
    UnityQuest::DEEDS_OFFSET,
    unity_lsb::DEEDS,
    "GP_SERV_COMMAND_UNITY.Deeds"
);
pin_s2c_offset!(
    UnityQuest::PLAUDITS_OFFSET,
    unity_lsb::PLAUDITS,
    "GP_SERV_COMMAND_UNITY.Plaudits"
);
pin_s2c_offset!(
    UnityQuest::ROE_UNITY_SHARED_OFFSET,
    unity_lsb::RO_E_UNITY_SHARED,
    "GP_SERV_COMMAND_UNITY.RoEUnityShared"
);
pin_s2c_offset!(
    UnityQuest::ROE_UNITY_LEADER_OFFSET,
    unity_lsb::RO_E_UNITY_LEADER,
    "GP_SERV_COMMAND_UNITY.RoEUnityLeader"
);

/// `GP_SERV_COMMAND_MISCDATA_TYPE::Unity` (vendor/server/src/map/packets/s2c/0x063_miscdata.h).
pub const MISCDATA_TYPE_UNITY: u16 = 0x07;

/// The Unity Concords a ranking set reports, `members[11]` / `points[11]`
/// (vendor/server/src/map/packets/s2c/0x063_miscdata_unity.h MEMBERS / POINTS).
pub const UNITY_CONCORD_COUNT: usize = 11;

/// One s2c 0x063 MISCDATA record of type Unity: one chunk (`dataType`) of one ranking result set,
/// laid out as LSB builds it (vendor/server/src/map/packets/s2c/0x063_miscdata_unity.h).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MiscdataUnity {
    /// `UNITY_RESULTSET`; any nonzero value is result set 1
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    pub result_set: u8,
    /// `UNITY_DATATYPE`: the chunk index, 0..=[`Self::LAST_CHUNK`].
    pub data_type: u8,
    /// Concords 1..=11 in order: LSB `members[11]` for [`Self::DATA_TYPE_MEMBERS`],
    /// `points[11]` for [`Self::DATA_TYPE_POINTS`], zero for every other chunk.
    pub concord_values: [u32; UNITY_CONCORD_COUNT],
}

impl MiscdataUnity {
    pub(crate) const TYPE_OFFSET: usize = 0x0;
    pub(crate) const RESULT_SET_OFFSET: usize = 0x4;
    pub(crate) const DATA_TYPE_OFFSET: usize = 0x5;
    /// LSB's `statusFlag` word, which sits ahead of the per-Concord values.
    pub const STATUS_FLAG_OFFSET: usize = 0xC;
    /// LSB `members[0]` / `points[0]` (u32 alignment puts them past `statusFlag`).
    pub const CONCORD_VALUES_OFFSET: usize = 0x10;
    pub const DATA_TYPE_MEMBERS: u8 = 0x01;
    pub const DATA_TYPE_POINTS: u8 = 0x02;
    /// Receiving this chunk completes a result set
    /// (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md).
    pub const LAST_CHUNK: u8 = 0x1F;
    pub(crate) const HEADER_LEN: usize = Self::DATA_TYPE_OFFSET + 1;
    pub(crate) const VALUES_END: usize = Self::CONCORD_VALUES_OFFSET + 4 * UNITY_CONCORD_COUNT;

    /// `None` when the record is not of type Unity.
    pub fn decode(body: &[u8]) -> Option<Result<Self, DecodeError>> {
        let kind = body.get(Self::TYPE_OFFSET..Self::TYPE_OFFSET + 2)?;
        if u16::from_le_bytes([kind[0], kind[1]]) != MISCDATA_TYPE_UNITY {
            return None;
        }
        if body.len() < Self::HEADER_LEN {
            return Some(Err(DecodeError::Truncated(Self::HEADER_LEN, body.len())));
        }
        let data_type = body[Self::DATA_TYPE_OFFSET];
        let mut concord_values = [0u32; UNITY_CONCORD_COUNT];
        if matches!(data_type, Self::DATA_TYPE_MEMBERS | Self::DATA_TYPE_POINTS) {
            if body.len() < Self::VALUES_END {
                return Some(Err(DecodeError::Truncated(Self::VALUES_END, body.len())));
            }
            for (concord, value) in concord_values.iter_mut().enumerate() {
                let o = Self::CONCORD_VALUES_OFFSET + 4 * concord;
                *value = u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]]);
            }
        }
        Some(Ok(Self {
            result_set: body[Self::RESULT_SET_OFFSET],
            data_type,
            concord_values,
        }))
    }
}

pin_s2c_offset!(
    MiscdataUnity::TYPE_OFFSET,
    members_lsb::TYPE,
    "GP_SERV_COMMAND_MISCDATA::UNITY::MEMBERS.type"
);
pin_s2c_offset!(
    MiscdataUnity::RESULT_SET_OFFSET,
    members_lsb::RESULT_SET,
    "GP_SERV_COMMAND_MISCDATA::UNITY::MEMBERS.resultSet"
);
pin_s2c_offset!(
    MiscdataUnity::DATA_TYPE_OFFSET,
    members_lsb::DATA_TYPE,
    "GP_SERV_COMMAND_MISCDATA::UNITY::MEMBERS.dataType"
);
pin_s2c_offset!(
    MiscdataUnity::STATUS_FLAG_OFFSET,
    members_lsb::STATUS_FLAG,
    "GP_SERV_COMMAND_MISCDATA::UNITY::MEMBERS.statusFlag"
);
pin_s2c_offset!(
    MiscdataUnity::CONCORD_VALUES_OFFSET,
    members_lsb::MEMBERS,
    "GP_SERV_COMMAND_MISCDATA::UNITY::MEMBERS.members"
);
pin_s2c_offset!(
    UNITY_CONCORD_COUNT,
    members_lsb::MEMBERS_COUNT,
    "GP_SERV_COMMAND_MISCDATA::UNITY::MEMBERS.members length"
);
pin_s2c_offset!(
    MiscdataUnity::CONCORD_VALUES_OFFSET,
    points_lsb::POINTS,
    "GP_SERV_COMMAND_MISCDATA::UNITY::POINTS.points"
);
pin_s2c_offset!(
    UNITY_CONCORD_COUNT,
    points_lsb::POINTS_COUNT,
    "GP_SERV_COMMAND_MISCDATA::UNITY::POINTS.points length"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alter_ego_points_reads_balance_levels_and_costs_by_upgrade_id() {
        const HP_UPGRADE: usize = 8;
        const MAGIC_SKILLS_UPGRADE: usize = 18;
        let mut body = vec![0u8; AlterEgoPoints::SIZE];
        body[..2].copy_from_slice(&850u16.to_le_bytes());
        body[AlterEgoPoints::UPGRADES_OFFSET + HP_UPGRADE] = 12;
        body[AlterEgoPoints::UPGRADES_OFFSET + MAGIC_SKILLS_UPGRADE] = 50;
        let cost_at = AlterEgoPoints::COSTS_OFFSET + 2 * HP_UPGRADE;
        body[cost_at..cost_at + 2].copy_from_slice(&2u16.to_le_bytes());

        let packet = AlterEgoPoints::decode(&body).expect("decode");
        assert_eq!(packet.points, 850);
        assert_eq!(packet.upgrades[HP_UPGRADE], 12);
        assert_eq!(packet.upgrades[MAGIC_SKILLS_UPGRADE], 50);
        assert_eq!(packet.costs[HP_UPGRADE], 2);
        assert_eq!(packet.costs[MAGIC_SKILLS_UPGRADE], 0);
        assert!(matches!(
            AlterEgoPoints::decode(&body[..AlterEgoPoints::SIZE - 1]),
            Err(DecodeError::Truncated(..))
        ));
    }

    /// The record's packet offsets (Points at 4, Upgrades at 8 + id, Costs at 0x28 + 2 id) are
    /// these body offsets plus the 4-byte header.
    #[test]
    fn alter_ego_points_body_offsets_are_the_record_packet_offsets() {
        const HEADER: usize = 4;
        assert_eq!(AlterEgoPoints::POINTS_OFFSET + HEADER, 4);
        assert_eq!(AlterEgoPoints::UPGRADES_OFFSET + HEADER, 8);
        assert_eq!(AlterEgoPoints::COSTS_OFFSET + HEADER, 0x28);
    }

    #[test]
    fn unity_quest_masks_sparks_to_24_bits() {
        let mut body = vec![0u8; unity_lsb::SIZE];
        body[..4].copy_from_slice(&0xAB12_3456u32.to_le_bytes());
        body[UnityQuest::DEEDS_OFFSET..UnityQuest::DEEDS_OFFSET + 2]
            .copy_from_slice(&40u16.to_le_bytes());
        body[UnityQuest::PLAUDITS_OFFSET..UnityQuest::PLAUDITS_OFFSET + 2]
            .copy_from_slice(&7u16.to_le_bytes());
        body[UnityQuest::ROE_UNITY_SHARED_OFFSET] = 5;
        body[UnityQuest::ROE_UNITY_LEADER_OFFSET] = 3;
        let quest = UnityQuest::decode(&body).expect("decode");
        assert_eq!(quest.sparks, 0x12_3456);
        assert_eq!(quest.deeds, 40);
        assert_eq!(quest.plaudits, 7);
        assert_eq!(quest.roe_unity_shared, 5);
        assert_eq!(quest.roe_unity_leader, 3);
    }

    /// An LSB MEMBERS / POINTS body: `statusFlag` then the eleven Concord values.
    fn lsb_unity_body(
        result_set: u8,
        data_type: u8,
        status_flag: u16,
        values: [u32; 11],
    ) -> Vec<u8> {
        let mut body = vec![0u8; members_lsb::SIZE];
        body[..2].copy_from_slice(&MISCDATA_TYPE_UNITY.to_le_bytes());
        body[MiscdataUnity::RESULT_SET_OFFSET] = result_set;
        body[MiscdataUnity::DATA_TYPE_OFFSET] = data_type;
        body[MiscdataUnity::STATUS_FLAG_OFFSET..MiscdataUnity::STATUS_FLAG_OFFSET + 2]
            .copy_from_slice(&status_flag.to_le_bytes());
        for (i, v) in values.iter().enumerate() {
            let o = MiscdataUnity::CONCORD_VALUES_OFFSET + 4 * i;
            body[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        body
    }

    const CONCORD_VALUES: [u32; 11] = [10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];

    #[test]
    fn miscdata_unity_decodes_members_by_lsb_layout() {
        const LSB_MENU_STATUS_FLAG: u16 = 0x0108;
        let body = lsb_unity_body(
            1,
            MiscdataUnity::DATA_TYPE_MEMBERS,
            LSB_MENU_STATUS_FLAG,
            CONCORD_VALUES,
        );
        let unity = MiscdataUnity::decode(&body)
            .expect("type 7")
            .expect("decode");
        assert_eq!(unity.result_set, 1);
        assert_eq!(unity.data_type, MiscdataUnity::DATA_TYPE_MEMBERS);
        assert_eq!(
            unity.concord_values, CONCORD_VALUES,
            "Concord u = members[u - 1]"
        );
    }

    /// The record's retail read takes Concord u's value from packet offset 0x0C + 4u, the
    /// 128-byte chunk starting at packet 0x0C. Against LSB's MEMBERS / POINTS layout that lands
    /// one u32 slot early: Concord 1 reads LSB's `statusFlag` word and Concord u (u >= 2) reads
    /// `members[u - 2]`. Kuluu decodes by LSB's struct, so this pins the difference.
    #[test]
    fn retail_chunk_read_is_one_slot_off_lsb_unity_layout() {
        const PACKET_HEADER: usize = 4;
        const RETAIL_CHUNK_PACKET_OFFSET: usize = 0x0C;
        const LSB_MENU_STATUS_FLAG: u16 = 0x0108;
        let body = lsb_unity_body(
            0,
            MiscdataUnity::DATA_TYPE_MEMBERS,
            LSB_MENU_STATUS_FLAG,
            CONCORD_VALUES,
        );
        let retail_value = |concord: usize| {
            let o = RETAIL_CHUNK_PACKET_OFFSET - PACKET_HEADER + 4 * concord;
            u32::from_le_bytes([body[o], body[o + 1], body[o + 2], body[o + 3]])
        };
        assert_eq!(retail_value(1), u32::from(LSB_MENU_STATUS_FLAG));
        for concord in 2..=UNITY_CONCORD_COUNT {
            assert_eq!(retail_value(concord), CONCORD_VALUES[concord - 2]);
        }
        let lsb = MiscdataUnity::decode(&body).unwrap().unwrap();
        for concord in 1..=UNITY_CONCORD_COUNT {
            assert_eq!(lsb.concord_values[concord - 1], CONCORD_VALUES[concord - 1]);
        }
    }

    #[test]
    fn miscdata_unity_other_chunks_carry_no_values_and_other_types_pass() {
        let mut body = lsb_unity_body(1, MiscdataUnity::LAST_CHUNK, 0, CONCORD_VALUES);
        let last = MiscdataUnity::decode(&body).unwrap().unwrap();
        assert_eq!(last.data_type, MiscdataUnity::LAST_CHUNK);
        assert_eq!(last.concord_values, [0; UNITY_CONCORD_COUNT]);

        body[..2].copy_from_slice(&0x0002u16.to_le_bytes());
        assert!(
            MiscdataUnity::decode(&body).is_none(),
            "merits is not Unity"
        );

        let short = lsb_unity_body(0, MiscdataUnity::DATA_TYPE_POINTS, 0, CONCORD_VALUES);
        assert!(matches!(
            MiscdataUnity::decode(&short[..MiscdataUnity::VALUES_END - 1]),
            Some(Err(DecodeError::Truncated(..)))
        ));
    }
}
