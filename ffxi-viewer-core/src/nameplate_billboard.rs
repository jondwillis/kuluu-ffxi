use std::sync::Arc;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use bevy::asset::RenderAssetUsages;
use bevy::image::{Image, ImageSampler};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ffxi_viewer_wire::EntityKind;

use crate::camera::{nameplate_anchor_y, OperatorCamera};
use crate::components::{InGameEntity, Nameplate, WorldEntity};
use crate::scene::BakedActor;
use crate::snapshot::SceneState;

const NAME_PX: f32 = 64.0;

const QUAD_BASE_WIDTH_YALMS: f32 = 1.1;

const MAX_QUAD_WIDTH_YALMS: f32 = 1.4;
const MIN_QUAD_WIDTH_YALMS: f32 = 0.8;

const OUTLINE_RADIUS_PX: i32 = 3;

const OUTLINE_COLOR: [u8; 4] = [0, 0, 0, 220];

const HP_BAR_HEIGHT_PX: u32 = 16;

const HP_BAR_TOP_GAP_PX: u32 = 8;

const HP_BAR_WIDTH_FRACTION: f32 = 1.0;

#[derive(Resource)]
pub struct BillboardFont(pub Arc<FontArc>);

impl FromWorld for BillboardFont {
    fn from_world(_: &mut World) -> Self {
        let font = FontArc::try_from_slice(crate::ui_font::DEJAVU_SANS_MONO)
            .expect("bundled DejaVuSansMono.ttf must parse as a valid TTF for ab_glyph");
        Self(Arc::new(font))
    }
}

#[derive(Component)]
pub struct NameplateBillboard {
    pub entity_id: u32,
    pub kind: EntityKind,

    pub base_name: String,

    pub last_rendered: String,

    pub last_color: [u8; 4],

    pub last_hp: Option<u8>,
}

#[derive(Component)]
pub struct BillboardAspect {
    pub width: u32,
    pub height: u32,
}

#[allow(clippy::too_many_arguments)]
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

    let raster = rasterize_text_to_image(font, name, NAME_PX, rgba, None).clone();
    let aspect = (raster.width(), raster.height());
    let image_handle = images.add(raster);

    let mesh_handle = meshes.add(Rectangle::new(1.0, 1.0));

    let material_handle = materials.add(StandardMaterial {
        base_color_texture: Some(image_handle),
        base_color: Color::WHITE,

        unlit: true,
        alpha_mode: AlphaMode::Blend,

        cull_mode: None,
        ..default()
    });

    commands
        .spawn((
            InGameEntity,
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
            Transform::from_translation(Vec3::new(0.0, -1_000_000.0, 0.0)),
            Visibility::Hidden,
            NotShadowCaster,
            NotShadowReceiver,
        ))
        .id()
}

/// Retail never draws the local player's own overhead name, so the self plate
/// is suppressed entirely. This also removes the near-degenerate overhead
/// projection (plate anchored just above the first-person eye) whose 1-frame
/// camera/plate skew dips and jitters on stutter frames (kuluu-gr2).
pub fn is_self_billboard(entity_id: u32, self_char_id: Option<u32>) -> bool {
    self_char_id.is_some_and(|cid| cid != 0 && cid == entity_id)
}

pub fn nameplate_color(kind: EntityKind, engaged: bool, dead: bool) -> Color {
    match kind {
        EntityKind::Pc => Color::srgb(0.55, 0.95, 1.0),
        EntityKind::Npc => Color::srgb(0.55, 1.0, 0.55),
        EntityKind::Mob => {
            if dead {
                Color::srgb(0.55, 0.55, 0.55)
            } else if engaged {
                Color::srgb(1.0, 0.55, 0.25)
            } else {
                Color::srgb(1.0, 0.95, 0.7)
            }
        }
        EntityKind::Pet => Color::srgb(0.55, 0.95, 0.65),
        EntityKind::Other => Color::srgb(0.85, 0.85, 0.85),
    }
}

pub fn format_billboard_label(base_name: &str, _hp_pct: Option<u8>, _kind: EntityKind) -> String {
    base_name.to_string()
}

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

    let mut pos_by_id: std::collections::HashMap<u32, (Vec3, f32)> =
        std::collections::HashMap::with_capacity(world_q.iter().len());
    for (t, w, baked) in &world_q {
        pos_by_id.insert(w.id, (t.translation, nameplate_anchor_y(baked)));
    }

    let self_char_id: Option<u32> = state.snapshot.self_char_id;
    let mut hp_by_id: std::collections::HashMap<u32, Option<u8>> = std::collections::HashMap::new();
    let mut claim_by_id: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for ent in &state.snapshot.entities {
        hp_by_id.insert(ent.id, ent.hp_pct);
        claim_by_id.insert(ent.id, ent.claim_id);
    }

    for (ui_entity, mut np, mut aspect, mut transform, mut vis, mat) in &mut billboards {
        // Covers plates spawned before self_char_id was known; the spawn path
        // in scene.rs already skips self once the id is resolved (kuluu-gr2).
        if is_self_billboard(np.entity_id, self_char_id) {
            commands.entity(ui_entity).try_despawn();
            continue;
        }

        let Some(&(entity_pos, head_y_offset)) = pos_by_id.get(&np.entity_id) else {
            commands.entity(ui_entity).try_despawn();
            continue;
        };

        let head_pos = entity_pos + Vec3::Y * head_y_offset;
        let to_cam = cam_pos - head_pos;
        let distance = to_cam.length();
        if distance < 0.001 {
            *vis = Visibility::Hidden;
            continue;
        }

        let yaw = to_cam.x.atan2(to_cam.z);
        let rotation = Quat::from_rotation_y(yaw);

        let aspect_ratio = aspect.width.max(1) as f32 / aspect.height.max(1) as f32;
        let scale = distance_to_scale(distance);
        let world_width =
            (QUAD_BASE_WIDTH_YALMS * scale).clamp(MIN_QUAD_WIDTH_YALMS, MAX_QUAD_WIDTH_YALMS);
        let world_height = world_width / aspect_ratio.max(0.01);

        transform.translation = head_pos;
        transform.rotation = rotation;
        transform.scale = Vec3::new(world_width, world_height, 1.0);
        *vis = Visibility::Visible;

        let engaged = matches!(np.kind, EntityKind::Mob)
            && self_char_id.is_some_and(|cid| {
                cid != 0 && claim_by_id.get(&np.entity_id).copied() == Some(cid)
            });

        let dead = matches!(np.kind, EntityKind::Mob)
            && hp_by_id.get(&np.entity_id).copied().flatten() == Some(0);
        let want_color = color_to_rgba8(nameplate_color(np.kind, engaged, dead));

        let snapshot_hp = hp_by_id.get(&np.entity_id).copied().flatten();
        let want_hp = if matches!(np.kind, EntityKind::Mob | EntityKind::Pet) {
            snapshot_hp
        } else {
            None
        };

        let want = format_billboard_label(&np.base_name, snapshot_hp, np.kind);
        if want != np.last_rendered || want_color != np.last_color || want_hp != np.last_hp {
            if let Some(mat_data) = materials.get_mut(&mat.0) {
                if let Some(handle) = mat_data.base_color_texture.clone() {
                    crate::perf_probe::note_nameplate_raster();
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

pub fn distance_to_scale(distance_yalms: f32) -> f32 {
    1.0 / (1.0 + distance_yalms * 0.03)
}

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

    let pad = (OUTLINE_RADIUS_PX + 1) as u32;
    let width = (max_x.ceil() as u32).max(1) + 2 * pad;
    let text_height = line_h + 2 * pad;

    let hp_strip = HP_BAR_TOP_GAP_PX + HP_BAR_HEIGHT_PX;
    let height = text_height + hp_strip;

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

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let r = OUTLINE_RADIUS_PX;
    let r2 = r * r;
    let w_i = width as i32;
    let text_h_i = text_height as i32;
    for y in 0..text_h_i {
        for x in 0..w_i {
            let text_alpha = coverage[(y * w_i + x) as usize];

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

    if let Some(pct) = hp_pct {
        let bar_pixel_w = (width as f32 * HP_BAR_WIDTH_FRACTION) as u32;
        let bar_x = (width.saturating_sub(bar_pixel_w)) / 2;
        let bar_y = text_height + HP_BAR_TOP_GAP_PX;
        let bar_h = HP_BAR_HEIGHT_PX;
        let fill_color = hp_color_rgba(pct);

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

fn hp_color_rgba(pct: u8) -> [u8; 4] {
    let f = (pct.min(100) as f32) / 100.0;
    let (r, g) = if f >= 0.5 {
        let t = (1.0 - f) * 2.0;
        (t, 1.0)
    } else {
        let t = f * 2.0;
        (1.0, t)
    };
    [(r * 255.0).round() as u8, (g * 255.0).round() as u8, 0, 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_billboard_suppressed_for_known_self_id() {
        assert!(is_self_billboard(0xCAFE, Some(0xCAFE)));
    }

    #[test]
    fn other_entities_keep_billboards() {
        assert!(!is_self_billboard(0x4242, Some(0xCAFE)));
    }

    #[test]
    fn unknown_self_id_suppresses_nothing() {
        assert!(!is_self_billboard(0xCAFE, None));
    }

    #[test]
    fn zero_self_id_is_unresolved_not_a_match() {
        assert!(!is_self_billboard(0, Some(0)));
    }
}
