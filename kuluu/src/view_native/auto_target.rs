//! /autoattack: when the main target dies, re-engage a mob that is hitting
//! self.

use bevy::prelude::{Local, Res, Resource};
use kuluu_render::{InputMode, SceneState, Target};
use kuluu_session::state::{ActionKind, AgentCommand};
use kuluu_snapshot::{Entity, EntityKind, PartyMember, Vec3};

use super::engage::{claimed_by_other, claimed_by_party, in_engage_range};
use super::input::CommandTx;

/// The /autoattack toggle. ON by default. The server has its own after-kill
/// retarget (vendor/server/src/map/ai/states/attack_state.cpp
/// CAttackState::UpdateTarget) that picks from the mobs attacking the player;
/// this path covers the same trigger client-side and applies the party-claim
/// priority.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoAttack {
    pub enabled: bool,
}

impl Default for AutoAttack {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Candidates: mobs or pets whose bt target is self, targetable, inside the
/// server's engage range, and not claimed by another party. A claim held by
/// self or the party beats an unclaimed candidate; ties break on the seed so
/// distinct deaths roll distinct attackers.
pub fn pick_auto_target<'a>(
    entities: &'a [Entity],
    self_char_id: Option<u32>,
    self_pos: Vec3,
    party: &[PartyMember],
    seed: u64,
) -> Option<&'a Entity> {
    let self_id = self_char_id?;
    let candidates: Vec<&'a Entity> = entities
        .iter()
        .filter(|e| e.bt_target_id == self_id)
        .filter(|e| matches!(e.kind, EntityKind::Mob | EntityKind::Pet))
        .filter(|e| e.is_targetable())
        .filter(|e| in_engage_range(e.pos, self_pos))
        .filter(|e| !claimed_by_other(e, Some(self_id), party))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let ours: Vec<&'a Entity> = candidates
        .iter()
        .copied()
        .filter(|e| claimed_by_party(e, Some(self_id), party))
        .collect();
    let pool: &[&'a Entity] = if ours.is_empty() { &candidates } else { &ours };
    Some(pool[seed as usize % pool.len()])
}

/// Re-engage on target death. Fires on the frame the main target leaves the
/// slot with no server retarget filling it: a 0x058 assist that already
/// committed a new main target
/// (vendor/server/src/map/packets/s2c/0x058_assist.cpp) leaves the slot
/// filled and is left alone, and a Switch Target confirm in flight owns the
/// slot until its 0x058 lands. The weapon must be out — the server's ATTACK
/// byte (0x037 CHAR_STATUS, vendor/server/src/map/packets/char_status.cpp) —
/// so a disengaged player never re-arms.
pub fn auto_attack_retarget_system(
    auto: Res<AutoAttack>,
    target: Res<Target>,
    state: Res<SceneState>,
    mode: Res<InputMode>,
    cmd_tx: Res<CommandTx>,
    mut prev_target: Local<Option<u32>>,
    mut frames: Local<u64>,
) {
    *frames += 1;
    let current = target.id;
    let prev = *prev_target;
    *prev_target = current;

    if !auto.enabled {
        return;
    }
    if prev.is_none() || current.is_some() {
        return;
    }
    if matches!(&*mode, InputMode::SubTarget(st) if st.pending_switch.is_some()) {
        return;
    }
    if state.snapshot.self_server_status != ffxi_proto::decode::animation::ATTACK {
        return;
    }
    let Some(self_id) = state.snapshot.self_char_id else {
        return;
    };
    let Some(pick) = pick_auto_target(
        &state.snapshot.entities,
        Some(self_id),
        state.snapshot.self_pos.pos,
        &state.snapshot.party,
        *frames,
    ) else {
        return;
    };
    let _ = cmd_tx.0.try_send(AgentCommand::Action {
        target_id: pick.id,
        target_index: pick.act_index,
        kind: ActionKind::ChangeTarget,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_snapshot::CharFlags;

    const SELF: u32 = 0x0100_0001;
    const MOB_A: u32 = 0x0100_0010;
    const MOB_B: u32 = 0x0100_0011;
    const PET: u32 = 0x0100_0012;
    const STRANGER: u32 = 0x0100_0020;
    const PARTY_MATE: u32 = 0x0100_0021;

    fn here() -> Vec3 {
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }

    fn mob(id: u32, kind: EntityKind, x: f32, z: f32, bt_target: u32, claim_id: u32) -> Entity {
        Entity {
            id,
            act_index: 7,
            kind,
            name: Some(format!("Mob {id}")),
            pos: Vec3 { x, y: 0.0, z },
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: bt_target,
            face_target: 0,
            claim_id,
            speed: 0,
            speed_base: 0,
            look: None,
            animation: 0,
            animationsub: 0,
            mount: None,
            status: 0,
            char_flags: CharFlags::default(),
            monstrosity: false,
            name_vis: None,
        }
    }

    fn party_member(id: u32) -> PartyMember {
        PartyMember {
            id,
            act_index: 0,
            name: None,
            hp: 0,
            mp: 0,
            tp: 0,
            hp_pct: 100,
            mp_pct: 100,
            zone_no: 0,
            main_job: 0,
            main_job_lv: 0,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            party_no: 0,
            in_mog_house: false,
        }
    }

    /// .is_none() rather than assert_eq!(.., None): rkyv's cross-type
    /// PartialEq impls (via ffxi-nav-recast) break bare-None inference in
    /// assert_eq!.
    #[test]
    fn nothing_hits_self_means_no_candidate() {
        let ents = vec![mob(MOB_A, EntityKind::Mob, 5.0, 0.0, 0, 0)];
        assert!(pick_auto_target(&ents, Some(SELF), here(), &[], 0).is_none());
    }

    #[test]
    fn a_mob_hitting_self_is_a_candidate() {
        let ents = vec![mob(MOB_A, EntityKind::Mob, 5.0, 0.0, SELF, 0)];
        let pick = pick_auto_target(&ents, Some(SELF), here(), &[], 0).unwrap();
        assert_eq!(pick.id, MOB_A);
    }

    #[test]
    fn a_pet_hitting_self_is_a_candidate() {
        let ents = vec![mob(PET, EntityKind::Pet, 5.0, 0.0, SELF, 0)];
        let pick = pick_auto_target(&ents, Some(SELF), here(), &[], 0).unwrap();
        assert_eq!(pick.id, PET);
    }

    #[test]
    fn a_pc_hitting_self_is_not_a_candidate() {
        let ents = vec![mob(MOB_A, EntityKind::Pc, 5.0, 0.0, SELF, 0)];
        assert!(pick_auto_target(&ents, Some(SELF), here(), &[], 0).is_none());
    }

    #[test]
    fn out_of_engage_range_is_not_a_candidate() {
        let ents = vec![mob(MOB_A, EntityKind::Mob, 30.0, 0.0, SELF, 0)];
        assert!(pick_auto_target(&ents, Some(SELF), here(), &[], 0).is_none());
    }

    #[test]
    fn claimed_by_a_stranger_is_not_a_candidate() {
        let ents = vec![mob(MOB_A, EntityKind::Mob, 5.0, 0.0, SELF, STRANGER)];
        assert!(pick_auto_target(&ents, Some(SELF), here(), &[], 0).is_none());
    }

    #[test]
    fn a_party_claimed_mob_beats_an_unclaimed_one() {
        let ents = vec![
            mob(MOB_A, EntityKind::Mob, 5.0, 0.0, SELF, 0),
            mob(MOB_B, EntityKind::Mob, 6.0, 0.0, SELF, PARTY_MATE),
        ];
        let party = vec![party_member(PARTY_MATE)];
        for seed in 0..8 {
            let pick = pick_auto_target(&ents, Some(SELF), here(), &party, seed).unwrap();
            assert_eq!(pick.id, MOB_B, "seed {seed}");
        }
    }

    #[test]
    fn a_self_claimed_mob_beats_an_unclaimed_one() {
        let ents = vec![
            mob(MOB_A, EntityKind::Mob, 5.0, 0.0, SELF, 0),
            mob(MOB_B, EntityKind::Mob, 6.0, 0.0, SELF, SELF),
        ];
        for seed in 0..8 {
            let pick = pick_auto_target(&ents, Some(SELF), here(), &[], seed).unwrap();
            assert_eq!(pick.id, MOB_B, "seed {seed}");
        }
    }

    #[test]
    fn ties_break_on_the_seed() {
        let ents = vec![
            mob(MOB_A, EntityKind::Mob, 5.0, 0.0, SELF, 0),
            mob(MOB_B, EntityKind::Mob, 6.0, 0.0, SELF, 0),
        ];
        let a = pick_auto_target(&ents, Some(SELF), here(), &[], 0).unwrap();
        let b = pick_auto_target(&ents, Some(SELF), here(), &[], 1).unwrap();
        assert_ne!(a.id, b.id);
        for seed in 0..64 {
            let pick = pick_auto_target(&ents, Some(SELF), here(), &[], seed).unwrap();
            assert!(matches!(pick.id, MOB_A | MOB_B));
        }
    }

    #[test]
    fn a_dead_mob_is_not_a_candidate() {
        let mut ents = vec![mob(MOB_A, EntityKind::Mob, 5.0, 0.0, SELF, 0)];
        ents[0].hp_pct = Some(0);
        assert!(pick_auto_target(&ents, Some(SELF), here(), &[], 0).is_none());
    }

    fn retarget_app() -> (
        bevy::prelude::App,
        tokio::sync::mpsc::Receiver<AgentCommand>,
    ) {
        let mut app = bevy::prelude::App::new();
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        app.init_resource::<AutoAttack>()
            .init_resource::<SceneState>()
            .init_resource::<Target>()
            .init_resource::<InputMode>()
            .insert_resource(CommandTx(tx))
            .add_systems(bevy::prelude::Update, auto_attack_retarget_system);
        (app, rx)
    }

    /// MOB_B hits self, MOB_A is the current target: the shape the system
    /// sees on the frame auto_clear drops the corpse.
    fn death_frame_scene(app: &mut bevy::prelude::App, server_status: u8) {
        let mut state = app.world_mut().resource_mut::<SceneState>();
        state.snapshot.self_server_status = server_status;
        state.snapshot.self_char_id = Some(SELF);
        state.snapshot.entities = vec![
            mob(MOB_A, EntityKind::Mob, 5.0, 0.0, 0, 0),
            mob(MOB_B, EntityKind::Mob, 6.0, 0.0, SELF, 0),
        ];
    }

    /// The first frame carries no previous target, so nothing fires; then
    /// auto_clear drops the dead target and the retarget queues behind.
    #[test]
    fn the_death_frame_sends_the_change_target() {
        let (mut app, mut rx) = retarget_app();
        death_frame_scene(&mut app, ffxi_proto::decode::animation::ATTACK);
        app.world_mut().resource_mut::<Target>().id = Some(MOB_A);

        app.update();
        assert!(rx.try_recv().is_err());

        app.world_mut().resource_mut::<Target>().id = None;
        app.update();
        assert_eq!(
            rx.try_recv().unwrap(),
            AgentCommand::Action {
                target_id: MOB_B,
                target_index: 7,
                kind: ActionKind::ChangeTarget,
            }
        );
    }

    /// The 0x058 assist committed a new main target instead of clearing the
    /// slot (vendor/server/src/map/packets/s2c/0x058_assist.cpp): the
    /// retarget stays suppressed.
    #[test]
    fn a_server_retarget_fill_suppresses_the_fire() {
        let (mut app, mut rx) = retarget_app();
        death_frame_scene(&mut app, ffxi_proto::decode::animation::ATTACK);
        app.world_mut().resource_mut::<Target>().id = Some(MOB_A);
        app.update();
        app.world_mut().resource_mut::<Target>().id = Some(MOB_B);
        app.update();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_disengaged_player_never_rearms() {
        let (mut app, mut rx) = retarget_app();
        death_frame_scene(&mut app, 0);
        app.world_mut().resource_mut::<Target>().id = Some(MOB_A);
        app.update();
        app.world_mut().resource_mut::<Target>().id = None;
        app.update();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_switch_in_flight_owns_the_slot() {
        let (mut app, mut rx) = retarget_app();
        death_frame_scene(&mut app, ffxi_proto::decode::animation::ATTACK);
        let mut st = kuluu_render::input_mode::SubTargetState::open(
            kuluu_render::input_mode::SubTargetAction::PickSub,
            0,
            InputMode::World,
        );
        st.pending_switch = Some(MOB_B);
        app.world_mut().insert_resource(InputMode::SubTarget(st));
        app.world_mut().resource_mut::<Target>().id = Some(MOB_A);
        app.update();
        app.world_mut().resource_mut::<Target>().id = None;
        app.update();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn off_disarms_the_fire() {
        let (mut app, mut rx) = retarget_app();
        death_frame_scene(&mut app, ffxi_proto::decode::animation::ATTACK);
        app.world_mut().resource_mut::<AutoAttack>().enabled = false;
        app.world_mut().resource_mut::<Target>().id = Some(MOB_A);
        app.update();
        app.world_mut().resource_mut::<Target>().id = None;
        app.update();
        assert!(rx.try_recv().is_err());
    }
}
