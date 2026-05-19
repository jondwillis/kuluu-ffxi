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
use ffxi_viewer_wire::{EntityKind, Position};

use crate::camera::OperatorCamera;
use crate::components::{InGameEntity, Nameplate, WorldEntity};
use crate::snapshot::SceneState;

/// Pixel-height of the rasterized name glyph. Bigger = sharper but
/// pricier per re-rasterization. 32px is a comfortable read at 1× quad
/// scale and re-rasterizes in <100 µs even with mojibake'd names.
const NAME_PX: f32 = 32.0;

/// Base world-space width of the quad in yalms before the
/// distance-curve scaling kicks in. ~1.6 yalms ≈ chest-to-shoulder
/// width on the default capsule, so a one-word name lands about
/// chin-height when the camera is at melee range.
const QUAD_BASE_WIDTH_YALMS: f32 = 1.6;

/// Vertical offset above the owning entity's translation where the
/// label sits. Matches the prior UI nameplate (`Vec3::Y * 2.4`) so the
/// label parks at roughly head-of-capsule height regardless of which
/// rendering path the operator is on.
const HEAD_Y_OFFSET: f32 = 2.4;

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
    /// Original `name` from the wire. We re-derive the rendered string
    /// (with optional `" 73%"` HP suffix) from this each frame, so a
    /// label that doesn't move in space doesn't trigger redundant
    /// re-rasterizations.
    pub base_name: String,
    /// Last text we actually rasterized into the material's texture.
    /// Drives a cheap string-equality check before we burn another
    /// allocation on a glyph atlas.
    pub last_rendered: String,
    /// RGBA color used when we rasterized `last_rendered`. Re-rasterize
    /// if it ever changes (currently constant per entity, but kept here
    /// so future palette swaps don't desync the cache).
    pub last_color: [u8; 4],
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
    let raster = rasterize_text_to_image(font, name, NAME_PX, rgba).clone();
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

/// Build the rendered label string for a billboard. Mobs and pets get
/// the `"Name 73%"` HP suffix; PCs and NPCs get the bare name. Mirrors
/// the previous UI `format_label` — kept here (not shared from
/// `nameplate.rs`) so the billboard module is the single owner of all
/// label-formatting choices going forward.
pub fn format_billboard_label(base_name: &str, hp_pct: Option<u8>, kind: EntityKind) -> String {
    let show_hp = matches!(kind, EntityKind::Mob | EntityKind::Pet);
    match (show_hp, hp_pct) {
        (true, Some(pct)) => format!("{base_name} {pct}%"),
        _ => base_name.to_string(),
    }
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
    world_q: Query<(&Transform, &WorldEntity), Without<NameplateBillboard>>,
    mut billboards: Query<(
        Entity,
        &mut NameplateBillboard,
        &mut BillboardAspect,
        &mut Transform,
        &mut Visibility,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    materials: Res<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    font: Res<BillboardFont>,
    mut commands: Commands,
) {
    let Ok(cam_t) = cam_q.single() else { return };
    let cam_pos = cam_t.translation;

    // World-position lookup keyed by wire id, built once per frame.
    let mut pos_by_id: std::collections::HashMap<u32, Vec3> =
        std::collections::HashMap::with_capacity(world_q.iter().len());
    for (t, w) in &world_q {
        pos_by_id.insert(w.id, t.translation);
    }

    // HP lookup keyed by wire id — only the entities the snapshot says
    // have a known HP appear here; missing means "no suffix this frame."
    let mut hp_by_id: std::collections::HashMap<u32, Option<u8>> = std::collections::HashMap::new();
    for ent in &state.snapshot.entities {
        hp_by_id.insert(ent.id, ent.hp_pct);
    }

    for (ui_entity, mut np, mut aspect, mut transform, mut vis, mat) in &mut billboards {
        let Some(&entity_pos) = pos_by_id.get(&np.entity_id) else {
            // Owner gone — same lifecycle the UI nameplates had.
            commands.entity(ui_entity).despawn();
            continue;
        };

        let head_pos = entity_pos + Vec3::Y * HEAD_Y_OFFSET;
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
        let world_width = (QUAD_BASE_WIDTH_YALMS * scale).clamp(0.4, 2.75);
        let world_height = world_width / aspect_ratio.max(0.01);

        transform.translation = head_pos;
        transform.rotation = rotation;
        transform.scale = Vec3::new(world_width, world_height, 1.0);
        *vis = Visibility::Visible;

        // Refresh the rasterized text if the displayed string changed.
        // String equality is cheap and avoids re-allocating a full
        // glyph atlas on every frame for stable labels.
        let hp = hp_by_id.get(&np.entity_id).copied().flatten();
        let want = format_billboard_label(&np.base_name, hp, np.kind);
        if want != np.last_rendered {
            if let Some(mat) = materials.get(&mat.0) {
                if let Some(handle) = mat.base_color_texture.clone() {
                    let new_img = rasterize_text_to_image(&font.0, &want, NAME_PX, np.last_color);
                    aspect.width = new_img.width();
                    aspect.height = new_img.height();
                    images.insert(&handle, new_img);
                    np.last_rendered = want;
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

/// Rasterize `text` into a tightly-cropped RGBA8 image using `ab_glyph`.
/// One image per nameplate — kept small (typically <128×40 px) so the
/// allocation cost is dominated by the glyph atlas, not the texture.
///
/// The result uses **sRGB** color encoding so a `StandardMaterial`'s
/// gamma-aware sampler reproduces the requested `color` accurately —
/// using `Rgba8Unorm` here instead would darken the text noticeably on
/// any modern desktop preset.
fn rasterize_text_to_image(font: &FontArc, text: &str, px: f32, color: [u8; 4]) -> Image {
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

    // +2 padding on each axis to give anti-aliased edges room without
    // bleeding into the texture wrap.
    let width = (max_x.ceil() as u32).max(1) + 2;
    let height = line_h + 2;
    let mut pixels = vec![0u8; (width * height * 4) as usize];

    for glyph in glyphs {
        if let Some(outline) = scaled.outline_glyph(glyph) {
            let bb = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px_x = bb.min.x as i32 + gx as i32 + 1;
                let px_y = bb.min.y as i32 + gy as i32 + 1;
                if px_x < 0 || px_y < 0 || px_x >= width as i32 || px_y >= height as i32 {
                    return;
                }
                let i = ((px_y as u32 * width + px_x as u32) * 4) as usize;
                let alpha = (coverage * color[3] as f32).round() as u8;
                // Straight (non-premultiplied) RGBA. `AlphaMode::Blend`
                // on `StandardMaterial` expects this; switching to
                // premult would require `AlphaMode::Premultiplied`.
                pixels[i] = color[0];
                pixels[i + 1] = color[1];
                pixels[i + 2] = color[2];
                pixels[i + 3] = pixels[i + 3].saturating_add(alpha);
            });
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
