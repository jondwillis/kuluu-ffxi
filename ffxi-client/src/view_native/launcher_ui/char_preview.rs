//! 3D character preview for the char-list screen.
//!
//! Renders the currently-cursored character as an **animated** 3D
//! model behind the launcher UI via the XIM-faithful render path
//! ([`ffxi_viewer_core::ffxi_actor_render`]). Drives off the
//! `CharCursor` resource maintained by [`super::char_list`] — when the
//! user moves the cursor, the previewed actor is despawned and a fresh
//! one is loaded (`load_pc`) + spawned (`spawn_loaded_actor`), then
//! driven through an idle pose by [`tick_ffxi_render_actors`] every
//! frame the char list is up.
//!
//! Doubles as a debugging surface for the faithful animation pipeline:
//! the launcher loads in seconds (no auth, no zoning), so iterating on
//! the pose/skinning math here is much faster than logging into the
//! live server. Cycle race 1..8 with the arrow keys and watch the
//! preview update.
//!
//! # Why the faithful path here (not the legacy bake)
//!
//! This used to render statically via `dat_vos2::prepare_equipped` +
//! `spawn_prepared_equipped` (a one-shot CPU bind-pose bake with no
//! animation). It now goes through the exact same loader/skinner the
//! in-game characters use, so the char-select model animates and
//! matches what the player will see once connected.
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
use ffxi_viewer_core::dat_vos2::spawn_equipped;
use ffxi_viewer_core::ffxi_actor_render::{
    inputs_for_pose, load_pc, spawn_loaded_actor, FfxiRenderActor, LoadedActor, PoseState,
};
use ffxi_viewer_core::look_resolver::{resolve_equipment_slot, resolve_face};
use ffxi_viewer_core::skinned_ffxi_material::{FfxiLightingUniform, FfxiSkinnedMaterial};

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
/// (the faithful actor-root + its mesh groups) while leaving the parent
/// in place.
#[derive(Component)]
pub(super) struct CharPreviewParent;

/// Marker on the faithful actor-root spawned by `spawn_loaded_actor`
/// under [`CharPreviewParent`]. Lets [`relight_preview_actor`] find the
/// preview's `FfxiSkinnedMaterial` mesh groups (via the root's
/// descendants) to re-stamp a legible launcher light uniform every
/// frame — the app-wide `update_ffxi_render_actor_lighting` otherwise
/// clobbers them with the (absent) in-game sun. The root also carries
/// the [`FfxiRenderActor`] component the shared tick advances.
#[derive(Component)]
pub(super) struct CharPreviewActorRoot;

/// What character is currently rendered. `None` when the cursor is
/// on the "+ New character" row or when the slot has no usable
/// appearance data (race == 0). We compare against the next
/// cursor tick to decide whether to refresh — avoids despawning
/// and respawning the same character every frame.
#[derive(Resource, Default)]
pub(super) struct PreviewedSlot {
    pub char_id: Option<u32>,
}

/// In-flight background load of the cursored character. The `u32` is
/// the target `char_id`; [`poll_pending_preview`] compares it against
/// the current cursor when the task lands and drops stale results from
/// rapid cursor movement. Loading the race skeleton + face + up to 8
/// equipment DATs is 100–500 ms of synchronous IO/parse, so it runs on
/// an `AsyncComputeTaskPool` task instead of blocking the main thread.
///
/// The task produces a fully-parsed [`LoadedActor`] (skeleton + meshes +
/// textures + animations) — all owned, `Send` data with no Bevy `Assets`
/// dependency — so the heavy parse happens off-thread and only the cheap
/// GPU-asset upload (`spawn_loaded_actor`) runs on the main thread.
#[derive(Resource, Default)]
pub(super) struct PendingPreview {
    pub task: Option<(u32, Task<Result<LoadedActor, String>>)>,
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

/// Root yaw (radians) handed to [`spawn_loaded_actor`] as `facing_dir`.
///
/// TUNABLE. `spawn_loaded_actor` bakes its OWN FFXI→Bevy basis (`Q_x(π)`)
/// onto the actor-root, which already stands the rig upright and facing
/// the camera at `facing_dir = 0` (see `ffxi_actor_render::
/// ffxi_to_bevy_basis`). That actor-root is parented under
/// `CharPreviewParent`, whose `Q_y(-π/2)` rotation then composes on top.
/// Net heading the operator sees = parent `Q_y(-π/2)` ∘ basis `Q_x(π)` ∘
/// root `Q_y(facing_dir)`. With `facing_dir = 0` and the parent's existing
/// `-π/2` the model should present its front to the camera; if it ends up
/// turned (side/back), nudge this value (or zero the parent rotation in
/// [`spawn_preview`]) until the face is toward the camera. Confirm
/// visually — the headless harness defaults `facing_dir = 0` with NO
/// parent wrapper, so it can't validate this composition.
const PREVIEW_FACING_DIR: f32 = 0.0;

/// Uniform scale handed to [`spawn_loaded_actor`]. The faithful path
/// applies no `min_y` foot pivot, so the rig sits with its skeleton root
/// at the parent origin; `1.0` keeps the rig at authoring scale (the
/// camera framing constants above assume a ~1.7-yalm humanoid).
const PREVIEW_SCALE: f32 = 1.0;

/// Pose the char-select portrait plays. Idle is the natural choice for a
/// standing portrait; flip to `Run`/`Walk`/etc. here for a quick visual
/// check of the locomotion clips against a known character.
const PREVIEW_POSE: PoseState = PoseState::Idle;

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

    // The parent entity the faithful actor-root attaches under.
    // Survives cursor-change refreshes; only its children (the actor-
    // root + its mesh groups) get despawned + respawned.
    //
    // Parent rotation (TUNABLE — see `PREVIEW_FACING_DIR`): the faithful
    // `spawn_loaded_actor` bakes its own FFXI→Bevy basis (`Q_x(π)`) onto
    // the actor-root, which stands the rig upright and facing the camera
    // at `facing_dir = 0` — it does NOT use the legacy `dat_vos2`
    // `bind_to_bevy` orientation. We keep the historical `Q_y(-π/2)`
    // here as a starting point; it composes on top of the baked basis,
    // so the operator-visible heading is parent ∘ basis ∘ root-yaw. If
    // the model presents its side/back to the camera, retune this
    // rotation (or `PREVIEW_FACING_DIR`) until the face is toward the
    // camera. The headless harness can't validate this composition (it
    // spawns the actor-root with no parent wrapper).
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

/// Deterministically keep every mesh of the faithful preview actor on the
/// preview render layer.
///
/// The [`tag_preview_meshes`] `On<Add, Mesh3d>` observer is a fast path, but it
/// races the spawn order: `spawn_loaded_actor` spawns the mesh groups as
/// children of the actor-root and only AFTER it returns does
/// [`poll_pending_preview`] attach the root under [`CharPreviewParent`]. So when
/// the observer fires for each mesh, the actor-root has no `CharPreviewParent`
/// ancestor yet — the `ChildOf`-walk fails and the mesh is left on the default
/// (backdrop, layer 0) render layer. It then renders in the *backdrop camera's*
/// pass, buried in the loaded zone near world origin (visible against a void,
/// occluded by terrain — never in the intended foreground portrait).
///
/// This per-frame net walks the actor-root's descendants and tags any still-
/// untagged `Mesh3d` with [`PREVIEW_RENDER_LAYER`]. It is idempotent
/// (`Without<RenderLayers>` skips already-tagged meshes), so it self-heals one
/// frame after spawn regardless of command-flush ordering.
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
    // Kick the new load on a background task. The new-char row and
    // empty/dummy slots (`race == 0`, all-zero `TC_OPERATION_MAKE`)
    // render nothing — clear any in-flight task instead of loading.
    match active {
        Some(slot) if slot.race != 0 => {
            let race = slot.race;
            // Resolve face + equipment-slot file_ids in the SAME
            // order/logic the in-game look dispatcher uses for a PC
            // (`look_resolver::dispatch_look_driven_models`): face first
            // (raw face index, not slot-encoded), then the 8 equipment
            // slots in canonical order, skipping any that sentinel to
            // `None`. `load_pc` itself loads the race skeleton DAT and
            // skins each of these onto it.
            let equipment = pc_equipment_file_ids(slot);
            let char_id = slot.char_id;
            let task = AsyncComputeTaskPool::get().spawn(async move { load_pc(race, &equipment) });
            pending.task = Some((char_id, task));
        }
        _ => pending.task = None,
    }
    previewed.char_id = new_id;
}

/// Poll the in-flight preview load. When the task lands, spawn the
/// faithful actor under `CharPreviewParent` — but only if the cursor is
/// still on the character the task loaded (rapid cursor movement
/// supersedes stale loads). Non-blocking: `poll_once` returns `None`
/// while the task is still running. The `tag_preview_meshes` observer
/// applies the render layer to each mesh group regardless of which frame
/// it spawns.
pub(super) fn poll_pending_preview(
    mut commands: Commands,
    chars: Res<CharListData>,
    cursor: Res<CharCursor>,
    mut pending: ResMut<PendingPreview>,
    q_parent: Query<Entity, With<CharPreviewParent>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Advance the task one poll without blocking; `ready` is owned
    // (the future's output moves out), so the `as_mut` borrow ends
    // before we `take()` below.
    let ready = match pending.task.as_mut() {
        Some((_, task)) => future::block_on(future::poll_once(task)),
        None => return,
    };
    let Some(loaded) = ready else {
        return; // still running
    };
    let (target_id, _) = pending.task.take().expect("task present when ready");
    // Staleness guard: if the cursor moved to a different character
    // while this load ran, drop the result. A fresh task for the
    // current slot is already in flight (refresh kicked it), or
    // `previewed.char_id` already matches and none is needed.
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

    // Spawn the faithful actor. `spawn_loaded_actor` creates its OWN
    // actor-root (carrying the FFXI→Bevy basis + the world position we
    // pass) and attaches every non-occluded mesh group as a child; it
    // returns that root. We pass `world_pos = ZERO` because the parent
    // (`CharPreviewParent`) already positions the rig at
    // `PREVIEW_PARENT_POS`, then re-parent the root under it so the
    // `tag_preview_meshes` observer puts each group on the preview
    // render layer (the observer walks up `ChildOf` to a
    // `CharPreviewParent`).
    let actor_root = spawn_loaded_actor(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut images,
        &loaded,
        Vec3::ZERO,
        PREVIEW_FACING_DIR,
        PREVIEW_SCALE,
    );
    commands
        .entity(actor_root)
        .insert((CharPreviewActorRoot, ChildOf(parent)));

    // The actor's pose inputs default to idle (`make_render_actor` seeds
    // `ActorAnimInputs::default()`, which is exactly
    // `inputs_for_pose(PoseState::Idle, false)`). `drive_preview_pose`
    // re-affirms that selection each frame, and `tick_ffxi_render_actors`
    // (both registered in `launcher_ui::register`, gated on `CharList`)
    // reads it, selects the matching `idl?` clips, advances the
    // coordinator, and stamps the animated bone matrices into every group
    // material — so the preview ANIMATES rather than freezing in bind
    // pose.

    debug!("char preview: char_id={target_id} faithful actor spawned");
}

/// Re-affirm the previewed actor's idle pose each frame. `world_id == 0`
/// keeps the live tick (`tick_live_ffxi_actors`) from touching this
/// actor, so the harness tick (`tick_ffxi_render_actors`, registered for
/// `CharList`) is the only driver — and it reads `inputs` verbatim. We
/// set it here (rather than once at spawn) because the component insert
/// is deferred past `poll_pending_preview`'s command flush, so the
/// entity isn't queryable in the same system; a per-frame setter is the
/// simplest deferred-safe place to drive the pose (and the single knob
/// to flip to run/walk/etc. for debugging).
pub(super) fn drive_preview_pose(mut q: Query<&mut FfxiRenderActor, With<CharPreviewActorRoot>>) {
    for mut actor in &mut q {
        let want = inputs_for_pose(PREVIEW_POSE, false);
        // Avoid spurious change-detection churn on the component.
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

/// Re-stamp a legible launcher light uniform onto the previewed actor's
/// `FfxiSkinnedMaterial` groups every frame.
///
/// The app-wide `ffxi_actor_render::update_ffxi_render_actor_lighting`
/// runs unconditionally and overwrites EVERY `FfxiRenderActor` material's
/// light uniform from the in-game zone sun/moon + `GlobalAmbientLight`.
/// In the launcher there is no sun/moon entity (both resolve to zero) and
/// `GlobalAmbientLight` sits at its dim default (the zone-atmosphere
/// applier is gated on the in-game `EntityMesh` canary, absent here), so
/// that system would leave the preview nearly black. We can't gate that
/// system (it lives in viewer-core, out of scope) and `FfxiRenderActor`'s
/// material-handle list is private — so instead we walk the previewed
/// actor-root's mesh-group descendants (each carries a
/// `MeshMaterial3d<FfxiSkinnedMaterial>`) and write a fixed neutral
/// uniform. Ordered `.after(update_ffxi_render_actor_lighting)` in
/// `register` so our write wins each frame.
pub(super) fn relight_preview_actor(
    q_root: Query<&Children, With<CharPreviewActorRoot>>,
    q_mat: Query<&MeshMaterial3d<FfxiSkinnedMaterial>>,
    mut materials: ResMut<Assets<FfxiSkinnedMaterial>>,
) {
    // `FfxiLightingUniform::default()` is the "legible from frame 0"
    // neutral fill (soft 0.5 ambient + one overhead key) the faithful
    // material ships — exactly what we want for a lit-from-everywhere
    // character portrait. Reuse it rather than hand-rolling values.
    let lighting = FfxiLightingUniform::default();
    for children in &q_root {
        for child in children.iter() {
            if let Ok(mat_handle) = q_mat.get(child) {
                if let Some(mat) = materials.get_mut(&mat_handle.0) {
                    mat.lighting = lighting.clone();
                    // Keep the faithful (non-realistic) shading branch so
                    // the portrait reads bright; leave `flags.x`
                    // (has_texture) untouched.
                    mat.material_flags.flags.y = 0.0;
                }
            }
        }
    }
}

/// Pick the character slot the cursor is currently on, or `None`
/// when the cursor lands on the "+ New character" row (cursor ==
/// chars.len()).
fn active_slot<'a>(chars: &'a CharListData, cursor: &CharCursor) -> Option<&'a CharSlot> {
    chars.0.get(cursor.0)
}

/// Resolve a [`CharSlot`]'s appearance to the ordered `Vec<u32>` of DAT
/// file_ids `load_pc` expects: face first (a raw face index, resolved by
/// [`resolve_face`]), then the 8 equipment slots
/// `[head, body, hands, legs, feet, main, sub, ranged]` (already
/// slot-encoded u16s, resolved by [`resolve_equipment_slot`]), skipping
/// any that sentinel to `None`. This MIRRORS the in-game look dispatcher
/// (`look_resolver::dispatch_look_driven_models`) so the char-select
/// portrait loads the exact same geometry the player sees in-world.
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
