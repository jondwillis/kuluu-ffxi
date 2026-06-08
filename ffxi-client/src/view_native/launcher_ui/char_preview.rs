//! 3D character preview for the char-list screen.
//!
//! Renders the currently-cursored character as a 3D model behind
//! the launcher UI. Drives off the `CharCursor` resource maintained
//! by [`super::char_list`] — when the user moves the cursor, the
//! preview's children are despawned and respawned with the new
//! character's appearance via
//! [`ffxi_viewer_core::dat_vos2::spawn_equipped`].
//!
//! Doubles as a debugging surface for the bind-pose bake: the
//! launcher loads in seconds (no auth, no zoning), so iterating on
//! the bake math here is much faster than logging into the live
//! server. Cycle race 1..8 with the arrow keys and watch the
//! preview update.
//!
//! # Why a viewport + Y offset
//!
//! The 3D camera renders to the full window by default and the UI
//! sits on top. Without an offset, the character mesh would
//! coincide with where the right-aligned UI sits, producing a
//! visually messy "buttons on top of head" composition. We anchor
//! the preview a unit to the left of world origin and aim the
//! camera there — the UI's `padding: right(40px)` keeps the column
//! clear of the model.

use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use ffxi_client::lobby_client::CharSlot;
use ffxi_viewer_core::dat_vos2::{
    prepare_equipped, spawn_equipped, spawn_prepared_equipped, PreparedEquipped,
};

use super::{char_list::CharCursor, CharListData};
use crate::view_native::launcher_backdrop::PREVIEW_RENDER_LAYER;

/// Root marker for everything spawned by the preview scene —
/// camera, lighting, character parent. One despawn-tree under this
/// entity cleans up the whole subgraph on `LauncherState::CharList`
/// exit.
#[derive(Component)]
pub(super) struct CharPreviewRoot;

/// Marker for the parent entity the character meshes attach under.
/// Refresh on cursor change despawns this entity's *descendants*
/// (the equipment slot meshes) while leaving the parent in place.
#[derive(Component)]
pub(super) struct CharPreviewParent;

/// What character is currently rendered. `None` when the cursor is
/// on the "+ New character" row or when the slot has no usable
/// appearance data (race == 0). We compare against the next
/// cursor tick to decide whether to refresh — avoids despawning
/// and respawning the same character every frame.
#[derive(Resource, Default)]
pub(super) struct PreviewedSlot {
    pub char_id: Option<u32>,
}

/// In-flight background bake of the cursored character. The `u32` is
/// the target `char_id`; [`poll_pending_preview`] compares it against
/// the current cursor when the task lands and drops stale results from
/// rapid cursor movement. Loading the face + up to 8 equipment DATs is
/// 100–500 ms of synchronous IO/parse, so it runs on an
/// `AsyncComputeTaskPool` task instead of blocking the main thread.
#[derive(Resource, Default)]
pub(super) struct PendingPreview {
    pub task: Option<(u32, Task<PreparedEquipped>)>,
}

const PREVIEW_PARENT_POS: Vec3 = Vec3::new(-1.4, 0.0, 0.0);
/// Camera position relative to the preview parent.
///
/// Geometry: the baked PC mesh's mesh-y=0 is **feet** (verified
/// against the head-slot bake-extent diagnostic which showed head
/// at y≈[0.84..1.59]). With the parent at y=0, the character
/// spans roughly y ∈ [0, +1.7]. Aim at chest height (~1.0 yalm)
/// and camera slightly higher for a gentle downward look.
const PREVIEW_CAMERA_OFFSET: Vec3 = Vec3::new(0.0, 1.3, 3.5);
/// Aim at chest height — head ends up in the upper third per the
/// rule-of-thirds composition.
const PREVIEW_LOOK_AT_OFFSET: Vec3 = Vec3::new(0.0, 1.0, 0.0);

pub(super) fn spawn_preview(mut commands: Commands) {
    // Start with no previewed character + no pending task. The first
    // `refresh_preview_on_cursor_change` Update frame sees
    // `previewed.char_id == None != active.char_id` and kicks the
    // initial bake on a background task — so even the first preview is
    // non-blocking, identical to every cursor move after it.
    commands.insert_resource(PreviewedSlot::default());
    commands.insert_resource(PendingPreview::default());

    // Root entity owns the whole subgraph — camera, light, parent.
    // Needs Transform + Visibility so children inherit GlobalTransform
    // and InheritedVisibility (Bevy hierarchy warning B0004 otherwise).
    let root = commands
        .spawn((CharPreviewRoot, Transform::default(), Visibility::default()))
        .id();

    // 3D camera. Default order=0 puts it behind UI which renders at
    // a higher order. Camera looks at the parent's world position
    // from a fixed distance — the character should fill the left
    // side of the screen.
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1, // behind UI (UI defaults to 0/+1)
            ..default()
        },
        // Render only PC-preview entities, not the backdrop zone.
        // Without this, the camera would also see the loaded La
        // Theine geometry (which the backdrop loads into world space)
        // and the PC would be visually buried under terrain.
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + PREVIEW_CAMERA_OFFSET)
            .looking_at(PREVIEW_PARENT_POS + PREVIEW_LOOK_AT_OFFSET, Vec3::Y),
        // Per-camera AmbientLight component overrides the global
        // resource just for this camera — pumping the global to fix
        // dark HXI textures would also wash out the backdrop zone.
        AmbientLight {
            color: Color::srgb(0.88, 0.90, 1.0),
            brightness: 2_500.0,
            ..default()
        },
        ChildOf(root),
    ));

    // Three-point lighting. Key on the camera side (+Z) so the
    // model's face — which `bind_to_bevy` flips to face +Z toward
    // the camera — actually gets lit. The old setup put the key at
    // z=3 between camera (z=3.5) and parent (z=0), which lit the
    // model's right shoulder more than the front; HXI's dark
    // ascetic-armor textures swallowed everything. RenderLayers
    // pins each light to the preview layer so they don't leak into
    // the backdrop zone's render pass.
    commands.spawn((
        DirectionalLight {
            illuminance: 15_000.0,
            shadows_enabled: false,
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
            shadows_enabled: false,
            ..default()
        },
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + Vec3::new(-2.5, 2.0, 3.0))
            .looking_at(PREVIEW_PARENT_POS + Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        ChildOf(root),
    ));
    // Rim light from behind to separate the model from the backdrop.
    commands.spawn((
        DirectionalLight {
            illuminance: 4_000.0,
            shadows_enabled: false,
            ..default()
        },
        RenderLayers::layer(PREVIEW_RENDER_LAYER),
        Transform::from_translation(PREVIEW_PARENT_POS + Vec3::new(0.0, 2.5, -3.0))
            .looking_at(PREVIEW_PARENT_POS + Vec3::new(0.0, 1.0, 0.0), Vec3::Y),
        ChildOf(root),
    ));

    // The parent entity the equipment meshes will attach under.
    // Survives cursor-change refreshes; only its children get
    // despawned + respawned.
    //
    // Parent rotation: the shared `bind_to_bevy` in `dat_vos2`
    // orients the character to face Bevy -Z (away from camera),
    // which is correct for in-world third-person where the chase
    // camera sits behind the player. In the launcher we want the
    // *face* toward the camera so the user can see what their
    // character looks like — apply an extra Q_y(-π/2) here so
    // composed world rotation flips bake-front from -Z to +Z
    // (toward camera). The -π/2 (not π) accounts for the fact that
    // the shared bind already contains a Q_y(π/2) component.
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
    let _ = parent; // populated asynchronously by `poll_pending_preview`
}

/// Tag every mesh spawned under `CharPreviewParent` (transitively)
/// with the preview render layer. `spawn_equipped` returns only a
/// count, so we can't insert RenderLayers at spawn time — instead
/// observe `OnAdd, Mesh3d` and walk up the `ChildOf` chain to decide
/// whether the new mesh belongs to our preview subtree. Without
/// this, baked PC meshes would render on the default backdrop layer
/// and clip through zone terrain.
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

/// Refresh the preview when the cursor moves to a different
/// character. Comparing the *char_id* (not the cursor index) keeps
/// the refresh quiet when the list is rebuilt without semantic
/// change.
pub(super) fn refresh_preview_on_cursor_change(
    mut commands: Commands,
    chars: Res<CharListData>,
    cursor: Res<CharCursor>,
    mut previewed: ResMut<PreviewedSlot>,
    mut pending: ResMut<PendingPreview>,
    q_parent: Query<(Entity, Option<&Children>), With<CharPreviewParent>>,
) {
    // No `cursor.is_changed()` gate here. Hover-driven cursor updates
    // (from the click/hover handler) should also force a refresh; the
    // `new_id == previewed.char_id` check below is the actual
    // de-duplicator so the work is bounded to one rebuild per
    // distinct selection regardless of how many frames the cursor
    // sits on a given row.
    let active = active_slot(&chars, &cursor);
    let new_id = active.map(|s| s.char_id);
    if new_id == previewed.char_id {
        return;
    }
    let Ok((_parent, kids)) = q_parent.single() else {
        return;
    };
    // Despawn the existing equipment meshes (children of the preview
    // parent) immediately, so the old character clears even while the
    // new bake is still running on the task pool. Leaves camera +
    // lights intact.
    if let Some(kids) = kids {
        for child in kids.iter() {
            commands.entity(child).despawn();
        }
    }
    // Kick the new bake on a background task. The new-char row and
    // empty/dummy slots (`race == 0`, all-zero `TC_OPERATION_MAKE`)
    // render nothing — clear any in-flight task instead of baking.
    match active {
        Some(slot) if slot.race != 0 => {
            let (race, face, head, body, hands, legs, feet, main, sub, ranged) = (
                slot.race, slot.face, slot.head, slot.body, slot.hands, slot.legs, slot.feet,
                slot.main, slot.sub, slot.ranged,
            );
            let char_id = slot.char_id;
            let task = AsyncComputeTaskPool::get().spawn(async move {
                prepare_equipped(race, face, head, body, hands, legs, feet, main, sub, ranged)
            });
            pending.task = Some((char_id, task));
        }
        _ => pending.task = None,
    }
    previewed.char_id = new_id;
}

/// Poll the in-flight preview bake. When the task lands, spawn its
/// meshes under `CharPreviewParent` — but only if the cursor is still
/// on the character the task baked (rapid cursor movement supersedes
/// stale bakes). Non-blocking: `poll_once` returns `None` while the
/// task is still running. The `tag_preview_meshes` observer applies
/// the render layer regardless of which frame the meshes spawn.
pub(super) fn poll_pending_preview(
    mut commands: Commands,
    chars: Res<CharListData>,
    cursor: Res<CharCursor>,
    mut pending: ResMut<PendingPreview>,
    q_parent: Query<Entity, With<CharPreviewParent>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Advance the task one poll without blocking; `ready` is owned
    // (the future's output moves out), so the `as_mut` borrow ends
    // before we `take()` below.
    let ready = match pending.task.as_mut() {
        Some((_, task)) => future::block_on(future::poll_once(task)),
        None => return,
    };
    let Some(prepared) = ready else {
        return; // still running
    };
    let (target_id, _) = pending.task.take().expect("task present when ready");
    // Staleness guard: if the cursor moved to a different character
    // while this bake ran, drop the result. A fresh task for the
    // current slot is already in flight (refresh kicked it), or
    // `previewed.char_id` already matches and none is needed.
    if active_slot(&chars, &cursor).map(|s| s.char_id) != Some(target_id) {
        return;
    }
    if let Ok(parent) = q_parent.single() {
        let spawned = spawn_prepared_equipped(
            &mut commands,
            &mut meshes,
            &mut materials,
            &mut images,
            parent,
            &prepared,
        );
        debug!("char preview: char_id={target_id} equipment_slots_spawned={spawned}");
    }
}

/// Pick the character slot the cursor is currently on, or `None`
/// when the cursor lands on the "+ New character" row (cursor ==
/// chars.len()).
fn active_slot<'a>(chars: &'a CharListData, cursor: &CharCursor) -> Option<&'a CharSlot> {
    chars.0.get(cursor.0)
}

/// Bake a PC preview from raw appearance fields (no equipment).
/// Shared with `char_create_preview` so the create-screen live
/// preview goes through the exact same `spawn_equipped` path as the
/// char-list preview — race/face changes look identical in both.
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
        commands, meshes, materials, images, parent, race, face, 0, 0, 0, 0, 0, 0, 0, 0,
    )
}

