use bevy::asset::embedded_asset;
use bevy::pbr::{Material, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::components::InGameEntity;
use crate::graphics_settings::GraphicsSettings;
use crate::sun_moon::VanaSky;

// Max lf0x flare elements packed into the uniform chain. Real lens-flare sheets
// carry only a handful of meshes; extra slots stay inert (count gates them).
pub const MAX_FLARE_ELEMENTS: usize = 16;

// Each element renders at its native sprite-texel size times this factor (the retail
// flares draw larger than their low-res source). The per-element generator projection
// scale isn't parsed yet, so this is the one tunable that sets absolute flare size.
const LENS_FLARE_SCALE: f32 = 1.5;

#[derive(Clone, Debug, ShaderType)]
pub struct LensFlareUniform {
    // xyz = normalized world-space sun direction (projected to screen in the
    // shader against the render-frame view matrix — no CPU frame lag), w = intensity.
    pub sun_dir_intensity: Vec4,

    pub tint: Vec4,

    // x = element count, y = 1.0 when the lf0x sheet/texture is loaded (data-driven
    // chain) else 0.0 (analytic fallback), zw unused.
    pub flare_params: Vec4,

    // Per-element: x = offset fraction along sun->opposite; yz = native sprite
    // half-size in texels (sized to screen in the shader); w unused.
    pub offsets: [Vec4; MAX_FLARE_ELEMENTS],

    // Per-element UV sub-rect (u0,v0,u1,v1) into the lf0x texture.
    pub frame_uv: [Vec4; MAX_FLARE_ELEMENTS],
}

impl Default for LensFlareUniform {
    fn default() -> Self {
        Self {
            sun_dir_intensity: Vec4::new(0.0, 1.0, 0.0, 0.0),
            tint: Vec4::new(1.0, 0.95, 0.85, 1.0),
            flare_params: Vec4::ZERO,
            offsets: [Vec4::ZERO; MAX_FLARE_ELEMENTS],
            frame_uv: [Vec4::new(0.0, 0.0, 1.0, 1.0); MAX_FLARE_ELEMENTS],
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

// Per-zone lf0x sheet (offsets + UV frames), loaded from the zone DAT. None where the
// zone ships no lens-flare sheet (then the analytic halo/ghost/streak is used).
#[derive(Resource, Default, Clone)]
pub struct LensFlareSheet {
    pub offsets: Vec<f32>,
    pub frames: Vec<Vec4>,
}

impl Material for LensFlareMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/lens_flare.wgsl".into()
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
fn lens_flare_system(
    settings: Res<GraphicsSettings>,
    sky: Res<VanaSky>,
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

    // The painterly flare is the Vanilla-mode sun glare; Enhanced uses bloom.
    let vanilla = !settings.sky_embellishments_enabled();
    let sun_up = sky.sun_altitude > 0.0;
    if !vanilla || !sun_up {
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

    // World-space sun direction (camera-independent; same formula as
    // sun_moon::sun_moon_system). The shader projects it against the live view
    // matrix, so the flare can't lag the camera.
    let sun_angle = (sky.hour / 24.0) * 2.0 * std::f32::consts::PI - std::f32::consts::FRAC_PI_2;
    let sun_dir = Vec3::new(sun_angle.cos(), sun_angle.sin(), 0.25).normalize();

    let elev = (sky.sun_altitude / std::f32::consts::FRAC_PI_2).clamp(0.0, 1.0);
    let intensity = 0.55 + 0.45 * elev;

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
        mat.data.sun_dir_intensity = sun_dir.extend(intensity);
    }
}

// Load the zone's lf0x lens-flare sprite sheet (per-mesh offset fractions + UV frames
// + texture) from the zone DAT, mirroring moon_material::load_moon_sprite_sheet. When
// present the shader draws the data-driven additive chain; otherwise it falls back to
// the analytic halo/ghost/streak.
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
        .and_then(|loc| std::fs::read(loc.path_under(dat_root.root())).ok())
        .and_then(|bytes| ffxi_dat::sprite_sheet::extract_lens_flare_sheet(&bytes));

    let mat = flare_q.single().ok().and_then(|m| mats.get_mut(&m.0));

    let Some(sheet) = sheet else {
        *sheet_res = LensFlareSheet::default();
        if let Some(mut mat) = mat {
            mat.flare_tex = None;
            mat.data.flare_params.x = 0.0;
            mat.data.flare_params.y = 0.0;
        }
        return;
    };

    let n = sheet.offsets.len().min(MAX_FLARE_ELEMENTS);
    let offsets: Vec<f32> = sheet.offsets[..n].to_vec();
    let frames: Vec<Vec4> = sheet.frames[..n]
        .iter()
        .map(|f| Vec4::new(f.u0, f.v0, f.u1, f.v1))
        .collect();

    let image = Image::new(
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
    let mut image = image;
    image.sampler = ImageSampler::linear();
    let handle = images.add(image);

    *sheet_res = LensFlareSheet {
        offsets: offsets.clone(),
        frames: frames.clone(),
    };

    let (tex_w, tex_h) = (sheet.texture.width as f32, sheet.texture.height as f32);
    if let Some(mut mat) = mat {
        mat.flare_tex = Some(handle);
        mat.data.flare_params.x = n as f32;
        mat.data.flare_params.y = 1.0;
        mat.data.flare_params.z = LENS_FLARE_SCALE;
        for i in 0..n {
            let f = frames[i];
            let half = Vec2::new((f.z - f.x) * tex_w, (f.w - f.y) * tex_h) * 0.5;
            mat.data.offsets[i] = Vec4::new(offsets[i], half.x, half.y, 0.0);
            mat.data.frame_uv[i] = f;
        }
    }
}

pub struct LensFlarePlugin;

impl Plugin for LensFlarePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "lens_flare.wgsl");
        app.init_resource::<LensFlareSheet>()
            .add_plugins(MaterialPlugin::<LensFlareMaterial>::default())
            .add_systems(Startup, spawn_lens_flare)
            .add_systems(Update, load_lens_flare_sheet)
            .add_systems(
                Update,
                lens_flare_system.after(crate::sun_moon::sun_moon_system),
            );
    }
}
