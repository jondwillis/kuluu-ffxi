use std::sync::Arc;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use bevy::image::{Image, ImageSampler};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use kuluu_snapshot::EntityKind;

use crate::camera::{nameplate_anchor_y, CameraMode, OperatorCamera};
use crate::components::{InGameEntity, Nameplate, WorldEntity};
use crate::scene::{BakedActor, Target};
// Retail advances the targeted-nameplate pulse once per rendered frame.
use crate::nameplate_icons::REFERENCE_LETTER;
use crate::scheduler_runtime::RETAIL_FPS;
use crate::snapshot::SceneState;

// Raster resolution of the plate texture, not its on-screen size: the world
// quad is derived from the texture's height *relative to* one line, so raising
// this only buys sharper glyphs at the same size.
const NAME_PX: f32 = 80.0;

// Retail's plate scale is pixel-exact for a 640x480 client (NAME_SCREEN_SCALE),
// which reads small on a modern display. A deliberate legibility nudge on top
// of it — the whole plate, so the icon/text proportions stay retail's.
const NAMEPLATE_LEGIBILITY_SCALE: f32 = 1.3;

// Retail's depth ramp shrinks a plate to 6% past ~500 yalms and to 31% at a
// routine 20-yalm engage distance — unreadable off a 640x480 CRT. A legibility
// floor over the ramp (scale_for_view_depth stays retail-pure): every drawable
// plate keeps at least this fraction of full size. Reached near ~13 yalms.
const NAMEPLATE_MIN_DEPTH_SCALE: f32 = 0.45;

// research/XIClient/src/XIClient/source/Game/GameManager.cpp:798-799 — retail's clip planes
// are fixed, so the nameplate ramp below must not read our camera's user-tunable projection.
const RETAIL_NEAR_CLIP_YALMS: f32 = 0.1;
const RETAIL_FAR_CLIP_YALMS: f32 = 65535.0;

// research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp:75
const NDC_DEPTH_FIXED_POINT_SCALE: u32 = 4096;
// research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp:261-262 rejects
// z >= 1.0, so the deepest drawable fixed-point depth is one step short of the scale.
const MAX_DRAWABLE_DEPTH_FIXED: u32 = NDC_DEPTH_FIXED_POINT_SCALE - 1;
// research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp:90-91
const FADE_START_DEPTH_FIXED: u32 = 0xFB4;
const FADE_END_DEPTH_FIXED: u32 = 0x1004;
// research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp:72-73 — the
// reciprocal-w gate (1/depth < 1) drops names inside one yalm of the view plane.
const MIN_VIEW_DEPTH_YALMS: f32 = 1.0;

// research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp:31 — glyph units
// to viewport fraction, applied to a pre-transformed (RHW=1) screen-space quad.
const NAME_SCREEN_SCALE: f32 = 0.002_343_75;
// research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp:35 — one name
// line is one glyph cell tall.
pub const NAME_LINE_HEIGHT_UNITS: f32 = 8.0;
const NAME_LINE_SCREEN_FRACTION: f32 = NAME_SCREEN_SCALE * NAME_LINE_HEIGHT_UNITS;

// research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp:111-112
const TARGET_PULSE_DEGREES_PER_FRAME: u32 = 16;
const FULL_TURN_DEGREES: u32 = 360;
const TARGET_PULSE_AMPLITUDE: f32 = 32.0;
const TARGET_PULSE_BIAS: f32 = 96.0;
// research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp:115 repacks
// the product as `(scaledAlpha & 0xFFFFFF80) << 17`, i.e. a shift right by 7.
const TARGET_PULSE_DIVISOR: f32 = 128.0;

// Heavier than a hairline on purpose: the plate is unlit and draws over
// arbitrary zone geometry, so the outline is what keeps a light name readable
// against a light wall. Scales with NAME_PX.
const OUTLINE_RADIUS_PX: i32 = 7;

const OUTLINE_COLOR: [u8; 4] = [0, 0, 0, 255];

// The bundled mono font's strokes are thinner than retail's chunky bitmap
// glyphs; a second coverage pass offset horizontally fakes the weight.
const BOLD_DILATE_PX: i32 = 2;

// Retail draws names in a screen-space pass after the world and its effects
// (research/XIClient/src/XIClient/source/Rendering/Active/CXiActorNameDraw.cpp,
// pre-transformed RHW quads). Bevy's Transparent3d instead sorts every blend
// draw by view-space AABB-centre Z, where camera-anchored quads (lens flare,
// weather) rank nearest and blend over the plates (same failure the sky layers
// hit, kuluu-w4jf). A uniform positive sort bias past the sky dome makes every
// plate outrank all unbiased world transparents while preserving
// plate-vs-plate order and the opaque-geometry depth test.
const NAMEPLATE_SORT_BIAS: f32 = crate::skybox::SKYBOX_RADIUS;

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

/// Everything the billboard texture is a function of. Comparing the whole key
/// is what keeps the raster off the hot path: it only re-runs when one of these
/// actually changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterKey {
    pub text: String,
    pub color: [u8; 4],
    pub hp: Option<u8>,
    pub markers: Vec<u8>,
    pub linkshell_tint: [u8; 4],
}

impl RasterKey {
    fn matches(&self, text: &str, other: &Self) -> bool {
        self.text == text
            && self.color == other.color
            && self.hp == other.hp
            && self.markers == other.markers
            && self.linkshell_tint == other.linkshell_tint
    }
}

#[derive(Component)]
pub struct NameplateBillboard {
    pub entity_id: u32,
    pub kind: EntityKind,

    pub base_name: String,

    /// `None` until the first raster, so a freshly spawned plate always gets
    /// one even when its resolved colour happens to match the placeholder.
    pub rastered: Option<RasterKey>,

    pub last_alpha: f32,
}

/// Per-frame billboard visibility breakdown for the Debug menu "Nameplate
/// Debug" panel: main-world mirror of what `Visibility::Hidden` is hiding,
/// with the reason — extract can only read the flag, not why it was set.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct NameplateBillboardDebug {
    /// Billboard entities present this frame.
    pub total: u32,
    /// Self-plate camera-mode cull (overhead self name in first person).
    pub hide_self: u32,
    /// View-depth gate: behind the camera forward plane or within
    /// MIN_VIEW_DEPTH_YALMS of it. This is a half-plane test, not frustum.
    pub hidden_depth: u32,
    /// Plates set Visible + transformed this frame.
    pub visible: u32,
    /// Billboards whose actor no longer exists — despawned mid-frame.
    pub despawned: u32,
}

#[derive(Component)]
pub struct BillboardAspect {
    pub width: u32,
    pub height: u32,

    /// Texture-space y of the text line's center. The world transform pins the
    /// line — not the icon-padded box — to the anchor, so an icon changes the
    /// plate's extent without moving the name.
    pub text_center_y_px: f32,
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

    let raster = rasterize_plate(font, name, NAME_PX, rgba, None, &[], rgba, None);
    let aspect = (
        raster.image.width(),
        raster.image.height(),
        raster.text_center_y_px,
    );
    let image_handle = images.add(raster.image);

    let mesh_handle = meshes.add(Rectangle::new(1.0, 1.0));

    let material_handle = materials.add(StandardMaterial {
        base_color_texture: Some(image_handle),
        base_color: Color::WHITE,

        unlit: true,
        alpha_mode: AlphaMode::Premultiplied,
        depth_bias: NAMEPLATE_SORT_BIAS,

        cull_mode: None,
        ..default()
    });

    commands
        .spawn((
            InGameEntity,
            crate::nameplate_overlay::nameplate_render_layers(),
            Nameplate { entity_id, kind },
            NameplateBillboard {
                entity_id,
                kind,
                base_name: name.to_string(),
                rastered: None,
                last_alpha: 1.0,
            },
            BillboardAspect {
                width: aspect.0,
                height: aspect.1,
                text_center_y_px: aspect.2,
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

pub fn is_self_billboard(entity_id: u32, self_char_id: Option<u32>) -> bool {
    self_char_id.is_some_and(|cid| cid != 0 && cid == entity_id)
}

/// Retail draws the self plate in the same PC styling as other players
/// (kuluu-hof), but in first-person the plate anchors just above the camera
/// eye — a near-degenerate projection that dips/jitters on stutter frames
/// (kuluu-gr2) and occludes the view — so it is hidden there.
pub fn self_plate_hidden(is_self: bool, mode: CameraMode) -> bool {
    is_self && matches!(mode, CameraMode::FirstPerson)
}

/// The colour a plate falls back to before the retail `ncol` table is
/// available — a DAT read that only fails when there is no retail install (the
/// headless/relay paths). Neutral white so a missing table never invents a
/// meaning the packet did not carry.
pub const NAMEPLATE_FALLBACK_COLOR: Color = Color::WHITE;

pub fn update_nameplate_billboards_system(
    state: Res<SceneState>,
    settings: Res<crate::graphics::settings::GraphicsSettings>,
    camera_mode: Res<CameraMode>,
    time: Res<Time>,
    target: Res<Target>,
    cam_q: Query<(&Transform, &Projection), (With<OperatorCamera>, Without<NameplateBillboard>)>,
    world_q: Query<
        (
            &Transform,
            &WorldEntity,
            Option<&BakedActor>,
            Has<crate::components::MountedRider>,
        ),
        Without<NameplateBillboard>,
    >,
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
    name_colors: Res<crate::nameplate_color::NameColorTable>,
    icons: Res<crate::nameplate_icons::NameplateIcons>,
    mut commands: Commands,
    mut dbg_out: ResMut<NameplateBillboardDebug>,
    mut raster_inputs: Local<std::collections::HashMap<u32, RasterKey>>,
) {
    let Ok((cam_t, projection)) = cam_q.single() else {
        return;
    };
    let Projection::Perspective(perspective) = projection else {
        return;
    };
    let cam_pos = cam_t.translation;
    let cam_forward = Vec3::from(cam_t.forward());
    let half_fov_tan = (perspective.fov * 0.5).tan();
    let line_px = text_line_height_px(&font.0, NAME_PX) as f32;
    let pulse_frame = (time.elapsed_secs() * RETAIL_FPS) as u32;

    let mut pos_by_id: std::collections::HashMap<u32, (Vec3, f32)> =
        std::collections::HashMap::with_capacity(world_q.iter().len());
    for (t, w, baked, mounted) in &world_q {
        pos_by_id.insert(w.id, (t.translation, nameplate_anchor_y(baked, mounted)));
    }

    let self_char_id: Option<u32> = state.snapshot.self_char_id;
    // HP/claim only change with a snapshot, and the re-raster inputs (name,
    // color, hp) derive from them — so the texture-regen check only runs on
    // snapshot frames. Billboard orientation/scale below stays per-frame.
    // The retail colour table and icon glyphs are read from the DAT a few
    // frames into the session, after the first plates have already rastered
    // against the fallback; their arrival has to re-raster them.
    // A Retail+ gate flip (mob HP under) must re-raster on the spot, not wait
    // for the next snapshot: settings changes are user actions, and the key
    // comparison below keeps unaffected plates from re-running.
    let dirty =
        state.dirty || name_colors.is_changed() || icons.is_changed() || settings.is_changed();
    if dirty {
        raster_inputs.clear();
        let ctx = crate::nameplate_color::SelfContext {
            self_id: self_char_id,
            party: &state.snapshot.party,
        };
        for ent in &state.snapshot.entities {
            raster_inputs.insert(
                ent.id,
                raster_key_for(ent, ctx, &name_colors, settings.mob_hp_under),
            );
        }
    }

    // Debug breakdown mirrors each gate below; see NameplateBillboardDebug.
    let mut total = 0u32;
    let mut hide_self = 0u32;
    let mut hidden_depth = 0u32;
    let mut visible_n = 0u32;
    let mut despawned = 0u32;

    for (ui_entity, mut np, mut aspect, mut transform, mut vis, mat) in &mut billboards {
        total += 1;
        let self_cull =
            self_plate_hidden(is_self_billboard(np.entity_id, self_char_id), *camera_mode);
        if self_cull {
            hide_self += 1;
            *vis = Visibility::Hidden;
            continue;
        }

        let Some(&(entity_pos, head_y_offset)) = pos_by_id.get(&np.entity_id) else {
            despawned += 1;
            commands.entity(ui_entity).try_despawn();
            continue;
        };

        let head_pos = entity_pos + Vec3::Y * head_y_offset;
        let view_depth = (head_pos - cam_pos).dot(cam_forward);
        let Some(scale) = legibility_scale_for_view_depth(view_depth) else {
            hidden_depth += 1;
            *vis = Visibility::Hidden;
            continue;
        };

        let aspect_ratio = aspect.width.max(1) as f32 / aspect.height.max(1) as f32;
        let plate_to_line = aspect.height.max(1) as f32 / line_px;
        let viewport_height_yalms = 2.0 * view_depth * half_fov_tan;
        let world_height = viewport_height_yalms
            * NAME_LINE_SCREEN_FRACTION
            * plate_to_line
            * scale
            * NAMEPLATE_LEGIBILITY_SCALE;
        let world_width = world_height * aspect_ratio;

        let rise = quad_center_rise(
            world_height / plate_to_line,
            line_px,
            aspect.height,
            aspect.text_center_y_px,
        );
        transform.translation = head_pos + Vec3::from(cam_t.up()) * rise;
        transform.rotation = cam_t.rotation;
        transform.scale = Vec3::new(world_width, world_height, 1.0);
        *vis = Visibility::Visible;
        visible_n += 1;

        // Pulse is time-driven (steps at RETAIL_FPS via pulse_frame, so the
        // last_alpha guard bounds writes to 30/s) and must run on non-snapshot
        // frames; only the snapshot-derived re-raster below is dirty-gated.
        let want_alpha = if target.id == Some(np.entity_id) {
            target_alpha_pulse(pulse_frame)
        } else {
            1.0
        };
        if want_alpha != np.last_alpha {
            if let Some(mut mat_data) = materials.get_mut(&mat.0) {
                // Premultiplied fade: the whole texel (color and coverage)
                // scales together, or the pulse would turn additive.
                mat_data.base_color = Color::LinearRgba(LinearRgba::new(
                    want_alpha, want_alpha, want_alpha, want_alpha,
                ));
                np.last_alpha = want_alpha;
            }
        }

        if !dirty {
            continue;
        }

        // The name lives on the component, not the snapshot: a later update can
        // drop it and the plate must keep the name it spawned with.
        let Some(inputs) = raster_inputs.get(&np.entity_id) else {
            continue;
        };
        if np
            .rastered
            .as_ref()
            .is_some_and(|done| done.matches(&np.base_name, inputs))
        {
            continue;
        }
        let want = RasterKey {
            text: np.base_name.clone(),
            ..inputs.clone()
        };
        // mob_hp diagnostic: fires only when the re-raster is driven by an hp
        // change (name/color/marker changes do not log). Paired with the
        // session-side "0x0E UPDATE_HP" line, this proves or breaks the
        // snapshot -> billboard link.
        if np.rastered.as_ref().map(|done| done.hp) != Some(want.hp) {
            tracing::info!(
                target: "mob_hp",
                id = np.entity_id,
                old = ?np.rastered.as_ref().map(|done| done.hp),
                new = ?want.hp,
                "nameplate re-raster (hp change)"
            );
        }
        let Some(mat_data) = materials.get_mut(&mat.0) else {
            continue;
        };
        let Some(handle) = mat_data.base_color_texture.clone() else {
            continue;
        };
        crate::perf_probe::note_nameplate_raster();
        let new_img = rasterize_plate(
            &font.0,
            &want.text,
            NAME_PX,
            want.color,
            want.hp,
            &want.markers,
            want.linkshell_tint,
            Some(&icons),
        );
        aspect.width = new_img.image.width();
        aspect.height = new_img.image.height();
        aspect.text_center_y_px = new_img.text_center_y_px;
        let _ = images.insert(&handle, new_img.image);
        np.rastered = Some(want.clone());
    }

    dbg_out.total = total;
    dbg_out.hide_self = hide_self;
    dbg_out.hidden_depth = hidden_depth;
    dbg_out.visible = visible_n;
    dbg_out.despawned = despawned;
}

/// The full raster input for one entity: retail's name colour, its icon
/// markers, the pearl tint those icons draw with, and — when the Retail+ gate
/// is on — the mob/pet HP bar.
fn raster_key_for(
    ent: &kuluu_snapshot::Entity,
    ctx: crate::nameplate_color::SelfContext<'_>,
    name_colors: &crate::nameplate_color::NameColorTable,
    show_mob_hp: bool,
) -> RasterKey {
    let color = crate::nameplate_color::name_color_choice(ent, ctx)
        .resolve(name_colors)
        .unwrap_or(NAMEPLATE_FALLBACK_COLOR);
    // `enhanced-mob-hp-under` is the compile-time half of this gate: without
    // it a persisted `mob_hp_under` from an enhanced build can never light a
    // bar in a plain one.
    #[cfg(feature = "enhanced-mob-hp-under")]
    let hp = if show_mob_hp {
        matches!(ent.kind, EntityKind::Mob | EntityKind::Pet)
            .then_some(ent.hp_pct)
            .flatten()
    } else {
        None
    };
    #[cfg(not(feature = "enhanced-mob-hp-under"))]
    let hp: Option<u8> = {
        let _ = show_mob_hp;
        None
    };
    RasterKey {
        text: String::new(),
        color: color_to_rgba8(color),
        hp,
        markers: crate::nameplate_marker::nameplate_markers(ent),
        linkshell_tint: color_to_rgba8(crate::nameplate_color::linkshell_tint(&ent.char_flags)),
    }
}

pub fn view_depth_to_fixed_point(view_depth_yalms: f32) -> Option<u32> {
    if view_depth_yalms <= MIN_VIEW_DEPTH_YALMS {
        return None;
    }
    let z_ndc = RETAIL_FAR_CLIP_YALMS / (RETAIL_FAR_CLIP_YALMS - RETAIL_NEAR_CLIP_YALMS)
        * (1.0 - RETAIL_NEAR_CLIP_YALMS / view_depth_yalms);
    if z_ndc < 0.0 {
        return None;
    }
    let depth_fixed = (z_ndc * NDC_DEPTH_FIXED_POINT_SCALE as f32) as u32;
    (depth_fixed <= MAX_DRAWABLE_DEPTH_FIXED).then_some(depth_fixed)
}

pub fn legibility_scale_for_view_depth(view_depth_yalms: f32) -> Option<f32> {
    scale_for_view_depth(view_depth_yalms).map(|s| s.max(NAMEPLATE_MIN_DEPTH_SCALE))
}

pub fn scale_for_view_depth(view_depth_yalms: f32) -> Option<f32> {
    let depth_fixed = view_depth_to_fixed_point(view_depth_yalms)?;
    if depth_fixed > FADE_END_DEPTH_FIXED {
        return None;
    }
    if depth_fixed < FADE_START_DEPTH_FIXED {
        return Some(1.0);
    }
    Some(
        (FADE_END_DEPTH_FIXED - depth_fixed) as f32
            / (FADE_END_DEPTH_FIXED - FADE_START_DEPTH_FIXED) as f32,
    )
}

/// The lift the quad center needs so the text line — not the center of the
/// icon-padded box — sits at the same anchor-relative height on every plate.
/// Zero for a bare plate (its box center already is the anchor, the
/// always-present HP strip balancing the outline pad), nonzero once an icon's
/// overhang grows the box asymmetrically: the name never moves, the overhang
/// just extends the box around it.
fn quad_center_rise(
    line_world: f32,
    line_px: f32,
    texture_height: u32,
    text_center_y_px: f32,
) -> f32 {
    let hp_strip = (HP_BAR_TOP_GAP_PX + HP_BAR_HEIGHT_PX) as f32;
    line_world * (hp_strip * 0.5 - (texture_height.max(1) as f32 * 0.5 - text_center_y_px))
        / line_px
}

pub fn target_alpha_pulse(frame: u32) -> f32 {
    let angle_deg = frame.wrapping_mul(TARGET_PULSE_DEGREES_PER_FRAME) % FULL_TURN_DEGREES;
    ((angle_deg as f32).to_radians().sin() * TARGET_PULSE_AMPLITUDE + TARGET_PULSE_BIAS)
        / TARGET_PULSE_DIVISOR
}

fn text_line_height_px(font: &FontArc, px: f32) -> u32 {
    let scaled = font.as_scaled(PxScale::from(px));
    (scaled.ascent() - scaled.descent()).ceil().max(1.0) as u32
}

// research/XIClient/.../CXiActorNameDraw.cpp:32-34 — an icon that is not the
// leftmost glyph draws at 0.8 and advances the pen by 0.625; the job-master
// tail draws at half scale and does not advance at all.
const ICON_TRAILING_SCALE: f32 = 0.8;
const ICON_TRAILING_ADVANCE: f32 = 0.625;
const ICON_TAIL_SCALE: f32 = 0.5;
// CXiActorNameDraw.cpp:366-367 — the tail glyph is nudged back over the star.
const ICON_TAIL_OFFSET_UNITS: f32 = -2.0;
// Retail boxes the status icons at 15 units against the 8-unit line
// (NAME_LINE_HEIGHT_UNITS), which lands near 1.5x the cap height on the bundled
// font and crowds the name. A deliberate legibility nudge — companion to
// NAMEPLATE_LEGIBILITY_SCALE — shrinking the whole icon run uniformly, so
// icon-to-icon proportions and advances stay retail's.
const ICON_DRAW_SCALE: f32 = 0.75;
// CXiActorNameDraw.cpp:623 — the icons' alpha runs through D3DTOP_MODULATE4X
// against a 0x80 diffuse, i.e. doubled.
const ICON_ALPHA_MODULATE: u16 = 2;

/// Where one icon glyph lands in the plate, in pixels relative to the text
/// box's top-left. `y_px` may be negative: retail's icons hang above the line.
struct IconPlacement {
    code: u8,
    x_px: f32,
    y_px: f32,
    width_px: f32,
    height_px: f32,
}

/// Retail's marker layout pass (CXiActorNameDraw.cpp:342-376), reduced to the
/// icon run that prefixes the name. Returns the placements and the pen advance
/// the name text starts after.
///
/// Retail lays icons and letters out in one run of shape-group cells, so an
/// icon's size is fixed against the *letter* cell (8x10 units against the
/// icon's 15x15), not against a line-height constant. `letter_px` is the same
/// ratio measured on our own font: one letter's advance and line box.
fn layout_icons(
    markers: &[u8],
    icons: Option<&crate::nameplate_icons::NameplateIcons>,
    letter_advance_px: f32,
    letter_box_px: f32,
) -> (Vec<IconPlacement>, f32) {
    let Some(icons) = icons else {
        return (Vec::new(), 0.0);
    };
    let Some(cell) = icons.letter_cell() else {
        return (Vec::new(), 0.0);
    };
    if cell.width_units <= 0.0 || cell.height_units <= 0.0 {
        return (Vec::new(), 0.0);
    }
    // One uniform unit, taken from the letter *advance*. Retail's glyph units
    // are square in cell space, so scaling each axis by its own ratio would
    // stretch a round icon into an egg on any font whose advance-to-line-box
    // aspect differs from retail's 8:10 cell. Sizing off the advance keeps the
    // icon-to-name width ratio retail has, and squares the icon.
    let unit_px = letter_advance_px / cell.width_units * ICON_DRAW_SCALE;
    let letter_center_units = cell.y_offset_units + cell.height_units / 2.0;

    let mut placements = Vec::with_capacity(markers.len());
    let mut pen = 0.0_f32;
    for (i, &code) in markers.iter().enumerate() {
        let Some(glyph) = icons.get(code) else {
            continue;
        };
        let is_tail = code == crate::nameplate_marker::glyph::JOB_MASTER_TAIL;
        let (scale, advance_scale, x_nudge, y_nudge) = if is_tail {
            (
                ICON_TAIL_SCALE,
                0.0,
                ICON_TAIL_OFFSET_UNITS - glyph.width_units,
                ICON_TAIL_OFFSET_UNITS,
            )
        } else if i == 0 {
            (1.0, 1.0, 0.0, 0.0)
        } else {
            (
                ICON_TRAILING_SCALE,
                ICON_TRAILING_ADVANCE,
                -glyph.width_units / 2.0,
                glyph.height_units / 2.0,
            )
        };
        let height_units = glyph.height_units * scale;
        // Centre the icon on the text line the way retail's cells do, then
        // apply retail's own nudge for this slot.
        let center_units = glyph.y_offset_units + y_nudge + height_units / 2.0;
        placements.push(IconPlacement {
            code,
            x_px: (pen + glyph.x_offset_units + x_nudge) * unit_px,
            y_px: letter_box_px / 2.0 + (center_units - letter_center_units) * unit_px
                - height_units * unit_px / 2.0,
            width_px: glyph.width_units * scale * unit_px,
            height_px: height_units * unit_px,
        });
        pen += glyph.width_units * advance_scale;
    }
    (placements, pen * unit_px)
}

/// The plate texture plus the texture-space y of its text line's center, so the
/// world transform can pin the line to the anchor however tall the icon overhang
/// makes the box.
struct PlateImage {
    image: Image,
    text_center_y_px: f32,
}

#[allow(clippy::too_many_arguments)]
fn rasterize_plate(
    font: &FontArc,
    text: &str,
    px: f32,
    color: [u8; 4],
    hp_pct: Option<u8>,
    markers: &[u8],
    linkshell_tint: [u8; 4],
    icons: Option<&crate::nameplate_icons::NameplateIcons>,
) -> PlateImage {
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let ascent = scaled.ascent();
    let line_h = text_line_height_px(font, px);

    let letter_advance_px = scaled.h_advance(scaled.glyph_id(char::from(REFERENCE_LETTER)));
    let (placements, icon_strip) = layout_icons(markers, icons, letter_advance_px, line_h as f32);
    // research/XIClient/.../ActorTelemetry.cpp:397-398 — retail separates the
    // marker run from the name with a space, so the icon never crowds the text.
    let separator_px = if placements.is_empty() {
        0.0
    } else {
        scaled.h_advance(scaled.glyph_id(' '))
    };
    let icon_strip_px = (icon_strip + separator_px).ceil().max(0.0) as u32;
    // Icons are taller than a text line and hang above and below it, so the
    // plate box grows to contain them.
    let icon_top_px = placements.iter().map(|p| p.y_px).fold(0.0_f32, f32::min);
    let icon_bottom_px = placements
        .iter()
        .map(|p| p.y_px + p.height_px)
        .fold(line_h as f32, f32::max);
    let top_extra_px = (-icon_top_px).max(0.0).ceil() as u32;
    let bottom_extra_px = (icon_bottom_px - line_h as f32).max(0.0).ceil() as u32;

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

    let pad = (OUTLINE_RADIUS_PX + BOLD_DILATE_PX + 1) as u32;
    let text_origin_x = pad + icon_strip_px;
    let text_origin_y = pad + top_extra_px;
    let width = (max_x.ceil() as u32).max(1) + 2 * pad + icon_strip_px;
    let text_height = line_h + 2 * pad + top_extra_px + bottom_extra_px;

    let hp_strip = HP_BAR_TOP_GAP_PX + HP_BAR_HEIGHT_PX;
    let height = text_height + hp_strip;

    let mut coverage = vec![0u8; (width * height) as usize];
    for glyph in glyphs {
        if let Some(outline_glyph) = scaled.outline_glyph(glyph) {
            let bb = outline_glyph.px_bounds();
            outline_glyph.draw(|gx, gy, c| {
                let px_y = bb.min.y as i32 + gy as i32 + text_origin_y as i32;
                if px_y < 0 || px_y >= text_height as i32 {
                    return;
                }
                let added = (c * 255.0).round().clamp(0.0, 255.0) as u8;
                for dx in 0..=BOLD_DILATE_PX {
                    let px_x = bb.min.x as i32 + gx as i32 + dx + text_origin_x as i32;
                    if px_x < 0 || px_x >= width as i32 {
                        continue;
                    }
                    let i = (px_y as u32 * width + px_x as u32) as usize;
                    coverage[i] = coverage[i].saturating_add(added);
                }
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
            // An opaque glyph pixel fully covers the outline ((1 - ta) = 0),
            // so the neighborhood scan is skippable there.
            if text_alpha < u8::MAX || color[3] < u8::MAX {
                let y0 = (y - r).max(0);
                let y1 = (y + r).min(text_h_i - 1);
                let x0 = (x - r).max(0);
                let x1 = (x + r).min(w_i - 1);
                'scan: for ny in y0..=y1 {
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
                            if outline_alpha == u8::MAX {
                                break 'scan;
                            }
                        }
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

    if let Some(icons) = icons {
        for placement in &placements {
            let Some(glyph) = icons.get(placement.code) else {
                continue;
            };
            // Only the linkshell pearl keeps a tint; retail forces every other
            // icon to the neutral diffuse (CXiActorNameDraw.cpp:404-407).
            let tint = if placement.code == crate::nameplate_marker::glyph::LINKSHELL {
                linkshell_tint
            } else {
                [u8::MAX; 4]
            };
            blit_icon(
                &mut pixels,
                width,
                text_height,
                &glyph.sprite,
                (pad as f32 + placement.x_px).round() as i32,
                (text_origin_y as f32 + placement.y_px).round() as i32,
                placement.width_px.round().max(1.0) as u32,
                placement.height_px.round().max(1.0) as u32,
                tint,
            );
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

    // Premultiply (in linear space) before the mip build. Box-filtering
    // straight alpha lets the RGB of transparent texels dilute minified
    // strokes — glyphs turned semi-transparent and took on whatever was behind
    // the plate (sky vs wall read differently, kuluu-iic9 follow-up). With
    // premultiplied texels the filter weights color by coverage, and
    // AlphaMode::Premultiplied keeps the GPU blend consistent with it.
    premultiply_linear(&mut pixels);

    // The plate rasters at NAME_PX but draws minified almost everywhere the
    // depth ramp is past its plateau; without mips that minification aliases
    // the glyph edges into sparkle. Clamp sampler: the HP strip touches the
    // texture edge, and a Repeat wrap would bleed it across.
    let mut image = crate::zone_texture::image_with_mips(
        pixels,
        width,
        height,
        crate::zone_texture::TextureQuality {
            mipmaps: true,
            anisotropy: 1,
        },
        false,
    );
    image.sampler = ImageSampler::linear();
    PlateImage {
        image,
        text_center_y_px: text_origin_y as f32 + line_h as f32 * 0.5,
    }
}

fn premultiply_linear(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        let a = px[3] as f32 / 255.0;
        if px[3] == u8::MAX {
            continue;
        }
        if px[3] == 0 {
            px[..3].fill(0);
            continue;
        }
        for c in &mut px[..3] {
            *c = crate::zone_texture::linear_to_srgb(crate::zone_texture::srgb_to_linear(*c) * a);
        }
    }
}

/// Scale one icon sprite into the plate and alpha-blend it over what is already
/// there. Retail filters these glyphs linearly
/// (CXiActorNameDraw.cpp:618-619), so the resample is bilinear.
#[allow(clippy::too_many_arguments)]
fn blit_icon(
    pixels: &mut [u8],
    width: u32,
    max_y: u32,
    sprite: &ffxi_dat::ui_element::UiSprite,
    dst_x: i32,
    dst_y: i32,
    dst_w: u32,
    dst_h: u32,
    tint: [u8; 4],
) {
    if sprite.width == 0 || sprite.height == 0 {
        return;
    }
    for row in 0..dst_h {
        let y = dst_y + row as i32;
        if y < 0 || y >= max_y as i32 {
            continue;
        }
        let sy = (row as f32 + 0.5) / dst_h as f32 * sprite.height as f32 - 0.5;
        for col in 0..dst_w {
            let x = dst_x + col as i32;
            if x < 0 || x >= width as i32 {
                continue;
            }
            let sx = (col as f32 + 0.5) / dst_w as f32 * sprite.width as f32 - 0.5;
            let texel = sample_bilinear(sprite, sx, sy);

            let src_a = (u16::from(texel[3]) * ICON_ALPHA_MODULATE).min(u16::from(u8::MAX)) as u32
                * u32::from(tint[3])
                / u32::from(u8::MAX);
            if src_a == 0 {
                continue;
            }
            let pi = ((y as u32 * width + x as u32) * 4) as usize;
            let dst_a = u32::from(pixels[pi + 3]);
            let out_a = src_a + dst_a * (u32::from(u8::MAX) - src_a) / u32::from(u8::MAX);
            for c in 0..3 {
                let src = u32::from(texel[c]) * u32::from(tint[c]) / u32::from(u8::MAX);
                let dst = u32::from(pixels[pi + c]);
                let blended = (src * src_a
                    + dst * dst_a * (u32::from(u8::MAX) - src_a) / u32::from(u8::MAX))
                    / out_a.max(1);
                pixels[pi + c] = blended.min(u32::from(u8::MAX)) as u8;
            }
            pixels[pi + 3] = out_a.min(u32::from(u8::MAX)) as u8;
        }
    }
}

fn sample_bilinear(sprite: &ffxi_dat::ui_element::UiSprite, x: f32, y: f32) -> [u8; 4] {
    let clamp = |v: f32, max: u32| v.clamp(0.0, (max - 1) as f32);
    let (x, y) = (clamp(x, sprite.width), clamp(y, sprite.height));
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = (
        (x0 + 1).min(sprite.width - 1),
        (y0 + 1).min(sprite.height - 1),
    );
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);

    let texel = |tx: u32, ty: u32| -> [f32; 4] {
        let i = ((ty * sprite.width + tx) * 4) as usize;
        [
            sprite.rgba[i] as f32,
            sprite.rgba[i + 1] as f32,
            sprite.rgba[i + 2] as f32,
            sprite.rgba[i + 3] as f32,
        ]
    };
    let (a, b, c, d) = (texel(x0, y0), texel(x1, y0), texel(x0, y1), texel(x1, y1));
    let mut out = [0u8; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bottom = c[i] + (d[i] - c[i]) * fx;
        out[i] = (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
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
mod icon_raster_tests {
    use super::*;
    use crate::nameplate_icons::NameplateIcons;
    use crate::nameplate_marker::glyph;

    const WHITE: [u8; 4] = [255, 255, 255, 255];

    fn font() -> FontArc {
        FontArc::try_from_slice(crate::ui_font::DEJAVU_SANS_MONO).expect("bundled font parses")
    }

    fn retail_icons() -> Option<NameplateIcons> {
        let root = ffxi_dat::archive::open_test_install()?;
        let codes = [
            glyph::PLAY_ONLINE,
            glyph::LINKDEAD,
            glyph::AWAY,
            glyph::SEEKING,
            glyph::LINKSHELL,
            glyph::BAZAAR,
            glyph::AUTO_PARTY,
            glyph::JOB_MASTER,
            glyph::JOB_MASTER_TAIL,
        ];
        let mut icons = NameplateIcons::default();
        let loaded = crate::ui_element_atlas::read_ui_dats(&root)
            .into_iter()
            .any(|(_, bytes)| icons.load_from_dat(&bytes, &codes));
        loaded.then_some(icons)
    }

    /// Premultiplied invariant: no texel may carry more color than coverage
    /// (decoded rgb <= alpha), else the GPU's One/OneMinusSrcAlpha blend turns
    /// the excess additive and the plate brightens over bright backgrounds.
    #[test]
    fn raster_is_premultiplied_no_texel_outshines_its_coverage() {
        let font = font();
        let plate = rasterize_plate(&font, "Test", NAME_PX, WHITE, Some(50), &[], WHITE, None);
        let data = plate.image.data.as_ref().expect("raster is CPU-side");
        let mip0 = (plate.image.width() * plate.image.height() * 4) as usize;
        let quantization_slack = 2.0 / 255.0;
        for px in data[..mip0].chunks_exact(4) {
            let a = px[3] as f32 / 255.0;
            for &c in &px[..3] {
                assert!(
                    crate::zone_texture::srgb_to_linear(c) <= a + quantization_slack,
                    "texel {px:?} carries color beyond its alpha"
                );
            }
        }
    }

    #[test]
    fn no_markers_leaves_the_plate_the_size_it_always_was() {
        let font = font();
        let bare = rasterize_plate(&font, "Test", NAME_PX, WHITE, None, &[], WHITE, None);
        let with_empty_icons =
            rasterize_plate(&font, "Test", NAME_PX, WHITE, None, &[], WHITE, None);
        assert_eq!(bare.image.width(), with_empty_icons.image.width());
        assert_eq!(bare.image.height(), with_empty_icons.image.height());
    }

    /// A bare plate's box center IS the anchor — pinned against the real
    /// raster, not invented numbers, so a layout change (outline pad, HP
    /// strip) that shifts every nameplate vertically cannot pass unnoticed.
    #[test]
    fn a_real_bare_raster_needs_no_quad_rise() {
        let font = font();
        let line_px = text_line_height_px(&font, NAME_PX) as f32;
        let bare = rasterize_plate(&font, "Test", NAME_PX, WHITE, None, &[], WHITE, None);
        assert_eq!(
            quad_center_rise(0.02, line_px, bare.image.height(), bare.text_center_y_px),
            0.0
        );
    }

    #[test]
    fn markers_without_a_loaded_glyph_set_are_a_no_op() {
        let font = font();
        let bare = rasterize_plate(&font, "Test", NAME_PX, WHITE, None, &[], WHITE, None);
        let unresolved = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[glyph::LINKSHELL],
            WHITE,
            None,
        );
        assert_eq!(
            (bare.image.width(), bare.image.height()),
            (unresolved.image.width(), unresolved.image.height()),
            "a plate must not reserve icon space it cannot draw"
        );
    }

    /// Gated on a retail install (self-skips). At ICON_DRAW_SCALE the pearl
    /// rides inside the text line; where its residual overhang still grows the
    /// box, quad_center_rise pins the line, so width is the only hard guarantee.
    #[test]
    fn real_dat_pearl_widens_the_plate() {
        let Some(icons) = retail_icons() else {
            return;
        };
        let font = font();
        let bare = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[],
            WHITE,
            Some(&icons),
        );
        let with_pearl = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[glyph::LINKSHELL],
            WHITE,
            Some(&icons),
        );
        assert!(
            with_pearl.image.width() > bare.image.width(),
            "the icon strip must widen the plate"
        );
        assert!(
            with_pearl.text_center_y_px >= bare.text_center_y_px,
            "an icon may push the line down inside the box, never lift it"
        );
    }

    /// Gated on a retail install (self-skips). The pearl is the one icon retail
    /// tints, so a coloured linkshell must actually change its pixels.
    #[test]
    fn real_dat_pearl_takes_the_linkshell_tint() {
        let Some(icons) = retail_icons() else {
            return;
        };
        let font = font();
        let red: [u8; 4] = [255, 0, 0, 255];
        let untinted = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[glyph::LINKSHELL],
            WHITE,
            Some(&icons),
        );
        let tinted = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[glyph::LINKSHELL],
            red,
            Some(&icons),
        );
        assert_eq!(untinted.image.width(), tinted.image.width());
        assert_ne!(
            untinted.image.data, tinted.image.data,
            "the pearl must respond to the linkshell colour"
        );

        let green_total: u64 = tinted
            .image
            .data
            .as_ref()
            .expect("raster is CPU-side")
            .chunks_exact(4)
            .map(|p| u64::from(p[1]) * u64::from(p[3]))
            .sum();
        let green_untinted: u64 = untinted
            .image
            .data
            .as_ref()
            .expect("raster is CPU-side")
            .chunks_exact(4)
            .map(|p| u64::from(p[1]) * u64::from(p[3]))
            .sum();
        assert!(
            green_total < green_untinted,
            "a red pearl must carry less green than an untinted one"
        );
    }

    /// Gated on a retail install (self-skips). Every other icon is forced to the
    /// neutral diffuse, so the tint must not reach it.
    #[test]
    fn real_dat_non_pearl_icons_ignore_the_linkshell_tint() {
        let Some(icons) = retail_icons() else {
            return;
        };
        let font = font();
        let plain = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[glyph::AWAY],
            WHITE,
            Some(&icons),
        );
        let with_tint = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[glyph::AWAY],
            [255, 0, 0, 255],
            Some(&icons),
        );
        assert_eq!(plain.image.data, with_tint.image.data);
    }

    /// Gated on a retail install (self-skips). The tail glyph does not advance
    /// the pen, so it must not widen the plate beyond the star alone.
    #[test]
    fn real_dat_job_master_tail_does_not_advance_the_pen() {
        let Some(icons) = retail_icons() else {
            return;
        };
        let font = font();
        let star = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[glyph::JOB_MASTER],
            WHITE,
            Some(&icons),
        );
        let star_with_tail = rasterize_plate(
            &font,
            "Test",
            NAME_PX,
            WHITE,
            None,
            &[glyph::JOB_MASTER, glyph::JOB_MASTER_TAIL],
            WHITE,
            Some(&icons),
        );
        assert_eq!(star.image.width(), star_with_tail.image.width());
        assert_ne!(
            star.image.data, star_with_tail.image.data,
            "the tail still draws, it just does not advance"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_line_pins_to_one_anchor_height_with_or_without_icons() {
        // (texture height, text-center y) pairs consistent with the raster
        // layout: an icon's overhang grows the box and pushes the line down
        // inside it by the same amount above and below.
        let line_world = 0.02;
        let line_px = 94.0;
        let bare = (130, 53.0);
        let pearl = (160, 68.0);
        // Top overhang exceeding the bottom: the box grows asymmetrically, so
        // a rise of exactly zero (a "pin the box center" impl) would move the
        // line. This is the fixture that makes the equality non-vacuous.
        let lopsided = (160, 75.0);
        // Where the line lands relative to the anchor: the quad rise plus the
        // line's own offset from the quad center.
        let line_height = |(h, tc): (u32, f32)| {
            quad_center_rise(line_world, line_px, h, tc)
                + line_world * (h as f32 * 0.5 - tc) / line_px
        };
        assert!((line_height(bare) - line_height(pearl)).abs() < 1e-9);
        assert!((line_height(bare) - line_height(lopsided)).abs() < 1e-9);
        assert!(
            quad_center_rise(line_world, line_px, lopsided.0, lopsided.1) > 0.0,
            "an upward overhang lifts the quad to keep the line put"
        );
        assert_eq!(
            quad_center_rise(line_world, line_px, bare.0, bare.1),
            0.0,
            "a bare plate keeps its box centered on the anchor"
        );
    }

    #[test]
    fn self_billboard_matches_known_self_id() {
        assert!(is_self_billboard(0xCAFE, Some(0xCAFE)));
    }

    #[test]
    fn other_entities_are_not_self() {
        assert!(!is_self_billboard(0x4242, Some(0xCAFE)));
    }

    #[test]
    fn unknown_self_id_matches_nothing() {
        assert!(!is_self_billboard(0xCAFE, None));
    }

    #[test]
    fn zero_self_id_is_unresolved_not_a_match() {
        assert!(!is_self_billboard(0, Some(0)));
    }

    #[test]
    fn self_plate_hidden_only_in_first_person() {
        assert!(self_plate_hidden(true, CameraMode::FirstPerson));
        assert!(!self_plate_hidden(true, CameraMode::Chase));
    }

    #[test]
    fn other_plates_visible_in_both_camera_modes() {
        assert!(!self_plate_hidden(false, CameraMode::FirstPerson));
        assert!(!self_plate_hidden(false, CameraMode::Chase));
    }

    const SCALE_EPSILON: f32 = 1e-5;
    // The deepest drawable plate: (0x1004 - 4095) / 80.
    const SCALE_FLOOR: f32 = 0.0625;

    #[test]
    fn scale_ramp_matches_retail_depth_table() {
        let table = [
            (3.0_f32, 1.0_f32),
            (5.0, 1.0),
            (5.38, 1.0),
            (5.5, 0.9875),
            (10.0, 0.5625),
            (20.0, 0.3125),
            (50.0, 0.1625),
            (100.0, 0.1125),
            (500.0, SCALE_FLOOR),
            (5000.0, SCALE_FLOOR),
        ];
        for (depth, want) in table {
            let got = scale_for_view_depth(depth).expect("depth is inside the drawable range");
            assert!(
                (got - want).abs() < SCALE_EPSILON,
                "depth {depth}: got {got}, want {want}"
            );
        }
    }

    #[test]
    fn plates_outrank_camera_anchored_transparents_at_any_populated_distance() {
        // Camera-anchored quads (lens flare, weather) rank ~0 in the
        // Transparent3d sort; a plate at view depth d ranks -d + bias. Server
        // entity visibility tops out near 50 yalms; 1000 bounds it with room.
        const FARTHEST_POPULATED_ENTITY_YALMS: f32 = 1000.0;
        const { assert!(NAMEPLATE_SORT_BIAS - FARTHEST_POPULATED_ENTITY_YALMS > 0.0) }
    }

    #[test]
    fn legibility_floor_caps_the_retail_shrink_without_touching_near_plates() {
        assert_eq!(legibility_scale_for_view_depth(3.0), Some(1.0));
        assert_eq!(
            legibility_scale_for_view_depth(10.0),
            scale_for_view_depth(10.0)
        );
        for depth in [20.0, 50.0, 100.0, 5000.0] {
            assert_eq!(
                legibility_scale_for_view_depth(depth),
                Some(NAMEPLATE_MIN_DEPTH_SCALE),
                "depth {depth} must sit on the legibility floor"
            );
        }
        assert_eq!(legibility_scale_for_view_depth(0.5), None);
        assert_eq!(legibility_scale_for_view_depth(1.0e6), None);
    }

    #[test]
    fn plateau_ends_at_the_fade_start_depth() {
        assert_eq!(
            view_depth_to_fixed_point(5.38),
            Some(FADE_START_DEPTH_FIXED - 1)
        );
        assert_eq!(view_depth_to_fixed_point(5.4), Some(FADE_START_DEPTH_FIXED));
        assert_eq!(scale_for_view_depth(5.38), Some(1.0));
        assert_eq!(scale_for_view_depth(5.4), Some(1.0));
        assert!(scale_for_view_depth(5.5).unwrap() < 1.0);
    }

    #[test]
    fn deepest_drawable_plate_sits_on_the_floor() {
        assert_eq!(
            view_depth_to_fixed_point(5000.0),
            Some(MAX_DRAWABLE_DEPTH_FIXED)
        );
        let floor = (FADE_END_DEPTH_FIXED - MAX_DRAWABLE_DEPTH_FIXED) as f32
            / (FADE_END_DEPTH_FIXED - FADE_START_DEPTH_FIXED) as f32;
        assert!((floor - SCALE_FLOOR).abs() < SCALE_EPSILON);
    }

    #[test]
    fn plates_inside_one_yalm_of_the_view_plane_are_dropped() {
        assert_eq!(scale_for_view_depth(1.0), None);
        assert_eq!(scale_for_view_depth(0.5), None);
        assert_eq!(scale_for_view_depth(-10.0), None);
    }

    #[test]
    fn plates_past_the_far_clip_are_dropped() {
        assert_eq!(scale_for_view_depth(1.0e6), None);
    }

    #[test]
    fn scale_never_grows_with_depth() {
        let mut prev = 1.0_f32;
        for step in 2..2000 {
            let depth = step as f32 * 0.5;
            let Some(scale) = scale_for_view_depth(depth) else {
                continue;
            };
            assert!(scale <= prev + SCALE_EPSILON, "depth {depth} scaled up");
            assert!(
                scale >= SCALE_FLOOR - SCALE_EPSILON,
                "depth {depth} below floor"
            );
            prev = scale;
        }
    }

    #[test]
    fn target_alpha_pulse_breathes_between_half_and_full() {
        let trough = (TARGET_PULSE_BIAS - TARGET_PULSE_AMPLITUDE) / TARGET_PULSE_DIVISOR;
        let crest = (TARGET_PULSE_BIAS + TARGET_PULSE_AMPLITUDE) / TARGET_PULSE_DIVISOR;
        for frame in 0..FULL_TURN_DEGREES {
            let alpha = target_alpha_pulse(frame);
            assert!((trough..=crest).contains(&alpha), "frame {frame}: {alpha}");
        }
        assert!(
            (target_alpha_pulse(0) - TARGET_PULSE_BIAS / TARGET_PULSE_DIVISOR).abs()
                < SCALE_EPSILON
        );
    }

    #[test]
    fn target_alpha_pulse_period_is_a_full_turn_of_frames() {
        fn gcd(a: u32, b: u32) -> u32 {
            if b == 0 {
                a
            } else {
                gcd(b, a % b)
            }
        }
        let period = FULL_TURN_DEGREES / gcd(FULL_TURN_DEGREES, TARGET_PULSE_DEGREES_PER_FRAME);
        for frame in 0..period {
            assert_eq!(
                target_alpha_pulse(frame),
                target_alpha_pulse(frame + period)
            );
        }
    }
}
