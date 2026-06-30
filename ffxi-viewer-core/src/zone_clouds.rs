#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::fs;

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use ffxi_dat::chunk::{walk_tree, ChunkNode};
use ffxi_dat::generator::{CloudGeneratorDef, Generator};
use ffxi_dat::mmb::{self, parse_models};
use ffxi_dat::particle_gen::KeyFrameTrack;
use ffxi_dat::texture::{decode_texture, extract_texture_name};
use ffxi_dat::weather::{weather_type_id, WeatherTypeId};
use ffxi_dat::{ChunkKind, DatRoot};

use crate::components::InGameEntity;
use crate::ffxi_zone_material::FfxiZoneMaterial;
use crate::graphics_settings::{GraphicsSettings, SkyStyle};
use crate::zone_texture::{decoded_texture_to_image, TextureQuality};

// research/xim ParticleGeneratorAttachment.kt:46-62 (Sun pos = getSunPosition +
// camera; None = camera-follow) + EnvironmentManager.kt:453-515 (updateWeatherEffects
// reads weat/<type>/). cld1/cld2 follow the camera at their base offset; sun1 sits
// on the sun direction. Distance the sun-attached layer rides out along sun_dir;
// matches the SKY_RADIUS the SunDisc uses so cloud/sun share one dome scale.
const SUN_LAYER_DISTANCE: f32 = 4000.0;

// The authored 0x0F canopy scale (research/xim ParticleGeneratorParser.kt) varies per
// zone/weather and often leaves the canopy rim nearer than the terrain, so the cloud
// sheet draped over zone geometry. Enforce a minimum rim radius (just inside
// skybox::SKYBOX_RADIUS=5500) so terrain is always nearer and depth-occludes the
// clouds — the same "sky is the farthest thing" rule the skybox dome relies on.
const CLOUD_MIN_RIM: f32 = 4000.0;

// research/xim EnvironmentManager.kt:351-369 switchWeather default 3.33s cross-fade
// between the old and new weat/<type>/ effect sets on a 0x0057 weather change.
const WEATHER_FADE_SECS: f32 = 3.33;

#[derive(Component)]
pub struct CloudMesh;

// research/xim ParticleUpdaters.kt:172-183 ClockValueUpdater: the cloud/sun mesh RGB
// (kcr1/kcg1/kcb1, ksr1/ksg1/ksb1) and alpha multiplier are 0x19 keyframe curves
// sampled at the Vana full-day fraction. White / unit-alpha defaults are no-ops.
#[derive(Clone, Default)]
struct CloudColorTracks {
    r: Option<KeyFrameTrack>,
    g: Option<KeyFrameTrack>,
    b: Option<KeyFrameTrack>,
    alpha: Option<KeyFrameTrack>,
}

impl CloudColorTracks {
    fn sample(&self, day_fraction: f32) -> Vec4 {
        Vec4::new(
            self.r.as_ref().map_or(1.0, |t| t.sample(day_fraction)),
            self.g.as_ref().map_or(1.0, |t| t.sample(day_fraction)),
            self.b.as_ref().map_or(1.0, |t| t.sample(day_fraction)),
            self.alpha.as_ref().map_or(1.0, |t| t.sample(day_fraction)),
        )
    }
}

// A cloud/sun layer's fade across a weather change: the old set fades 1->0 while the
// incoming set fades 0->1 over WEATHER_FADE_SECS (xim switchWeather).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FadeDir {
    In,
    Out,
}

#[derive(Component)]
struct CloudFade {
    dir: FadeDir,
    elapsed: f32,
}

impl CloudFade {
    fn alpha(&self) -> f32 {
        let t = (self.elapsed / WEATHER_FADE_SECS).clamp(0.0, 1.0);
        match self.dir {
            FadeDir::In => t,
            FadeDir::Out => 1.0 - t,
        }
    }
    fn finished_out(&self) -> bool {
        self.dir == FadeDir::Out && self.elapsed >= WEATHER_FADE_SECS
    }
}

// research/xim ParticleGeneratorAttachment.kt:46-62: the generator's attachment
// decides whether a layer rides the camera (None) or the sun direction (Sun).
#[derive(Clone, Copy, PartialEq, Eq)]
enum CloudAttach {
    Camera,
    Sun,
}

#[derive(Component)]
struct CloudLayer {
    attach: CloudAttach,
    // FFXI-space base offset added camera-relative (cld1 [0,0,0] / cld2 [0,30,0]).
    base_position: Vec3,
    tracks: CloudColorTracks,
}

// Mesh + material handles + placement extracted for one weat/<type>/ cloud or sun
// generator. Spawned as CloudMesh entities; tracked so a zone/weather change can
// despawn and rebuild them (zone change keeps AppPhase::InGame, so the OnExit
// teardown never runs — see MEMORY zone-change-not-clean-lifecycle).
struct CloudLayerBuild {
    mesh: Handle<Mesh>,
    material: Handle<FfxiZoneMaterial>,
    attach: CloudAttach,
    base_position: Vec3,
    scale: Vec3,
    tracks: CloudColorTracks,
}

#[derive(Resource, Default)]
struct ZoneCloudState {
    // (zone_id, weather fourcc) the spawned entities currently mirror.
    key: Option<(u16, WeatherTypeId)>,
    entities: Vec<Entity>,
}

fn ffxi_to_bevy_basis() -> Quat {
    Quat::from_rotation_x(std::f32::consts::PI)
}

// Find the `weat/<type>` directory node for the requested weather type anywhere in
// the zone dir tree (it lives under the zone root dir, e.g. f_ro/weat/clod).
fn find_weat_type<'a>(node: &'a ChunkNode<'a>, want: WeatherTypeId) -> Option<&'a ChunkNode<'a>> {
    for child in &node.children {
        if child.chunk.kind != 0x01 {
            continue;
        }
        if child.chunk.name == *b"weat" {
            for type_node in &child.children {
                if type_node.chunk.kind == 0x01 && type_node.chunk.name == want {
                    return Some(type_node);
                }
            }
        }
        if let Some(found) = find_weat_type(child, want) {
            return Some(found);
        }
    }
    None
}

fn resolve_mesh_chunk<'a>(dir: &'a ChunkNode<'a>, id: [u8; 4]) -> Option<&'a ChunkNode<'a>> {
    dir.children
        .iter()
        .find(|c| c.chunk.kind == ChunkKind::Mmb as u8 && c.chunk.name == id)
}

fn resolve_keyframe(dir: &ChunkNode, id: Option<[u8; 4]>) -> Option<KeyFrameTrack> {
    let id = id?;
    dir.children
        .iter()
        .find(|c| c.chunk.kind == ChunkKind::KeyFrame as u8 && c.chunk.name == id)
        .map(|c| KeyFrameTrack::parse(c.chunk.data))
}

fn resolve_color_tracks(dir: &ChunkNode, def: &CloudGeneratorDef) -> CloudColorTracks {
    CloudColorTracks {
        r: resolve_keyframe(dir, def.color_r_track),
        g: resolve_keyframe(dir, def.color_g_track),
        b: resolve_keyframe(dir, def.color_b_track),
        alpha: resolve_keyframe(dir, def.alpha_mult_track),
    }
}

// Returns the assembled mesh plus its horizontal half-extent (max |x|,|z| over all
// verts) so the caller can scale the canopy rim out to a fixed sky radius.
fn build_mesh(decrypted: &[u8]) -> Option<(Mesh, f32)> {
    let models = parse_models(decrypted);
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut colors: Vec<[f32; 4]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut half_xz = 0.0f32;

    for m in &models {
        if m.vertices.is_empty() || m.indices.is_empty() {
            continue;
        }
        let base = positions.len() as u32;
        let vert_count = m.vertices.len() as u16;
        for v in &m.vertices {
            half_xz = half_xz.max(v.pos[0].abs()).max(v.pos[2].abs());
            positions.push(v.pos);
            normals.push(v.normal);
            uvs.push(v.uv);
            colors.push([
                v.rgba[0] as f32 / 128.0,
                v.rgba[1] as f32 / 128.0,
                v.rgba[2] as f32 / 128.0,
                v.rgba[3] as f32 / 128.0,
            ]);
        }
        for t in m.indices.chunks_exact(3) {
            if t[0] < vert_count && t[1] < vert_count && t[2] < vert_count {
                indices.push(base + t[0] as u32);
                indices.push(base + t[1] as u32);
                indices.push(base + t[2] as u32);
            }
        }
    }

    if positions.is_empty() || indices.is_empty() {
        return None;
    }

    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    mesh.insert_indices(Indices::U32(indices));
    Some((mesh, half_xz))
}

fn build_cloud_layers(
    weat_type: &ChunkNode,
    quality: TextureQuality,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<FfxiZoneMaterial>,
) -> Vec<CloudLayerBuild> {
    // Decode the 0x20 textures in this weat/<type> dir, keyed by name, with a
    // first-texture fallback (mirrors the MMB texture-pool resolution).
    let mut tex_by_name: HashMap<String, Handle<Image>> = HashMap::new();
    let mut first_texture: Option<Handle<Image>> = None;
    for c in &weat_type.children {
        if c.chunk.kind != ChunkKind::Img as u8 {
            continue;
        }
        if let Ok(tex) = decode_texture(c.chunk.data) {
            let handle = images.add(decoded_texture_to_image(&tex, quality));
            if first_texture.is_none() {
                first_texture = Some(handle.clone());
            }
            if let Some(name) = extract_texture_name(c.chunk.data) {
                if !name.is_empty() {
                    tex_by_name.insert(name, handle.clone());
                }
            }
        }
    }

    let mut out = Vec::new();
    for c in &weat_type.children {
        if c.chunk.kind != ChunkKind::Generator as u8 {
            continue;
        }
        // Only cld1/cld2 (camera clouds) and sun1 (sun-attached) are placed here;
        // moon/star/lens-flare generators live in their own subdirs/pillars.
        let is_cloud = c.chunk.name == *b"cld1" || c.chunk.name == *b"cld2";
        let is_sun = c.chunk.name == *b"sun1";
        if !is_cloud && !is_sun {
            continue;
        }
        let Ok(Some(def)) = Generator::parse_cloud_generator(c.chunk.name, c.chunk.data) else {
            continue;
        };
        let attach = if def.is_sun_attached() {
            CloudAttach::Sun
        } else if def.is_camera_cloud() {
            CloudAttach::Camera
        } else {
            continue;
        };

        let Some(mesh_chunk) = resolve_mesh_chunk(weat_type, def.linked_id) else {
            continue;
        };
        let Ok(decrypted) = mmb::decrypt(mesh_chunk.chunk.data) else {
            continue;
        };
        let Some((mesh, half_xz)) = build_mesh(&decrypted) else {
            continue;
        };

        let texture = tex_by_name
            .get(&id_str(def.linked_id))
            .or(first_texture.as_ref())
            .cloned();
        let material = materials.add(cloud_material(texture));

        out.push(CloudLayerBuild {
            mesh: meshes.add(mesh),
            material,
            attach,
            base_position: def_base_to_vec(&def),
            scale: layer_scale(attach, &def, half_xz),
            tracks: resolve_color_tracks(weat_type, &def),
        });
    }
    out
}

fn def_base_to_vec(def: &CloudGeneratorDef) -> Vec3 {
    Vec3::new(
        def.base_position[0],
        def.base_position[1],
        def.base_position[2],
    )
}

// Sun discs are placed out along sun_dir at SUN_LAYER_DISTANCE, so their authored
// 0x0F scale is used as-is. Camera-follow cloud canopies sit on the camera, so their
// rim (half_xz * authored scale) is pushed out to at least CLOUD_MIN_RIM — keeping the
// authored aspect ratio — so distant terrain stays nearer and occludes them.
fn layer_scale(attach: CloudAttach, def: &CloudGeneratorDef, half_xz: f32) -> Vec3 {
    let authored = Vec3::from_array(def.scale);
    match attach {
        CloudAttach::Sun => authored,
        CloudAttach::Camera => {
            let rim = half_xz * authored.x.max(authored.z);
            let factor = if rim > 1.0 {
                (CLOUD_MIN_RIM / rim).max(1.0)
            } else {
                1.0
            };
            authored * factor
        }
    }
}

fn id_str(id: [u8; 4]) -> String {
    id.iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect::<String>()
        .trim()
        .to_string()
}

// FfxiZoneMaterial with the 2x overbright vertex-lit path; clouds blend over the
// sky dome so they use AlphaMode::Blend with the texture's own alpha.
fn cloud_material(texture: Option<Handle<Image>>) -> FfxiZoneMaterial {
    let has_texture = if texture.is_some() { 1.0 } else { 0.0 };
    FfxiZoneMaterial {
        lighting: crate::skinned_ffxi_material::FfxiLightingUniform::default(),
        base_color_texture: texture,
        material_flags: crate::skinned_ffxi_material::FfxiMaterialFlags {
            flags: Vec4::new(has_texture, 1.0, 0.0, 0.0),
        },
        tint: Vec4::ONE,
        alpha_mode: AlphaMode::Blend,
    }
}

#[allow(clippy::too_many_arguments)]
fn rebuild_zone_clouds(
    scene_state: Res<crate::snapshot::SceneState>,
    current_weather: Res<crate::weather_fx::CurrentWeather>,
    mut state: ResMut<ZoneCloudState>,
    settings: Res<GraphicsSettings>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
) {
    let zone_id = scene_state.snapshot.zone_id;
    let want = weather_type_id(current_weather.0.map(|w| w as u16).unwrap_or(0));
    let key = zone_id.map(|z| (z, want));
    if key == state.key {
        return;
    }

    // A weather change within the same zone cross-fades the old set out (xim
    // switchWeather); a zone change despawns immediately — the old weat/ set
    // belongs to a different DAT and the camera teleports, so a fade would smear.
    let same_zone = match (state.key, key) {
        (Some((prev_zone, _)), Some((next_zone, _))) => prev_zone == next_zone,
        _ => false,
    };
    for e in state.entities.drain(..) {
        if same_zone {
            commands.entity(e).insert(CloudFade {
                dir: FadeDir::Out,
                elapsed: 0.0,
            });
        } else {
            commands.entity(e).despawn();
        }
    }
    state.key = key;

    let Some(zone_id) = zone_id else {
        return;
    };
    let Some(file_id) = ffxi_dat::zone_dat::zone_id_to_mzb_file_id(zone_id) else {
        return;
    };
    let Ok(root) = DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(location) = root.resolve(file_id) else {
        return;
    };
    let Ok(bytes) = fs::read(location.path_under(root.root())) else {
        return;
    };

    let tree = walk_tree(&bytes);
    let weat_type = match find_weat_type(&tree, want).or_else(|| find_weat_type(&tree, *b"fine")) {
        Some(n) => n,
        None => return,
    };

    let quality = TextureQuality {
        mipmaps: settings.texture_filtering.mipmaps(),
        anisotropy: settings.texture_filtering.anisotropy(),
    };
    let layers = build_cloud_layers(weat_type, quality, &mut meshes, &mut images, &mut materials);

    let vanilla = settings.sky_style() == SkyStyle::Vanilla;
    for layer in layers {
        let visibility = if vanilla {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        let e = commands
            .spawn((
                InGameEntity,
                CloudMesh,
                CloudLayer {
                    attach: layer.attach,
                    base_position: layer.base_position,
                    tracks: layer.tracks,
                },
                CloudFade {
                    dir: FadeDir::In,
                    elapsed: if same_zone { 0.0 } else { WEATHER_FADE_SECS },
                },
                Mesh3d(layer.mesh),
                MeshMaterial3d(layer.material),
                Transform::from_rotation(ffxi_to_bevy_basis()).with_scale(layer.scale),
                visibility,
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .id();
        state.entities.push(e);
    }

    if !state.entities.is_empty() {
        info!(
            zone_id,
            type_ = id_str(want),
            count = state.entities.len(),
            "zone clouds spawned"
        );
    }
}

#[allow(clippy::type_complexity)]
fn drive_zone_clouds(
    time: Res<Time>,
    settings: Res<GraphicsSettings>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut commands: Commands,
    mut state: ResMut<ZoneCloudState>,
    cam_q: Query<&Transform, (With<crate::camera::OperatorCamera>, Without<CloudLayer>)>,
    mut clouds: Query<(
        Entity,
        &mut Transform,
        &mut Visibility,
        &CloudLayer,
        &mut CloudFade,
        &MeshMaterial3d<FfxiZoneMaterial>,
    )>,
) {
    let vanilla = settings.sky_style() == SkyStyle::Vanilla;
    let cam_pos = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
    let sun_dir = crate::sun_moon::sun_direction(sky.hour);
    let basis = ffxi_to_bevy_basis();
    let day_fraction = crate::hud::vana_clock::full_day_fraction(vana_clock.earth_unix_secs_now());
    let dt = time.delta_secs();

    for (entity, mut xf, mut vis, layer, mut fade, mat) in &mut clouds {
        let want = if vanilla {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        xf.rotation = basis;
        xf.translation = match layer.attach {
            CloudAttach::Camera => cam_pos + basis * layer.base_position,
            CloudAttach::Sun => cam_pos + sun_dir * SUN_LAYER_DISTANCE,
        };

        fade.elapsed += dt;
        if fade.finished_out() {
            state.entities.retain(|&e| e != entity);
            commands.entity(entity).despawn();
            continue;
        }

        // ToD color sets RGB; the keyframe alpha and the cross-fade alpha both
        // multiply the emitted alpha (xim color.a multiplier × switchWeather fade).
        if let Some(material) = materials.get_mut(&mat.0) {
            let mut tint = layer.tracks.sample(day_fraction);
            tint.w *= fade.alpha();
            material.tint = tint;
        }
    }
}

pub struct ZoneCloudsPlugin;

impl Plugin for ZoneCloudsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneCloudState>()
            .add_systems(Update, (rebuild_zone_clouds, drive_zone_clouds).chain());
    }
}
