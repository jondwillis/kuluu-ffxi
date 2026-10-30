use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::components::InGameEntity;
use crate::sun_moon::VanaSky;

// Max lf0x flare elements packed into the uniform chain. The longest retail chain measured
// across the zone DATs is 26 meshes (lf03, file 201), so 32 covers every shipped sheet with
// headroom; extra slots stay inert (count gates them).
pub const MAX_FLARE_ELEMENTS: usize = 32;

// research/xim ZoneDrawer.kt `scale = Vector3f(width/32, height/32, 1)` — a flare
// mesh's local units are screen fractions of 1/32, so a quad spanning 32 units covers the
// whole screen. Dividing the parsed quad half-extent by this yields the element's half-size
// in screen-UV directly, which is what the shader consumes.
const LENS_FLARE_SCREEN_UNITS: f32 = 32.0;

#[derive(Clone, Debug, ShaderType)]
pub struct LensFlareUniform {
    // xyz = normalized world-space sun direction (projected to screen in the
    // shader against the render-frame view matrix — no CPU frame lag), w unused.
    pub sun_dir: Vec4,

    /// The stage-1 TEXTUREFACTOR F. In retail this is the lf0x particle's own colour
    /// (research/xim ZoneDrawer.kt drawLensFlare `effectColor = effect.textureFactor`), which the
    /// generator's time-of-day curves drive; until those generators run (kuluu-b98u) the
    /// neutral F is the honest stand-in, leaving the sheet's own colours in charge.
    pub texture_factor: Vec4,

    // x = element count, yz unused, w = sun visibility [0,1] from SunOcclusion.
    pub flare_params: Vec4,

    // Per-element: x = offset fraction along sun->opposite; yz = half-size in screen-UV;
    // w unused.
    pub offsets: [Vec4; MAX_FLARE_ELEMENTS],

    // Per-element UV sub-rect (u0,v0,u1,v1) into the lf0x texture.
    pub frame_uv: [Vec4; MAX_FLARE_ELEMENTS],

    /// Per-element stage-0 D: the mesh's authored vertex colour over
    /// [`ffxi_dat::d3m::VERTEX_COLOR_DIVISOR`], which is where the chain's core/halo/ghost
    /// intensity ramp lives.
    pub element_color: [Vec4; MAX_FLARE_ELEMENTS],
}

impl Default for LensFlareUniform {
    fn default() -> Self {
        Self {
            sun_dir: Vec4::new(0.0, 1.0, 0.0, 0.0),
            texture_factor: Vec4::ONE,
            flare_params: Vec4::ZERO,
            offsets: [Vec4::ZERO; MAX_FLARE_ELEMENTS],
            frame_uv: [Vec4::new(0.0, 0.0, 1.0, 1.0); MAX_FLARE_ELEMENTS],
            element_color: [Vec4::ONE; MAX_FLARE_ELEMENTS],
        }
    }
}

#[derive(Asset, AsBindGroup, Clone, Debug, TypePath, Default)]
pub struct LensFlareMaterial {
    #[uniform(0)]
    pub data: LensFlareUniform,

    #[texture(1)]
    #[sampler(2)]
    pub flare_tex: Option<Handle<Image>>,
}

// Fraction of the sun left unoccluded, written by a client-side disc-tap raycast
// against the zone collision BVH plus the drawn actors' pose-tracked Aabbs
// (kuluu sun_occlusion.rs). Consumers without those (wasm viewer, headless
// example) keep the default: fully visible.
#[derive(Resource, Clone, Copy, Debug)]
pub struct SunOcclusion {
    pub visibility: f32,
}

impl Default for SunOcclusion {
    fn default() -> Self {
        Self { visibility: 1.0 }
    }
}

// Per-zone lf0x sheet (offsets + UV frames), loaded from the zone DAT. Empty where the
// zone ships no lens-flare sheet, which is where retail draws no flare at all.
#[derive(Resource, Default, Clone)]
pub struct LensFlareSheet {
    pub offsets: Vec<f32>,
    pub frames: Vec<Vec4>,
}

// One drawable element of the chain, in the units the shader consumes.
struct FlareElement {
    offset: f32,
    half: Vec2,
    frame_uv: Vec4,
    color: Vec4,
}

impl Material for LensFlareMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://kuluu_render/lens_flare.wesl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
}

#[derive(Component)]
pub struct LensFlareQuad;

const FLARE_DISTANCE: f32 = 0.2;

// The quad is placed from the camera's current transform (lag-free, since the
// projection now happens in the shader), but oversize it so it still covers the
// whole frustum during a fast camera swing.
const FLARE_OVERSCAN: f32 = 1.15;

fn spawn_lens_flare(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<LensFlareMaterial>>,
) {
    let quad = meshes.add(Rectangle::new(1.0, 1.0));
    let material = materials.add(LensFlareMaterial::default());
    commands.spawn((
        InGameEntity,
        LensFlareQuad,
        Mesh3d(quad),
        MeshMaterial3d(material),
        Transform::default(),
        Visibility::Hidden,
        bevy::light::NotShadowCaster,
        bevy::light::NotShadowReceiver,
    ));
}

#[allow(clippy::type_complexity)]
pub fn lens_flare_system(
    sky: Res<VanaSky>,
    sheet: Res<LensFlareSheet>,
    occlusion: Res<SunOcclusion>,
    cam_q: Query<
        (&Transform, &Camera, &Projection),
        (With<crate::camera::OperatorCamera>, Without<LensFlareQuad>),
    >,
    mut flare_q: Query<
        (
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<LensFlareMaterial>,
        ),
        With<LensFlareQuad>,
    >,
    mut mats: ResMut<Assets<LensFlareMaterial>>,
) {
    let Ok((mut flare_xf, mut vis, flare_mat)) = flare_q.single_mut() else {
        return;
    };

    // Retail's flare is the lf0x chain the zone ships; where a zone ships none, retail
    // draws no flare at all.
    let data_driven = !sheet.frames.is_empty();
    let sun_up = sky.sun_altitude > 0.0;
    if !sun_up || occlusion.visibility <= 0.0 || !data_driven {
        *vis = Visibility::Hidden;
        return;
    }

    let Ok((cam_t, camera, proj)) = cam_q.single() else {
        *vis = Visibility::Hidden;
        return;
    };
    let Some(vp) = camera.logical_viewport_size() else {
        *vis = Visibility::Hidden;
        return;
    };

    // World-space sun direction (camera-independent). The shader projects it
    // against the live view matrix, so the flare can't lag the camera.
    let sun_dir = crate::sun_moon::sun_direction(sky.hour);

    let fov_y = match proj {
        Projection::Perspective(p) => p.fov,
        _ => std::f32::consts::FRAC_PI_3,
    };
    let aspect = vp.x / vp.y.max(1.0);
    let height = 2.0 * FLARE_DISTANCE * (fov_y * 0.5).tan();
    let width = height * aspect;

    flare_xf.translation = cam_t.translation + cam_t.forward() * FLARE_DISTANCE;
    flare_xf.rotation = cam_t.rotation;
    flare_xf.scale = Vec3::new(width, height, 1.0) * FLARE_OVERSCAN;
    *vis = Visibility::Inherited;

    if let Some(mut mat) = mats.get_mut(&flare_mat.0) {
        mat.data.sun_dir = sun_dir.extend(0.0);
        mat.data.flare_params.w = occlusion.visibility;
    }
}

// Load the zone's lf0x lens-flare sprite sheet (per-mesh offset fractions + UV frames
// + texture) from the zone DAT, mirroring moon_material::load_moon_sprite_sheet.
#[allow(clippy::type_complexity)]
fn load_lens_flare_sheet(
    scene_state: Res<crate::snapshot::SceneState>,
    dat_root: Option<Res<crate::moon_material::MoonDatRoot>>,
    mut sheet_res: ResMut<LensFlareSheet>,
    mut images: ResMut<Assets<Image>>,
    flare_q: Query<&MeshMaterial3d<LensFlareMaterial>, With<LensFlareQuad>>,
    mut mats: ResMut<Assets<LensFlareMaterial>>,
    mut loaded_zone: Local<Option<Option<u32>>>,
) {
    use bevy::asset::RenderAssetUsages;
    use bevy::image::ImageSampler;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    let current = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    if *loaded_zone == Some(current) {
        return;
    }
    let Some(dat_root) = dat_root.and_then(|r| r.0.clone()) else {
        return;
    };
    *loaded_zone = Some(current);

    let sheet = current
        .and_then(|file_id| dat_root.resolve(file_id).ok())
        .and_then(|loc| std::fs::read(loc.path_under(&dat_root)).ok())
        .and_then(|bytes| ffxi_dat::sprite_sheet::extract_lens_flare_sheet(&bytes));

    let mat = flare_q.single().ok().and_then(|m| mats.get_mut(&m.0));

    let Some(sheet) = sheet else {
        *sheet_res = LensFlareSheet::default();
        if let Some(mut mat) = mat {
            mat.flare_tex = None;
            mat.data.flare_params.x = 0.0;
        }
        return;
    };

    // Retail multiplies each element's quad by the screen-derived draw scale, so a mesh with
    // no extent covers no pixels — the shipped chains are padded with them (20 of lf03's 26).
    // Dropping them here keeps the shader's `(uv - pos) / half` off a division by zero.
    let drawable: Vec<FlareElement> = sheet
        .offsets
        .iter()
        .zip(sheet.half_extents.iter())
        .zip(sheet.frames.iter().zip(sheet.colors.iter()))
        .filter_map(|((offset, half), (frame, color))| {
            let half = Vec2::from_array(*half) / LENS_FLARE_SCREEN_UNITS;
            (half.x > 0.0 && half.y > 0.0).then(|| FlareElement {
                offset: *offset,
                half,
                frame_uv: Vec4::new(frame.u0, frame.v0, frame.u1, frame.v1),
                color: Vec4::new(
                    color[0] as f32,
                    color[1] as f32,
                    color[2] as f32,
                    color[3] as f32,
                ) / ffxi_dat::d3m::VERTEX_COLOR_DIVISOR,
            })
        })
        .collect();
    if drawable.len() > MAX_FLARE_ELEMENTS {
        warn!(
            elements = drawable.len(),
            cap = MAX_FLARE_ELEMENTS,
            "lens flare: chain truncated"
        );
    }
    let n = drawable.len().min(MAX_FLARE_ELEMENTS);

    // No ffxi_alpha_remap here, unlike the moon sheet: the stage-1 MODULATE4X below is retail's
    // own compensation for the half-range alpha convention (lf03 peaks at 0x80, and
    // 4 x 0.78 x 0.5 saturates the core exactly), so remapping first would double it.
    let mut image = Image::new(
        Extent3d {
            width: sheet.texture.width,
            height: sheet.texture.height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        sheet.texture.rgba.clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = ImageSampler::linear();
    let handle = images.add(image);

    *sheet_res = LensFlareSheet {
        offsets: drawable[..n].iter().map(|e| e.offset).collect(),
        frames: drawable[..n].iter().map(|e| e.frame_uv).collect(),
    };

    info!(
        elements = n,
        authored = sheet.offsets.len(),
        tex = format!("{}×{}", sheet.texture.width, sheet.texture.height),
        "lens flare: loaded zone lf0x sheet"
    );

    if let Some(mut mat) = mat {
        mat.flare_tex = Some(handle);
        mat.data.flare_params.x = n as f32;
        for (i, e) in drawable[..n].iter().enumerate() {
            mat.data.offsets[i] = Vec4::new(e.offset, e.half.x, e.half.y, 0.0);
            mat.data.frame_uv[i] = e.frame_uv;
            mat.data.element_color[i] = e.color;
        }
    }
}

pub struct LensFlarePlugin;

impl Plugin for LensFlarePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "lens_flare.wesl");
        app.init_resource::<LensFlareSheet>()
            .init_resource::<SunOcclusion>()
            .add_plugins(MaterialPlugin::<LensFlareMaterial>::default())
            .add_systems(Startup, spawn_lens_flare)
            .add_systems(Update, load_lens_flare_sheet)
            .add_systems(
                Update,
                lens_flare_system.after(crate::sun_moon::sun_moon_system),
            );
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wgsl_sun_sky_radius_matches_sun_moon() {
        let wgsl = include_str!("lens_flare.wesl");
        let expected = format!(
            "const SUN_SKY_RADIUS: f32 = {:?};",
            crate::sun_moon::SKY_RADIUS
        );
        assert!(
            wgsl.contains(&expected),
            "lens_flare.wesl SUN_SKY_RADIUS drifted from sun_moon::SKY_RADIUS ({expected})"
        );
    }
}
