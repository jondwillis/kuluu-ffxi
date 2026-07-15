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

    pub model_id: Option<String>,

    pub clip: Option<String>,
}

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

const PREVIEW_ENTITY_ID: u32 = 1;

const PREVIEW_PARENT_POS: Vec3 = Vec3::ZERO;
const PREVIEW_CAMERA_OFFSET: Vec3 = Vec3::new(0.0, 1.3, 3.5);
const PREVIEW_LOOK_AT_OFFSET: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const TURNTABLE_RAD_PER_SEC: f32 = 0.3;

const REBAKE_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Resource, Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum ViewerMode {
    #[default]
    Pc,
    Npc,
}

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

#[derive(Component)]
struct PreviewRoot;

#[derive(Component)]
struct PreviewParent;

#[derive(Component)]
struct PreviewTurntable;

#[derive(Resource)]
struct RebakeState {
    pending_since: Option<Instant>,

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

    app.add_plugins(bevy::feathers::FeathersPlugins)
        .insert_resource(bevy::feathers::theme::UiTheme(
            bevy::feathers::dark_theme::create_dark_theme(),
        ))
        .add_plugins(widgets::WidgetsPlugin);

    app.init_resource::<SceneState>()
        .init_resource::<ffxi_viewer_core::GraphicsSettings>()
        .init_resource::<TrackedEntities>()
        .init_resource::<ffxi_viewer_core::combat_stance::EntityMotion>()
        .init_resource::<ffxi_viewer_core::combat_stance::AnimationBlends>()
        .init_resource::<RestStance>()
        .init_resource::<RebakeState>()
        .init_resource::<ClipList>()
        .insert_resource(mode)
        .insert_resource(pc_form)
        .insert_resource(npc_form)
        .insert_resource(ModelViewerClipOverride::new(initial_clip));

    if dat_root.is_none() {
        warn!(
            "model viewer: no FFXI DAT install reachable; the preview will be empty. \
             Set FFXI_DAT_PATH or run `ffxi-client --require-dat model-viewer` to fail fast."
        );
    }

    app.add_message::<LoadVos2Request>();

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
    let root = commands
        .spawn((PreviewRoot, Transform::default(), Visibility::default()))
        .id();

    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            ..default()
        },
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + PREVIEW_CAMERA_OFFSET)
            .looking_at(PREVIEW_PARENT_POS + PREVIEW_LOOK_AT_OFFSET, Vec3::Y),
        ChildOf(root),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 8_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(PREVIEW_PARENT_POS, Vec3::Y),
        ChildOf(root),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 3_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(-2.0, 2.0, -2.0).looking_at(PREVIEW_PARENT_POS, Vec3::Y),
        ChildOf(root),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.85, 0.88, 1.0),
        brightness: 200.0,
        ..default()
    });

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
            BakedActor {
                min_mesh_y: 0.0,
                actor_height: 1.7,
            },
        ))
        .id();

    tracked.by_id.insert(PREVIEW_ENTITY_ID, parent);

    let _ = (meshes, materials, images);
    let skel_file_id = match mode {
        ViewerMode::Pc => {
            let skel = pc_race_to_skel(pc.race)?;
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

fn tag_preview_meshes(
    trigger: On<Add, Mesh3d>,
    parents: Query<&ChildOf>,
    preview_parents: Query<(), With<PreviewParent>>,
    skinned: Query<(), With<SkinnedActor>>,
    mut commands: Commands,
) {
    let entity = trigger.event().event_target();
    let mut cur = entity;

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
