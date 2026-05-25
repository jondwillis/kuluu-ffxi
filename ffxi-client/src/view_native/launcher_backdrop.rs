//! Launcher 3D backdrop — load a real FFXI zone behind the launcher
//! screens (login form, server-select, char-list, char-create).
//!
//! **Plumbing only.** The existing UI screens (`launcher_ui/*.rs`) still
//! render opaque `BackgroundColor` panels on their full-screen roots,
//! which will cover this 3D view until those panels are migrated to
//! translucent surfaces in Phase 2. The backdrop *is* alive, the camera
//! *is* rendering, and the zone *is* loading — but it won't be visible
//! to the operator until the UI layer cooperates. See the module-level
//! comment in `launcher_ui/login.rs` (and siblings) for the cleanup
//! target. Setting `ClearColor(Color::NONE)` here gives the right
//! baseline so the first non-opaque screen reveals the backdrop
//! immediately, no further plumbing needed.
//!
//! # How it loads zones
//!
//! Re-uses the existing post-auth zone loader: `dat_mzb::
//! auto_load_zone_geometry_system` watches `SceneState.snapshot.zone_id`
//! and fires a `LoadMzbRequest` on every transition. We just write the
//! desired zone id into that field from the Launcher phase — the
//! loader is in an unconditional `Update` chain and doesn't know or
//! care that no network session exists.
//!
//! # Camera
//!
//! Spawns a bare `Camera3d` with `order = -2` (behind char-preview's
//! `-1` and UI's default 0). The viewer-core camera controllers
//! (chase, first-person, collision clamp, polish) all key on the
//! `OperatorCamera` marker component — by deliberately NOT attaching
//! that marker we get a static viewpoint with zero controller
//! interference, no `cfg`/excludes needed.

use bevy::prelude::*;
use bevy::camera::visibility::RenderLayers;
use ffxi_client::lobby_client::CharSlot;
use ffxi_viewer_core::SceneState;

use super::launcher_ui::{char_list::CharCursor, CharListData};
use super::AppPhase;

/// Render layer for the backdrop zone (and its camera). Anything that
/// must NOT mix with the foreground PC-preview pipeline lives here.
/// PC previews (char-list, char-create) live on
/// [`PREVIEW_RENDER_LAYER`]; the two cameras see disjoint layer
/// masks so the preview model can't clip into zone terrain.
pub const BACKDROP_RENDER_LAYER: usize = 0;
/// Render layer for the launcher's foreground PC previews
/// (char-list `char_preview` and char-create `char_create_preview`).
/// They never run simultaneously (different launcher states), so they
/// can share one layer cheaply.
pub const PREVIEW_RENDER_LAYER: usize = 1;

/// West La Theine Plateau (retail zone id 102). See
/// `ffxi-nav/src/zone_names.rs:132` for the canonical id → name
/// mapping; `ffxi-dat/src/zone_dat.rs::zone_id_to_mzb_file_id`
/// resolves it to DAT file_id 202.
pub const DEFAULT_BACKDROP_ZONE: u16 = 102;

/// No time-based debounce on cursor moves: the fade state machine
/// itself throttles. A second target captured mid-fade waits in
/// `PendingBackdropSwap` until the current FadingOut → Holding →
/// FadingIn cycle completes, then a new fade kicks. Net effect:
/// at most one zone load per ~1s fade cycle, but every change is
/// acknowledged immediately.

/// The zone id currently driving the launcher backdrop. Written by
/// the plugin's selection-watcher (and on startup); read by a system
/// that mirrors it into `SceneState.snapshot.zone_id` to trigger the
/// existing zone-load chain.
#[derive(Resource, Debug, Clone, Copy)]
pub struct LauncherBackdropZone(pub u16);

/// Marker for the backdrop's `Camera3d`. Spawned on
/// `OnEnter(AppPhase::Launcher)`, despawned on
/// `OnExit(AppPhase::Launcher)`. Deliberately NOT carrying
/// `OperatorCamera` so the chase/firstperson/collision systems
/// ignore it — see module docs.
#[derive(Component)]
pub struct BackdropCamera;

/// Marker for any backdrop-scoped entity (camera, lights). Used
/// only by the `OnExit(Launcher)` despawn pass so the lights die
/// with the camera and don't leak into the in-game scene.
#[derive(Component)]
struct BackdropScoped;

/// Most-recent cursor-derived zone target. Latched every frame
/// `update_backdrop_from_selection` runs. When a fade completes and
/// this still differs from the current zone, a new fade kicks.
#[derive(Resource, Default)]
struct PendingBackdropSwap {
    target: Option<u16>,
}

/// Crossfade state for zone changes. Hides the visual gap between
/// despawning the old zone geometry and the new one's first lit
/// frame; without it the swap is a hard pop (often through an
/// empty/black frame).
#[derive(Resource, Default, Clone, Copy)]
enum BackdropFade {
    #[default]
    Idle,
    /// Lerping overlay alpha 0 → 1. At end: commit `target` to
    /// [`LauncherBackdropZone`] and transition to `Holding`.
    FadingOut { target: u16, elapsed: f32 },
    /// Overlay fully opaque while the new zone loads. Avoids
    /// fading back in onto empty/half-loaded geometry.
    Holding { elapsed: f32 },
    /// Lerping overlay alpha 1 → 0.
    FadingIn { elapsed: f32 },
}

impl BackdropFade {
    fn is_idle(&self) -> bool {
        matches!(self, BackdropFade::Idle)
    }
    /// Current overlay alpha in 0..=1.
    fn alpha(&self) -> f32 {
        match *self {
            BackdropFade::Idle => 0.0,
            BackdropFade::FadingOut { elapsed, .. } => (elapsed / FADE_OUT_SECS).clamp(0.0, 1.0),
            BackdropFade::Holding { .. } => 1.0,
            BackdropFade::FadingIn { elapsed } => 1.0 - (elapsed / FADE_IN_SECS).clamp(0.0, 1.0),
        }
    }
}

const FADE_OUT_SECS: f32 = 0.25;
/// Time to leave the overlay opaque while the new zone's MZB load
/// (mesh parse + spawn) completes. Tuned conservatively — most zones
/// load in well under 250ms but a cold cache for a large zone (e.g.
/// city) can take longer; better to hold a beat too long than to
/// fade back into a partially-spawned scene.
const FADE_HOLD_SECS: f32 = 0.40;
const FADE_IN_SECS: f32 = 0.35;

/// Color the overlay fades toward. Matches the launcher's panel
/// background so the transition reads as "panel-tinted blackout"
/// rather than a sudden hard cut to pure black.
const FADE_COLOR: Color = Color::srgb(0.04, 0.04, 0.05);

/// Marker for the camera-facing quad that crossfades between zones.
/// Lives on `BACKDROP_RENDER_LAYER` so only the backdrop camera
/// renders it — the PC-preview camera (on `PREVIEW_RENDER_LAYER`)
/// stays visible through the entire transition. The quad is parented
/// to the backdrop camera so it tracks any future viewpoint change
/// for free.
#[derive(Component)]
struct BackdropFadeQuad;

/// Handle to the fade quad's material, kept in a Resource so
/// [`apply_overlay_alpha`] can mutate `base_color.alpha` each frame
/// without a `Query<&MeshMaterial3d<…>>` round-trip.
#[derive(Resource)]
struct BackdropFadeMaterial(Handle<StandardMaterial>);

pub struct LauncherBackdropPlugin;

impl Plugin for LauncherBackdropPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(LauncherBackdropZone(DEFAULT_BACKDROP_ZONE))
            .init_resource::<PendingBackdropSwap>()
            .init_resource::<BackdropFade>()
            // Transparent clear so the 3D backdrop shows through any
            // UI layer that doesn't paint its own opaque background.
            // The launcher's existing roots DO paint opaque bg today
            // (Phase 2 cleanup); once those go translucent the
            // backdrop is visible with no further changes.
            .insert_resource(ClearColor(Color::NONE))
            .add_systems(OnEnter(AppPhase::Launcher), spawn_backdrop_camera)
            .add_systems(OnExit(AppPhase::Launcher), despawn_backdrop_camera)
            .add_systems(
                Update,
                (
                    // Selection watcher runs before the mirror so a
                    // same-frame commit lands on this frame's mirror
                    // pass and the auto-loader sees the change next
                    // frame (one-tick latency, fine for a backdrop).
                    update_backdrop_from_selection,
                    drive_backdrop_fade,
                    apply_overlay_alpha,
                    mirror_backdrop_to_scene_state,
                )
                    .chain()
                    .run_if(in_state(AppPhase::Launcher)),
            );
    }
}

fn spawn_backdrop_camera(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let cam = commands
        .spawn((
            BackdropCamera,
            BackdropScoped,
            Camera3d::default(),
            Camera {
                // Behind char-preview (-1) and UI (0). The launcher's
                // 2D camera defaults to order 0 too — Bevy renders
                // higher-order cameras on top, so -2 keeps us at the
                // bottom of the stack.
                order: -2,
                ..default()
            },
            // Lock backdrop to its own render layer. Without this, the
            // char-list PC preview (which spawns meshes at world origin)
            // gets rendered by this camera *inside* the loaded zone and
            // clips through terrain. PC previews run on
            // PREVIEW_RENDER_LAYER and use their own camera.
            RenderLayers::layer(BACKDROP_RENDER_LAYER),
            // Placeholder viewpoint — eye-height above the FFXI world
            // origin (which is the zone-local origin for the MZB load),
            // looking slightly down toward the horizon. Real per-zone
            // viewpoints can be a later polish pass; this just gives
            // the zone *something* to render against.
            Transform::from_xyz(0.0, 6.0, 12.0).looking_at(Vec3::new(0.0, 2.0, 0.0), Vec3::Y),
        ))
        .id();

    // Camera-facing fade quad parented to the backdrop camera. Lives
    // on the backdrop's render layer only, so the PC-preview camera
    // (layer 1) sees clean through the fade — the previewed model
    // stays visible across zone swaps. Positioned just past the
    // default 0.1 near plane, sized to safely overfill the FOV.
    // alpha starts at 0; `apply_overlay_alpha` writes the active
    // fade alpha into base_color each frame.
    let quad = meshes.add(Rectangle::new(40.0, 40.0));
    let mat = materials.add(StandardMaterial {
        base_color: FADE_COLOR.with_alpha(0.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..default()
    });
    commands.entity(cam).with_children(|c| {
        c.spawn((
            BackdropFadeQuad,
            BackdropScoped,
            Mesh3d(quad),
            MeshMaterial3d(mat.clone()),
            RenderLayers::layer(BACKDROP_RENDER_LAYER),
            // Rectangle's normal is +Z by default — a child of the
            // camera at local (0,0,-0.2) faces back toward camera
            // origin (i.e., the camera sees its front face). The
            // negative z places it past the near plane.
            Transform::from_xyz(0.0, 0.0, -0.2),
        ));
    });
    commands.insert_resource(BackdropFadeMaterial(mat));

    // Lighting so the backdrop isn't pitch-black. The in-game sun/
    // moon system gates on `EntityMesh` (not present in Launcher
    // phase), so without this the zone geometry loads but renders
    // pure black. Cheap directional + ambient — no shadows.
    commands.spawn((
        BackdropScoped,
        DirectionalLight {
            illuminance: 8_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

fn despawn_backdrop_camera(mut commands: Commands, q: Query<Entity, With<BackdropScoped>>) {
    for e in q.iter() {
        commands.entity(e).despawn();
    }
    // The fade material handle outlived its quad — drop it so a
    // re-entry to Launcher rebuilds a fresh one.
    commands.remove_resource::<BackdropFadeMaterial>();
}

/// Mirror [`LauncherBackdropZone`] into `SceneState.snapshot.zone_id`.
/// The viewer-core `auto_load_zone_geometry_system` watches that
/// field and fires the same `LoadMzbRequest` chain the post-auth
/// path uses — by writing here we reuse the entire zone-load
/// pipeline without duplicating any parse / spawn logic.
fn mirror_backdrop_to_scene_state(
    zone: Res<LauncherBackdropZone>,
    mut scene: ResMut<SceneState>,
) {
    // No `is_changed()` gate: a return-to-Launcher path resets
    // `SceneState` via `despawn_ingame_entities` without mutating
    // our `LauncherBackdropZone`, so we need to detect the
    // divergence by comparing values directly. Cheap read every
    // frame; only writes when out of sync.
    let desired = Some(zone.0);
    if scene.snapshot.zone_id == desired {
        return;
    }
    scene.snapshot.zone_id = desired;
}

/// Watch the char-list cursor's currently-hovered character and
/// debounce-update [`LauncherBackdropZone`] to its saved zone. Runs
/// every frame; cheap reads only when nothing is hovered or the
/// hover hasn't changed.
///
/// `CharCursor` is the per-row highlight (moves on arrow keys), not
/// `SelectedChar` (which only fires on Enter/click commit). Reading
/// `SelectedChar` here meant the backdrop only swapped at login-
/// commit time, never on cursor navigation. `CharCursor` only exists
/// while `LauncherState::CharList` is active — Option<Res<_>>
/// pattern.
fn update_backdrop_from_selection(
    cursor: Option<Res<CharCursor>>,
    chars: Res<CharListData>,
    mut pending: ResMut<PendingBackdropSwap>,
    zone: Res<LauncherBackdropZone>,
    mut fade: ResMut<BackdropFade>,
) {
    // CharCursor.0 indexes into chars.0; out-of-bounds (e.g. the
    // synthetic "+ New character" row at chars.len()) → None, which
    // leaves the backdrop on whatever zone was last loaded.
    let hovered: Option<u16> = cursor
        .as_deref()
        .and_then(|c| chars.0.get(c.0))
        .and_then(slot_zone_for_backdrop);
    pending.target = hovered;

    // Kick a new fade only when idle. The fade itself is the rate
    // limit — arrow-keying through 8 chars updates `pending.target`
    // 8 times but only the latest one ends up driving the next
    // fade cycle. `drive_backdrop_fade` re-checks pending.target
    // when it returns to Idle.
    if !fade.is_idle() {
        return;
    }
    let Some(target) = hovered else {
        return;
    };
    if target == zone.0 {
        return;
    }
    *fade = BackdropFade::FadingOut { target, elapsed: 0.0 };
}

/// Tick the fade state machine. Driven by `Time` so it's frame-rate
/// independent. Transitions: FadingOut → (commit target) → Holding →
/// FadingIn → Idle. On returning to Idle, immediately re-checks
/// `PendingBackdropSwap.target` and kicks another fade if the cursor
/// moved to a different zone during the previous cycle.
fn drive_backdrop_fade(
    time: Res<Time>,
    mut fade: ResMut<BackdropFade>,
    mut zone: ResMut<LauncherBackdropZone>,
    pending: Res<PendingBackdropSwap>,
) {
    let dt = time.delta_secs();
    *fade = match *fade {
        BackdropFade::Idle => return,
        BackdropFade::FadingOut { target, elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_OUT_SECS {
                zone.0 = target;
                BackdropFade::Holding { elapsed: 0.0 }
            } else {
                BackdropFade::FadingOut { target, elapsed: next }
            }
        }
        BackdropFade::Holding { elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_HOLD_SECS {
                BackdropFade::FadingIn { elapsed: 0.0 }
            } else {
                BackdropFade::Holding { elapsed: next }
            }
        }
        BackdropFade::FadingIn { elapsed } => {
            let next = elapsed + dt;
            if next >= FADE_IN_SECS {
                // Cycle done — but if the cursor moved during the
                // fade and points at a different zone, kick the
                // next cycle without a frame of Idle in between.
                match pending.target {
                    Some(target) if target != zone.0 => {
                        BackdropFade::FadingOut { target, elapsed: 0.0 }
                    }
                    _ => BackdropFade::Idle,
                }
            } else {
                BackdropFade::FadingIn { elapsed: next }
            }
        }
    };
}

/// Apply the current fade alpha to the camera-facing quad's material.
/// Cheap; only writes through `Assets<StandardMaterial>` when the
/// alpha actually changes (avoids spurious change-detection for the
/// renderer's material upload).
fn apply_overlay_alpha(
    fade: Res<BackdropFade>,
    mat_handle: Option<Res<BackdropFadeMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(mat_handle) = mat_handle else {
        return;
    };
    let Some(mat) = materials.get_mut(&mat_handle.0) else {
        return;
    };
    let want = fade.alpha();
    let current = mat.base_color.alpha();
    if (current - want).abs() < 0.001 {
        return;
    }
    mat.base_color = FADE_COLOR.with_alpha(want);
}

/// Filter: only return a zone id we can actually load. Empty char
/// slots (race=0) and the synthetic "new character" row report
/// `zone_id == 0`, which would force the backdrop to a zone that
/// doesn't exist; skip those and let the previous backdrop stay.
fn slot_zone_for_backdrop(slot: &CharSlot) -> Option<u16> {
    if slot.race == 0 || slot.zone_id == 0 {
        return None;
    }
    Some(slot.zone_id)
}
