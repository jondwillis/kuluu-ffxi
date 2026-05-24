//! Live 3D preview for the character-create screen.
//!
//! Mirrors `char_preview` (which renders an *existing* slot on the
//! char-list screen) but drives off the in-progress `CharCreateForm`
//! so the user sees their race/face choices update as they edit.
//! Equipment slots are zero — a fresh PC has no gear.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::camera::visibility::RenderLayers;

use super::char_preview::spawn_preview_pc;
use super::{CharCreateForm, LauncherState};
use crate::view_native::launcher_backdrop::PREVIEW_RENDER_LAYER;

#[derive(Component)]
struct CharCreatePreviewRoot;

#[derive(Component)]
struct CharCreatePreviewParent;

/// Marker on the rotating turntable transform.
#[derive(Component)]
struct CharCreatePreviewTurntable;

/// Debounce state for re-bake. Holds the time of the most recent
/// form change plus a "dirty" flag — the bake fires only once the
/// form has been quiet for [`DEBOUNCE`], avoiding a full re-spawn
/// every frame while the user arrow-keys through hair/face options.
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
const PREVIEW_CAMERA_OFFSET: Vec3 = Vec3::new(0.0, 1.3, 3.0);
const PREVIEW_LOOK_AT_OFFSET: Vec3 = Vec3::new(0.0, 1.0, 0.0);

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

/// Tag baked meshes under `CharCreatePreviewParent` with the preview
/// render layer so they don't render into the backdrop zone's pass.
/// See `char_preview::tag_preview_meshes` for the same pattern on the
/// char-list side; both share `PREVIEW_RENDER_LAYER` since the two
/// states are mutually exclusive.
fn tag_create_preview_meshes(
    trigger: On<Add, Mesh3d>,
    parents: Query<&ChildOf>,
    preview_parents: Query<(), With<CharCreatePreviewParent>>,
    mut commands: Commands,
) {
    let entity = trigger.target();
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

    // Dedicated 3D camera at order=-1 so it sits behind the UI
    // (UI defaults to order 0/+1). Matches char_preview convention.
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            ..default()
        },
        // Isolate from the backdrop zone's render pass — see
        // `char_preview::spawn_preview` for the same reasoning.
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + PREVIEW_CAMERA_OFFSET)
            .looking_at(PREVIEW_PARENT_POS + PREVIEW_LOOK_AT_OFFSET, Vec3::Y),
        ChildOf(root),
    ));

    // Neutral key + fill — the launcher can run before any zone
    // (and thus any sun system) exists, so don't rely on world
    // lighting. Match char_preview's three-point tuning.
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

    // Parent transform carries both the bind-pose flip (so the
    // model faces the camera instead of away — see char_preview
    // for the math) and the turntable rotation. We compose them at
    // spawn-bake time by feeding the turntable system a `Quat` that
    // multiplies the bind-flip on the right.
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

fn despawn_preview(
    mut commands: Commands,
    q_root: Query<Entity, With<CharCreatePreviewRoot>>,
) {
    for e in q_root.iter() {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<PreviewRebakeState>();
}

fn mark_dirty_on_form_change(
    form: Res<CharCreateForm>,
    mut state: ResMut<PreviewRebakeState>,
) {
    if !form.is_changed() {
        return;
    }
    // Only race/face affect the visible PC bake; ignore name/job/
    // nation/size churn so typing a name doesn't trigger a re-bake.
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

fn turntable(
    time: Res<Time>,
    mut q: Query<&mut Transform, With<CharCreatePreviewTurntable>>,
) {
    let delta = TURNTABLE_RAD_PER_SEC * time.delta_secs();
    for mut t in q.iter_mut() {
        t.rotation = Quat::from_rotation_y(delta) * t.rotation;
    }
}
