use bevy::prelude::*;
use ffxi_viewer_wire::EntityLook;

use crate::components::{EntityModel, LookComp, WorldEntity};
use crate::dat_mmb::LoadMmbRequest;
use crate::graphics_settings::GraphicsSettings;
use crate::scene::TrackedEntities;
use crate::snapshot::SceneState;

const EQUIP_SLOT_ORDER_LEN: usize = 8;

// Transcribed from research/xim NpcTable.kt:107-114 (getNpcModelIndex);
// the top bucket's base is flagged "Speculated" upstream.
pub fn npc_dat_id(modelid: u16) -> u32 {
    let m = modelid as u32;
    if m < 1500 {
        m + 0x514
    } else if m < 3000 {
        m + 0xC477
    } else if m < 3193 {
        m + 0x17A8B
    } else {
        m + 0x180F2
    }
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
        return None;
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
    if base == 0 || u16::from(face) >= count {
        return None;
    }
    Some(base + u32::from(face))
}

pub fn dispatch_look_driven_models(
    state: Res<SceneState>,
    tracked: Res<TrackedEntities>,
    q_changed: Query<(&WorldEntity, &LookComp, Option<&EntityModel>), Changed<LookComp>>,
    load_mmb_tx: MessageWriter<LoadMmbRequest>,
    mut load_actor_tx: MessageWriter<crate::ffxi_actor_render::LoadActorRequest>,
    mut commands: Commands,
    settings: Res<GraphicsSettings>,
) {
    let Some(zone_id) = state.snapshot.zone_id else {
        return;
    };

    let _ = &settings;
    for (we, look, current_model) in q_changed.iter() {
        if let Some(EntityModel(sig)) = current_model {
            if *sig == look.0 {
                continue;
            }
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
            }

            let slot_models = [head, body, hands, legs, feet, main, sub, ranged];
            debug_assert_eq!(slot_models.len(), EQUIP_SLOT_ORDER_LEN);
            let mut slot_trace: [(u8, u16, Option<u32>); 8] = Default::default();
            for (i, &model_id) in slot_models.iter().enumerate() {
                let slot_index = (i + 1) as u8;
                let file_id = resolve_equipment_model(slot_index, model_id, race);
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
                    equipment: equipment.clone(),

                    main_weapon: resolve_equipment_model(6, main, race),
                    sub_weapon: resolve_equipment_model(7, sub, race),
                },
            });
            info!(
                "actor dispatch (pc): entity_id={} race={} equip={}",
                we.id,
                race,
                equipment.len()
            );
            if let Some(&bevy_e) = tracked.by_id.get(&we.id) {
                commands.entity(bevy_e).try_insert(EntityModel(look.0));
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
            commands.entity(bevy_e).try_insert(EntityModel(look.0));
        }

        let _ = &load_mmb_tx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn npc_dat_id_bucket_lower_edges() {
        assert_eq!(npc_dat_id(0), 1300);
        assert_eq!(npc_dat_id(1500), 51795);
        assert_eq!(npc_dat_id(3000), 99907);
        assert_eq!(npc_dat_id(3193), 101739);
    }

    #[test]
    fn npc_dat_id_bucket_boundary_off_by_one() {
        assert_eq!(npc_dat_id(1499), 1499 + 1300);
        assert_eq!(npc_dat_id(1500), 51795);
        assert_eq!(npc_dat_id(2999), 2999 + 0xC477);
        assert_eq!(npc_dat_id(3000), 99907);
        assert_eq!(npc_dat_id(3192), 3192 + 0x17A8B);
        assert_eq!(npc_dat_id(3193), 3193 + 0x180F2);
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

        assert_eq!(resolve_equipment_slot(0x12A0, 1), None);
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
        // 32 face entries (0..31); index 31 is the last face file, 32 collides
        // with the head slot (HumeM head base 7112).
        assert_eq!(resolve_face(31, 1), Some(7111));
        assert_eq!(resolve_face(32, 1), None);
        assert_eq!(resolve_equipment_slot(0x1000, 1), Some(7112));
        // Invalid races reject.
        assert_eq!(resolve_face(0, 0), None);
        assert_eq!(resolve_face(0, 9), None);
    }
}
