use std::time::{Duration, Instant};

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;

use super::char_preview::spawn_preview_pc;
use super::{CharCreateForm, LauncherState};
use crate::view_native::launcher_backdrop::PREVIEW_RENDER_LAYER;

#[derive(Component)]
struct CharCreatePreviewRoot;

#[derive(Component)]
struct CharCreatePreviewParent;

#[derive(Component)]
struct CharCreatePreviewTurntable;

#[derive(Resource)]
struct PreviewRebakeState {
    last_change: Instant,
    dirty: bool,
    last_race: u8,
    last_face: u8,
}

const DEBOUNCE: Duration = Duration::from_millis(150);
const TURNTABLE_RAD_PER_SEC: f32 = 0.3;

const PREVIEW_PARENT_POS: Vec3 = Vec3::new(0.0, 0.0, 0.0);
// Slide the camera (and its look-at) left of the model by the same amount so the
// view axis stays parallel to -Z and the model sits off-centre to the RIGHT of
// the full-screen preview render, beside the left-aligned create panel rather
// than behind it. Tune alongside MODEL_AREA_PCT in char_create.rs.
const MODEL_SCREEN_SHIFT: f32 = 1.05;
const PREVIEW_CAMERA_OFFSET: Vec3 = Vec3::new(-MODEL_SCREEN_SHIFT, 1.3, 3.0);
const PREVIEW_LOOK_AT_OFFSET: Vec3 = Vec3::new(-MODEL_SCREEN_SHIFT, 1.0, 0.0);

pub(super) fn register(app: &mut App) {
    app.add_observer(tag_create_preview_meshes)
        .add_systems(OnEnter(LauncherState::CharCreate), spawn_preview)
        .add_systems(OnExit(LauncherState::CharCreate), despawn_preview)
        .add_systems(
            Update,
            (mark_dirty_on_form_change, rebake_if_debounced, turntable)
                .run_if(in_state(LauncherState::CharCreate)),
        );
}

fn tag_create_preview_meshes(
    trigger: On<Add, Mesh3d>,
    parents: Query<&ChildOf>,
    preview_parents: Query<(), With<CharCreatePreviewParent>>,
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

fn spawn_preview(
    mut commands: Commands,
    form: Res<CharCreateForm>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let root = commands
        .spawn((
            CharCreatePreviewRoot,
            Transform::default(),
            Visibility::default(),
        ))
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
            CharCreatePreviewParent,
            CharCreatePreviewTurntable,
            Transform {
                translation: PREVIEW_PARENT_POS,
                rotation: Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2),
                scale: Vec3::ONE,
            },
            Visibility::default(),
            ChildOf(root),
        ))
        .id();

    spawn_preview_pc(
        &mut commands,
        parent,
        form.race,
        form.face,
        &mut meshes,
        &mut materials,
        &mut images,
    );

    commands.insert_resource(PreviewRebakeState {
        last_change: Instant::now(),
        dirty: false,
        last_race: form.race,
        last_face: form.face,
    });
}

fn despawn_preview(mut commands: Commands, q_root: Query<Entity, With<CharCreatePreviewRoot>>) {
    for e in q_root.iter() {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<PreviewRebakeState>();
}

fn mark_dirty_on_form_change(form: Res<CharCreateForm>, mut state: ResMut<PreviewRebakeState>) {
    if !form.is_changed() {
        return;
    }

    if form.race == state.last_race && form.face == state.last_face {
        return;
    }
    state.last_change = Instant::now();
    state.dirty = true;
}

fn rebake_if_debounced(
    mut commands: Commands,
    form: Res<CharCreateForm>,
    mut state: ResMut<PreviewRebakeState>,
    q_parent: Query<(Entity, Option<&Children>), With<CharCreatePreviewParent>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if !state.dirty {
        return;
    }
    if state.last_change.elapsed() < DEBOUNCE {
        return;
    }
    let Ok((parent, kids)) = q_parent.single() else {
        return;
    };
    if let Some(kids) = kids {
        for child in kids.iter() {
            commands.entity(child).despawn();
        }
    }
    spawn_preview_pc(
        &mut commands,
        parent,
        form.race,
        form.face,
        &mut meshes,
        &mut materials,
        &mut images,
    );
    state.dirty = false;
    state.last_race = form.race;
    state.last_face = form.face;
}

fn turntable(time: Res<Time>, mut q: Query<&mut Transform, With<CharCreatePreviewTurntable>>) {
    let delta = TURNTABLE_RAD_PER_SEC * time.delta_secs();
    for mut t in q.iter_mut() {
        t.rotation = Quat::from_rotation_y(delta) * t.rotation;
    }
}
