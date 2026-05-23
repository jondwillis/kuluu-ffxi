//! World-space nameplate **3D billboards** — rasterize the entity name
//! into a small RGBA image, stamp it onto a `Mesh3d` quad placed at the
//! entity's head, and let the regular 3D depth buffer decide whether
//! the label is visible. Walls, ceilings, other characters, MMB meshes
//! — anything that writes to the depth buffer this frame occludes the
//! label *for free*, no per-entity raycast needed.
//!
//! # Why we abandoned UI nameplates
//!
//! Bevy UI nodes render in a post-3D 2D pass with no depth interaction;
//! the previous `nameplate.rs` module + `view_native/nameplate_occlude.rs`
//! tried to fake occlusion via a navmesh-slide approximation, but that
//! only caught walkable-surface walls and missed every other category
//! the renderer would have hidden naturally (other characters, ceilings,
//! non-navmesh detail meshes, foliage). Switching to a 3D-quad billboard
//! moves occlusion from "heuristic" to "whatever the depth buffer says,"
//! which is exactly what the operator sees.
//!
//! # Why not `Text2d` in world space
//!
//! `Text2d` is rendered by Bevy's 2D pipeline. Even when given a 3D
//! `Transform`, its render pass does not sample the 3D depth buffer and
//! it draws on top of meshes. The only way to get a label that *truly*
//! z-tests against the scene is to route it through the 3D pipeline —
//! hence the Image-onto-Mesh3d approach.
//!
//! # Lifecycle
//!
//! - Spawn: `scene::sync_entities_system` calls [`spawn_nameplate_billboard`]
//!   when a wire entity first surfaces with a non-empty name.
//! - Update: [`update_nameplate_billboards_system`] runs each frame.
//!   It reads the owning `WorldEntity`'s world position, points the quad
//!   at the camera (Y-locked so text stays upright), rescales by camera
//!   distance, and re-rasterizes the image only when the displayed
//!   string actually changed.
//! - Despawn: the same system despawns billboards whose owner is gone,
//!   matching the old UI lifecycle exactly.

use std::sync::Arc;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_viewer_wire::EntityKind;

use crate::camera::{nameplate_anchor_y, OperatorCamera};
use crate::components::{InGameEntity, Nameplate, WorldEntity};
use crate::scene::BakedActor;
use crate::snapshot::SceneState;

/// Pixel-height of the rasterized name glyph. Bigger = sharper at the
/// close-up end of the distance range (the quad gets bilinearly
/// upsampled to whatever screen pixels it covers, so a low base px
/// looks blurry within melee range). 64px gives ~256 pixels of glyph
/// width for a typical 8-char name — enough headroom that even a 1.4
/// yalm quad 2 yalms from the camera samples close to 1:1 instead of
/// 8:1. Re-rasterization is still <0.3 ms per name on a 2024 laptop;
/// the path only fires on name/state change, not per-frame.
const NAME_PX: f32 = 64.0;

/// Base world-space width of the quad in yalms before the
/// distance-curve scaling kicks in. Tuned down from the original 1.6
/// because the previous value visibly overshot the model silhouette at
/// melee range — the label dominated the screen and clipped through
/// hats/heads on close orbits.
const QUAD_BASE_WIDTH_YALMS: f32 = 1.1;

/// Hard caps on per-frame world-space width of a billboard quad. The
/// distance curve runs from `~1.0` at d=0 to `~0` as d→∞, multiplied by
/// `QUAD_BASE_WIDTH_YALMS`. Without these clamps, close-up labels
/// would clip through hats and far-distance labels would shrink to
/// unreadable specks — the natural 1/d shrink overshoots the human
/// "I want to read names across a Jeuno plaza" range pretty fast.
///
/// `MAX` fires at near distances. `MIN` fires past ~mid-range (~9
/// yalms with the current curve) and pins the quad at this width for
/// the rest of the distance range. That's deliberate: past mid-range
/// we trade "perceptually 3D-anchored shrink" for "still readable"
/// because the alternative is unreadable text.
const MAX_QUAD_WIDTH_YALMS: f32 = 1.4;
const MIN_QUAD_WIDTH_YALMS: f32 = 0.8;

// Per-entity head anchor (= `actor_height + NAMEPLATE_OFFSET_ABOVE_CROWN`)
// is computed via `camera::nameplate_anchor_y(baked)` so the label tracks
// each actor's real crown — Galka tall, Taru short, mob whatever the bake
// measured. Capsule-only / pre-skin-load entities fall back to a PC-sized
// default inside that helper.

/// Pixel radius of the dark halo drawn behind the glyphs. The halo is
/// what makes a yellow name readable against a sand-colored Western
/// Sarutabaruta floor and a green name readable against tree canopy —
/// without it the label dissolves into matching backgrounds. At
/// `NAME_PX = 64` a 3-pixel radius is roughly 5% of glyph height,
/// which matches the contrast halo conventional MMO nameplates use.
const OUTLINE_RADIUS_PX: i32 = 3;

/// Color of the outline halo. Slightly transparent black so the halo
/// reads as a soft shadow rather than a hard inked stroke.
const OUTLINE_COLOR: [u8; 4] = [0, 0, 0, 220];

/// Pixel-height of the HP bar rendered below the name on Mob/Pet
/// labels. Relative to `NAME_PX = 64`, this is ~16% of glyph height —
/// large enough to read at distance, small enough not to dominate the
/// label silhouette.
const HP_BAR_HEIGHT_PX: u32 = 16;

/// Vertical gap between the name baseline padding and the top of the
/// HP bar. A few pixels of breathing room so the bar doesn't look
/// glued to the descenders of `g`/`p`/`y`.
const HP_BAR_TOP_GAP_PX: u32 = 8;

/// Horizontal fraction of the texture width the HP bar occupies. The
/// bar is centered, so the remaining `1.0 - HP_BAR_WIDTH_FRACTION` is
/// split equally as left/right margin. 0.85 gives a noticeable margin
/// without making the bar look truncated next to a long name.
const HP_BAR_WIDTH_FRACTION: f32 = 1.0;

/// Shared `ab_glyph` font loaded from Bevy's embedded default
/// (`bevy_text::DEFAULT_FONT_DATA` — same FiraMono-subset every Bevy
/// app ships with). Lazy-init via `FromWorld`; one allocation for the
/// whole process.
#[derive(Resource)]
pub struct BillboardFont(pub Arc<FontArc>);

impl FromWorld for BillboardFont {
    fn from_world(_: &mut World) -> Self {
        let font = FontArc::try_from_slice(bevy::text::DEFAULT_FONT_DATA)
            .expect("bevy default font must parse as a valid TTF for ab_glyph");
        Self(Arc::new(font))
    }
}

/// Marker on the 3D-quad billboard parent. Carries the same
/// `entity_id` + `kind` payload as the deprecated UI `Nameplate`
/// component so the per-frame system can re-derive the label text
/// (mob-HP suffix etc.) the same way the old code did.
#[derive(Component)]
pub struct NameplateBillboard {
    pub entity_id: u32,
    pub kind: EntityKind,
    /// Original `name` from the wire — the bare name with no HP
    /// suffix. The texture is rebuilt from this whenever the rendered
    /// string OR the cached HP percentage OR the color changes.
    pub base_name: String,
    /// Last text we actually rasterized into the material's texture.
    /// Drives a cheap string-equality check before we burn another
    /// allocation on a glyph atlas.
    pub last_rendered: String,
    /// RGBA color used when we rasterized `last_rendered`. Re-rasterize
    /// if it ever changes.
    pub last_color: [u8; 4],
    /// HP percentage embedded in the most recent rasterization
    /// (`None` when the kind doesn't get an HP bar). Comparing this
    /// against the current snapshot HP gates the re-rasterization so
    /// a stable mob doesn't burn allocations per frame.
    pub last_hp: Option<u8>,
}

/// `(width_px, height_px)` of the most recently rasterized image, so
/// we can keep the quad's aspect ratio in sync with the texture
/// without poking the `Image` asset on every frame.
#[derive(Component)]
pub struct BillboardAspect {
    pub width: u32,
    pub height: u32,
}

/// Spawn a 3D billboard for a wire entity. Returns the spawned root
/// entity so callers can keep a handle if they want; ignoring the
/// return is fine — [`update_nameplate_billboards_system`] reconciles
/// via `entity_id`.
///
/// The quad starts hidden far below the world so it doesn't flash a
/// stray triangle at the origin while we wait for the first update tick
/// to position it.
pub fn spawn_nameplate_billboard(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    images: &mut Assets<Image>,
    font: &FontArc,
    entity_id: u32,
    kind: EntityKind,
    name: &str,
    color: Color,
) -> Entity {
    let rgba = color_to_rgba8(color);
    // Spawn-time rasterization has no HP knowledge yet (combat state
    // is reconciled by the update system on the next tick), so the
    // initial texture is text-only. The first HP-bearing snapshot
    // triggers a re-rasterization that adds the bar.
    let raster = rasterize_text_to_image(font, name, NAME_PX, rgba, None).clone();
    let aspect = (raster.width(), raster.height());
    let image_handle = images.add(raster);

    // 1x1 quad we'll scale per-frame. Default `Rectangle` mesh sits in
    // the XY plane with its normal along +Z, which is exactly what the
    // billboard rotation math below assumes.
    let mesh_handle = meshes.add(Rectangle::new(1.0, 1.0));

    let material_handle = materials.add(StandardMaterial {
        base_color_texture: Some(image_handle),
        base_color: Color::WHITE,
        // Unlit so the label brightness doesn't blink with the sun/moon
        // cycle, and so it stays readable inside Dynamis or any dark
        // interior atmosphere preset.
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        // Show both faces of the quad — if the billboard math ever
        // glitches we get a flipped label rather than nothing at all.
        // Single-sided would also be fine since we orient it each frame.
        cull_mode: None,
        ..default()
    });

    commands
        .spawn((
            InGameEntity,
            // Keep the legacy marker so any pre-existing query (e.g.
            // `q_nameplates` in scene.rs) still finds the right
            // entity-id bookkeeping. The marker is purely for ECS
            // queries — no UI behavior is attached anymore.
            Nameplate { entity_id, kind },
            NameplateBillboard {
                entity_id,
                kind,
                base_name: name.to_string(),
                last_rendered: name.to_string(),
                last_color: rgba,
                last_hp: None,
            },
            BillboardAspect {
                width: aspect.0,
                height: aspect.1,
            },
            Mesh3d(mesh_handle),
            MeshMaterial3d(material_handle),
            // Hide off-world until the first update; avoids a one-frame
            // flash at (0,0,0).
            Transform::from_translation(Vec3::new(0.0, -1_000_000.0, 0.0)),
            Visibility::Hidden,
        ))
        .id()
}

/// Resolve the rendered text color for a billboard from entity kind
/// and live combat state.
///
/// - **PC**: warm cyan.
/// - **NPC**: green. Vanilla FFXI uses yellow for friendly NPCs, but
///   yellow collides too easily with the unengaged-mob cue below;
///   green reads unambiguously as "talk to me, not fight me."
/// - **Mob**: depends on combat state, evaluated per-frame:
///     * **reddish-orange** when *either* the player is engaged with
///       this mob (`self.bt_target_id == mob.id`) or the mob is
///       aggroing the player (`mob.bt_target_id == self.uniqueNo`).
///       Mirrors the same two signals the chase ring and the body
///       material use, so the cues stay in lockstep across the HUD.
///     * **whitish-yellow** otherwise — the "wandering, not yet a
///       threat" baseline.
/// - **Pet**: pale green.
/// - **Other**: neutral gray.
pub fn nameplate_color(kind: EntityKind, is_engaged_with: bool, is_aggroing: bool) -> Color {
    match kind {
        EntityKind::Pc => Color::srgb(0.55, 0.95, 1.0),
        EntityKind::Npc => Color::srgb(0.55, 1.0, 0.55),
        EntityKind::Mob => {
            if is_engaged_with || is_aggroing {
                Color::srgb(1.0, 0.55, 0.25)
            } else {
                Color::srgb(1.0, 0.95, 0.7)
            }
        }
        EntityKind::Pet => Color::srgb(0.55, 0.95, 0.65),
        EntityKind::Other => Color::srgb(0.85, 0.85, 0.85),
    }
}

/// Build the rendered label string for a billboard. Always the bare
/// name now: the HP percentage is drawn as a filled-rectangle bar
/// below the text inside the same rasterized texture (see the
/// `hp_pct` parameter on [`rasterize_text_to_image`]), so duplicating
/// it as a "73%" string suffix would be visual noise.
///
/// Kept as a function (rather than inlining `base_name.to_string()`)
/// so a future label tweak — pet-owner suffix, level prefix, claim
/// indicator — has one obvious place to land.
pub fn format_billboard_label(base_name: &str, _hp_pct: Option<u8>, _kind: EntityKind) -> String {
    base_name.to_string()
}

/// Per-frame update: position, orient, scale, and refresh the texture
/// of every nameplate billboard.
///
/// Schedule expectation: runs in `Update`, after the systems that move
/// `WorldEntity` transforms (so the head position is current this
/// frame) and after `OperatorCamera`'s transform is settled.
///
/// We deliberately do NOT do any geometry occlusion test here — the
/// whole point of the migration is that the 3D depth buffer handles
/// occlusion via the normal mesh render pass.
pub fn update_nameplate_billboards_system(
    state: Res<SceneState>,
    cam_q: Query<&Transform, (With<OperatorCamera>, Without<NameplateBillboard>)>,
    world_q: Query<(&Transform, &WorldEntity, Option<&BakedActor>), Without<NameplateBillboard>>,
    mut billboards: Query<(
        Entity,
        &mut NameplateBillboard,
        &mut BillboardAspect,
        &mut Transform,
        &mut Visibility,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    font: Res<BillboardFont>,
    mut commands: Commands,
) {
    let Ok(cam_t) = cam_q.single() else { return };
    let cam_pos = cam_t.translation;

    // World-position + per-entity nameplate-Y lookup keyed by wire id.
    // The Y offset comes from each entity's `BakedActor.actor_height`
    // when present, so race-tall PCs (Galka) get a higher label and
    // race-short ones (Taru) a lower label.
    let mut pos_by_id: std::collections::HashMap<u32, (Vec3, f32)> =
        std::collections::HashMap::with_capacity(world_q.iter().len());
    for (t, w, baked) in &world_q {
        pos_by_id.insert(w.id, (t.translation, nameplate_anchor_y(baked)));
    }

    // HP lookup keyed by wire id — only the entities the snapshot says
    // have a known HP appear here; missing means "no suffix this frame."
    //
    // While we're walking `snapshot.entities`, also pull the player's
    // `uniqueNo` and current `bt_target_id` so we can decide aggro vs.
    // engaged state per billboard below. `sync_in` is the same low-16
    // unique-id the server uses in `bt_target_id`, mirroring the exact
    // comparison `sync_aggro_system` does (scene.rs).
    let self_uid: Option<u16> = state.snapshot.diagnostics.sync_in;
    let self_char_id: Option<u32> = state.snapshot.self_char_id;
    let mut hp_by_id: std::collections::HashMap<u32, Option<u8>> = std::collections::HashMap::new();
    let mut bt_target_by_id: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    let mut self_bt_target: u32 = 0;
    for ent in &state.snapshot.entities {
        hp_by_id.insert(ent.id, ent.hp_pct);
        bt_target_by_id.insert(ent.id, ent.bt_target_id);
        if Some(ent.id) == self_char_id {
            self_bt_target = ent.bt_target_id;
        }
    }

    for (ui_entity, mut np, mut aspect, mut transform, mut vis, mat) in &mut billboards {
        let Some(&(entity_pos, head_y_offset)) = pos_by_id.get(&np.entity_id) else {
            // Owner gone — same lifecycle the UI nameplates had.
            commands.entity(ui_entity).despawn();
            continue;
        };

        let head_pos = entity_pos + Vec3::Y * head_y_offset;
        let to_cam = cam_pos - head_pos;
        let distance = to_cam.length();
        if distance < 0.001 {
            // Camera coincident with the label position — common during
            // first-person zero-distance frames. Hide rather than emit
            // a degenerate `looking_at`.
            *vis = Visibility::Hidden;
            continue;
        }

        // Y-locked billboard: project the camera direction into the XZ
        // plane and yaw the quad to face along that vector. The label
        // never tilts when the camera pitches down, which keeps text
        // upright and legible from any angle. Trade-off: looking
        // straight down on a label produces a thin edge — acceptable
        // because the operator camera's pitch is clamped well short of
        // top-down by the chase-camera logic.
        let yaw = to_cam.x.atan2(to_cam.z);
        let rotation = Quat::from_rotation_y(yaw);

        // Pixel-aspect ratio drives the quad's shape so wider names
        // don't get squashed into a square.
        let aspect_ratio = aspect.width.max(1) as f32 / aspect.height.max(1) as f32;
        let scale = distance_to_scale(distance);
        let world_width =
            (QUAD_BASE_WIDTH_YALMS * scale).clamp(MIN_QUAD_WIDTH_YALMS, MAX_QUAD_WIDTH_YALMS);
        let world_height = world_width / aspect_ratio.max(0.01);

        transform.translation = head_pos;
        transform.rotation = rotation;
        transform.scale = Vec3::new(world_width, world_height, 1.0);
        *vis = Visibility::Visible;

        // Combat-state-derived color. Recomputed every frame so a mob
        // turning hostile / breaking aggro flips the label color even
        // for an otherwise unchanged name.
        let is_engaged_with = self_bt_target != 0 && self_bt_target == np.entity_id;
        let is_aggroing = match (self_uid, bt_target_by_id.get(&np.entity_id).copied()) {
            // Match `sync_aggro_system`'s comparison exactly: cast the
            // entity-side u32 down to u16 and compare against the
            // player's UniqueNo. Limit to Mob/Pet kinds so a PC that
            // happens to bt_target the player isn't painted red.
            (Some(uid), Some(bt)) if matches!(np.kind, EntityKind::Mob | EntityKind::Pet) => {
                bt as u16 == uid && bt != 0
            }
            _ => false,
        };
        let want_color = color_to_rgba8(nameplate_color(np.kind, is_engaged_with, is_aggroing));

        // HP bar is only rendered for kinds whose snapshot carries
        // an HP percentage (Mob/Pet). NPCs/PCs/Others stay bar-less
        // even when the wire happens to surface an `hp_pct` value.
        let snapshot_hp = hp_by_id.get(&np.entity_id).copied().flatten();
        let want_hp = if matches!(np.kind, EntityKind::Mob | EntityKind::Pet) {
            snapshot_hp
        } else {
            None
        };

        // Refresh the rasterized texture if any of (text, color, HP)
        // changed. Three cheap equality checks save a glyph-atlas
        // allocation per frame for the common case (stable label).
        let want = format_billboard_label(&np.base_name, snapshot_hp, np.kind);
        if want != np.last_rendered || want_color != np.last_color || want_hp != np.last_hp {
            // `get_mut` (not `get`) on the material: Bevy caches each
            // material's bind group keyed by the material's change tick.
            // Replacing the image at the texture handle regenerates the
            // GpuImage on the render side, but the StandardMaterial's
            // bind group keeps its TextureView reference to the *prior*
            // GPU texture (the old one stays alive through that Arc).
            // Net effect: HP=100 from the first re-rasterization sticks
            // because that bake matched the freshly-built bind group,
            // but subsequent HP-decrement bakes never reach the screen.
            // `get_mut` emits `AssetEvent::Modified` on the material,
            // which forces the render world to re-extract it and rebuild
            // the bind group against the new GPU texture.
            if let Some(mat_data) = materials.get_mut(&mat.0) {
                if let Some(handle) = mat_data.base_color_texture.clone() {
                    let new_img =
                        rasterize_text_to_image(&font.0, &want, NAME_PX, want_color, want_hp);
                    aspect.width = new_img.width();
                    aspect.height = new_img.height();
                    let _ = images.insert(&handle, new_img);
                    np.last_rendered = want;
                    np.last_color = want_color;
                    np.last_hp = want_hp;
                }
            }
        }
    }
}

/// Map camera distance (in yalms) to a billboard scale multiplier.
///
/// This is the visual taste knob that decides how aggressively
/// nameplates resist perspective shrink. The default (full perspective)
/// would be `1.0` for any distance, which lets a quad of fixed world
/// width shrink as `1/distance` on screen — the natural behavior of any
/// 3D object. We want partial compensation: text stays readable from
/// across the zone without becoming so big up close that it covers the
/// model.
///
/// **TODO (operator-defined):** implement the curve. See the function
/// body for the call sites and constraints.
pub fn distance_to_scale(distance_yalms: f32) -> f32 {
    // ── Hand-off point for the operator to shape ──
    //
    // Inputs: `distance_yalms` ≥ 0.  Typical operating range: ~2..150.
    // Output: positive scale factor applied to the quad's world width
    // before the aspect-corrected height is derived. `1.0` = the
    // `QUAD_BASE_WIDTH_YALMS` constant unmodified.
    //
    // Constraints the renderer assumes:
    //   * Must be > 0 for all reachable distances (returning 0 collapses
    //     the quad to a point and stops it from depth-writing, which
    //     re-introduces the "no occlusion" bug we just fixed).
    //   * Should be monotonically non-decreasing in distance, otherwise
    //     pulling the camera *back* could make a label snap *smaller*,
    //     which feels broken.
    //
    // Common shapes to consider:
    //   * Linear comp: `distance * k`  — constant screen-space size,
    //     looks UI-like, may feel detached from the 3D world.
    //   * Sub-linear: `(distance / d_ref).powf(α) * base` with
    //     `α ∈ [0.4, 0.8]` — partial shrink, the "feels right" middle
    //     ground we sketched in the AskUserQuestion exchange.
    //   * Piecewise: full perspective up to some near distance, then
    //     compensated past it — gives close-up labels the natural model
    //     scale but stops far-distance labels from disappearing.
    //
    // Pick whichever maps to the perceptual goal you have in mind, then
    // delete this comment and replace the body with the formula.
    // Gentle inverse — quad stays close to QUAD_BASE_WIDTH_YALMS and
    // shrinks modestly as the camera pulls back, like a natural 3D
    // object rather than a UI label locked to screen size.
    1.0 / (1.0 + distance_yalms * 0.03)
}

/// Rasterize `text` into an RGBA8 image with a dark contrast halo.
///
/// Two-pass:
///   1. Lay out glyphs and write per-pixel glyph coverage (alpha only)
///      into a single-channel scratch buffer, padded by
///      `OUTLINE_RADIUS_PX + 1` on each side so the halo has room.
///   2. For each output pixel, take `text_alpha` from the coverage
///      buffer and derive `outline_alpha` as the max coverage inside
///      a disk of radius `OUTLINE_RADIUS_PX` around that pixel. Then
///      composite **text over outline** with straight-alpha blending.
///
/// Disk-shaped dilation (vs. a square neighborhood) keeps the corners
/// of the halo rounded, which reads as "soft shadow" rather than as a
/// blocky stroke. Per-pixel cost is `O(R²)`, but the pass only fires
/// on name/state change.
///
/// The result uses **sRGB** color encoding so a `StandardMaterial`'s
/// gamma-aware sampler reproduces the requested `color` accurately —
/// using `Rgba8Unorm` here instead would darken the text noticeably on
/// any modern desktop preset.
fn rasterize_text_to_image(
    font: &FontArc,
    text: &str,
    px: f32,
    color: [u8; 4],
    hp_pct: Option<u8>,
) -> Image {
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let line_h = (ascent - descent).ceil().max(1.0) as u32;

    // Lay out glyphs along a horizontal baseline at `ascent`. We pick
    // up the kerning advance from `ab_glyph` so wide-glyph names
    // (capital W, M) don't visually crash into the next character.
    let mut pen_x = 0.0_f32;
    let mut max_x = 0.0_f32;
    let mut glyphs = Vec::with_capacity(text.chars().count());
    let mut prev = None;
    for ch in text.chars() {
        let g = scaled.scaled_glyph(ch);
        if let Some(p) = prev {
            pen_x += scaled.kern(p, g.id);
        }
        let advance = scaled.h_advance(g.id);

        let positioned = ab_glyph::Glyph {
            id: g.id,
            position: ab_glyph::point(pen_x, ascent),
            scale: g.scale,
        };
        pen_x += advance;
        max_x = max_x.max(pen_x);
        prev = Some(positioned.id);
        glyphs.push(positioned);
    }

    // Pad by R+1 on every side so the halo has room without clipping
    // at the texture edge.
    let pad = (OUTLINE_RADIUS_PX + 1) as u32;
    let width = (max_x.ceil() as u32).max(1) + 2 * pad;
    let text_height = line_h + 2 * pad;
    // Always reserve the HP-bar strip in the texture, even when
    // `hp_pct` is None (e.g., spawn-time before the first HP-bearing
    // CHAR_NPC, or NPCs/PCs which never get a bar). Keeping texture
    // dimensions stable across re-rasterizations avoids the bug where
    // the GPU's allocated texture stays at the spawn-time size while
    // `Assets::insert` writes a larger RGBA buffer — the bar pixels
    // get written into the buffer but rendered into a region the GPU
    // texture doesn't cover, so the bar is invisible. The strip is
    // transparent (all zeros) when `hp_pct.is_none()`, so the label
    // looks identical to the old text-only behavior in that case.
    let hp_strip = HP_BAR_TOP_GAP_PX + HP_BAR_HEIGHT_PX;
    let height = text_height + hp_strip;

    // Pass 1: glyph coverage into a single-channel scratch buffer.
    // The scratch buffer is the SAME size as the final texture (text
    // strip + optional HP-bar strip) so the row indices match the
    // composite-pass loop below. The HP strip rows stay at coverage=0,
    // which the composite treats as transparent — the bar is painted
    // directly into the RGBA buffer afterward.
    let mut coverage = vec![0u8; (width * height) as usize];
    for glyph in glyphs {
        if let Some(outline_glyph) = scaled.outline_glyph(glyph) {
            let bb = outline_glyph.px_bounds();
            outline_glyph.draw(|gx, gy, c| {
                let px_x = bb.min.x as i32 + gx as i32 + pad as i32;
                let px_y = bb.min.y as i32 + gy as i32 + pad as i32;
                if px_x < 0 || px_y < 0 || px_x >= width as i32 || px_y >= text_height as i32 {
                    return;
                }
                let i = (px_y as u32 * width + px_x as u32) as usize;
                let added = (c * 255.0).round().clamp(0.0, 255.0) as u8;
                coverage[i] = coverage[i].saturating_add(added);
            });
        }
    }

    // Pass 2: composite text-over-outline into the final RGBA texture.
    // Only the text strip needs the disk-dilation halo math; the HP
    // strip below it is left at zero alpha and painted next.
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let r = OUTLINE_RADIUS_PX;
    let r2 = r * r;
    let w_i = width as i32;
    let text_h_i = text_height as i32;
    for y in 0..text_h_i {
        for x in 0..w_i {
            let text_alpha = coverage[(y * w_i + x) as usize];

            // Max coverage inside a disk of radius R around this pixel.
            // The dilation reads from `coverage`, which only has glyph
            // marks in the text strip — clamping `y1` to `text_h_i - 1`
            // keeps the halo from "leaking" into the HP-bar rows below.
            let mut outline_alpha: u8 = 0;
            let y0 = (y - r).max(0);
            let y1 = (y + r).min(text_h_i - 1);
            let x0 = (x - r).max(0);
            let x1 = (x + r).min(w_i - 1);
            for ny in y0..=y1 {
                let dy = ny - y;
                let dy2 = dy * dy;
                for nx in x0..=x1 {
                    let dx = nx - x;
                    if dx * dx + dy2 > r2 {
                        continue;
                    }
                    let na = coverage[(ny * w_i + nx) as usize];
                    if na > outline_alpha {
                        outline_alpha = na;
                    }
                }
            }

            // Combine text and outline alphas through their declared
            // RGBA color alphas. Straight-alpha "text over outline":
            // out_a = ta + (1 - ta) * oa
            // out_rgb = (text * ta + outline * (1 - ta) * oa) / out_a
            let ta = (text_alpha as f32 / 255.0) * (color[3] as f32 / 255.0);
            let oa = (outline_alpha as f32 / 255.0) * (OUTLINE_COLOR[3] as f32 / 255.0);
            let out_a = ta + (1.0 - ta) * oa;
            if out_a <= 0.0 {
                continue;
            }
            let inv = 1.0 / out_a;
            let or = color[0] as f32 * ta + OUTLINE_COLOR[0] as f32 * (1.0 - ta) * oa;
            let og = color[1] as f32 * ta + OUTLINE_COLOR[1] as f32 * (1.0 - ta) * oa;
            let ob = color[2] as f32 * ta + OUTLINE_COLOR[2] as f32 * (1.0 - ta) * oa;
            let pi = ((y * w_i + x) * 4) as usize;
            pixels[pi] = (or * inv).round().clamp(0.0, 255.0) as u8;
            pixels[pi + 1] = (og * inv).round().clamp(0.0, 255.0) as u8;
            pixels[pi + 2] = (ob * inv).round().clamp(0.0, 255.0) as u8;
            pixels[pi + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }

    // Pass 3 (HP-bar entities only): draw the bar directly into the
    // RGBA buffer. Outline = 1px of `OUTLINE_COLOR` (matches the text
    // halo); interior fills left-to-right with the HP-color lerp.
    if let Some(pct) = hp_pct {
        let bar_pixel_w = (width as f32 * HP_BAR_WIDTH_FRACTION) as u32;
        let bar_x = (width.saturating_sub(bar_pixel_w)) / 2;
        let bar_y = text_height + HP_BAR_TOP_GAP_PX;
        let bar_h = HP_BAR_HEIGHT_PX;
        let fill_color = hp_color_rgba(pct);

        // 1px outline.
        for x in 0..bar_pixel_w {
            paint_pixel(&mut pixels, width, bar_x + x, bar_y, OUTLINE_COLOR);
            paint_pixel(
                &mut pixels,
                width,
                bar_x + x,
                bar_y + bar_h - 1,
                OUTLINE_COLOR,
            );
        }
        for y in 0..bar_h {
            paint_pixel(&mut pixels, width, bar_x, bar_y + y, OUTLINE_COLOR);
            paint_pixel(
                &mut pixels,
                width,
                bar_x + bar_pixel_w - 1,
                bar_y + y,
                OUTLINE_COLOR,
            );
        }

        // Filled interior (1px inside the outline). Width scales by HP%.
        let interior_w = bar_pixel_w.saturating_sub(2);
        let fill_w = (interior_w as f32 * pct.min(100) as f32 / 100.0).round() as u32;
        for y in 1..(bar_h - 1) {
            for x in 0..fill_w {
                paint_pixel(&mut pixels, width, bar_x + 1 + x, bar_y + y, fill_color);
            }
        }
    }

    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    image
}

fn color_to_rgba8(c: Color) -> [u8; 4] {
    let s = c.to_srgba();
    [
        (s.red.clamp(0.0, 1.0) * 255.0).round() as u8,
        (s.green.clamp(0.0, 1.0) * 255.0).round() as u8,
        (s.blue.clamp(0.0, 1.0) * 255.0).round() as u8,
        (s.alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

/// Write `color` at `(x, y)` in an RGBA8 buffer of stride `width * 4`.
/// Out-of-bounds coordinates are silently ignored — callers compute
/// bar geometry from texture width and the saturating-sub guards in
/// the layout code can produce zero-width rectangles for very narrow
/// textures, in which case painting just no-ops.
#[inline]
fn paint_pixel(pixels: &mut [u8], width: u32, x: u32, y: u32, color: [u8; 4]) {
    let pi = ((y * width + x) * 4) as usize;
    if pi + 4 > pixels.len() {
        return;
    }
    pixels[pi] = color[0];
    pixels[pi + 1] = color[1];
    pixels[pi + 2] = color[2];
    pixels[pi + 3] = color[3];
}

/// HP-percentage → bar fill color. Green at 100%, yellow at 50%, red
/// at 0% — the canonical "vitals" lerp every MMO HUD uses. Returns
/// fully-opaque sRGB so the bar reads as a solid color against the
/// outlined glyphs above it (the bar's *alpha* is the OUTLINE_COLOR
/// border, not the fill).
fn hp_color_rgba(pct: u8) -> [u8; 4] {
    let f = (pct.min(100) as f32) / 100.0;
    let (r, g) = if f >= 0.5 {
        // green → yellow as HP drops from 100% to 50%
        let t = (1.0 - f) * 2.0;
        (t, 1.0)
    } else {
        // yellow → red as HP drops from 50% to 0%
        let t = f * 2.0;
        (1.0, t)
    };
    [(r * 255.0).round() as u8, (g * 255.0).round() as u8, 0, 255]
}
