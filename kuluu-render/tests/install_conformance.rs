//! The look resolver and the shipped fallback tables against the install's own
//! FFXiMain.dll. Self-skips without an install (`DatRoot::from_env_or_default`),
//! so point `FFXI_DAT_PATH` at each KNOWN_CLIENTS install to prove both.

use ffxi_dat::main_dll::MainDll;
use ffxi_dat::resource_dir::ResourceDir;
use ffxi_dat::DatRoot;
use kuluu_render::combat_stance::{motion_dat_fallback, motion_dat_for_race, motion_dat_for_skel};
use kuluu_render::dat_vos2::{skeleton_file_id_fallback, skeleton_file_id_for_race};
use kuluu_render::look_resolver::{
    equipment_dat_id, equipment_slot_dat_id, face_dat_id, npc_dat_id, resolve_equipment_model,
    resolve_face, PC_LOOK_RACES,
};
use kuluu_render::scheduler_runtime::install_root_from_env;

/// Retail slot numbers: 0 face, 1 head .. 5 feet, 6 main, 7 sub, 8 ranged.
const SLOTS: std::ops::RangeInclusive<u8> = 0..=8;
const FACE_SLOT: u8 = 0;
const MAIN_SLOT: u8 = 6;
const SUB_SLOT: u8 = 7;
/// A wire slot id carries a 12-bit model id.
const MODEL_ID_SPACE: u16 = 0x1000;
const SLOT_ID_SHIFT: u16 = 12;

fn install() -> Option<(DatRoot, MainDll)> {
    let root = ffxi_dat::archive::open_test_install()?;
    let dll = MainDll::load(root.root()).ok()?;
    Some((root, dll))
}

/// Every id of every (race, slot) row: the resolver is the dll walk, and past
/// the row's last band it is the row's model 0 (retail's "wrong GRP number"
/// clamp), not a dropped part. A full sweep covers every band's first id,
/// last id and the one past it without knowing where the bands fall.
#[test]
fn equipment_resolver_is_the_dll_walk_with_the_model_zero_clamp() {
    let Some((_, dll)) = install() else {
        eprintln!("no FFXI install; skipping");
        return;
    };
    for race in PC_LOOK_RACES {
        for slot in SLOTS {
            let walk = |id: u16| dll.equipment_model_index(race, slot, id);
            let model_zero = walk(0)
                .unwrap_or_else(|| panic!("race {race} slot {slot}: the dll row has no model 0"));
            let mut clamped = 0usize;
            for id in 0..MODEL_ID_SPACE {
                let want = walk(id).unwrap_or(model_zero);
                if walk(id).is_none() {
                    clamped += 1;
                }
                let got = if slot == FACE_SLOT {
                    if id > u16::from(u8::MAX) {
                        continue;
                    }
                    face_dat_id(&dll, id as u8, race)
                } else {
                    let via_index = equipment_dat_id(&dll, slot, id, race);
                    let via_slot_id =
                        equipment_slot_dat_id(&dll, (u16::from(slot) << SLOT_ID_SHIFT) | id, race);
                    assert_eq!(via_index, via_slot_id, "race {race} slot {slot} id {id}");
                    via_index
                };
                assert_eq!(got, Some(want), "race {race} slot {slot} id {id}");
            }
            assert!(
                clamped > 0,
                "race {race} slot {slot}: no id past the row's bands, the clamp is untested"
            );
        }
    }
}

/// The cells the retired hand table got wrong, as the dll has them on
/// KNOWN_CLIENTS horizonxi-2023 and retail-2026-09 (identical tables).
#[test]
fn measured_equipment_cells() {
    let Some((root, dll)) = install() else {
        return;
    };
    const TARUTARU_F: u8 = 6;
    const HUME_M: u8 = 1;
    const HEAD_SLOT: u8 = 1;

    assert_eq!(
        dll.equipment_model_index(TARUTARU_F, FACE_SLOT, 0),
        Some(22952)
    );
    assert_eq!(face_dat_id(&dll, 0, TARUTARU_F), Some(22952));
    for face in 24u8..32 {
        assert_eq!(
            face_dat_id(&dll, face, TARUTARU_F),
            Some(22952 + u32::from(face)),
            "Tarutaru F face {face} stays inside the face row"
        );
    }
    assert_eq!(
        equipment_dat_id(&dll, HEAD_SLOT, 304, TARUTARU_F),
        Some(65171)
    );
    assert_eq!(
        equipment_dat_id(&dll, HEAD_SLOT, 576, TARUTARU_F),
        Some(99443)
    );
    assert_eq!(equipment_dat_id(&dll, MAIN_SLOT, 928, HUME_M), Some(107333));
    assert_eq!(equipment_dat_id(&dll, SUB_SLOT, 928, HUME_M), Some(105233));

    let main_1000 = dll.equipment_model_index(HUME_M, MAIN_SLOT, 1000);
    assert!(
        main_1000.is_some(),
        "HumeM main-hand id 1000 is a real band, not the clamp"
    );
    assert_ne!(main_1000, dll.equipment_model_index(HUME_M, MAIN_SLOT, 0));

    assert_eq!(
        install_root_from_env().as_deref(),
        Some(root.root()),
        "the environment-resolved wrappers read this same install"
    );
    assert_eq!(resolve_face(0, TARUTARU_F), Some(22952));
    assert_eq!(resolve_equipment_model(MAIN_SLOT, 1000, HUME_M), main_1000);
}

/// The skeleton and battle-motion ids come from the dll's race tables, and the
/// shipped fallbacks equal them on every known build.
#[test]
fn race_tables_match_the_dll_and_the_fallbacks() {
    let Some((_, dll)) = install() else {
        return;
    };
    for race in PC_LOOK_RACES {
        let skel = dll
            .base_race_config_index(race)
            .map(u32::from)
            .unwrap_or_else(|| panic!("race {race}: no race-config entry"));
        let battle = dll
            .base_battle_animation_index(race)
            .map(u32::from)
            .unwrap_or_else(|| panic!("race {race}: no battle-animation entry"));

        assert_eq!(skeleton_file_id_for_race(Some(&dll), race), Some(skel));
        assert_eq!(
            skeleton_file_id_fallback(race),
            Some(skel),
            "race {race} fallback skeleton"
        );

        assert_eq!(motion_dat_for_race(Some(&dll), race), Some(battle));
        assert_eq!(motion_dat_for_skel(skel), Some(battle));
        assert_eq!(
            motion_dat_fallback(skel),
            Some(battle),
            "race {race} fallback battle DAT"
        );
    }
}

/// Every range of the NPC model-id formula lands on a real skinned model: the
/// four range starts, plus 3192, the last model id the 3000 range registers on
/// KNOWN_CLIENTS horizonxi-2023 and retail-2026-09 (3193..=3499 are VTABLE-absent
/// on both, which is why the split at 3500 can only come from the disassembly
/// cited at `NPC_DAT_ID_BASES`, not from the registered extent).
#[test]
fn npc_formula_ranges_resolve_to_skeleton_dats() {
    let Some((root, _)) = install() else {
        return;
    };
    for modelid in [0u16, 1500, 3000, 3192, 3500] {
        let file_id = npc_dat_id(modelid);
        let loc = root
            .resolve(file_id)
            .unwrap_or_else(|e| panic!("modelid {modelid} -> file {file_id}: {e}"));
        let bytes = std::fs::read(loc.path_under(&root))
            .unwrap_or_else(|e| panic!("modelid {modelid} -> file {file_id}: {e}"));
        assert!(
            !ResourceDir::from_bytes(bytes)
                .collect_skeletons()
                .is_empty(),
            "modelid {modelid} -> file {file_id} carries no Bone chunk"
        );
    }
}
