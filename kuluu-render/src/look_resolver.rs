use bevy::prelude::*;
use kuluu_snapshot::EntityLook;

use crate::components::{EntityModel, LookComp, WorldEntity};
use crate::dat_mmb::LoadMmbRequest;
use crate::graphics_settings::GraphicsSettings;
use crate::scene::TrackedEntities;
use crate::snapshot::SceneState;

const EQUIP_SLOT_ORDER_LEN: usize = 8;

// Slot numbering retail switches on when collecting per-slot CIB bytes
// (research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp:1656-1688:
// 2 = body, 5 = feet, 6 = main, 7 = sub, 8 = ranged), matching the order of
// `slot_models` below.
const EQUIP_SLOT_BODY: u8 = 2;
const EQUIP_SLOT_MAIN: u8 = 6;
const EQUIP_SLOT_SUB: u8 = 7;
const EQUIP_SLOT_RANGED: u8 = 8;
const WEAPON_SLOTS: [u8; 3] = [EQUIP_SLOT_MAIN, EQUIP_SLOT_SUB, EQUIP_SLOT_RANGED];

// FFXiMain `.text` VA 0x100C513D (retail client disassembly; the full quote
// lives in out-of-tree cexi research notes, not in this repo): four ranges
// split at 1500 / 3000 / 3500, the top one computed as `(m - 3500) + 101739`.
//
// The 3000-range is only *registered* for 3000..=3193 — every fid for 3194..3499
// is VTABLE=0 and no retail mob_pools row uses one — so range 3's extent looks
// like a boundary from the data alone. Folding that hole into the split makes
// modelid 3193 land on 101739, the same fid retail reaches at 3500, which is why
// this read the part right and the whole wrong; every modelid above it was off by
// the 307-slot gap. Do not re-derive these from registration extent.
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

pub fn resolve_equipment_slot(slot_id: u16, race: u8) -> Option<u32> {
    let slot = u32::from((slot_id >> 12) & 0xF);
    let id = u32::from(slot_id & 0x0FFF);

    if slot == 0 || slot > 8 || race == 0 || race > 8 {
        return None;
    }
    let bps = PC_MODEL_IDS.get((race - 1) as usize)?.get(slot as usize)?;

    let mut chosen: Option<(u16, u32)> = None;
    for &(thr, base) in *bps {
        if u32::from(thr) <= id {
            chosen = Some((thr, base));
        } else {
            break;
        }
    }
    let (thr, base) = chosen?;
    if base == 0 {
        // Retail clamps a model id past the slot's table to model 0 instead of
        // dropping the part ("wrong GRP number",
        // research/XIClient/src/XIClient/source/World/Actor/SkeletalMeshActor.cpp:489-494),
        // so an out-of-band id renders the slot's base model, never a missing
        // body part.
        let (_, first_base) = bps.first()?;
        if *first_base == 0 {
            return None;
        }
        return Some(*first_base);
    }
    Some(base + id - u32::from(thr))
}

pub fn resolve_equipment_model(slot_index: u8, model_id: u16, race: u8) -> Option<u32> {
    if slot_index == 0 || slot_index > 8 {
        return None;
    }
    let slot_id = (u16::from(slot_index) << 12) | (model_id & 0x0FFF);
    resolve_equipment_slot(slot_id, race)
}

type Breakpoints = &'static [(u16, u32)];
const PC_MODEL_IDS: [[Breakpoints; 9]; 8] = [
    [
        &[(0, 7080), (32, 0)],
        &[
            (0, 7112),
            (256, 63323),
            (320, 71247),
            (576, 98787),
            (608, 102961),
            (672, 0),
        ],
        &[
            (0, 7368),
            (256, 63387),
            (320, 71503),
            (576, 98819),
            (608, 103025),
            (672, 0),
        ],
        &[
            (0, 7624),
            (256, 63451),
            (320, 71759),
            (576, 98851),
            (608, 103089),
            (672, 0),
        ],
        &[
            (0, 7880),
            (256, 63515),
            (320, 72015),
            (576, 98883),
            (608, 103153),
            (672, 0),
        ],
        &[
            (0, 8136),
            (256, 63579),
            (320, 72271),
            (576, 98915),
            (608, 103217),
            (672, 0),
        ],
        &[
            (0, 8392),
            (512, 63643),
            (640, 72527),
            (896, 107301),
            (928, 0),
        ],
        &[
            (0, 41199),
            (512, 66459),
            (640, 81999),
            (896, 105201),
            (928, 0),
        ],
        &[(0, 9416), (256, 0)],
    ],
    [
        &[(0, 10256), (32, 0)],
        &[
            (0, 10288),
            (256, 63771),
            (320, 72783),
            (576, 98947),
            (608, 103281),
            (672, 0),
        ],
        &[
            (0, 10544),
            (256, 63835),
            (320, 73039),
            (576, 98979),
            (608, 103345),
            (672, 0),
        ],
        &[
            (0, 10800),
            (256, 63899),
            (320, 73295),
            (576, 99011),
            (608, 103409),
            (672, 0),
        ],
        &[
            (0, 11056),
            (256, 63963),
            (320, 73551),
            (576, 99043),
            (608, 103473),
            (672, 0),
        ],
        &[
            (0, 11312),
            (256, 64027),
            (320, 73807),
            (576, 99075),
            (608, 103537),
            (672, 0),
        ],
        &[
            (0, 11568),
            (512, 64091),
            (640, 74063),
            (896, 107601),
            (928, 0),
        ],
        &[
            (0, 42479),
            (512, 66587),
            (640, 82255),
            (896, 105501),
            (928, 0),
        ],
        &[(0, 12592), (256, 0)],
    ],
    [
        &[(0, 13432), (32, 0)],
        &[
            (0, 13464),
            (256, 64219),
            (320, 74319),
            (576, 99107),
            (608, 103601),
            (672, 0),
        ],
        &[
            (0, 13720),
            (256, 64283),
            (320, 74575),
            (576, 99139),
            (608, 103665),
            (672, 0),
        ],
        &[
            (0, 13976),
            (256, 64347),
            (320, 74831),
            (576, 99171),
            (608, 103729),
            (672, 0),
        ],
        &[
            (0, 14232),
            (256, 64411),
            (320, 75087),
            (576, 99203),
            (608, 103793),
            (672, 0),
        ],
        &[
            (0, 14488),
            (256, 64475),
            (320, 75343),
            (576, 99235),
            (608, 103857),
            (672, 0),
        ],
        &[
            (0, 14744),
            (512, 64539),
            (640, 75599),
            (896, 107901),
            (928, 0),
        ],
        &[
            (0, 43759),
            (512, 66715),
            (640, 82511),
            (896, 105801),
            (928, 0),
        ],
        &[(0, 15768), (256, 0)],
    ],
    [
        &[(0, 16608), (32, 0)],
        &[
            (0, 16640),
            (256, 64667),
            (320, 75855),
            (576, 99267),
            (608, 103921),
            (672, 0),
        ],
        &[
            (0, 16896),
            (256, 64731),
            (320, 76111),
            (576, 99299),
            (608, 103985),
            (672, 0),
        ],
        &[
            (0, 17152),
            (256, 64795),
            (320, 76367),
            (576, 99331),
            (608, 104049),
            (672, 0),
        ],
        &[
            (0, 17408),
            (256, 64859),
            (320, 76623),
            (576, 99363),
            (608, 104113),
            (672, 0),
        ],
        &[
            (0, 17664),
            (256, 64923),
            (320, 76879),
            (576, 99395),
            (608, 104177),
            (672, 0),
        ],
        &[
            (0, 17920),
            (512, 64987),
            (640, 77135),
            (896, 108201),
            (928, 0),
        ],
        &[
            (0, 45039),
            (512, 66843),
            (640, 82767),
            (896, 106101),
            (928, 0),
        ],
        &[(0, 18944), (256, 0)],
    ],
    [
        &[(0, 19784), (32, 0)],
        &[
            (0, 19816),
            (256, 65115),
            (320, 77391),
            (576, 99427),
            (608, 104241),
            (672, 0),
        ],
        &[
            (0, 20072),
            (256, 65179),
            (320, 77647),
            (576, 99459),
            (608, 104305),
            (672, 0),
        ],
        &[
            (0, 20328),
            (256, 65243),
            (320, 77903),
            (576, 99491),
            (608, 104369),
            (672, 0),
        ],
        &[
            (0, 20584),
            (256, 65307),
            (320, 78159),
            (576, 99523),
            (608, 104433),
            (672, 0),
        ],
        &[
            (0, 20840),
            (256, 65371),
            (320, 78415),
            (576, 99555),
            (608, 104497),
            (672, 0),
        ],
        &[
            (0, 21096),
            (512, 65435),
            (640, 78671),
            (896, 108501),
            (928, 0),
        ],
        &[
            (0, 46319),
            (512, 66971),
            (640, 83023),
            (896, 106401),
            (928, 0),
        ],
        &[(0, 22120), (256, 0)],
    ],
    [
        &[(0, 22960), (32, 0)],
        &[
            (0, 19816),
            (256, 65115),
            (320, 77391),
            (576, 99427),
            (608, 104241),
            (672, 0),
        ],
        &[
            (0, 20072),
            (256, 65179),
            (320, 77647),
            (576, 99459),
            (608, 104305),
            (672, 0),
        ],
        &[
            (0, 20328),
            (256, 65243),
            (320, 77903),
            (576, 99491),
            (608, 104369),
            (672, 0),
        ],
        &[
            (0, 20584),
            (256, 65307),
            (320, 78159),
            (576, 99523),
            (608, 104433),
            (672, 0),
        ],
        &[
            (0, 20840),
            (256, 65371),
            (320, 78415),
            (576, 99555),
            (608, 104497),
            (672, 0),
        ],
        &[
            (0, 21096),
            (512, 65435),
            (640, 78671),
            (896, 108501),
            (928, 0),
        ],
        &[
            (0, 46319),
            (512, 66971),
            (640, 83023),
            (896, 106401),
            (928, 0),
        ],
        &[(0, 22120), (256, 0)],
    ],
    [
        &[(0, 23184), (32, 0)],
        &[
            (0, 23216),
            (256, 65563),
            (320, 78927),
            (576, 99587),
            (608, 104561),
            (672, 0),
        ],
        &[
            (0, 23472),
            (256, 65627),
            (320, 79183),
            (576, 99619),
            (608, 104625),
            (672, 0),
        ],
        &[
            (0, 23728),
            (256, 65691),
            (320, 79439),
            (576, 99651),
            (608, 104689),
            (672, 0),
        ],
        &[
            (0, 23984),
            (256, 65755),
            (320, 79695),
            (576, 99683),
            (608, 104753),
            (672, 0),
        ],
        &[
            (0, 24240),
            (256, 65819),
            (320, 79951),
            (576, 99715),
            (608, 104817),
            (672, 0),
        ],
        &[
            (0, 24496),
            (512, 65883),
            (640, 80207),
            (896, 108801),
            (928, 0),
        ],
        &[
            (0, 47599),
            (512, 67099),
            (640, 83279),
            (896, 106701),
            (928, 0),
        ],
        &[(0, 25520), (256, 0)],
    ],
    [
        &[(0, 26360), (32, 0)],
        &[
            (0, 26392),
            (256, 66011),
            (320, 80463),
            (576, 99747),
            (608, 104881),
            (672, 0),
        ],
        &[
            (0, 26648),
            (256, 66075),
            (320, 80719),
            (576, 99779),
            (608, 104945),
            (672, 0),
        ],
        &[
            (0, 26904),
            (256, 66139),
            (320, 80975),
            (576, 99811),
            (608, 105009),
            (672, 0),
        ],
        &[
            (0, 27160),
            (256, 66203),
            (320, 81231),
            (576, 99843),
            (608, 105073),
            (672, 0),
        ],
        &[
            (0, 27416),
            (256, 66267),
            (320, 81487),
            (576, 99875),
            (608, 105137),
            (672, 0),
        ],
        &[
            (0, 27672),
            (512, 66331),
            (640, 81743),
            (896, 109101),
            (928, 0),
        ],
        &[
            (0, 48879),
            (512, 67227),
            (640, 83535),
            (896, 107001),
            (928, 0),
        ],
        &[(0, 28696), (256, 0)],
    ],
];

pub fn resolve_face(face: u8, race: u8) -> Option<u32> {
    // The face byte is the 0-based index into the per-race Face sub-table (slot 0
    // of the FFXiMain.dll equipment lookup): file = base + face, no -1. LSB caps
    // creation faces at 15 ("Face 8B", vendor/server/src/login/login_helpers.cpp),
    // the stylist spans the full slot; xim EquipmentModelTable.getItemModelPath
    // indexes the Face slot directly the same way. PC_MODEL_IDS slot 0 is the
    // single source for base/count — `[(0, base), (count, 0)]` — so don't
    // hand-duplicate the bases.
    if race == 0 || race > 8 {
        return None;
    }
    let face_band = PC_MODEL_IDS[(race - 1) as usize][0];
    let base = face_band.first()?.1;
    let count = face_band.get(1).map_or(u16::MAX, |&(thr, _)| thr);
    if base == 0 {
        return None;
    }
    if u16::from(face) >= count {
        // Retail clamp: an id past the slot's table renders model 0, never a
        // missing part ("wrong GRP number", research/XIClient/src/XIClient/
        // source/World/Actor/SkeletalMeshActor.cpp:489-494). For the face slot
        // that means an out-of-band face byte renders face 0 instead of a
        // decapitated PC. Loud because it means the server sent a face this
        // client's tables don't know -- the wrong-face render needs explaining.
        warn!("face {face} out of band for race {race}: clamping to face 0 (retail behavior)");
        return Some(base);
    }
    Some(base + u32::from(face))
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
                crate::ffxi_actor_render::ActorSubject::Npc { file_id }
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

pub fn dispatch_look_driven_models(
    state: Res<SceneState>,
    tracked: Res<TrackedEntities>,
    q_changed: Query<(&WorldEntity, &LookComp, Option<&EntityModel>)>,
    load_mmb_tx: MessageWriter<LoadMmbRequest>,
    mut load_actor_tx: MessageWriter<crate::ffxi_actor_render::LoadActorRequest>,
    mut commands: Commands,
    settings: Res<GraphicsSettings>,
) {
    let Some(zone_id) = state.snapshot.zone_id else {
        return;
    };
    // LookComp is only ever written on a dirty frame (sync_entity_looks_system
    // bails otherwise), so gating here loses no edge and keeps the unfiltered
    // query — which mount changes need, since they move no LookComp — cheap.
    if !state.dirty {
        return;
    }

    let _ = &settings;
    let mounted_riders: std::collections::HashSet<u32> = state
        .snapshot
        .entities
        .iter()
        .filter(|e| state.snapshot.mount_of(e).is_some())
        .map(|e| e.id)
        .collect();
    for (we, look, current_model) in q_changed.iter() {
        let mounted = mounted_riders.contains(&we.id);
        let signature = EntityModel {
            look: look.0,
            mounted,
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
            let mut equipment: Vec<u32> = Vec::new();
            if let Some(file_id) = resolve_face(face, race) {
                equipment.push(file_id);
            } else {
                // Only reachable for a race outside 1..=8; the face DAT carries
                // the head and hair, so it must be loud enough for a user's
                // stderr to explain a decapitated screenshot.
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
                    .then(|| resolve_equipment_model(slot_index, model_id, race))
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
                    // Slot 2 is the body (SkeletalMeshActor.cpp:1659 takes
                    // waist_type from that slot's CIB); `equipment` above drops
                    // slot identity, so pass it separately.
                    body: resolve_equipment_model(EQUIP_SLOT_BODY, body, race),

                    // Still resolved while mounted even though the model is
                    // suppressed: load_pc reads their CIBs for the waist/shield
                    // motion selectors, which the seat pose still needs.
                    main_weapon: resolve_equipment_model(EQUIP_SLOT_MAIN, main, race),
                    sub_weapon: resolve_equipment_model(EQUIP_SLOT_SUB, sub, race),
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
        if !crate::dat_vos2::dat_has_skinned_mesh(dat_id) {
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
            subject: crate::ffxi_actor_render::ActorSubject::Npc { file_id: dat_id },
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

    #[test]
    fn mount_dat_id_maps_the_block_and_rejects_the_chocobo_ids() {
        // Verified against the retail DAT 2026-08-04 by dumping each file's
        // skeleton chunk: the block is MOUNTTYPE-ordered from QUEST_RAPTOR.
        assert_eq!(mount_dat_id(1), Some(0x0001_9131)); // MOUNT_QUEST_RAPTOR, "wyve"
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

    // 3194..=3499 is a registration hole, not a range boundary: retail keeps
    // applying the 3000-range base across it. Reading the hole as the split is
    // what made every modelid at or above it resolve 307 slots high.
    #[test]
    fn npc_dat_id_spans_the_unregistered_hole_with_the_3000_range_base() {
        for m in [3193u16, 3194, 3300, 3499] {
            assert_eq!(npc_dat_id(m), u32::from(m) + 96907, "modelid {m}");
        }
        assert_ne!(npc_dat_id(3500), npc_dat_id(3499) + 1);
    }

    #[test]
    fn equipment_slot_extraction() {
        assert_eq!(resolve_equipment_slot(0x1000, 3), Some(13464));

        assert_eq!(resolve_equipment_slot(0x2004, 3), Some(13724));
    }

    #[test]
    fn equipment_model_retags_bare_wire_ids() {
        assert_eq!(resolve_equipment_model(2, 4, 3), Some(13724));

        assert_eq!(resolve_equipment_slot(4, 3), None);

        assert_eq!(resolve_equipment_model(2, 0x2004, 3), Some(13724));

        assert_eq!(resolve_equipment_model(0, 4, 3), None);
        assert_eq!(resolve_equipment_model(9, 4, 3), None);
    }

    #[test]
    fn equipment_sentinels_return_none() {
        assert_eq!(resolve_equipment_slot(0x0000, 3), None);
        assert_eq!(resolve_equipment_slot(0x2004, 0), None);
        assert_eq!(resolve_equipment_slot(0x2000, 3), Some(13720));
    }

    #[test]
    fn equipment_table_band_samples_race1_head() {
        assert_eq!(resolve_equipment_slot(0x1001, 1), Some(7113));

        assert_eq!(resolve_equipment_slot(0x1100, 1), Some(63323));

        assert_eq!(resolve_equipment_slot(0x1140, 1), Some(71247));

        assert_eq!(resolve_equipment_slot(0x1240, 1), Some(98787));

        assert_eq!(resolve_equipment_slot(0x1260, 1), Some(102961));

        // Past the last band: retail clamps to model 0 of the slot ("wrong GRP
        // number", SkeletalMeshActor.cpp:489-494), so the head slot's base file
        // comes back instead of a dropped body part.
        assert_eq!(resolve_equipment_slot(0x12A0, 1), Some(7112));
    }

    #[test]
    fn equipment_per_race_correctness() {
        assert_eq!(resolve_equipment_slot(0x2008, 8), Some(26656));

        assert_eq!(resolve_equipment_slot(0x4004, 7), Some(23988));
    }

    #[test]
    fn equipment_rejects_high_race_codes() {
        assert_eq!(resolve_equipment_slot(0x2004, 29), None);
    }

    #[test]
    fn face_is_zero_based_direct_index() {
        // xim EquipmentModelTable indexes the Face slot directly: file = base + face.
        // HumeM face base is 7080 (PC_MODEL_IDS[0][0]).
        assert_eq!(resolve_face(0, 1), Some(7080));
        assert_eq!(resolve_face(1, 1), Some(7081));
        assert_eq!(resolve_face(17, 1), Some(7097));
        // Mithra (race 7) face base is 23184.
        assert_eq!(resolve_face(0, 7), Some(23184));
        // Face 8B == 15 is LSB's creation maximum.
        assert_eq!(resolve_face(15, 7), Some(23199));
    }

    #[test]
    fn face_band_boundaries() {
        // 32 face entries (0..31); index 31 is the last face file. An
        // out-of-band face clamps to face 0 the way retail does ("wrong GRP
        // number", SkeletalMeshActor.cpp:489-494) -- never a decapitated PC.
        assert_eq!(resolve_face(31, 1), Some(7111));
        assert_eq!(resolve_face(32, 1), Some(7080));
        assert_eq!(resolve_face(255, 5), Some(19784));
        assert_eq!(resolve_equipment_slot(0x1000, 1), Some(7112));
        // Invalid races reject.
        assert_eq!(resolve_face(0, 0), None);
        assert_eq!(resolve_face(0, 9), None);
    }
}
