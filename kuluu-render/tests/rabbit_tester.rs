//! Rabbit (Savanna Rarab) front-to-end animation tester - kuluu-df9t.
//!
//! Drives a deterministic Bevy app with the real `SchedulerRuntimePlugin` plus the pose path,
//! feeds hand-packed BATTLE2 bytes through the real session decoder, and asserts what the
//! retail DATs say should happen: swing clips on the attacker, victim reactions AT the inlined
//! DamageCallback impact frame (not packet arrival), flinch on idle hosts (dfm? for PCs / dfi? for mobs,
//! D3/D4), death fall-over on Defeated (D5), and limb selection from BATTLE2's animation field
//! with the ati0 fallback (D6).
//!
//! Ground truth: the retail DATs - Rarab = 1569 =
//! ROM/4/109.DAT (zone 115 entity 17248272, no weapon; ships dfi?/dfm?, wlk0/idl0/run0/ded0/cor0,
//! routines ati0..2/atf0/dead/corp/damg/sdam/ldam/gurd/pary/sway + degenerate `init`); HumeM
//! skeleton 7072 (ROM/27/82.DAT) with main-hand weapon 8392 whose motion base is 9672
//! (ROM/32/13.DAT - ships ati0..2, NO bti0). XIM: EffectRoutineInterpolatedEffects.kt
//! FlinchAnimationInstance, poc/Actor.kt onDisplayDeath. LSB: vendor/server/src/map/packets/
//! s2c/0x028_battle2.cpp GP_SERV_COMMAND_BATTLE2::pack (wire layout), attack.h AttackAnimation
//! (limb). Assertions are
//! ordering/ranges, not exact frames.

use std::sync::Arc;
use std::time::Duration;

use bevy::prelude::*;

use kuluu_render::audio::SfxEvent;
use kuluu_render::combat_stance::{
    EntityMotion, MotionSample, RestStance, SelfMoveIntent, WalkMode,
};
use kuluu_render::components::WorldEntity;
use kuluu_render::ffxi_actor_render::{
    dispatch_action_overlay, kick_load_actor_tasks, load_npc, load_pc, make_render_actor,
    poll_load_actor_tasks, tick_live_ffxi_actors, ActorLoadInFlight, ActorSubject, FfxiRenderActor,
    FfxiRenderRoot, LoadActorRequest, LoadedActor, SpellSuffixCache,
};
use kuluu_render::scene::{
    apply_invis_flag_system, EntityMaterials, EntityMesh, Target, TrackedEntities,
};
use kuluu_render::scheduler_runtime::{
    ActionDatRoot, ActiveSchedulers, GlobalEffectDir, SchedulerRuntimePlugin,
};
use kuluu_render::skinned_ffxi_material::{FfxiSkinRegistry, FfxiSkinnedMaterialCache};
use kuluu_render::snapshot::{EventLog, SceneState};
use kuluu_render::EntityTable;
use kuluu_snapshot::{EntityKind, ViewerEvent};

/// Savanna Rarab - ROM/4/109.DAT. No weapon; the "both event sets" pairing is HumeM below.
const RARAB_FILE: u32 = 1569;
/// A mob model that ships `bti0` in its own DAT (ROM/352/115.DAT) - S10's positive limb case.
const LIMB_MODEL_FILE: u32 = 101806;
/// Carrion Worm family - ROM/5/64.DAT (FTABLE-verified). Ships `ini1` = Motion sp1? + dirt
/// generators + sound, and `init` = Motion sp0? + dirt generators + sound: the full special-pose
/// pair S12 drives through the pose pass.
const WORM_FILE: u32 = 1724;
/// ROM/172/67.DAT - one of exactly three retail models that ship `damg` without `ldam`
/// (verified against the install: routines shot/damg/chit/cate/cast/pop0/init/corp/dead/setr/
/// kil0/bom0/kil1/efon). The S6c/S6d victim: a crit on it must fall back to damg, and only
/// when the global dir's ldam is out of reach.
const NOLDA_FILE: u32 = 52087;
/// ROM/4/106.DAT - flying bat; its 0x45 Info chunk carries movement byte 3 (Flying) and scale
/// byte 85, so the live pipeline must load it at 85 percent with no wire stride scale.
const BAT_FILE: u32 = 1564;
/// ROM/3/60.DAT - walking mob whose Info chunk carries scale byte 100: the unchanged control
/// for S8's model-scale assertion.
const WALKER_FILE: u32 = 1386;

// World ids used by the hand-packed packets. Rarab carries its real zone-115 entity id.
const RARAB_W: u32 = 17_248_272;
const HUMEM_W: u32 = 9_000_001;
const RARAB2_W: u32 = 9_000_002;
const LIMB_W: u32 = 9_000_003;
const WORM_W: u32 = 9_000_004;
const NOLDA_W: u32 = 9_000_005;
const BAT_W: u32 = 9_000_006;
const WALKER_W: u32 = 9_000_007;

// HumeM main-hand weapon model 0 (look_resolver::PC_MODEL_IDS[HumeM][main-hand] base).
const HUME_M_MAIN_WEAPON_FILE: u32 = 8392;

fn install() -> Option<ffxi_dat::DatRoot> {
    ffxi_dat::archive::open_test_install()
}

// ---------------------------------------------------------------------------
// BATTLE2 wire packing - vendor/server/src/map/packets/s2c/0x028_battle2.cpp GP_SERV_COMMAND_BATTLE2::pack, from bit 8,
// LSB-first per byte: actor_id(32), trg_sum(6), res_sum(4), action_kind(4), action_id(32),
// info(32); if trg_sum>0: target(32), nres(4); if nres>0: resolution(3), kind(2), animation(12),
// info(5), hit_distortion(2), knockback(3). Wire values: resolution 0=Hit/1=Miss/2=Guard/
// 3=Parry/4=Block; animation 0..4 = RightAttack/LeftAttack/RightKick/LeftKick/Throw;
// info bit1=Defeated, bit2=CriticalHit.
// ---------------------------------------------------------------------------

struct Bits {
    bytes: Vec<u8>,
    pos: u32, // absolute bit index; the packet header occupies bits 0..7
}

impl Bits {
    fn new() -> Self {
        Self {
            bytes: vec![0u8; 32],
            pos: 8,
        }
    }

    fn write(&mut self, value: u32, nbits: u32) {
        for i in 0..nbits {
            if (value >> i) & 1 == 1 {
                let bit = self.pos + i;
                self.bytes[(bit / 8) as usize] |= 1 << (bit % 8);
            }
        }
        self.pos += nbits;
    }

    /// LSB rounds the body to a 4-byte size (vendor/server/src/map/packets/basic.h CBasicPacket::setSize).
    fn finish(self) -> Vec<u8> {
        let end = (self.pos + 7).div_ceil(8) * 4;
        let end = end.max(4);
        let mut out = self.bytes;
        out.truncate(end as usize);
        out
    }
}

/// One per-result block: (resolution, animation, info, hit_distortion, knockback).
type ResultBlock = (u32, u32, u32, u32, u32);

fn pack_battle2(
    actor_id: u32,
    action_kind: u8,
    target: Option<u32>,
    result: Option<ResultBlock>,
) -> Vec<u8> {
    let mut b = Bits::new();
    b.write(actor_id, 32);
    b.write(u32::from(target.is_some()), 6); // trg_sum
    b.write(0, 4); // res_sum (unused by the client reader)
    b.write(action_kind as u32, 4);
    b.write(0, 32); // action_id - BATTLE2 cmd_arg is FourCC::BasicAttack for every swing
    b.write(0, 32); // info
    if let Some(t) = target {
        b.write(t, 32);
        b.write(u32::from(result.is_some()), 4); // nres
        if let Some((resolution, animation, info, distortion, knockback)) = result {
            b.write(resolution, 3);
            b.write(0, 2); // kind (uninterpreted)
            b.write(animation, 12);
            b.write(info, 5);
            b.write(distortion, 2);
            b.write(knockback, 3);
        }
    }
    b.finish()
}

/// Decode through the real session reader and map to the viewer event exactly like
/// kuluu-session/src/wire_translate.rs does for AgentEvent::ActionStarted.
fn action_event(bytes: &[u8]) -> ViewerEvent {
    let h =
        kuluu_session::session::decode_battle2_header(bytes).expect("hand-packed BATTLE2 decodes");
    ViewerEvent::ActionStarted {
        actor_id: h.actor_id,
        action_id: h.action_id,
        action_kind: h.action_kind,
        target_id: h.primary_target_id,
        result: h.first_result.map(|r| r.to_wire()),
        animation: h.animation,
        outcome: h.first_outcome.map(|o| o.to_wire()),
    }
}

// ---------------------------------------------------------------------------
// App + entity rig
// ---------------------------------------------------------------------------

/// Deterministic app: real scheduler plugin + pose path, no GPU. dt = 1/60 s per update, so the
/// routine clock (ROUTINE_FPS=60) advances exactly one frame per `step`.
fn build_app() -> App {
    // A bare `App::new()` skips bevy's TaskPoolPlugin, but the scheduler plugin spawns its
    // global-effect-dir load on the async compute pool - initialize it exactly like the
    // dat_mzb/zone_doors tests do.
    bevy::tasks::AsyncComputeTaskPool::get_or_init(Default::default);
    let mut app = App::new();
    // Every action-DAT read (ROM/0/0.DAT's global effect dir included) resolves through the
    // shared root the host wires; without one here Bevy auto-inserts the default None and the
    // global dir lands empty, so S6c/S6d's ldam precondition can never hold. Wire it from the
    // same test install load_npc/load_pc use.
    app.insert_resource(ActionDatRoot(install().map(Arc::new)));
    app.init_resource::<Time>();
    // The plugin's particle systems take asset stores as ResMut; a bare app has none of them.
    app.init_resource::<bevy::asset::Assets<bevy::prelude::Mesh>>();
    app.init_resource::<bevy::asset::Assets<kuluu_render::ffxi_particle_material::FfxiParticleMaterial>>();
    app.init_resource::<bevy::asset::Assets<bevy::image::Image>>();
    app.add_plugins(SchedulerRuntimePlugin);
    // The plugin's chain is .after(dispatch_action_overlay), and
    // stop_cast_effects_when_cast_ends is .after(tick_live_ffxi_actors) - register both, in the
    // production order (lib.rs: overlay before tick). Pinning the pose pass between the melee
    // dispatcher and the routine tick makes S9's D5 hold path run on the event frame itself:
    // the latch + queued `dead` are visible to the pose pass BEFORE tick_active_schedulers fires
    // the fall-over, so dead_fall_over_pending() is exercised instead of being dead code here.
    app.add_systems(
        Update,
        (
            dispatch_action_overlay.before(tick_live_ffxi_actors),
            apply_invis_flag_system.before(tick_live_ffxi_actors),
            // One subject use for the pose pass (Bevy 0.19: each subject registration is a
            // distinct instance; ordering against an ambiguous name panics at schedule init).
            tick_live_ffxi_actors
                .after(kuluu_render::scheduler_runtime::dispatch_melee_action_started)
                .before(kuluu_render::scheduler_runtime::tick_active_schedulers),
        ),
    );
    // The plugin's sound stages write SfxEvent; an unregistered message would panic.
    app.add_message::<SfxEvent>();
    app.init_resource::<EventLog>();
    app.init_resource::<TrackedEntities>();
    app.init_resource::<SpellSuffixCache>();
    app.init_resource::<SceneState>();
    app.init_resource::<EntityMotion>();
    app.init_resource::<RestStance>();
    app.init_resource::<WalkMode>();
    app.init_resource::<SelfMoveIntent>();
    app.init_resource::<Target>();
    app.init_resource::<FfxiSkinRegistry>();
    // Production visibility ownership (lib.rs): apply_invis_flag_system resets every skinned model
    // root's Visibility from the wire invis flag each frame, and tick_live_ffxi_actors runs after it
    // so a status-INVISIBLE entity's Hidden write wins for that frame. The rig must mirror both: with
    // no reset system in place, a once-set Hidden would latch forever here (s12's resurface).
    app.init_resource::<EntityTable>();
    app.insert_resource(EntityMaterials {
        pc: Default::default(),
        self_pc: Default::default(),
        npc: Default::default(),
        mob: Default::default(),
        pet: Default::default(),
        other: Default::default(),
        aggro: Default::default(),
        mob_claimed_self: Default::default(),
        mob_claimed_other: Default::default(),
        invis_orb: Default::default(),
    });
    app
}

/// Tracked parent (WorldEntity + Children) with one render-actor child. The scheduler systems
/// arm/run routines on the PARENT; the pose path and stage consumers act on the CHILD.
fn spawn_actor(
    app: &mut App,
    world_id: u32,
    kind: EntityKind,
    loaded: &LoadedActor,
) -> (Entity, Entity) {
    let actor = make_render_actor(loaded, 0, Vec::new(), world_id, 0.0, 1.0);
    // The pose path's query requires GlobalTransform + Visibility on the model root; a bare
    // spawn in this rig gets neither (Bevy does not auto-insert them here), so insert both -
    // matching what scene::spawn_live_actor gives every production actor.
    let child = app
        .world_mut()
        .spawn((
            actor,
            Transform::default(),
            GlobalTransform::default(),
            Visibility::default(),
        ))
        .id();
    let parent = app
        .world_mut()
        .spawn(WorldEntity {
            id: world_id,
            act_index: 0,
            kind,
        })
        .id();
    // Bevy maintains the parent's Children immediately via the ChildOf component hook.
    app.world_mut().entity_mut(child).insert(ChildOf(parent));
    // Production shape (scene.rs): the wire entity carries FfxiRenderRoot pointing at its model
    // root; that link is how apply_invis_flag_system finds each frame's visibility reset target.
    app.world_mut()
        .entity_mut(parent)
        .insert(FfxiRenderRoot(child));

    let mut tracked = app.world_mut().resource_mut::<TrackedEntities>();
    tracked.by_id.insert(world_id, parent);

    let mut state = app.world_mut().resource_mut::<SceneState>();
    state.snapshot.entities.push(kuluu_snapshot::Entity {
        id: world_id,
        act_index: 0,
        kind,
        name: None,
        pos: kuluu_snapshot::Vec3::default(),
        heading: 0,
        hp_pct: Some(100),
        bt_target_id: 0,
        face_target: 0,
        claim_id: 0,
        speed: 0,
        speed_base: 0,
        look: None,
        animation: 0,
        animationsub: 0,
        mount: None,
        status: 0,
        char_flags: Default::default(),
        monstrosity: false,
        name_vis: None,
    });
    (parent, child)
}

/// None only when there is no retail install (the guard prints its skip); a model that exists
/// but fails to load is a failure, not a skip.
fn load_model(file: u32) -> Option<LoadedActor> {
    install()?;
    Some(load_npc(file).unwrap_or_else(|e| panic!("model DAT {file} failed to load: {e:?}")))
}

fn load_rarab() -> Option<LoadedActor> {
    load_model(RARAB_FILE)
}

fn load_worm() -> Option<LoadedActor> {
    load_model(WORM_FILE)
}

fn load_nolda() -> Option<LoadedActor> {
    load_model(NOLDA_FILE)
}

/// The inlined DamageCallback of ati0 lands at routine frame 36 (dada @32 + 4 delay); a
/// reaction earlier than this fired on packet arrival, not at the callback.
const IMPACT_FRAME_MIN: u32 = 30;

/// HumeM skeleton with a main-hand weapon: the armed-race base whose motion DAT ships ati0..2
/// but no bti0/cti0/dti0 (the D6 fallback case).
fn load_humem() -> Option<LoadedActor> {
    install()?;
    let mut equipment = vec![HUME_M_MAIN_WEAPON_FILE];
    equipment.extend(
        (1u16..=5)
            .filter_map(|slot| kuluu_render::look_resolver::resolve_equipment_slot(slot << 12, 1)),
    );
    load_pc(
        1,
        false,
        &equipment,
        None,
        Some(HUME_M_MAIN_WEAPON_FILE),
        None,
    )
    .ok()
}

fn step(app: &mut App) {
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_secs_f32(1.0 / 60.0));
    app.update();
}

/// One routine frame per call; returns the update index (1-based).
fn step_n(app: &mut App, n: u32) -> u32 {
    for _ in 0..n {
        step(app);
    }
    n
}

// The probes read through a `&World` handed in by [`watch`] so the closure never borrows
// the `App` that `step` mutably drives.
fn active_clip(world: &bevy::prelude::World, child: Entity) -> Option<String> {
    world
        .entity(child)
        .get::<FfxiRenderActor>()?
        .active_action_clip()
        .map(|c| c.as_str().to_string())
}

/// The clip the pose pass selected (idl?/wlk?/run?/cor?...), or None before its first run.
fn pose_clip(world: &bevy::prelude::World, child: Entity) -> Option<String> {
    world
        .entity(child)
        .get::<FfxiRenderActor>()?
        .current_clip_id()
        .map(|c| c.as_str().to_string())
}

fn routines(world: &bevy::prelude::World, parent: Entity) -> Vec<[u8; 4]> {
    world
        .entity(parent)
        .get::<ActiveSchedulers>()
        .map(|s| s.routine_names().collect())
        .unwrap_or_default()
}

/// Push a BATTLE2 event and return the update index it was pushed after.
fn push_battle2(
    app: &mut App,
    actor_id: u32,
    action_kind: u8,
    target: Option<u32>,
    result: Option<ResultBlock>,
) -> u32 {
    let ev = action_event(&pack_battle2(actor_id, action_kind, target, result));
    app.world_mut().resource_mut::<EventLog>().push(ev);
    0
}

/// Wait for the async global effect dir load (ROM/0/0.DAT) to land, then remove it so no lookup
/// can rescue a routine from there. The poll system inserts exactly once and never re-inserts,
/// so the removal holds for the rest of the scenario. Asserting ldam is present first keeps the
/// S6c/S6d fallback honest: without this step the global dir's own ldam would satisfy the guard.
fn drop_global_effect_dir(app: &mut App) {
    for _ in 0..600 {
        if app.world().contains_resource::<GlobalEffectDir>() {
            break;
        }
        step(app);
    }
    let g = app.world().resource::<GlobalEffectDir>();
    assert!(
        g.schedulers.iter().any(|s| s.name == *b"ldam"),
        "ROM/0/0.DAT ships ldam (the fallback under test must be reachable without it)"
    );
    app.world_mut().remove_resource::<GlobalEffectDir>();
}

/// Drive `window` updates after the event; return (first update index in [1..=window] where
/// `probe` holds, per-update samples). The probe sees post-update state through a fresh
/// `&World`, so it cannot hold a borrow across `step`.
fn watch(
    app: &mut App,
    window: u32,
    mut probe: impl FnMut(u32, &bevy::prelude::World) -> bool,
) -> (Option<u32>, Vec<bool>) {
    let mut first = None;
    let mut samples = Vec::with_capacity(window as usize);
    for i in 1..=window {
        step(app);
        let hit = probe(i, app.world());
        samples.push(hit);
        if hit && first.is_none() {
            first = Some(i);
        }
    }
    (first, samples)
}

// ---------------------------------------------------------------------------
// S1/S2 - spawn + idle baseline
// ---------------------------------------------------------------------------

/// S1: spawn Rarab, no events. No panic; the pose settles on an idl? clip and stays there.
#[test]
fn s1_spawn_settles_on_idle() {
    let Some(loaded) = load_rarab() else { return };
    let mut app = build_app();
    let (_, child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &loaded);

    step_n(&mut app, 30);
    let clip = pose_clip(app.world(), child).expect("pose pass ran");
    assert!(
        clip.starts_with("idl"),
        "fresh Rarab idles on {clip}, not a battle/death clip"
    );

    // S2: three more seconds of idle - still the same family, no panic.
    step_n(&mut app, 180);
    let clip = pose_clip(app.world(), child).expect("pose pass ran");
    assert!(clip.starts_with("idl"), "idle held for 3 s, now {clip}");
}

// ---------------------------------------------------------------------------
// S3/S4 - gait selection from the motion sample
// ---------------------------------------------------------------------------

fn moving_sample(speed: f32) -> MotionSample {
    // Only `moving` drives the pose pass in this rig (track_entity_motion_system is not
    // registered); the speed value no longer feeds gait selection.
    MotionSample {
        speed,
        moving: true,
        ..Default::default()
    }
}

/// Set the wire speed bytes on an entity's snapshot entry; the next pose pass rebuilds the
/// index from them and applies the gait rule (run = speed > speed_base).
fn set_wire_gait(app: &mut App, world_id: u32, speed: u8, speed_base: u8) {
    let mut state = app.world_mut().resource_mut::<SceneState>();
    for e in &mut state.snapshot.entities {
        if e.id == world_id {
            e.speed = speed;
            e.speed_base = speed_base;
        }
    }
}

/// S3: moving with the wire speed at its base selects the wlk? hop clip.
#[test]
fn s3_walk_gait_selects_wlk_clip() {
    let Some(loaded) = load_rarab() else { return };
    let mut app = build_app();
    let (_, child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &loaded);
    step_n(&mut app, 10);

    app.world_mut()
        .resource_mut::<EntityMotion>()
        .by_id
        .insert(RARAB_W, moving_sample(1.0));
    set_wire_gait(&mut app, RARAB_W, 40, 40); // speed at base = walk

    let (first, _) = watch(&mut app, 60, |i, w| {
        i >= 2 && pose_clip(w, child).is_some_and(|c| c.starts_with("wlk"))
    });
    assert!(
        first.is_some(),
        "walk gait selected wlk? within 1 s (Rarab's hop clip)"
    );
}

/// S4: moving with the wire speed above its base selects run?.
#[test]
fn s4_run_gait_selects_run_clip() {
    let Some(loaded) = load_rarab() else { return };
    let mut app = build_app();
    let (_, child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &loaded);
    step_n(&mut app, 10);

    app.world_mut()
        .resource_mut::<EntityMotion>()
        .by_id
        .insert(RARAB_W, moving_sample(4.0));
    set_wire_gait(&mut app, RARAB_W, 50, 40); // run factor lifted speed above base = run

    let (first, _) = watch(&mut app, 60, |i, w| {
        i >= 2 && pose_clip(w, child).is_some_and(|c| c.starts_with("run"))
    });
    assert!(first.is_some(), "run gait selected run? within 1 s");
}

// ---------------------------------------------------------------------------
// S5/S5b - swing impact hands off to the victim reaction AT the inlined DamageCallback frame
// ---------------------------------------------------------------------------

/// Rarab swings RightAttack at HumeM (Hit, dist=0, kb=0). Expect: at0? on the attacker from
/// ~frame 1; at the inlined DamageCallback impact (~36 for ati0) HumeM runs `damg` (it ships no sdam of
/// its own - the reaction table falls through to damg) and its flinch stage starts dfm? on the PC host.
#[test]
fn s5_swing_impact_runs_damg_and_flinches_the_pc() {
    let (Some(rarab), Some(humem)) = (load_rarab(), load_humem()) else {
        return;
    };
    let mut app = build_app();
    let (_, atk_child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, vic_child) = spawn_actor(&mut app, HUMEM_W, EntityKind::Pc, &humem);
    step_n(&mut app, 10);

    // res=Hit(0), anim=RightAttack(0), info=0, dist=0, kb=0.
    push_battle2(&mut app, RARAB_W, 1, Some(HUMEM_W), Some((0, 0, 0, 0, 0)));

    // The overlay/pose path picks the swing clip from BATTLE2's animation field (D6).
    let (swing_at, _) = watch(&mut app, 5, |i, w| {
        i >= 1 && active_clip(w, atk_child).is_some_and(|c| c.starts_with("at0"))
    });
    assert!(swing_at.is_some(), "attacker plays the at0? swing clip");

    // Impact: damg queued on the victim AND its flinch stage started dfm?. The inlined DamageCallback
    // fires at routine frame 36 (ati0 calls dada @32, +4 delay); allow dispatch slack.
    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"damg")
            && active_clip(w, vic_child).is_some_and(|c| c.starts_with("dfm"))
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "victim reaction (damg + dfm? flinch) fired at the inlined-0x2B frame (~36), not on \
         packet arrival"
    );
}

/// S5b: same swing, victim = a second Rarab. Retail's dam0 branch table routes every non-crit
/// Hit to damg/damh - both carry the 0x21 flinch stage (ROM/0/0.DAT), so the mob victim runs
/// its own `damg` and flinches with dfi? on a normal hit. This is the "animations not playing"
/// case: before kuluu-df9t's damg routing, sdam-shipping models like Rarab got sound-only hits.
#[test]
fn s5b_mob_victim_normal_hit_runs_damg_and_flinches() {
    let Some(rarab) = load_rarab() else { return };
    let mut app = build_app();
    let (_, atk_child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, vic_child) = spawn_actor(&mut app, RARAB2_W, EntityKind::Mob, &rarab);
    step_n(&mut app, 10);

    push_battle2(&mut app, RARAB_W, 1, Some(RARAB2_W), Some((0, 0, 0, 0, 0)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"damg")
            && active_clip(w, vic_child).is_some_and(|c| c.starts_with("dfi"))
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "normal hit runs damg + dfi? flinch on the mob victim"
    );
    // The attacker still swung (sanity: the chain armed from this swing's DamageCallback).
    assert!(active_clip(app.world(), atk_child).is_some());
}

// ---------------------------------------------------------------------------
// S6/S6b - crits: ldam + flinch, no sway at kb=0
// ---------------------------------------------------------------------------

/// S6: Hit with info=CriticalHit runs `ldam` on HumeM and its
/// flinch stage starts dfm?; kb=0 adds no sway.
#[test]
fn s6_crit_runs_ldam_and_flinches_the_pc() {
    let (Some(rarab), Some(humem)) = (load_rarab(), load_humem()) else {
        return;
    };
    let mut app = build_app();
    let (_, atk_child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, vic_child) = spawn_actor(&mut app, HUMEM_W, EntityKind::Pc, &humem);
    step_n(&mut app, 10);

    // res=Hit(0), anim=RightAttack(0), info=0, dist=3 (Heavy/crit), kb=0.
    push_battle2(&mut app, RARAB_W, 1, Some(HUMEM_W), Some((0, 0, 2, 3, 0)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"ldam")
            && active_clip(w, vic_child).is_some_and(|c| c.starts_with("dfm"))
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "crit runs ldam + dfm? flinch at the impact frame"
    );

    let sway = routines(app.world(), vic_parent).contains(b"sway");
    assert!(!sway, "kb=0 adds no sway alongside the crit reaction");
    // The attacker still swung (sanity: the chain armed from this swing's DamageCallback).
    assert!(active_clip(app.world(), atk_child).is_some());
}

/// S6b: same crit on a Rarab victim - ldam's flinch stage starts dfi? on the mob host. This is
/// the "crits animations do not play" case from the field report.
#[test]
fn s6b_crit_flinches_the_mob_with_dfi() {
    let Some(rarab) = load_rarab() else { return };
    let mut app = build_app();
    let (_, atk_child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, vic_child) = spawn_actor(&mut app, RARAB2_W, EntityKind::Mob, &rarab);
    step_n(&mut app, 10);

    push_battle2(&mut app, RARAB_W, 1, Some(RARAB2_W), Some((0, 0, 2, 3, 0)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"ldam")
            && active_clip(w, vic_child).is_some_and(|c| c.starts_with("dfi"))
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "crit flinches the mob victim with dfi?"
    );
    // The attacker still swung (sanity: the chain armed from this swing's DamageCallback).
    assert!(active_clip(app.world(), atk_child).is_some());
}

/// S6c: crit on a victim whose DAT ships no `ldam` of its own (ROM/172/67.DAT), with the global
/// effect dir removed so ROM/0/0.DAT's ldam cannot rescue it. The crit guard must fall back to
/// the normal `damg` reaction instead of arming an unresolvable ldam, which would fall
/// through to nothing. All eight retail PC skeletons ship their own ldam (verified against the
/// install), so this fallback is reachable only on mob victims; S6 covers the PC side of the
/// matrix.
#[test]
fn s6c_crit_without_ldam_falls_back_to_damg() {
    let (Some(rarab), Some(nolda)) = (load_rarab(), load_nolda()) else {
        return;
    };
    let mut app = build_app();
    let (_, atk_child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, _) = spawn_actor(&mut app, NOLDA_W, EntityKind::Mob, &nolda);
    step_n(&mut app, 10);
    drop_global_effect_dir(&mut app);

    // res=Hit(0), anim=RightAttack(0), info=0, dist=3 (Heavy/crit), kb=0.
    push_battle2(&mut app, RARAB_W, 1, Some(NOLDA_W), Some((0, 0, 2, 3, 0)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"damg") && !routines(w, vic_parent).contains(b"ldam")
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "crit on a no-ldam victim runs the damg fallback at the impact frame, not an \
         unresolvable ldam"
    );
    // The attacker still swung (sanity: the chain armed from this swing's DamageCallback).
    assert!(active_clip(app.world(), atk_child).is_some());
}

/// S6d: same rig (no ldam anywhere), non-crit Medium hit still routes to damg - the crit guard
/// must not leak into the None/Light/Medium cases.
#[test]
fn s6d_medium_hit_without_ldam_still_runs_damg() {
    let (Some(rarab), Some(nolda)) = (load_rarab(), load_nolda()) else {
        return;
    };
    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, _) = spawn_actor(&mut app, NOLDA_W, EntityKind::Mob, &nolda);
    step_n(&mut app, 10);
    drop_global_effect_dir(&mut app);

    // res=Hit(0), dist=2 (Medium), kb=0.
    push_battle2(&mut app, RARAB_W, 1, Some(NOLDA_W), Some((0, 0, 0, 2, 0)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"damg")
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "non-crit hits on a no-ldam victim still run damg (the crit guard does not leak into \
         dist 0/1/2)"
    );
}

// ---------------------------------------------------------------------------
// S8 - the Cib Info chunk through the live load pipeline: model scale and movement type
// ---------------------------------------------------------------------------

/// The (scale, movement_type) of an entity's LIVE render root, or None while the placeholder
/// (empty instance slots) still stands in. poll_load_actor_tasks despawns the placeholder and
/// spawns the real actor with the PreparedActor's resolved scale; FfxiRenderActor.scale is the
/// value advance_actor_pose feeds to RootTransform every frame.
fn live_root_probe(
    world: &bevy::prelude::World,
    parent: Entity,
) -> Option<(f32, ffxi_dat::cib::MovementType)> {
    let root = world.entity(parent).get::<FfxiRenderRoot>()?.0;
    let actor = world.entity(root).get::<FfxiRenderActor>()?;
    if actor.instance_slots().is_empty() {
        return None;
    }
    Some((actor.scale, actor.movement_type()))
}

/// S8: the Cib Info chunk drives two live-pipeline decisions. The bat's scale byte (85) must
/// reach its render root as a 0.85 multiplier and its movement byte (3 = Flying) must land on
/// the actor; the walker's scale byte (100) must leave it at exactly 1.0 with Walking.
#[test]
fn s8_info_chunk_scale_and_movement_reach_the_live_actor() {
    let Some(bat) = install().and_then(|_| load_npc(BAT_FILE).ok()) else {
        return;
    };
    let Some(walker) = install().and_then(|_| load_npc(WALKER_FILE).ok()) else {
        return;
    };

    // Data level: the parse itself (vekien/xi-model-viewer ui/js/dat/inspect.js
    // parseInspectInfo :1378 - b[0] movement, b[10] scale).
    let bat_cib = bat.cib().expect("bat DAT carries a 0x45 Info chunk");
    assert_eq!(bat_cib.movement_type, ffxi_dat::cib::MovementType::Flying);
    assert!(
        (bat_cib.scale_factor() - 0.85).abs() < f32::EPSILON,
        "scale byte 85 -> 0.85"
    );
    let walker_cib = walker.cib().expect("walker DAT carries a 0x45 Info chunk");
    assert_eq!(
        walker_cib.movement_type,
        ffxi_dat::cib::MovementType::Walking
    );
    assert!(
        (walker_cib.scale_factor() - 1.0).abs() < f32::EPSILON,
        "scale byte 100 -> 1.0"
    );

    // Live pipeline: build_app lacks the load-pipeline resources; add exactly what production's
    // dat_mmb chain (kuluu-render/src/dat_mmb.rs DatOverlayPlugin) registers for kick/poll.
    let mut app = build_app();
    app.init_resource::<bevy::asset::Assets<kuluu_render::skinned_ffxi_material::FfxiSkinnedMaterial>>();
    app.init_resource::<FfxiSkinnedMaterialCache>();
    app.init_resource::<bevy::asset::Assets<StandardMaterial>>();
    app.insert_resource(kuluu_render::graphics_settings::GraphicsSettings::default());
    app.init_resource::<ActorLoadInFlight>();
    app.add_message::<LoadActorRequest>();
    app.insert_resource(EntityMesh {
        default: bevy::asset::Handle::default(),
        pc: bevy::asset::Handle::default(),
        mob: bevy::asset::Handle::default(),
        pet: bevy::asset::Handle::default(),
        morph_orb: bevy::asset::Handle::default(),
    });
    app.add_systems(
        Update,
        (kick_load_actor_tasks, poll_load_actor_tasks).chain(),
    );

    let (bat_parent, _) = spawn_actor(&mut app, BAT_W, EntityKind::Mob, &bat);
    let (walker_parent, _) = spawn_actor(&mut app, WALKER_W, EntityKind::Mob, &walker);

    // The production request shape (kuluu-render/src/look_resolver.rs dispatch_look_driven_models
    // writes the same message; kuluu-render/src/picking.rs send_click shows the World-side write
    // through the Messages resource).
    for (id, file) in [(BAT_W, BAT_FILE), (WALKER_W, WALKER_FILE)] {
        app.world_mut()
            .resource_mut::<bevy::ecs::message::Messages<LoadActorRequest>>()
            .write(LoadActorRequest {
                entity_id: id,
                subject: ActorSubject::Npc { file_id: file },
            });
    }

    // Poll spawns at most two actors per frame and the loads are async; 900 frames (15 s of
    // sim time) is far past both DATs' load times.
    let mut bat_live = None;
    let mut walker_live = None;
    watch(&mut app, 900, |_i, w| {
        if bat_live.is_none() {
            bat_live = live_root_probe(w, bat_parent);
        }
        if walker_live.is_none() {
            walker_live = live_root_probe(w, walker_parent);
        }
        bat_live.is_some() && walker_live.is_some()
    });

    let Some((bat_scale, bat_move)) = bat_live else {
        panic!("bat never left the placeholder root within 900 frames");
    };
    assert!(
        (bat_scale - 0.85).abs() < f32::EPSILON,
        "bat live root scale is {bat_scale}, expected 0.85 from Info byte 85"
    );
    assert_eq!(bat_move, ffxi_dat::cib::MovementType::Flying);

    let Some((walker_scale, walker_move)) = walker_live else {
        panic!("walker never left the placeholder root within 900 frames");
    };
    assert!(
        (walker_scale - 1.0).abs() < f32::EPSILON,
        "scale byte 100 must leave the walker at exactly 1.0, got {walker_scale}"
    );
    assert_eq!(walker_move, ffxi_dat::cib::MovementType::Walking);
}

// ---------------------------------------------------------------------------
// S7 - miss / guard / parry / knockback
// ---------------------------------------------------------------------------

/// S7a: Miss runs `sway` on the victim (sound-only for Rarab - assert the routine, not a clip).
#[test]
fn s7a_miss_runs_sway() {
    let Some(rarab) = load_rarab() else { return };
    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, _) = spawn_actor(&mut app, RARAB2_W, EntityKind::Mob, &rarab);
    step_n(&mut app, 10);

    // res=Miss(1).
    push_battle2(&mut app, RARAB_W, 1, Some(RARAB2_W), Some((1, 0, 0, 0, 0)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"sway")
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "miss runs sway on the victim at the impact frame"
    );
}

/// S7b: Guard runs `gurd`, whose Motion stage plays the gud? clip.
#[test]
fn s7b_guard_plays_gud_clip() {
    let Some(rarab) = load_rarab() else { return };
    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, vic_child) = spawn_actor(&mut app, RARAB2_W, EntityKind::Mob, &rarab);
    step_n(&mut app, 10);

    // res=Guard(2).
    push_battle2(&mut app, RARAB_W, 1, Some(RARAB2_W), Some((2, 0, 0, 0, 0)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"gurd")
            && active_clip(w, vic_child).is_some_and(|c| c.starts_with("gud"))
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "guard plays the gud? clip via gurd's Motion stage"
    );
}

/// S7c: Parry runs `pary`, which also carries a gud? Motion stage.
#[test]
fn s7c_parry_plays_gud_clip() {
    let Some(rarab) = load_rarab() else { return };
    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, vic_child) = spawn_actor(&mut app, RARAB2_W, EntityKind::Mob, &rarab);
    step_n(&mut app, 10);

    // res=Parry(3).
    push_battle2(&mut app, RARAB_W, 1, Some(RARAB2_W), Some((3, 0, 0, 0, 0)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"pary")
            && active_clip(w, vic_child).is_some_and(|c| c.starts_with("gud"))
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "parry plays the gud? clip via pary's Motion stage"
    );
}

/// S7d: Hit with knockback level 2 runs the damage reaction AND `sway` alongside. The
/// victim is fresh - no ActiveSchedulers yet - so both routines land in one same-batch insert;
/// this pins the merge fix that kept the sway insert from overwriting the damage reaction.
#[test]
fn s7d_knockback_adds_sway_alongside_the_damage_reaction() {
    let Some(rarab) = load_rarab() else { return };
    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, _) = spawn_actor(&mut app, RARAB2_W, EntityKind::Mob, &rarab);
    step_n(&mut app, 10);

    // res=Hit(0), dist=0, kb=2.
    push_battle2(&mut app, RARAB_W, 1, Some(RARAB2_W), Some((0, 0, 0, 0, 2)));

    let (impact_at, _) = watch(&mut app, 45, |_i, w| {
        routines(w, vic_parent).contains(b"damg") && routines(w, vic_parent).contains(b"sway")
    });
    assert!(
        impact_at.is_some_and(|f| f >= IMPACT_FRAME_MIN),
        "kb>0 runs the damage reaction and sway together (F52)"
    );
}

// ---------------------------------------------------------------------------
// S8b - stun: observation only
// ---------------------------------------------------------------------------

/// S8b: BATTLE2 carries no stun payload - the status byte path is untraced in LSB, so this
/// scenario documents what the snapshot carries instead of asserting a DAT-driven stun clip.
/// A result-less body must not arm any reaction and must not panic.
#[test]
fn s8b_resultless_body_arms_nothing() {
    let Some(rarab) = load_rarab() else { return };
    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, _) = spawn_actor(&mut app, RARAB2_W, EntityKind::Mob, &rarab);
    step_n(&mut app, 10);

    // Target present but nres=0: no result block at all.
    push_battle2(&mut app, RARAB_W, 1, Some(RARAB2_W), None);
    step_n(&mut app, 60);

    // The only routine the victim may carry is `init`, the create-time load routine: first
    // observation takes the hidden->visible resurface path and every model that ships an
    // init runs it on spawn, Rarab's degenerate one included. That is not a reaction to this
    // BATTLE2; anything else would be.
    let got = routines(app.world(), vic_parent);
    assert!(
        got.iter().all(|r| *r == *b"init"),
        "a result-less BATTLE2 arms no victim reaction, got {:?}",
        got.iter()
            .map(|r| std::str::from_utf8(r).unwrap_or("?"))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// S9 - Defeated: the dead routine falls over instead of popping to a corpse
// ---------------------------------------------------------------------------

/// S9: Hit with info=Defeated on a Rarab victim. The `dead` routine runs immediately:
/// ded? fall-over at its first Motion stage, and the pose pass holds idle across the gap -
/// never flashing cor? before ded? owns the pose (D5). build_app pins the pose pass between
/// dispatch_melee_action_started and tick_active_schedulers so the D5 hold path runs on the
/// event frame itself; dead_fall_over_pending() closes intra-update when the tick fires the
/// fall-over, so it is asserted through its observable effects (queued + ded? start + no cor?
/// flash) rather than sampled directly.
#[test]
fn s9_defeated_runs_dead_routine_and_holds_idle_across_the_gap() {
    let Some(rarab) = load_rarab() else { return };
    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (vic_parent, vic_child) = spawn_actor(&mut app, RARAB2_W, EntityKind::Mob, &rarab);
    step_n(&mut app, 10);

    // res=Hit(0), info bit1 = Defeated.
    push_battle2(&mut app, RARAB_W, 1, Some(RARAB2_W), Some((0, 0, 1, 0, 0)));

    // The dead routine is queued on the event's frame (same-frame death path).
    let (queued_at, _) = watch(&mut app, 3, |i, w| {
        i >= 1 && routines(w, vic_parent).contains(b"dead")
    });
    assert!(
        queued_at.is_some(),
        "Defeated latches the death path on this frame (F49)"
    );

    // No cor? flash across the gap: from the event through the fall-over start the pose stays
    // off the corpse clip, and the ded? fall-over starts within a few frames.
    let mut cor_flashed = false;
    let (ded_at, _) = watch(&mut app, 12, |_i, w| {
        cor_flashed |= pose_clip(w, vic_child).is_some_and(|c| c.starts_with("cor"));
        active_clip(w, vic_child).is_some_and(|c| c.starts_with("ded"))
    });
    assert!(
        ded_at.is_some(),
        "the ded? fall-over clip starts within a few frames"
    );
    assert!(
        !cor_flashed,
        "the pose pass held idle across the fall-over gap - no cor? flash"
    );
}

// ---------------------------------------------------------------------------
// S10 - limb selection from BATTLE2's animation field (D6)
// ---------------------------------------------------------------------------

/// S10: HumeM swings LeftAttack (anim=1). Its motion DAT ships no bti0, so the overlay/pose
/// path must fall back to ati0 and play at0? - a model that lacks the limb still swings.
#[test]
fn s10_left_attack_without_bti0_falls_back_to_ati0() {
    let (Some(rarab), Some(humem)) = (load_rarab(), load_humem()) else {
        return;
    };
    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &rarab);
    let (_, atk_child) = spawn_actor(&mut app, HUMEM_W, EntityKind::Pc, &humem);
    step_n(&mut app, 10);

    // res=Hit(0), anim=LeftAttack(1).
    push_battle2(&mut app, HUMEM_W, 1, Some(RARAB_W), Some((0, 1, 0, 0, 0)));

    let (swing_at, _) = watch(&mut app, 5, |i, w| {
        i >= 1 && active_clip(w, atk_child).is_some_and(|c| c.starts_with("at0"))
    });
    assert!(
        swing_at.is_some(),
        "HumeM ships no bti0 - LeftAttack falls back to ati0's at0? clip"
    );
}

/// S10b: a model that DOES ship bti0 (ROM/352/115.DAT) swings LeftAttack and plays bti0's own
/// Motion clip instead of the ati0 fallback. DAT ground truth for this model: bti0's single
/// Motion stage is `at2?` @ frame 0 - a shared swing clip, NOT a dedicated "bt" clip (the DAT
/// also ships an unused btl0 animation). at2? vs ati0's at0? is what proves the overlay picked
/// bti0 rather than falling back.
#[test]
fn s10b_left_attack_with_bti0_plays_the_limb_clip() {
    let Some(loaded) = install().and_then(|_| load_npc(LIMB_MODEL_FILE).ok()) else {
        return;
    };
    // The limb model must actually carry bti0 with a Motion clip, or the scenario is void.
    use ffxi_dat::datid::DatId;
    let routines = loaded.all_routines();
    let Some(bti) = routines.get(&DatId::from_str("bti0")) else {
        return;
    };
    assert!(
        bti.stages
            .iter()
            .any(|t| t.stage.kind == ffxi_dat::scheduler::StageKind::Motion),
        "LIMB_MODEL_FILE's bti0 carries a Motion stage"
    );

    let mut app = build_app();
    spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &loaded);
    let (_, atk_child) = spawn_actor(&mut app, LIMB_W, EntityKind::Mob, &loaded);
    step_n(&mut app, 10);

    push_battle2(&mut app, LIMB_W, 1, Some(RARAB_W), Some((0, 1, 0, 0, 0)));

    let (swing_at, _) = watch(&mut app, 5, |i, w| {
        i >= 1 && active_clip(w, atk_child).is_some_and(|c| c.starts_with("at2"))
    });
    assert!(
        swing_at.is_some(),
        "a model that ships bti0 plays its own limb clip (at2?) for LeftAttack, not ati0's \
         at0? fallback"
    );
}

// ---------------------------------------------------------------------------
// S11 - missing-routine fall-through: an active animationsub on a model without the named
// routine must not freeze
// ---------------------------------------------------------------------------

/// S11: the frozen-mob regression. A nonzero animationsub names a special routine on the wire
/// (sub 1 -> `ini1`, FFXiMain.dll); retail plays that name on the model and no-ops when the
/// model does not ship it. Rarab's DAT ships no `ini1` routine, so the special tier must
/// fall through to locomotion instead of pinning current_clip: a not-moving mob idles on idl?
/// and keeps animating. Before the fall-through fix the miss registered the idle fallback as a
/// one-shot, which held its end frame forever (the spawn-pose freeze).
#[test]
fn s11_missing_routine_falls_through_on_a_model_without_ini1() {
    let Some(loaded) = load_rarab() else { return };
    // The model genuinely ships no ini1 routine: the fall-through is what has to save us.
    use ffxi_dat::datid::DatId;
    assert!(
        !loaded.all_routines().contains_key(&DatId::from_str("ini1")),
        "Rarab's DAT must lack an ini1 routine for this scenario to mean anything"
    );
    let mut app = build_app();
    let (_, child) = spawn_actor(&mut app, RARAB_W, EntityKind::Mob, &loaded);

    // First observation at sub 0: retail's create path runs 'init' on the new actor.
    // Rarab ships no usable init motion, so the pose stays on idle; stepping also establishes
    // the prev state that makes the sub change below a genuine table trigger instead of another
    // create.
    step_n(&mut app, 2);

    // Active animationsub while visible: next_special_pose triggers `ini1` on the next index
    // rebuild; the pose pass must resolve that name against this model and fall through.
    {
        let mut state = app.world_mut().resource_mut::<SceneState>();
        for e in &mut state.snapshot.entities {
            if e.id == RARAB_W {
                e.animationsub = 1;
            }
        }
    }

    step_n(&mut app, 30);
    let clip = pose_clip(app.world(), child).expect("pose pass ran");
    assert!(
        clip.starts_with("idl"),
        "an active sub on a model without an ini1 routine falls through to idle: got {clip}"
    );

    // And it is not frozen: the idle loop keeps advancing its frame. Prime sampling gaps keep
    // this from aliasing with any plausible loop length.
    let mut frames = Vec::new();
    for _ in 0..5 {
        step_n(&mut app, 7);
        frames.push(
            app.world()
                .entity(child)
                .get::<FfxiRenderActor>()
                .unwrap()
                .last_frame,
        );
    }
    assert!(
        frames.windows(2).any(|w| w[0] != w[1]),
        "the idle loop is playing, not pinned on one frame: {frames:?}"
    );
}

// ---------------------------------------------------------------------------
// S12 - worm special-pose cycle: create fires init; hiding follows status only, never clip
// completion
// ---------------------------------------------------------------------------

/// S12: the full special-pose lifecycle on a model that ships both routines (ROM/5/64.DAT).
/// First observation is a retail actor create and runs 'init': the pop-up sp0? plays once
/// and holds its end frame while the wire state stays up, with the model root visible throughout.
/// Retail hides only on status INVISIBLE, never on clip completion. A sub change then fires ini1
/// (dig-down sp1?), the buried window hides on status, resurface replays init instead of re-firing
/// ini1, and a sub clear settles back to locomotion.
#[test]
fn s12_worm_special_cycle_hides_only_on_status() {
    let Some(loaded) = load_worm() else { return };
    // Ground truth: both routines ship in this model's DAT (the pose pass takes each one's first
    // Motion stage).
    use ffxi_dat::datid::DatId;
    assert!(
        loaded.all_routines().contains_key(&DatId::from_str("init")),
        "worm DAT must ship the init load routine"
    );
    assert!(
        loaded.all_routines().contains_key(&DatId::from_str("ini1")),
        "worm DAT must ship the ini1 dig routine"
    );

    let mut app = build_app();
    let (parent, child) = spawn_actor(&mut app, WORM_W, EntityKind::Mob, &loaded);

    // Create path: the first visible observation runs 'init' -> sp0? pop-up.
    step_n(&mut app, 2);
    assert!(
        routines(app.world(), parent).contains(b"init"),
        "the create path fires init on the wire entity"
    );
    let clip = pose_clip(app.world(), child).expect("pose pass ran");
    assert!(
        clip.starts_with("sp0"),
        "worm spawn plays the init pop-up, got {clip}"
    );

    // sp0? completes and pins its end frame while the state stays up; the model root must stay
    // visible throughout - no hide on clip completion. The window far exceeds any plausible one-
    // shot length at this rig's 1 frame per step.
    for _ in 0..4 {
        step_n(&mut app, 300);
        assert_eq!(
            app.world().get::<Visibility>(child),
            Some(&Visibility::default()),
            "a completed or holding pop-up must not hide the model"
        );
    }

    // Dig start: sub set while visible fires ini1 -> sp1?.
    {
        let mut state = app.world_mut().resource_mut::<SceneState>();
        for e in &mut state.snapshot.entities {
            if e.id == WORM_W {
                e.animationsub = 1;
            }
        }
    }
    step_n(&mut app, 2);
    let clip = pose_clip(app.world(), child).expect("pose pass ran");
    assert!(
        clip.starts_with("sp1"),
        "dig start plays the ini1 dig-down, got {clip}"
    );

    // Buried: status INVISIBLE hides the model root outright.
    {
        let mut state = app.world_mut().resource_mut::<SceneState>();
        for e in &mut state.snapshot.entities {
            if e.id == WORM_W {
                e.status = 3;
            }
        }
    }
    step_n(&mut app, 2);
    assert_eq!(
        app.world().get::<Visibility>(child),
        Some(&Visibility::Hidden),
        "status INVISIBLE hides the model"
    );

    // Resurface: visible again with the sub still set -> 'init' replays (the create path), not a
    // re-fire of ini1.
    {
        let mut state = app.world_mut().resource_mut::<SceneState>();
        for e in &mut state.snapshot.entities {
            if e.id == WORM_W {
                e.status = 0;
            }
        }
    }
    step_n(&mut app, 2);
    assert_eq!(
        app.world().get::<Visibility>(child),
        Some(&Visibility::default())
    );
    let clip = pose_clip(app.world(), child).expect("pose pass ran");
    assert!(
        clip.starts_with("sp0"),
        "resurface replays the init pop-up, got {clip}"
    );

    // Settle: sub clears -> back to locomotion.
    {
        let mut state = app.world_mut().resource_mut::<SceneState>();
        for e in &mut state.snapshot.entities {
            if e.id == WORM_W {
                e.animationsub = 0;
            }
        }
    }
    step_n(&mut app, 30);
    let clip = pose_clip(app.world(), child).expect("pose pass ran");
    assert!(
        clip.starts_with("idl"),
        "settled worm idles on locomotion, got {clip}"
    );
}
