//! Standalone model viewer — opens a Bevy window with a 3D preview and a
//! side panel for inspecting arbitrary PC race/face/equipment combos and
//! NPC model_ids with animation playback.
//!
//! Bypasses every networking surface (auth, lobby, map, relay, reactor).
//! Only the local DAT install is read — invoked via the `model-viewer`
//! subcommand on `ffxi-client`. Mirrors the launcher `char_preview`
//! camera/lighting setup (`view_native/launcher_ui/char_preview.rs`) but
//! drives off form inputs rather than a server-pushed `CharSlot`.

use std::time::{Duration, Instant};

use anyhow::Result;
use bevy::camera::visibility::RenderLayers;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use ffxi_viewer_core::combat_stance::{
    enumerate_clips_for_skel, ModelViewerClipOverride, RestStance,
};
use ffxi_viewer_core::components::WorldEntity;
use ffxi_viewer_core::dat_vos2::{
    enumerate_vos2_chunks, process_load_vos2_requests, tick_skinned_actors, LoadVos2Request,
    SkinnedActor,
};
use ffxi_viewer_core::look_resolver::{npc_dat_id, resolve_equipment_slot, resolve_face};
use ffxi_viewer_core::scene::{BakedActor, TrackedEntities};
use ffxi_viewer_core::snapshot::SceneState;
use ffxi_viewer_wire::EntityKind;

use super::launcher_backdrop::PREVIEW_RENDER_LAYER;
use super::widgets;

mod panel;

/// Inputs for [`run`]. The DAT root is the one hard dependency; every
/// other field maps 1:1 to a form input and is pre-populated when set
/// so a `/look <name>` line drops straight into a command. `None` for
/// any field falls back to the form's default. Numeric ids accept hex
/// (`0x1006`) or decimal — see [`parse_id_lenient`].
pub struct ModelViewerArgs {
    pub dat_root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
    pub race: Option<u8>,
    pub face: Option<u8>,
    pub head: Option<String>,
    pub body: Option<String>,
    pub hands: Option<String>,
    pub legs: Option<String>,
    pub feet: Option<String>,
    pub main: Option<String>,
    pub sub: Option<String>,
    pub ranged: Option<String>,
    /// When set, the viewer starts in NPC mode with this `model_id`
    /// and the PC fields are ignored.
    pub model_id: Option<String>,
    /// Initial clip name (3-char prefix). Defaults to `"idl"`.
    pub clip: Option<String>,
}

/// Parse a hex (`0x…`) or decimal numeric id from a CLI string. `None`
/// on parse failure — caller logs a warning and the form field keeps
/// its default. Same semantics as `panel::parse_u16_lenient`.
fn parse_id_lenient(s: &str) -> Option<u16> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u16::from_str_radix(hex, 16).ok()
    } else {
        s.parse::<u16>().ok()
    }
}

/// Synthetic wire-entity id used for the preview parent. Picked
/// arbitrarily — it just needs to be stable across rebakes so
/// `TrackedEntities` keeps pointing at the same Bevy entity, and
/// nonzero so it never collides with a "no entity" sentinel.
const PREVIEW_ENTITY_ID: u32 = 1;

const PREVIEW_PARENT_POS: Vec3 = Vec3::ZERO;
const PREVIEW_CAMERA_OFFSET: Vec3 = Vec3::new(0.0, 1.3, 3.5);
const PREVIEW_LOOK_AT_OFFSET: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const TURNTABLE_RAD_PER_SEC: f32 = 0.3;

/// Debounce window for form-driven rebakes — typing a digit at a time
/// in the head-id box shouldn't trigger a rebake per keystroke. Matches
/// `char_create_preview::DEBOUNCE`.
const REBAKE_DEBOUNCE: Duration = Duration::from_millis(150);

/// Which input set the form is showing: PC equipment vs. NPC modelid.
#[derive(Resource, Debug, Clone, Copy, Eq, PartialEq)]
#[derive(Default)]
pub enum ViewerMode {
    #[default]
    Pc,
    Npc,
}


/// The currently-displayed PC inputs. Edited by [`panel`] via
/// `ValueChange<String>` triggers; consumed on rebake by
/// [`spawn_for_mode`].
#[derive(Resource, Debug, Clone)]
pub struct PcForm {
    pub race: u8,
    pub face: u8,
    pub head: u16,
    pub body: u16,
    pub hands: u16,
    pub legs: u16,
    pub feet: u16,
    pub main: u16,
    pub sub: u16,
    pub ranged: u16,
}

impl Default for PcForm {
    fn default() -> Self {
        Self {
            race: 1,
            face: 0,
            head: 0,
            body: 0,
            hands: 0,
            legs: 0,
            feet: 0,
            main: 0,
            sub: 0,
            ranged: 0,
        }
    }
}

#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct NpcForm {
    pub model_id: u16,
}

/// Marker for the camera + lighting subgraph. Survives rebakes.
#[derive(Component)]
struct PreviewRoot;

/// Marker for the parent the spawned meshes attach under. Despawned and
/// re-created on every rebake (the underlying `SkinnedActor` bone
/// hierarchy is per-skeleton, so we don't try to reuse it across races
/// or PC↔NPC swaps).
#[derive(Component)]
struct PreviewParent;

/// Apply turntable rotation to this entity.
#[derive(Component)]
struct PreviewTurntable;

/// Debounced rebake state. Mirrors `char_create_preview::PreviewRebakeState`.
#[derive(Resource)]
struct RebakeState {
    /// `Some(_)` when the form has been edited and we're waiting for
    /// the typing storm to die down. `None` between rebakes.
    pending_since: Option<Instant>,
    /// Run a one-shot bake on the first frame.
    initial_bake_pending: bool,
}

impl Default for RebakeState {
    fn default() -> Self {
        Self {
            pending_since: None,
            initial_bake_pending: true,
        }
    }
}

/// Enumerated clip names for the current skeleton + (PC mode only)
/// motion DAT. Refreshed on rebake. Empty until the first bake fires.
#[derive(Resource, Default, Debug, Clone)]
pub struct ClipList {
    pub names: Vec<String>,
    pub index: usize,
}

impl ClipList {
    pub fn current(&self) -> Option<&str> {
        self.names.get(self.index).map(String::as_str)
    }
}

pub fn run(args: ModelViewerArgs) -> Result<()> {
    let ModelViewerArgs {
        dat_root,
        race,
        face,
        head,
        body,
        hands,
        legs,
        feet,
        main,
        sub,
        ranged,
        model_id,
        clip,
    } = args;

    // Compose the initial form values from the CLI. `--model-id` forces
    // NPC mode and the PC fields are ignored; otherwise PC mode is the
    // default and the slot/race/face fields fall back to the form's
    // built-in defaults whenever a flag was omitted.
    let mut pc_form = PcForm::default();
    if let Some(v) = race {
        pc_form.race = v;
    }
    if let Some(v) = face {
        pc_form.face = v;
    }
    let apply = |dst: &mut u16, src: Option<String>, name: &str| {
        let Some(s) = src else { return };
        match parse_id_lenient(&s) {
            Some(v) => *dst = v,
            None => warn!("model viewer: ignoring --{name}={s:?} (not a u16)"),
        }
    };
    apply(&mut pc_form.head, head, "head");
    apply(&mut pc_form.body, body, "body");
    apply(&mut pc_form.hands, hands, "hands");
    apply(&mut pc_form.legs, legs, "legs");
    apply(&mut pc_form.feet, feet, "feet");
    apply(&mut pc_form.main, main, "main");
    apply(&mut pc_form.sub, sub, "sub");
    apply(&mut pc_form.ranged, ranged, "ranged");

    let mut npc_form = NpcForm::default();
    let mode = if let Some(s) = model_id {
        match parse_id_lenient(&s) {
            Some(v) => {
                npc_form.model_id = v;
                ViewerMode::Npc
            }
            None => {
                warn!(
                    "model viewer: ignoring --model-id={s:?} (not a u16); falling back to PC mode"
                );
                ViewerMode::Pc
            }
        }
    } else {
        ViewerMode::Pc
    };

    let initial_clip = clip.unwrap_or_else(|| "idl".to_string());

    let mut app = App::new();

    // Same plugin discipline as the main native window: skip LogPlugin
    // because main.rs already installed the tracing subscriber.
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "ffxi model viewer".into(),
                    resolution: (1280u32, 800u32).into(),
                    ..default()
                }),
                ..default()
            })
            .build()
            .disable::<LogPlugin>(),
    );

    // Feathers + the local widgets crate (for TextField).
    app.add_plugins(bevy::feathers::FeathersPlugins)
        .insert_resource(bevy::feathers::theme::UiTheme(
            bevy::feathers::dark_theme::create_dark_theme(),
        ))
        .add_plugins(widgets::WidgetsPlugin);

    // Resources `tick_skinned_actors` + `process_load_vos2_requests`
    // expect to find — the viewer skips `ViewerCorePlugin` to keep
    // start-up free of zone loaders, audio, snapshot ingestion, etc.
    app.init_resource::<SceneState>()
        .init_resource::<TrackedEntities>()
        .init_resource::<ffxi_viewer_core::combat_stance::EntityMotion>()
        .init_resource::<ffxi_viewer_core::combat_stance::AnimationBlends>()
        .init_resource::<RestStance>()
        .init_resource::<RebakeState>()
        .init_resource::<ClipList>()
        // CLI-seeded values override the form's built-in defaults. PcForm
        // and NpcForm both carry every field the panel binds to, so the
        // panel reads + writes the same resource the CLI populated.
        .insert_resource(mode)
        .insert_resource(pc_form)
        .insert_resource(npc_form)
        // The override resource is what tells `tick_skinned_actors` to
        // bypass the engagement / motion / rest matrix and just play
        // whatever clip we name. Always present in viewer mode.
        .insert_resource(ModelViewerClipOverride::new(initial_clip));

    // Dat root is what `spawn_equipped` / `enumerate_vos2_chunks` /
    // `load_anim_with_prefix` read from. Without it the bake silently
    // produces nothing — we surface the absence in a startup info log so
    // the operator can fix `FFXI_DAT_PATH` and re-run.
    if dat_root.is_none() {
        warn!(
            "model viewer: no FFXI DAT install reachable; the preview will be empty. \
             Set FFXI_DAT_PATH or run `ffxi-client --require-dat model-viewer` to fail fast."
        );
    }

    // The bake pipeline reads DatRoot via env (`DatRoot::from_env_or_default`),
    // not via a Bevy resource, so we don't need to insert the Arc.

    // Required message channel for the NPC dispatch path.
    app.add_message::<LoadVos2Request>();

    // Systems:
    //   1. Spawn the static scene (camera, lights, ground) once.
    //   2. Refresh clip list when the override resource was just set
    //      so the cycler sees the right clip pool for the new skeleton.
    //   3. Apply the named clip override → updates the resource the
    //      tick reads.
    //   4. Process pending NPC load requests (PC path is a direct call
    //      inside `do_rebake`, so it doesn't need the system).
    //   5. Tick bones from the named clip.
    //   6. Spin the turntable.
    //   7. Debounced rebake on form change.
    //   8. Tag any new mesh that's a descendant of the preview parent
    //      with the preview render layer (mirrors `char_preview::
    //      tag_preview_meshes`).
    app.add_systems(Startup, spawn_static_scene)
        .add_systems(
            Update,
            (
                trigger_rebake_on_form_change,
                debounced_rebake,
                turntable,
                process_load_vos2_requests,
                tick_skinned_actors,
            ),
        )
        .add_observer(tag_preview_meshes);

    panel::register(&mut app);

    app.run();
    Ok(())
}

fn spawn_static_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Root carries the camera + lights so we can keep them across
    // rebakes (rebake despawns `PreviewParent` only).
    let root = commands
        .spawn((PreviewRoot, Transform::default(), Visibility::default()))
        .id();

    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            ..default()
        },
        // Layer-isolate the camera so it doesn't pick up the panel UI's
        // render layer (UI cameras render via their own pass — this is
        // belt-and-suspenders against future UI 3D widgets).
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + PREVIEW_CAMERA_OFFSET)
            .looking_at(PREVIEW_PARENT_POS + PREVIEW_LOOK_AT_OFFSET, Vec3::Y),
        ChildOf(root),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 8_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(PREVIEW_PARENT_POS, Vec3::Y),
        ChildOf(root),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 3_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(-2.0, 2.0, -2.0).looking_at(PREVIEW_PARENT_POS, Vec3::Y),
        ChildOf(root),
    ));
    commands.insert_resource(AmbientLight {
        color: Color::srgb(0.85, 0.88, 1.0),
        brightness: 200.0,
        ..default()
    });

    // Ground quad on the preview layer so we get a visual anchor for
    // feet placement without loading a real zone.
    let ground_mesh = meshes.add(Plane3d::default().mesh().size(8.0, 8.0));
    let ground_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.15, 0.16, 0.18),
        perceptual_roughness: 0.95,
        ..default()
    });
    commands.spawn((
        Mesh3d(ground_mesh),
        MeshMaterial3d(ground_mat),
        Transform::from_xyz(0.0, 0.0, 0.0),
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        ChildOf(root),
    ));
}

/// On every rebake we despawn the preview parent (if any) and create a
/// fresh one carrying a synthetic [`WorldEntity`] so `tick_skinned_actors`
/// picks it up. Same `PREVIEW_ENTITY_ID` across rebakes — keeps
/// `TrackedEntities` stable.
fn do_rebake(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    tracked: &mut TrackedEntities,
    npc_loads: &mut MessageWriter<LoadVos2Request>,
    mode: ViewerMode,
    pc: &PcForm,
    npc: NpcForm,
    clip_list: &mut ClipList,
    existing_parent: Option<Entity>,
) -> Option<Entity> {
    if let Some(prev) = existing_parent {
        commands.entity(prev).despawn();
        tracked.by_id.remove(&PREVIEW_ENTITY_ID);
    }

    let parent = commands
        .spawn((
            PreviewParent,
            PreviewTurntable,
            // Composed bind-flip from `char_preview`: turn the model
            // 180° so its face points at the camera (the in-world bake
            // is authored facing -Z, away from the chase camera).
            Transform {
                translation: PREVIEW_PARENT_POS,
                rotation: Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
                scale: Vec3::ONE,
            },
            Visibility::default(),
            WorldEntity {
                id: PREVIEW_ENTITY_ID,
                act_index: 0,
                kind: match mode {
                    ViewerMode::Pc => EntityKind::Pc,
                    ViewerMode::Npc => EntityKind::Npc,
                },
            },
            // BakedActor + nameplate anchor; viewer doesn't render
            // nameplates but `tick_skinned_actors` reads neither, so
            // a placeholder is fine.
            BakedActor {
                min_mesh_y: 0.0,
                actor_height: 1.7,
            },
        ))
        .id();

    tracked.by_id.insert(PREVIEW_ENTITY_ID, parent);

    let _ = (meshes, materials, images); // reserved for a future CPU-bake fallback
    let skel_file_id = match mode {
        ViewerMode::Pc => {
            // Force the GPU-skinned path by setting `skeleton_file_id`
            // on every dispatched slot. Mirrors live-play's Equipped
            // dispatch (`look_resolver.rs:715-744`) since `0454f11`
            // flipped that branch onto the same path.
            //
            // `enumerate_vos2_chunks` returns every OS2 chunk index in
            // the DAT — face DATs and some slot DATs ship more than
            // one (head LOD variants etc.). Hardcoding `chunk_idx: 4`
            // (the live-play convention for equipment slots) silently
            // missed the face mesh entirely. Match `spawn_equipped`'s
            // pass-1 loop: enumerate per-DAT, dispatch one request
            // per chunk.
            let Some(skel) = pc_race_to_skel(pc.race) else {
                return None;
            };
            let mut dispatch_dat = |file_id: u32| {
                let chunks = enumerate_vos2_chunks(file_id);
                if chunks.is_empty() {
                    return;
                }
                for chunk_idx in chunks {
                    npc_loads.write(LoadVos2Request {
                        file_id,
                        chunk_idx,
                        entity_id: PREVIEW_ENTITY_ID,
                        race: pc.race,
                        skeleton_file_id: Some(skel),
                    });
                }
            };
            if let Some(face_file) = resolve_face(pc.face, pc.race) {
                dispatch_dat(face_file);
            } else {
                warn!(
                    race = pc.race,
                    face = pc.face,
                    "model viewer: resolve_face returned None"
                );
            }
            for slot_id in [
                pc.head, pc.body, pc.hands, pc.legs, pc.feet, pc.main, pc.sub, pc.ranged,
            ] {
                let Some(file_id) = resolve_equipment_slot(slot_id, pc.race) else {
                    continue;
                };
                dispatch_dat(file_id);
            }
            Some(skel)
        }
        ViewerMode::Npc => {
            let dat_id = npc_dat_id(npc.model_id);
            let chunks = enumerate_vos2_chunks(dat_id);
            if chunks.is_empty() {
                warn!(
                    model_id = npc.model_id,
                    dat_id, "model viewer: no VOS2 chunks in NPC DAT"
                );
            }
            for chunk_idx in chunks {
                npc_loads.write(LoadVos2Request {
                    file_id: dat_id,
                    chunk_idx,
                    entity_id: PREVIEW_ENTITY_ID,
                    race: 0,
                    skeleton_file_id: Some(dat_id),
                });
            }
            Some(dat_id)
        }
    };

    refresh_clip_list(clip_list, skel_file_id);

    Some(parent)
}

fn pc_race_to_skel(race: u8) -> Option<u32> {
    match race {
        1 => Some(7072),
        2 => Some(10248),
        3 => Some(13424),
        4 => Some(16600),
        5 | 6 => Some(19776),
        7 => Some(23176),
        8 => Some(26352),
        _ => None,
    }
}

fn refresh_clip_list(list: &mut ClipList, skel_file_id: Option<u32>) {
    let Some(skel) = skel_file_id else {
        list.names.clear();
        list.index = 0;
        return;
    };
    let new_names: Vec<String> = enumerate_clips_for_skel(skel)
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    // Preserve cursor on the same clip name across rebakes when
    // possible — switching equipment shouldn't bounce the user off the
    // clip they were watching.
    let prior = list.current().map(str::to_string);
    list.names = new_names;
    list.index = prior
        .and_then(|p| list.names.iter().position(|n| *n == p))
        .unwrap_or(0);
}

fn trigger_rebake_on_form_change(
    mode: Res<ViewerMode>,
    pc: Res<PcForm>,
    npc: Res<NpcForm>,
    mut state: ResMut<RebakeState>,
) {
    if !(mode.is_changed() || pc.is_changed() || npc.is_changed()) {
        return;
    }
    state.pending_since = Some(Instant::now());
}

fn debounced_rebake(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut tracked: ResMut<TrackedEntities>,
    mut npc_loads: MessageWriter<LoadVos2Request>,
    mut clip_list: ResMut<ClipList>,
    mut state: ResMut<RebakeState>,
    mode: Res<ViewerMode>,
    pc: Res<PcForm>,
    npc: Res<NpcForm>,
    q_existing: Query<Entity, With<PreviewParent>>,
) {
    let should_bake = if state.initial_bake_pending {
        true
    } else if let Some(t) = state.pending_since {
        t.elapsed() >= REBAKE_DEBOUNCE
    } else {
        false
    };
    if !should_bake {
        return;
    }
    state.pending_since = None;
    state.initial_bake_pending = false;

    let existing = q_existing.iter().next();
    do_rebake(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        &mut tracked,
        &mut npc_loads,
        *mode,
        &pc,
        *npc,
        &mut clip_list,
        existing,
    );
}

fn turntable(time: Res<Time>, mut q: Query<&mut Transform, With<PreviewTurntable>>) {
    let delta = TURNTABLE_RAD_PER_SEC * time.delta_secs();
    for mut t in q.iter_mut() {
        t.rotation = Quat::from_rotation_y(delta) * t.rotation;
    }
}

/// Tag every freshly-spawned mesh under the preview parent with the
/// preview render layer. Same pattern + reasoning as
/// `char_preview::tag_preview_meshes` and `char_create_preview::
/// tag_create_preview_meshes`.
fn tag_preview_meshes(
    trigger: On<Add, Mesh3d>,
    parents: Query<&ChildOf>,
    preview_parents: Query<(), With<PreviewParent>>,
    skinned: Query<(), With<SkinnedActor>>,
    mut commands: Commands,
) {
    let entity = trigger.event().event_target();
    let mut cur = entity;
    // Walk up until we hit either the PreviewParent (then the mesh is
    // part of the bake) or the world root.
    for _ in 0..32 {
        let Ok(child_of) = parents.get(cur) else {
            break;
        };
        let parent = child_of.parent();
        if preview_parents.contains(parent) || skinned.contains(parent) {
            commands
                .entity(entity)
                .insert(RenderLayers::layer(PREVIEW_RENDER_LAYER));
            return;
        }
        cur = parent;
    }
}
