use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use ffxi_client::lobby_client::CharSlot;
use ffxi_viewer_core::dat_vos2::spawn_equipped;
use ffxi_viewer_core::ffxi_actor_render::{
    inputs_for_pose, load_pc, spawn_loaded_actor, FfxiRenderActor, LoadedActor, PoseState,
};
use ffxi_viewer_core::look_resolver::{resolve_equipment_slot, resolve_face};
use ffxi_viewer_core::skinned_ffxi_material::{FfxiLightingUniform, FfxiSkinnedMaterial};

use super::{char_list::CharCursor, CharListData};
use crate::view_native::launcher_backdrop::PREVIEW_RENDER_LAYER;

#[derive(Component)]
pub(super) struct CharPreviewRoot;

#[derive(Component)]
pub(super) struct CharPreviewParent;

#[derive(Component)]
pub(super) struct CharPreviewActorRoot;

#[derive(Resource, Default)]
pub(super) struct PreviewedSlot {
    pub char_id: Option<u32>,
}

#[derive(Resource, Default)]
pub(super) struct PendingPreview {
    pub task: Option<(u32, Task<Result<LoadedActor, String>>)>,
}

const PREVIEW_PARENT_POS: Vec3 = Vec3::new(-1.4, 0.0, 0.0);

const PREVIEW_CAMERA_OFFSET: Vec3 = Vec3::new(0.0, 1.3, 3.5);

const PREVIEW_LOOK_AT_OFFSET: Vec3 = Vec3::new(0.0, 1.0, 0.0);

const PREVIEW_FACING_DIR: f32 = 0.0;

const PREVIEW_SCALE: f32 = 1.0;

const PREVIEW_POSE: PoseState = PoseState::Idle;

pub(super) fn spawn_preview(mut commands: Commands) {
    commands.insert_resource(PreviewedSlot::default());
    commands.insert_resource(PendingPreview::default());

    let root = commands
        .spawn((CharPreviewRoot, Transform::default(), Visibility::default()))
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
        AmbientLight {
            color: Color::srgb(0.88, 0.90, 1.0),
            brightness: 2_500.0,
            ..default()
        },
        ChildOf(root),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 15_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + Vec3::new(1.5, 3.0, 5.0))
            .looking_at(PREVIEW_PARENT_POS + Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        ChildOf(root),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 7_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + Vec3::new(-2.5, 2.0, 3.0))
            .looking_at(PREVIEW_PARENT_POS + Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        ChildOf(root),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 4_000.0,
            shadow_maps_enabled: false,
            ..default()
        },
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + Vec3::new(0.0, 2.5, -3.0))
            .looking_at(PREVIEW_PARENT_POS + Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        ChildOf(root),
    ));

    let parent = commands
        .spawn((
            CharPreviewParent,
            Transform {
                translation: PREVIEW_PARENT_POS,
                rotation: Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
                scale: Vec3::ONE,
            },
            Visibility::default(),
            ChildOf(root),
        ))
        .id();
    let _ = parent;
}

pub(super) fn tag_preview_meshes(
    trigger: On<Add, Mesh3d>,
    parents: Query<&ChildOf>,
    preview_parents: Query<(), With<CharPreviewParent>>,
    mut commands: Commands,
) {
    let entity = trigger.event().event_target();
    let mut cur = entity;
    loop {
        let Ok(child_of) = parents.get(cur) else {
            return;
        };
        let parent = child_of.parent();
        if preview_parents.contains(parent) {
            commands
                .entity(entity)
                .insert(RenderLayers::layer(PREVIEW_RENDER_LAYER));
            return;
        }
        cur = parent;
    }
}

pub(super) fn ensure_preview_render_layer(
    roots: Query<Entity, With<CharPreviewActorRoot>>,
    children: Query<&Children>,
    untagged_meshes: Query<(), (With<Mesh3d>, Without<RenderLayers>)>,
    mut commands: Commands,
) {
    for root in &roots {
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if untagged_meshes.contains(entity) {
                commands
                    .entity(entity)
                    .insert(RenderLayers::layer(PREVIEW_RENDER_LAYER));
            }
            if let Ok(kids) = children.get(entity) {
                stack.extend(kids.iter());
            }
        }
    }
}

pub(super) fn despawn_preview(
    mut commands: Commands,
    q_root: Query<Entity, With<CharPreviewRoot>>,
) {
    for e in q_root.iter() {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<PreviewedSlot>();
    commands.remove_resource::<PendingPreview>();
}

pub(super) fn refresh_preview_on_cursor_change(
    mut commands: Commands,
    chars: Res<CharListData>,
    cursor: Res<CharCursor>,
    mut previewed: ResMut<PreviewedSlot>,
    mut pending: ResMut<PendingPreview>,
    q_parent: Query<(Entity, Option<&Children>), With<CharPreviewParent>>,
) {
    let active = active_slot(&chars, &cursor);
    let new_id = active.map(|s| s.char_id);
    if new_id == previewed.char_id {
        return;
    }
    let Ok((_parent, kids)) = q_parent.single() else {
        return;
    };

    if let Some(kids) = kids {
        for child in kids.iter() {
            commands.entity(child).despawn();
        }
    }

    match active {
        Some(slot) if slot.race != 0 => {
            let race = slot.race;

            let equipment = pc_equipment_file_ids(slot);
            let char_id = slot.char_id;

            let task = AsyncComputeTaskPool::get()
                .spawn(async move { load_pc(race, &equipment, None, None) });
            pending.task = Some((char_id, task));
        }
        _ => pending.task = None,
    }
    previewed.char_id = new_id;
}

pub(super) fn poll_pending_preview(
    mut commands: Commands,
    chars: Res<CharListData>,
    cursor: Res<CharCursor>,
    mut pending: ResMut<PendingPreview>,
    q_parent: Query<Entity, With<CharPreviewParent>>,
    settings: Res<ffxi_viewer_core::GraphicsSettings>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let ready = match pending.task.as_mut() {
        Some((_, task)) => future::block_on(future::poll_once(task)),
        None => return,
    };
    let Some(loaded) = ready else {
        return;
    };
    let (target_id, _) = pending.task.take().expect("task present when ready");

    if active_slot(&chars, &cursor).map(|s| s.char_id) != Some(target_id) {
        return;
    }
    let loaded = match loaded {
        Ok(l) => l,
        Err(e) => {
            warn!("char preview: load_pc failed for char_id={target_id}: {e}");
            return;
        }
    };
    let Ok(parent) = q_parent.single() else {
        return;
    };

    let quality = ffxi_viewer_core::zone_texture::TextureQuality {
        mipmaps: settings.texture_filtering.mipmaps(),
        anisotropy: settings.texture_filtering.anisotropy(),
    };
    let actor_root = spawn_loaded_actor(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        &loaded,
        Vec3::ZERO,
        PREVIEW_FACING_DIR,
        PREVIEW_SCALE,
        quality,
    );
    commands
        .entity(actor_root)
        .insert((CharPreviewActorRoot, ChildOf(parent)));

    debug!("char preview: char_id={target_id} faithful actor spawned");
}

pub(super) fn drive_preview_pose(mut q: Query<&mut FfxiRenderActor, With<CharPreviewActorRoot>>) {
    for mut actor in &mut q {
        let want = inputs_for_pose(PREVIEW_POSE, false);

        if actor.inputs.moving != want.moving
            || actor.inputs.walking != want.walking
            || actor.inputs.forward_vel != want.forward_vel
            || actor.inputs.strafe_vel != want.strafe_vel
            || actor.inputs.dead != want.dead
            || actor.inputs.rest != want.rest
            || actor.inputs.engage_state != want.engage_state
        {
            actor.inputs = want;
        }
    }
}

pub(super) fn relight_preview_actor(
    q_root: Query<&Children, With<CharPreviewActorRoot>>,
    q_mat: Query<&MeshMaterial3d<FfxiSkinnedMaterial>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
) {
    let lighting = FfxiLightingUniform::default();
    for children in &q_root {
        for child in children.iter() {
            if let Ok(mat_handle) = q_mat.get(child) {
                if let Some(mut mat) = materials.get_mut(&mat_handle.0) {
                    mat.lighting = lighting.clone();

                    mat.material_flags.flags.y = 0.0;
                }
            }
        }
    }
}

fn active_slot<'a>(chars: &'a CharListData, cursor: &CharCursor) -> Option<&'a CharSlot> {
    chars.0.get(cursor.0)
}

fn pc_equipment_file_ids(slot: &CharSlot) -> Vec<u32> {
    let race = slot.race;
    let mut equipment: Vec<u32> = Vec::new();
    if let Some(file_id) = resolve_face(slot.face, race) {
        equipment.push(file_id);
    }
    let slot_ids = [
        slot.head,
        slot.body,
        slot.hands,
        slot.legs,
        slot.feet,
        slot.main,
        slot.sub,
        slot.ranged,
    ];
    for slot_id in slot_ids {
        if let Some(file_id) = resolve_equipment_slot(slot_id, race) {
            equipment.push(file_id);
        }
    }
    equipment
}

// A freshly-created character wears no gear, but FFXI still draws the naked
// default body: each clothing slot's model id 0, slot-prefixed (slot_index << 12)
// so resolve_equipment_slot tags it to the per-race base instead of reading the
// prefix as "empty". Head and weapons stay 0 (truly empty) so the chosen hair
// shows and no weapon is held. Mirrors ffxi_actor_render::default_pc_equipment.
const SLOT_PREFIX_SHIFT: u16 = 12;
const NAKED_BODY: u16 = 2 << SLOT_PREFIX_SHIFT;
const NAKED_HANDS: u16 = 3 << SLOT_PREFIX_SHIFT;
const NAKED_LEGS: u16 = 4 << SLOT_PREFIX_SHIFT;
const NAKED_FEET: u16 = 5 << SLOT_PREFIX_SHIFT;

pub(super) fn spawn_preview_pc(
    commands: &mut Commands,
    parent: Entity,
    race: u8,
    face: u8,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) -> usize {
    if race == 0 {
        return 0;
    }
    spawn_equipped(
        commands,
        meshes,
        materials,
        images,
        parent,
        race,
        face,
        0,
        NAKED_BODY,
        NAKED_HANDS,
        NAKED_LEGS,
        NAKED_FEET,
        0,
        0,
        0,
    )
}
