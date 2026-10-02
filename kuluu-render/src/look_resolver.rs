use bevy::log::warn_once;
use bevy::prelude::*;
use ffxi_dat::main_dll::MainDll;
use kuluu_snapshot::EntityLook;

use crate::components::{EntityModel, LookComp, WorldEntity};
use crate::dat_mmb::LoadMmbRequest;
use crate::graphics_settings::GraphicsSettings;
use crate::scene::TrackedEntities;
use crate::scheduler_runtime::{main_dll_from_env, ActionMainDll};
use crate::snapshot::SceneState;

const EQUIP_SLOT_ORDER_LEN: usize = 8;

// Slot numbering retail switches on when collecting per-slot CIB bytes
// (research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp SkeletalMeshActor::SetEquipModel:
// 2 = body, 5 = feet, 6 = main, 7 = sub, 8 = ranged), matching the order of
// `slot_models` below. Slot 0 is the face row of the same table.
const EQUIP_SLOT_FACE: u8 = 0;
const EQUIP_SLOT_BODY: u8 = 2;
const EQUIP_SLOT_MAIN: u8 = 6;
const EQUIP_SLOT_SUB: u8 = 7;
const EQUIP_SLOT_RANGED: u8 = 8;
const WEAPON_SLOTS: [u8; 3] = [EQUIP_SLOT_MAIN, EQUIP_SLOT_SUB, EQUIP_SLOT_RANGED];

/// The playable look races, HumeM=1..Galka=8: the equipment table rows the look
/// race byte indexes directly (the non-playable configs map to other rows, see
/// `MainDll::equipment_model_index`).
pub const PC_LOOK_RACES: std::ops::RangeInclusive<u8> = 1..=8;

const EQUIP_SLOT_ID_SHIFT: u32 = 12;
const EQUIP_SLOT_ID_SLOT_MASK: u16 = 0xF;
const EQUIP_SLOT_ID_MODEL_MASK: u16 = 0x0FFF;

// FFXiMain.dll horizonxi-2023 RVA 0xC417D / retail-2026-09 RVA 0xC520D (identical
// bytes; pointed at by research/xi-tools/docs/ffximain/ffximain.md "Monster
// Model ID → File ID Formula"): four ranges split at 1500 / 3000 / 3500, the top
// one computed as `(m - 3500) + 101739`.
const NPC_DAT_ID_BASES: [(u32, u32); 4] = [
    (1500, 1300),
    (3000, 50295),
    (3500, 96907),
    (u32::MAX, 98239),
];

pub fn npc_dat_id(modelid: u16) -> u32 {
    let m = modelid as u32;
    let base = NPC_DAT_ID_BASES
        .iter()
        .find_map(|&(limit, base)| (m < limit).then_some(base))
        .unwrap_or(NPC_DAT_ID_BASES[NPC_DAT_ID_BASES.len() - 1].1);
    m + base
}

/// Model DAT for one retail equipment slot (1 head .. 8 ranged) of a playable
/// race, from the FFXiMain.dll equipment lookup table
/// (`MainDll::equipment_model_index`; the look race byte is the table row for
/// the playable races). A model id past the slot's bands renders the slot's
/// model 0 instead of dropping the part: retail's "wrong GRP number" clamp
/// (research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp
/// SkeletalMeshActor::SetEquipModel). A race outside [`PC_LOOK_RACES`] is `None`.
pub fn equipment_dat_id(dll: &MainDll, slot_index: u8, model_id: u16, race: u8) -> Option<u32> {
    if slot_index == EQUIP_SLOT_FACE
        || slot_index > EQUIP_SLOT_RANGED
        || !PC_LOOK_RACES.contains(&race)
    {
        return None;
    }
    let model_id = model_id & EQUIP_SLOT_ID_MODEL_MASK;
    dll.equipment_model_index(race, slot_index, model_id)
        .or_else(|| dll.equipment_model_index(race, slot_index, 0))
}

/// [`equipment_dat_id`] for a wire slot id: the slot number in the high nibble
/// over a 12-bit model id (the shape s2c 0x00D / 0x051 carry per slot, the wire
/// half of vendor/server/src/map/packets/s2c/0x051_grap_list.cpp). A bare model
/// id with no slot nibble is slot 0 and resolves to nothing, which is how an
/// empty slot reads.
pub fn equipment_slot_dat_id(dll: &MainDll, slot_id: u16, race: u8) -> Option<u32> {
    let slot = ((slot_id >> EQUIP_SLOT_ID_SHIFT) & EQUIP_SLOT_ID_SLOT_MASK) as u8;
    equipment_dat_id(dll, slot, slot_id & EQUIP_SLOT_ID_MODEL_MASK, race)
}

/// The face byte is the 0-based index into the race's Face row (slot 0 of the
/// FFXiMain.dll equipment lookup): file = base + face, no -1. LSB caps creation
/// faces at 15 ("Face 8B", vendor/server/src/login/login_helpers.cpp), the
/// stylist spans the full row; xim EquipmentModelTable.getItemModelPath indexes
/// the Face slot directly the same way.
pub fn face_dat_id(dll: &MainDll, face: u8, race: u8) -> Option<u32> {
    if !PC_LOOK_RACES.contains(&race) {
        return None;
    }
    let face_zero = dll.equipment_model_index(race, EQUIP_SLOT_FACE, 0)?;
    match dll.equipment_model_index(race, EQUIP_SLOT_FACE, u16::from(face)) {
        Some(file_id) => Some(file_id),
        None => {
            // Retail clamp: an id past the slot's table renders model 0, never a
            // missing part ("wrong GRP number", research/XIClient/src/XIClient/
            // source/World/Actor/SkeletalMeshActor.cpp SkeletalMeshActor::SetEquipModel),
            // so an out-of-band face byte draws face 0 rather than a decapitated
            // PC. Loud because the server named a face this install's table does
            // not know, and the wrong-face render needs explaining.
            warn!("face {face} out of band for race {race}: clamping to face 0 (retail behavior)");
            Some(face_zero)
        }
    }
}

/// [`face_dat_id`] against the install the environment names
/// (`scheduler_runtime::main_dll_from_env`), for callers that open their
/// `DatRoot` from the environment the same way (the launcher's character
/// preview, `//actordiag`). In-world dispatch reads the wired [`ActionMainDll`].
pub fn resolve_face(face: u8, race: u8) -> Option<u32> {
    face_dat_id(&*main_dll_from_env()?, face, race)
}

/// [`equipment_slot_dat_id`] against the environment's install; see [`resolve_face`].
pub fn resolve_equipment_slot(slot_id: u16, race: u8) -> Option<u32> {
    equipment_slot_dat_id(&*main_dll_from_env()?, slot_id, race)
}

/// [`equipment_dat_id`] against the environment's install; see [`resolve_face`].
pub fn resolve_equipment_model(slot_index: u8, model_id: u16, race: u8) -> Option<u32> {
    equipment_dat_id(&*main_dll_from_env()?, slot_index, model_id, race)
}

/// Loads the model for each ridden mount. Kept apart from
/// [`dispatch_look_driven_models`] because a mount is not chosen by the rider's
/// look at all — it is a separate actor whose model comes from the mount id.
pub fn dispatch_mount_models(
    state: Res<SceneState>,
    tracked: Res<TrackedEntities>,
    q_current: Query<&crate::components::MountModel>,
    mut load_actor_tx: MessageWriter<crate::ffxi_actor_render::LoadActorRequest>,
    mut commands: Commands,
) {
    if !state.dirty {
        return;
    }
    for wire in &state.snapshot.entities {
        let Some(mount) = state.snapshot.mount_of(wire) else {
            continue;
        };
        let id = crate::scene::mount_actor_id(wire.id);
        let Some(&bevy_e) = tracked.by_id.get(&id) else {
            continue;
        };
        if q_current.get(bevy_e).is_ok_and(|m| m.0 == mount) {
            continue;
        }

        let subject = match mount {
            kuluu_snapshot::Mount::Chocobo { colour } => {
                crate::ffxi_actor_render::ActorSubject::Mount {
                    race: crate::ffxi_actor_render::chocobo_race_for_colour(colour),
                }
            }
            // Every non-chocobo mount is an ordinary NPC-shaped model, in one
            // contiguous file-table block ordered by MOUNTTYPE.
            kuluu_snapshot::Mount::Other { mount_id } => {
                let Some(file_id) = mount_dat_id(mount_id) else {
                    warn!("mount id {mount_id} is outside the mount model block");
                    continue;
                };
                crate::ffxi_actor_render::ActorSubject::Npc {
                    file_id,
                    graph_size: 0,
                }
            }
        };
        load_actor_tx.write(crate::ffxi_actor_render::LoadActorRequest {
            entity_id: id,
            subject,
        });
        commands
            .entity(bevy_e)
            .try_insert(crate::components::MountModel(mount));
        info!("actor dispatch (mount): rider={} mount={mount:?}", wire.id);
    }
}

/// File table index of a non-chocobo mount's model. The block runs from
/// `MOUNT_QUEST_RAPTOR` (the first `MOUNTTYPE` with a model here) upward, one
/// file per id — verified against the retail DAT 2026-08-04: 0x19131 raptor,
/// 0x19133 tiger, 0x19136 bomb, 0x19141 hippogryph, each carrying a `moun`
/// chunk. Both chocobo ids are absent, hence `checked_sub`.
/// research/xim poc/game/event/ActorMountEvent.kt, ActorMountEvent.apply.
fn mount_dat_id(mount_id: u8) -> Option<u32> {
    const MOUNT_BLOCK_BASE: u32 = 0x0001_9131;
    const FIRST_MODELLED_MOUNT: u8 = 1;
    Some(MOUNT_BLOCK_BASE + u32::from(mount_id.checked_sub(FIRST_MODELLED_MOUNT)?))
}

/// Dispatches look-driven model loads. Every PC part is named by the wired
/// install's equipment table, so until `ActionMainDll` lands (it loads
/// off-thread after the root is wired) the look is left unsigned and comes
/// back on the next dirty frame; NPCs need no table and are not held up.
/// A face that cannot resolve is only reachable for a race outside 1..=8;
/// the face DAT carries the head and hair, so the failure is a loud warn —
/// a user's stderr should explain a decapitated screenshot.
pub fn dispatch_look_driven_models(
    state: Res<SceneState>,
    tracked: Res<TrackedEntities>,
    q_changed: Query<(&WorldEntity, &LookComp, Option<&EntityModel>)>,
    load_mmb_tx: MessageWriter<LoadMmbRequest>,
    mut load_actor_tx: MessageWriter<crate::ffxi_actor_render::LoadActorRequest>,
    mut commands: Commands,
    settings: Res<GraphicsSettings>,
    dll: Option<Res<ActionMainDll>>,
    dat_root: Res<crate::dat_root::SharedDatRoot>,
) {
    let Some(zone_id) = state.snapshot.zone_id else {
        return;
    };
    let Some(root) = dat_root.get() else {
        return;
    };
    // LookComp is only ever written on a dirty frame (sync_entity_looks_system
    // bails otherwise), so gating here loses no edge and keeps the unfiltered
    // query — which mount changes need, since they move no LookComp — cheap.
    if !state.dirty {
        return;
    }

    let _ = &settings;
    let dll: Option<&MainDll> = dll.as_ref().and_then(|dll| dll.0.as_deref());
    let mounted_riders: std::collections::HashSet<u32> = state
        .snapshot
        .entities
        .iter()
        .filter(|e| state.snapshot.mount_of(e).is_some())
        .map(|e| e.id)
        .collect();
    let graph_size_by_id: std::collections::HashMap<u32, u8> = state
        .snapshot
        .entities
        .iter()
        .map(|e| (e.id, e.char_flags.graph_size))
        .collect();
    for (we, look, current_model) in q_changed.iter() {
        let mounted = mounted_riders.contains(&we.id);
        let graph_size = graph_size_by_id.get(&we.id).copied().unwrap_or_default();
        let signature = EntityModel {
            look: look.0,
            mounted,
            graph_size,
        };
        if current_model == Some(&signature) {
            continue;
        }

        if let EntityLook::Equipped {
            face,
            race,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
        } = look.0
        {
            let Some(dll) = dll else {
                warn_once!(
                    "pc dispatch deferred: the wired install's FFXiMain.dll has not loaded (entity {})",
                    we.id
                );
                continue;
            };
            let mut equipment: Vec<u32> = Vec::new();
            if let Some(file_id) = face_dat_id(dll, face, race) {
                equipment.push(file_id);
            } else {
                warn!(
                    "pc face unresolved (entity {}): race {} is not a PC race (face {}) -- head/hair will not render",
                    we.id, race, face
                );
            }

            let slot_models = [head, body, hands, legs, feet, main, sub, ranged];
            debug_assert_eq!(slot_models.len(), EQUIP_SLOT_ORDER_LEN);
            let mut slot_trace: [(u8, u16, Option<u32>); 8] = Default::default();
            for (i, &model_id) in slot_models.iter().enumerate() {
                let slot_index = (i + 1) as u8;
                // A rider's hands are on the reins, so retail drops the three
                // weapon slots from the model while mounted
                // (research/xim poc/ActorModel.kt,
                // ActorModel.getHiddenSlotIds).
                let file_id = (!(mounted && WEAPON_SLOTS.contains(&slot_index)))
                    .then(|| equipment_dat_id(dll, slot_index, model_id, race))
                    .flatten();
                slot_trace[i] = (slot_index, model_id, file_id);
                if let Some(file_id) = file_id {
                    equipment.push(file_id);
                }
            }

            if slot_trace.iter().any(|(_, _, r)| r.is_none()) {
                info!(
                    "pc equip unresolved (entity {} race {}): {:?}",
                    we.id, race, slot_trace
                );
            }

            load_actor_tx.write(crate::ffxi_actor_render::LoadActorRequest {
                entity_id: we.id,
                subject: crate::ffxi_actor_render::ActorSubject::Pc {
                    race,
                    mounted,
                    equipment: equipment.clone(),
                    // Slot 2 is the body (SkeletalMeshActor.cpp SkeletalMeshActor::SetEquipModel takes
                    // waist_type from that slot's CIB); `equipment` above drops
                    // slot identity, so pass it separately.
                    body: equipment_dat_id(dll, EQUIP_SLOT_BODY, body, race),

                    // Still resolved while mounted even though the model is
                    // suppressed: load_pc reads their CIBs for the waist/shield
                    // motion selectors, which the seat pose still needs.
                    main_weapon: equipment_dat_id(dll, EQUIP_SLOT_MAIN, main, race),
                    sub_weapon: equipment_dat_id(dll, EQUIP_SLOT_SUB, sub, race),
                },
            });
            info!(
                "actor dispatch (pc): entity_id={} race={} equip={}",
                we.id,
                race,
                equipment.len()
            );
            if let Some(&bevy_e) = tracked.by_id.get(&we.id) {
                commands.entity(bevy_e).try_insert(signature);
            }
            continue;
        }

        let modelid = match look.0 {
            EntityLook::Standard { modelid } => modelid,

            EntityLook::Equipped { .. } => unreachable!(),

            EntityLook::Door { .. } | EntityLook::Transport { .. } => continue,
        };

        if modelid == 0 {
            continue;
        }

        let dat_id = npc_dat_id(modelid);
        let _ = zone_id;
        // Monster/beastmen models nest the skinned mesh under a "mode" subdir
        // (research/xim NpcModel.getMeshResources), so the gate must recurse
        // like load_npc's collect_skel_meshes — not just scan top-level chunks.
        if !crate::dat_vos2::dat_has_skinned_mesh(root, dat_id) {
            warn!(
                "actor dispatch (npc): no skinned mesh at dat_id={} for modelid={} \
                 (entity_id={}) — spawns as a nameplate with no body",
                dat_id, modelid, we.id
            );
            continue;
        }
        debug_assert!(tracked.by_id.contains_key(&we.id));

        load_actor_tx.write(crate::ffxi_actor_render::LoadActorRequest {
            entity_id: we.id,
            subject: crate::ffxi_actor_render::ActorSubject::Npc {
                file_id: dat_id,
                graph_size,
            },
        });
        info!(
            "actor dispatch (npc): entity_id={} modelid={} dat_id={}",
            we.id, modelid, dat_id
        );
        if let Some(&bevy_e) = tracked.by_id.get(&we.id) {
            commands.entity(bevy_e).try_insert(signature);
        }

        let _ = &load_mmb_tx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A FFXiMain.dll carrying only the tables the resolver reads, laid out the
    // way ffxi-dat's reader locates them (ffxi-dat/src/main_dll.rs): each table
    // is found by its big-endian marker word inside the fallback scan window (no
    // PE header here), the two required per-race tables are present so
    // `MainDll::load` accepts the file, and the equipment table's marker is its
    // own first `(file_id, count)` pair -- HumeM's face row -- so that row is
    // fixed at the real base. Layout constants are the reader's own.
    use ffxi_dat::main_dll::{
        DANCE_SKILL_HINT as DLL_DANCE_SKILL_MARKER, EQUIPMENT_HINT as DLL_EQUIPMENT_MARKER,
        EQUIPMENT_RACE_STRIDE as DLL_EQUIPMENT_RACE_STRIDE,
        EQUIPMENT_SLOT_STRIDE as DLL_EQUIPMENT_SLOT_STRIDE, SCAN_START as DLL_SCAN_START,
        WEAPON_SKILL_HINT as DLL_WEAPON_SKILL_MARKER,
    };
    const DLL_EQUIPMENT_TABLE_AT: usize = DLL_SCAN_START + 0x1000;
    const HUME_M_FACE_BASE: u32 = DLL_EQUIPMENT_MARKER.swap_bytes();

    /// `rows`: `(race, slot, bands)` with bands as the `(first_file_id, count)`
    /// pairs the table stores.
    struct SyntheticDll {
        dir: std::path::PathBuf,
        dll: MainDll,
    }

    impl SyntheticDll {
        fn new(tag: &str, rows: &[(u8, u8, &[(u32, u32)])]) -> Self {
            let mut bytes = vec![
                0u8;
                DLL_EQUIPMENT_TABLE_AT
                    + DLL_EQUIPMENT_RACE_STRIDE * PC_LOOK_RACES.count()
            ];
            bytes[DLL_SCAN_START..DLL_SCAN_START + 4]
                .copy_from_slice(&DLL_WEAPON_SKILL_MARKER.to_be_bytes());
            bytes[DLL_SCAN_START + 0x10..DLL_SCAN_START + 0x14]
                .copy_from_slice(&DLL_DANCE_SKILL_MARKER.to_be_bytes());
            for &(race, slot, bands) in rows {
                let row = DLL_EQUIPMENT_TABLE_AT
                    + DLL_EQUIPMENT_RACE_STRIDE * usize::from(race - 1)
                    + DLL_EQUIPMENT_SLOT_STRIDE * usize::from(slot);
                for (i, &(first, count)) in bands.iter().enumerate() {
                    let at = row + i * 8;
                    bytes[at..at + 4].copy_from_slice(&first.to_le_bytes());
                    bytes[at + 4..at + 8].copy_from_slice(&count.to_le_bytes());
                }
            }
            assert_eq!(
                &bytes[DLL_EQUIPMENT_TABLE_AT..DLL_EQUIPMENT_TABLE_AT + 4],
                &DLL_EQUIPMENT_MARKER.to_be_bytes(),
                "race 1 face row must start at HumeM's base for the reader to find the table"
            );
            let dir = std::env::temp_dir().join(format!(
                "kuluu-render-look-resolver-{tag}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("temp dir");
            std::fs::write(dir.join("FFXiMain.dll"), &bytes).expect("write synthetic dll");
            let dll = MainDll::load(&dir).expect("synthetic dll loads");
            Self { dir, dll }
        }
    }

    impl Drop for SyntheticDll {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// HumeM face and head, Tarutaru F face, Elvaan M body: the retail bands the
    /// unit assertions below step along.
    fn fixture(tag: &str) -> SyntheticDll {
        SyntheticDll::new(
            tag,
            &[
                (1, 0, &[(HUME_M_FACE_BASE, 32)]),
                (
                    1,
                    1,
                    &[
                        (7112, 256),
                        (63323, 64),
                        (71247, 256),
                        (98787, 32),
                        (102961, 64),
                    ],
                ),
                (
                    1,
                    6,
                    &[(8392, 512), (63643, 128), (72527, 256), (107301, 32)],
                ),
                (3, 2, &[(13720, 256)]),
                (6, 0, &[(22952, 32)]),
            ],
        )
    }

    #[test]
    fn mount_dat_id_maps_the_block_and_rejects_the_chocobo_ids() {
        /// Verified against the retail DAT 2026-08-04 by dumping each file's
        /// skeleton chunk: the block is MOUNTTYPE-ordered from QUEST_RAPTOR.
        /// The base stays literal here: it is the dump's second source, not
        /// the resolver's own const.
        const MOUNT_BLOCK_BASE_PINNED: u32 = 0x0001_9131;
        assert_eq!(
            mount_dat_id(1),
            Some(MOUNT_BLOCK_BASE_PINNED),
            "MOUNT_QUEST_RAPTOR (wyve)"
        );
        assert_eq!(mount_dat_id(3), Some(0x0001_9133)); // MOUNT_TIGER, "tige"
        assert_eq!(mount_dat_id(17), Some(0x0001_9141)); // MOUNT_HIPPOGRYPH, "kiri"

        // MOUNT_CHOCOBO has no file here at all — it is a PC race config — so a
        // chocobo must never reach this path.
        assert_eq!(mount_dat_id(0), None);
    }

    #[test]
    fn mounted_rider_loses_only_the_weapon_slots() {
        assert_eq!(WEAPON_SLOTS, [6, 7, 8]);
        for slot in [1u8, 2, 3, 4, 5] {
            assert!(
                !WEAPON_SLOTS.contains(&slot),
                "slot {slot} is body armour and must still render while mounted"
            );
        }
    }

    #[test]
    fn npc_dat_id_bucket_lower_edges() {
        assert_eq!(npc_dat_id(0), 1300);
        assert_eq!(npc_dat_id(1500), 51795);
        assert_eq!(npc_dat_id(3000), 99907);
        // The disassembly's own anchor: `(m - 3500) + 101739`.
        assert_eq!(npc_dat_id(3500), 101739);
    }

    #[test]
    fn npc_dat_id_bucket_boundary_off_by_one() {
        assert_eq!(npc_dat_id(1499), 1499 + 1300);
        assert_eq!(npc_dat_id(1500), 51795);
        assert_eq!(npc_dat_id(2999), 2999 + 50295);
        assert_eq!(npc_dat_id(3000), 99907);
        assert_eq!(npc_dat_id(3499), 3499 + 96907);
        assert_eq!(npc_dat_id(3500), 3500 + 98239);
    }

    /// The 3000-range base applies all the way to 3499: the split is at 3500,
    /// not wherever the install's registered files happen to thin out.
    #[test]
    fn npc_dat_id_keeps_the_3000_range_base_through_3499() {
        for m in [3193u16, 3194, 3300, 3499] {
            assert_eq!(npc_dat_id(m), u32::from(m) + 96907, "modelid {m}");
        }
        assert_ne!(npc_dat_id(3500), npc_dat_id(3499) + 1);
    }

    #[test]
    fn equipment_slot_extraction() {
        let f = fixture("slot-extraction");
        assert_eq!(equipment_slot_dat_id(&f.dll, 0x1000, 1), Some(7112));
        assert_eq!(equipment_slot_dat_id(&f.dll, 0x2004, 3), Some(13724));
    }

    #[test]
    fn equipment_model_retags_bare_wire_ids() {
        let f = fixture("retag");
        assert_eq!(equipment_dat_id(&f.dll, 2, 4, 3), Some(13724));

        assert_eq!(
            equipment_slot_dat_id(&f.dll, 4, 3),
            None,
            "a bare model id is slot 0: an empty slot, not a face"
        );

        assert_eq!(equipment_dat_id(&f.dll, 2, 0x2004, 3), Some(13724));

        assert_eq!(equipment_dat_id(&f.dll, 0, 4, 3), None);
        assert_eq!(equipment_dat_id(&f.dll, 9, 4, 3), None);
    }

    #[test]
    fn equipment_sentinels_return_none() {
        let f = fixture("sentinels");
        assert_eq!(equipment_slot_dat_id(&f.dll, 0x0000, 3), None);
        assert_eq!(equipment_slot_dat_id(&f.dll, 0x2004, 0), None);
        assert_eq!(equipment_slot_dat_id(&f.dll, 0x2004, 9), None);
        assert_eq!(equipment_slot_dat_id(&f.dll, 0x2000, 3), Some(13720));
        assert_eq!(
            equipment_dat_id(&f.dll, EQUIP_SLOT_RANGED, 0, 1),
            None,
            "a slot the race's row leaves empty resolves to nothing, not to a clamp"
        );
    }

    #[test]
    fn equipment_bands_partition_the_model_id_space_in_order() {
        let f = fixture("bands");
        let head = |id: u16| equipment_slot_dat_id(&f.dll, 0x1000 | id, 1);
        assert_eq!(head(0), Some(7112));
        assert_eq!(head(1), Some(7113));
        assert_eq!(head(255), Some(7367));
        assert_eq!(head(256), Some(63323));
        assert_eq!(head(319), Some(63386));
        assert_eq!(head(320), Some(71247));
        assert_eq!(head(575), Some(71502));
        assert_eq!(head(576), Some(98787));
        assert_eq!(head(607), Some(98818));
        assert_eq!(head(608), Some(102961));
        assert_eq!(head(671), Some(103024));

        // Past the last band: retail clamps to model 0 of the slot ("wrong GRP
        // number", SkeletalMeshActor.cpp SkeletalMeshActor::SetEquipModel), so
        // the head slot's base file comes back instead of a dropped body part.
        assert_eq!(head(672), Some(7112));
        assert_eq!(head(0xFFF), Some(7112));
    }

    #[test]
    fn main_hand_ids_past_the_old_hand_table_still_resolve() {
        let f = fixture("main-hand");
        let main = |id: u16| equipment_dat_id(&f.dll, EQUIP_SLOT_MAIN, id, 1);
        assert_eq!(main(0), Some(8392));
        assert_eq!(main(896), Some(107301));
        assert_eq!(main(927), Some(107332));
        assert_eq!(main(928), Some(8392), "past every band: the clamp");
    }

    #[test]
    fn face_is_zero_based_direct_index() {
        let f = fixture("face-index");
        assert_eq!(face_dat_id(&f.dll, 0, 1), Some(7080));
        assert_eq!(face_dat_id(&f.dll, 1, 1), Some(7081));
        assert_eq!(face_dat_id(&f.dll, 17, 1), Some(7097));
        // Face 8B == 15 is LSB's creation maximum.
        assert_eq!(face_dat_id(&f.dll, 15, 1), Some(7095));
        assert_eq!(
            face_dat_id(&f.dll, 0, 6),
            Some(22952),
            "Tarutaru F: the row the dll places at 22952"
        );
        assert_eq!(face_dat_id(&f.dll, 31, 6), Some(22983));
    }

    #[test]
    fn face_band_boundaries() {
        let f = fixture("face-bounds");
        // 32 face entries (0..31); index 31 is the last face file. An
        // out-of-band face clamps to face 0 the way retail does ("wrong GRP
        // number", SkeletalMeshActor.cpp SkeletalMeshActor::SetEquipModel) --
        // never a decapitated PC.
        assert_eq!(face_dat_id(&f.dll, 31, 1), Some(7111));
        assert_eq!(face_dat_id(&f.dll, 32, 1), Some(7080));
        assert_eq!(face_dat_id(&f.dll, 255, 6), Some(22952));
        assert_eq!(
            face_dat_id(&f.dll, 0, 0),
            None,
            "invalid races reject; a race with no face row is nothing, not a clamp"
        );
        assert_eq!(face_dat_id(&f.dll, 0, 9), None);
        assert_eq!(face_dat_id(&f.dll, 0, 3), None);
    }
}
