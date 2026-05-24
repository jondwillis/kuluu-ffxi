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

use bevy::prelude::*;
use ffxi_client::lobby_client::CharSlot;
use ffxi_viewer_core::dat_vos2::spawn_equipped;

use super::{char_list::CharCursor, CharListData};

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

pub(super) fn spawn_preview(
    mut commands: Commands,
    chars: Res<CharListData>,
    cursor: Res<CharCursor>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands.insert_resource(PreviewedSlot::default());

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
        Transform::from_translation(PREVIEW_PARENT_POS + PREVIEW_CAMERA_OFFSET)
            .looking_at(PREVIEW_PARENT_POS + PREVIEW_LOOK_AT_OFFSET, Vec3::Y),
        ChildOf(root),
    ));

    // Three-point lighting: key from front-right, fill from back-
    // left, ambient floor. Tuned to keep the dark equipment
    // textures legible without blowing out the highlights.
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

    // Initial spawn for whichever slot the cursor lands on.
    if let Some(slot) = active_slot(&chars, &cursor) {
        spawn_for_slot(
            &mut commands,
            parent,
            slot,
            &mut meshes,
            &mut materials,
            &mut images,
        );
        commands.insert_resource(PreviewedSlot {
            char_id: Some(slot.char_id),
        });
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
    q_parent: Query<(Entity, Option<&Children>), With<CharPreviewParent>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
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
    let Ok((parent, kids)) = q_parent.single() else {
        return;
    };
    // Despawn the existing equipment meshes (children of the
    // preview parent). Leaves camera + lights intact.
    if let Some(kids) = kids {
        for child in kids.iter() {
            commands.entity(child).despawn();
        }
    }
    if let Some(slot) = active {
        spawn_for_slot(
            &mut commands,
            parent,
            slot,
            &mut meshes,
            &mut materials,
            &mut images,
        );
    }
    previewed.char_id = new_id;
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

/// Call `spawn_equipped` for a single slot, gated on race != 0.
/// Empty/dummy chr_info2 slots arrive with all-zero
/// `TC_OPERATION_MAKE`; rendering them would spawn 8 empty meshes
/// (each `resolve_equipment_slot(0, 0)` returns None) — harmless
/// but noisy in logs. The `race == 0` gate avoids that.
fn spawn_for_slot(
    commands: &mut Commands,
    parent: Entity,
    slot: &CharSlot,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
) {
    if slot.race == 0 {
        return;
    }
    let spawned = spawn_equipped(
        commands,
        meshes,
        materials,
        images,
        parent,
        slot.race,
        slot.face,
        slot.head,
        slot.body,
        slot.hands,
        slot.legs,
        slot.feet,
        slot.main,
        slot.sub,
        slot.ranged,
    );
    info!(
        "char preview: char_id={} race={} face={} \
         head=0x{:04X} body=0x{:04X} hands=0x{:04X} legs=0x{:04X} \
         feet=0x{:04X} main=0x{:04X} sub=0x{:04X} ranged=0x{:04X} \
         equipment_slots_spawned={}",
        slot.char_id,
        slot.race,
        slot.face,
        slot.head,
        slot.body,
        slot.hands,
        slot.legs,
        slot.feet,
        slot.main,
        slot.sub,
        slot.ranged,
        spawned,
    );
}
