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
use ffxi_dat::weather::{weather_type_id_or_default, WeatherTypeId, WEATHER_TYPE_FALLBACK};
use ffxi_dat::{ChunkKind, DatRoot};

use crate::components::InGameEntity;
use crate::ffxi_zone_material::{zone_fog_flag, FfxiZoneMaterial};
use crate::graphics_settings::GraphicsSettings;
// research/xim ParticleUpdaters.kt: TextureCoordinateUpdater velocities are per rendered frame.
use crate::scheduler_runtime::RETAIL_FPS;
use crate::zone_texture::{decoded_sky_texture_to_image, TextureQuality};

// research/xim EnvironmentManager.kt updateWeatherEffects reads weat/<type>/.
// Only the cld1/cld2 camera-follow canopies are drawn here; the sun (sun1, attach=0xE)
// is the single additive SunDisc in sun_moon.rs, so it shines through these clouds
// rather than being a second, opaque blend mesh fighting it.

// The authored 0x0F canopy scale (research/xim ParticleGeneratorParser.kt) varies per
// zone/weather and often leaves the canopy rim nearer than the terrain, so the blended
// (non-depth-writing) cloud sheet drapes over zone geometry farther out than the rim.
// Push the rim to just inside the gradient sky dome so all terrain — which fits within
// that dome — is nearer and depth-occludes the clouds (the "sky is the farthest thing"
// rule). Derived from SKYBOX_RADIUS so this can't drift outside the dome it must sit
// under — a rim past the dome is bug kuluu-g64c.
//
// Fixed, not frustum-derived: layer_scale only ever pushes the canopy OUT, so a rim that
// shrinks with the draw distance stops pushing at all and leaves the sheet at its
// authored scale, sitting on the camera with one texture tile filling the whole sky.
const CLOUD_RIM_MARGIN: f32 = 100.0;
pub const CLOUD_MIN_RIM: f32 = crate::skybox::SKYBOX_RADIUS - CLOUD_RIM_MARGIN;

// research/xim EnvironmentManager.kt switchWeather default 3.33s cross-fade
// between the old and new weat/<type>/ effect sets on a 0x0057 weather change.
const WEATHER_FADE_SECS: f32 = 3.33;

// The weat/<type>/ camera-follow canopy generators this module renders as the
// primary cloud canopy. dat_mzb's generator-water path rejects ALL camera-follow
// sky generators structurally (follow_camera config bit), so this list is only
// this module's own include filter, not a shared exclusion contract. Per-weather
// cloud variants beyond cld1/cld2 (e.g. ~4cl) are drawn by no module at all
// (kuluu-zi3t) but are correctly kept out of world geometry by the follow_camera gate.
pub(crate) const CLOUD_CANOPY_GENERATOR_NAMES: [[u8; 4]; 2] = [*b"cld1", *b"cld2"];

#[derive(Component)]
pub struct CloudMesh;

// research/xim ParticleUpdaters.kt ClockValueUpdater: the cloud/sun mesh RGB
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

#[derive(Component)]
struct CloudLayer {
    // FFXI-space base offset added camera-relative (cld1 [0,0,0] / cld2 [0,30,0]).
    base_position: Vec3,
    max_alpha: f32,
    // research/xim TextureCoordinateUpdater: per-frame UV-translate wind velocity.
    uv_scroll: Vec2,
    tracks: CloudColorTracks,
}

// Mesh + material handles + placement extracted for one weat/<type>/ cloud or sun
// generator. Spawned as CloudMesh entities; tracked so a zone/weather change can
// despawn and rebuild them (zone change keeps AppPhase::InGame, so the OnExit
// teardown never runs — see MEMORY zone-change-not-clean-lifecycle).
struct CloudLayerBuild {
    mesh: Handle<Mesh>,
    material: Handle<FfxiZoneMaterial>,
    base_position: Vec3,
    scale: Vec3,
    max_alpha: f32,
    uv_scroll: Vec2,
    tracks: CloudColorTracks,
}

#[derive(Resource, Default)]
struct ZoneCloudState {
    // (resolved zone-DAT file id, weather fourcc) the spawned entities currently mirror.
    key: Option<(u32, WeatherTypeId)>,
    entities: Vec<Entity>,
}

fn ffxi_to_bevy_basis() -> Quat {
    Quat::from_rotation_x(std::f32::consts::PI)
}

// Find the `weat/<type>` directory node for the requested weather type anywhere in
// the zone dir tree (it lives under the zone root dir, e.g. f_ro/weat/clod).
//
// Most zones author only a handful of the 20 weather containers — `rain` ships in
// 16 zones against `suny`'s 130 — so an exact-match-or-nothing lookup leaves the
// sky bare for every weather the zone does not carry. Retail searches the
// container for the requested tag and falls back to `suny` on a miss
// (research/XIClient/src/XIClient/source/World/Weather/WeatherTransition.cpp WeatherTransition::WeatherTransition),
// which is the same single hop the 0x2F record selection takes.
pub(crate) fn find_weat_type<'a>(
    node: &'a ChunkNode<'a>,
    want: WeatherTypeId,
) -> Option<&'a ChunkNode<'a>> {
    find_weat_type_exact(node, want).or_else(|| {
        (want != WEATHER_TYPE_FALLBACK)
            .then(|| find_weat_type_exact(node, WEATHER_TYPE_FALLBACK))
            .flatten()
    })
}

fn find_weat_type_exact<'a>(
    node: &'a ChunkNode<'a>,
    want: WeatherTypeId,
) -> Option<&'a ChunkNode<'a>> {
    for child in &node.children {
        if child.chunk.kind != ChunkKind::Rmp as u8 {
            continue;
        }
        if child.chunk.name == *b"weat" {
            for type_node in &child.children {
                if type_node.chunk.kind == ChunkKind::Rmp as u8 && type_node.chunk.name == want {
                    return Some(type_node);
                }
            }
        }
        if let Some(found) = find_weat_type_exact(child, want) {
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

// The f32 is the mesh's horizontal half-extent (max |x|,|z|), used for rim scaling.
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
            colors.push(mmb::vertex_color_to_linear(v.rgba));
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
    opacity: f32,
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
            let handle = images.add(decoded_sky_texture_to_image(&tex, quality));
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
        // Only the cld1/cld2 camera canopies render here (attach 0x0 or 0x5, both
        // camera-relative). The sun (sun1, attach=0xE) is the additive SunDisc in
        // sun_moon.rs; moon/star/lens-flare live in their own subdirs.
        if !CLOUD_CANOPY_GENERATOR_NAMES.contains(&c.chunk.name) {
            continue;
        }
        let Ok(Some(def)) = Generator::parse_cloud_generator(c.chunk.name, c.chunk.data) else {
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
        let material = materials.add(cloud_material(texture, def.fog_enabled));

        out.push(CloudLayerBuild {
            mesh: meshes.add(mesh),
            material,
            base_position: def_base_to_vec(&def),
            scale: layer_scale(&def, half_xz),
            max_alpha: opacity,
            uv_scroll: Vec2::from_array(def.uv_scroll),
            tracks: resolve_color_tracks(weat_type, &def),
        });
    }
    out
}

// The cloud generators ship no 0x63 alpha keyframe, so canopy coverage can't be read
// from the DAT — it's a deliberate per-weather tuning (clear sparse, storm dense).
fn weather_opacity(want: WeatherTypeId) -> f32 {
    match &want {
        b"fine" | b"suny" => 0.35,
        b"clod" | b"mist" => 0.70,
        b"rain" | b"thdr" => 0.90,
        _ => 0.50,
    }
}

fn def_base_to_vec(def: &CloudGeneratorDef) -> Vec3 {
    Vec3::new(
        def.base_position[0],
        def.base_position[1],
        def.base_position[2],
    )
}

// Camera-follow cloud canopies sit on the camera, so their rim (half_xz * authored
// 0x0F scale) is pushed out to at least CLOUD_MIN_RIM — keeping the authored aspect
// ratio — so distant terrain stays nearer and depth-occludes them.
fn layer_scale(def: &CloudGeneratorDef, half_xz: f32) -> Vec3 {
    let authored = Vec3::from_array(def.scale);
    let rim = half_xz * authored.x.max(authored.z);
    let factor = if rim > 1.0 {
        (CLOUD_MIN_RIM / rim).max(1.0)
    } else {
        1.0
    };
    authored * factor
}

fn id_str(id: [u8; 4]) -> String {
    id.iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect::<String>()
        .trim()
        .to_string()
}

// Clouds blend over the sky dome, so AlphaMode::Blend with the texture's own alpha.
fn cloud_material(texture: Option<Handle<Image>>, fog_enabled: bool) -> FfxiZoneMaterial {
    let has_texture = if texture.is_some() { 1.0 } else { 0.0 };
    FfxiZoneMaterial::new(
        texture,
        crate::skinned_ffxi_material::FfxiMaterialFlags {
            flags: Vec4::new(has_texture, 1.0, zone_fog_flag(fog_enabled), 0.0),
        },
        Vec4::ONE,
        Vec4::ZERO,
        AlphaMode::Blend,
        // Clouds are a canopy hung off a weat/ generator, so they take the two-stage
        // CMoD3m chain with the ToD `tint` as its TEXTUREFACTOR. Otherwise a synthetic
        // layer with no DAT render-state word: no cull, no bias.
        crate::ffxi_zone_material::FfxiZoneMaterialKey {
            generator_stage_chain: true,
            ..crate::ffxi_zone_material::FfxiZoneMaterialKey::LEGACY
        },
    )
    .with_sort_depth_bias(crate::skybox::SKY_SORT_DEPTH_CLOUDS)
}

fn read_zone_dat(file_id: u32) -> Option<Vec<u8>> {
    let root = DatRoot::from_env_or_default().ok()?;
    let location = root.resolve(file_id).ok()?;
    fs::read(location.path_under(&root)).ok()
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
    let file_id = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    // No weather yet is not weather id 0 — id 0 is a real row (`fine`). Resolve the
    // unknown case to the same container retail falls back to instead of indexing
    // the table with a sentinel.
    let want = weather_type_id_or_default(current_weather.0.map(|w| w as u16));
    let key = file_id.map(|f| (f, want));
    if key == state.key {
        return;
    }

    // Read the DAT BEFORE the teardown commits, so a failed read leaves the current
    // canopy up instead of swapping the sky for nothing (kuluu-grbo). The key still
    // latches: every failure here is deterministic for this file id, and re-probing
    // reloads every VTABLE/FTABLE (DatRoot::open) on each frame that it stays broken.
    let loaded = file_id.and_then(read_zone_dat);
    if file_id.is_some() && loaded.is_none() {
        warn!(
            ?file_id,
            "zone clouds: zone DAT unreadable, canopy left as-is"
        );
        state.key = key;
        return;
    }

    // Resolve and BUILD the replacement before the teardown commits (kuluu-grbo): every
    // remaining abort — no weat container, no cld1/cld2 under it, an unresolvable mesh —
    // would otherwise fade the canopy out with nothing to put back. An empty result is
    // content-correct for the 206 shipped DATs whose tags author no canopy, so it still
    // tears down; it is just no longer indistinguishable from a failed rebuild.
    let tree = loaded.as_deref().map(walk_tree);
    let weat_type = tree.as_ref().and_then(|t| find_weat_type(t, want));
    let quality = TextureQuality {
        mipmaps: settings.texture_filtering.mipmaps(),
        anisotropy: settings.texture_filtering.anisotropy(),
    };
    let layers = weat_type
        .map(|node| {
            build_cloud_layers(
                node,
                quality,
                weather_opacity(want),
                &mut meshes,
                &mut images,
                &mut materials,
            )
        })
        .unwrap_or_default();

    // A weather change within the same zone DAT cross-fades the old set out (xim
    // switchWeather); a DAT change despawns immediately — the old weat/ set
    // belongs to a different DAT and the camera teleports, so a fade would smear.
    let same_dat = match (state.key, key) {
        (Some((prev_file, _)), Some((next_file, _))) => prev_file == next_file,
        _ => false,
    };
    for e in state.entities.drain(..) {
        if same_dat {
            commands.entity(e).insert(CloudFade {
                dir: FadeDir::Out,
                elapsed: 0.0,
            });
        } else {
            commands.entity(e).try_despawn();
        }
    }
    state.key = key;

    let Some(file_id) = file_id else {
        return;
    };

    for layer in layers {
        let e = commands
            .spawn((
                InGameEntity,
                CloudMesh,
                CloudLayer {
                    base_position: layer.base_position,
                    max_alpha: layer.max_alpha,
                    uv_scroll: layer.uv_scroll,
                    tracks: layer.tracks,
                },
                CloudFade {
                    dir: FadeDir::In,
                    elapsed: if same_dat { 0.0 } else { WEATHER_FADE_SECS },
                },
                Mesh3d(layer.mesh),
                MeshMaterial3d(layer.material),
                Transform::from_rotation(ffxi_to_bevy_basis()).with_scale(layer.scale),
                Visibility::Inherited,
                bevy::light::NotShadowCaster,
                bevy::light::NotShadowReceiver,
            ))
            .id();
        state.entities.push(e);
    }

    info!(
        file_id,
        type_ = id_str(want),
        count = state.entities.len(),
        weat_container = weat_type.is_some(),
        "zone clouds rebuilt"
    );
}

#[allow(clippy::type_complexity)]
fn drive_zone_clouds(
    time: Res<Time>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut touched: ResMut<crate::ffxi_zone_material::ZoneMaterialTouched>,
    mut commands: Commands,
    mut state: ResMut<ZoneCloudState>,
    cam_q: Query<&Transform, (With<crate::camera::OperatorCamera>, Without<CloudLayer>)>,
    mut clouds: Query<(
        Entity,
        &mut Transform,
        &CloudLayer,
        &mut CloudFade,
        &MeshMaterial3d<FfxiZoneMaterial>,
    )>,
) {
    let cam_pos = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let basis = ffxi_to_bevy_basis();
    let day_fraction = crate::vana_time::full_day_fraction(vana_clock.earth_unix_secs_now());
    let dt = time.delta_secs();
    let frames = time.elapsed_secs() * RETAIL_FPS;

    for (entity, mut xf, layer, mut fade, mat) in &mut clouds {
        xf.rotation = basis;
        xf.translation = cam_pos + basis * layer.base_position;

        fade.elapsed += dt;
        if fade.finished_out() {
            state.entities.retain(|&e| e != entity);
            commands.entity(entity).try_despawn();
            continue;
        }

        // get_mut_untracked: tint/uv flow to the GPU through the persistent
        // buffers in upload_zone_material_buffers; marking the asset Modified
        // here would needlessly rebuild its bind group every frame.
        if let Some(material) = materials.get_mut_untracked(&mat.0) {
            let mut tint = layer.tracks.sample(day_fraction);
            tint.w *= fade.alpha() * layer.max_alpha;
            material.tint = tint;
            // TextureCoordinateUpdater integrates UV velocity over elapsed frames.
            let uv = layer.uv_scroll * frames;
            material.uv_offset = Vec4::new(uv.x, uv.y, 0.0, 0.0);
            touched.mark(mat.0.id());
        }
    }
}

// Star dome (weat/<type>/star/ sta1 mesh + sta2 texture). Placed just inside the
// skybox sphere so it reads as the farthest sky layer, behind the cloud canopy.
const STAR_RADIUS: f32 = 5000.0;

// Sun-altitude (radians below the horizon) over which the star field fades fully in.
const STAR_TWILIGHT_BAND_RAD: f32 = 0.30;

#[derive(Component)]
struct StarDome;

#[derive(Resource, Default)]
struct ZoneStarState {
    // The dome is read out of the CURRENT weather's weat/<tag>/star, and 123 shipped zone
    // DATs carry one under only some of their tags, so keying on the file id alone latched
    // whichever weather happened to be live at zone-in: a later change to a tag that does
    // ship a dome could never rebuild, and one that does not kept the previous tag's.
    key: Option<(u32, WeatherTypeId)>,
    entity: Option<Entity>,
}

fn find_star_dir<'a>(weat_type: &'a ChunkNode<'a>) -> Option<&'a ChunkNode<'a>> {
    weat_type
        .children
        .iter()
        .find(|c| c.chunk.kind == ChunkKind::Rmp as u8 && c.chunk.name == *b"star")
}

#[allow(clippy::too_many_arguments)]
fn rebuild_zone_stars(
    scene_state: Res<crate::snapshot::SceneState>,
    current_weather: Res<crate::weather_fx::CurrentWeather>,
    settings: Res<GraphicsSettings>,
    mut state: ResMut<ZoneStarState>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let file_id = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    let want = weather_type_id_or_default(current_weather.0.map(|w| w as u16));
    let key = file_id.map(|f| (f, want));
    if key == state.key {
        return;
    }
    if let Some(e) = state.entity.take() {
        commands.entity(e).try_despawn();
    }
    state.key = key;

    let Some(file_id) = file_id else {
        return;
    };
    let Some(bytes) = read_zone_dat(file_id) else {
        return;
    };

    let tree = walk_tree(&bytes);
    let weat_type = match find_weat_type(&tree, want) {
        Some(n) => n,
        None => return,
    };
    let Some(star_dir) = find_star_dir(weat_type) else {
        return;
    };

    let Some(mesh_chunk) = star_dir
        .children
        .iter()
        .find(|c| c.chunk.kind == ChunkKind::Mmb as u8)
    else {
        return;
    };
    let Ok(decrypted) = mmb::decrypt(mesh_chunk.chunk.data) else {
        return;
    };
    let Some((mesh, half_xz)) = build_mesh(&decrypted) else {
        return;
    };
    let scale = if half_xz > 1.0 {
        STAR_RADIUS / half_xz
    } else {
        1.0
    };

    let quality = TextureQuality {
        mipmaps: settings.texture_filtering.mipmaps(),
        anisotropy: settings.texture_filtering.anisotropy(),
    };
    let texture = star_dir
        .children
        .iter()
        .find(|c| c.chunk.kind == ChunkKind::Img as u8)
        .and_then(|c| decode_texture(c.chunk.data).ok())
        .map(|t| images.add(decoded_sky_texture_to_image(&t, quality)));

    // Unlit additive: stars are self-luminous points on a black field, so scene
    // lighting must not dim them and the black background must add nothing.
    // Bevy applies DistanceFog to unlit StandardMaterials too (vendor/bevy_pbr
    // pbr.wgsl:89 main_pass_post_lighting_processing), and the star generators
    // clear fog in the DAT (measured star/sta1 = 0x02440204).
    let material = materials.add(StandardMaterial {
        base_color_texture: texture,
        unlit: true,
        alpha_mode: AlphaMode::Add,
        fog_enabled: false,
        depth_bias: crate::skybox::SKY_SORT_DEPTH_STARS,
        ..default()
    });

    // Spawn hidden; drive_zone_stars (chained right after) sets visibility from
    // the night factor this same frame.
    let e = commands
        .spawn((
            InGameEntity,
            StarDome,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(material),
            Transform::from_rotation(ffxi_to_bevy_basis()).with_scale(Vec3::splat(scale)),
            Visibility::Hidden,
            bevy::light::NotShadowCaster,
            bevy::light::NotShadowReceiver,
        ))
        .id();
    state.entity = Some(e);
    info!(file_id, half_xz, scale, "zone star dome spawned");
}

#[allow(clippy::type_complexity)]
fn drive_zone_stars(
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    cam_q: Query<&Transform, (With<crate::camera::OperatorCamera>, Without<StarDome>)>,
    mut stars: Query<
        (
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<StandardMaterial>,
        ),
        With<StarDome>,
    >,
    mut prev_visible: Local<Option<bool>>,
    mut prev_night: Local<Option<f32>>,
) {
    // iter_mut, not single_mut: the InGame lifecycle can leave orphaned StarDome
    // entities (OnExit bulk-despawn races the rebuild), and single_mut() silently
    // returns Err on >1, leaving every dome stuck at its spawn-time Hidden.
    let count = stars.iter().count();
    if count == 0 {
        return;
    }
    let cam_pos = cam_q.single().map(|t| t.translation).unwrap_or(Vec3::ZERO);
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);

    // Stars fade in as the sun drops below the horizon, in both sky styles (both
    // share the gradient dome now).
    let night = (-sky.sun_altitude / STAR_TWILIGHT_BAND_RAD).clamp(0.0, 1.0);
    let want = if night > 0.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if *prev_visible != Some(night > 0.0) {
        info!(
            sun_altitude = sky.sun_altitude,
            night,
            visible = night > 0.0,
            dome_count = count,
            "zone stars visibility"
        );
        *prev_visible = Some(night > 0.0);
    }

    // Sub-8-bit brightness steps are invisible; skipping them keeps the per-frame
    // StandardMaterial Modified churn (bind-group rebuilds) out of the twilight fade.
    const STAR_NIGHT_STEP: f32 = 1.0 / 255.0;
    let write_night = prev_night.is_none_or(|p| (night - p).abs() >= STAR_NIGHT_STEP);
    if write_night {
        *prev_night = Some(night);
    }

    // One slow celestial roll per Vana day.
    let frac = crate::vana_time::full_day_fraction(vana_clock.earth_unix_secs_now());
    for (mut xf, mut vis, mat) in stars.iter_mut() {
        if *vis != want {
            *vis = want;
        }
        let scale = xf.scale;
        xf.translation = cam_pos;
        xf.rotation = Quat::from_rotation_y(frac * std::f32::consts::TAU) * ffxi_to_bevy_basis();
        xf.scale = scale;
        if write_night {
            if let Some(mut m) = materials.get_mut(&mat.0) {
                m.base_color = Color::linear_rgb(night, night, night);
            }
        }
    }
}

pub struct ZoneCloudsPlugin;

impl Plugin for ZoneCloudsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneCloudState>()
            .init_resource::<ZoneStarState>()
            .add_systems(Update, (rebuild_zone_clouds, drive_zone_clouds).chain())
            .add_systems(Update, (rebuild_zone_stars, drive_zone_stars).chain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canopy_include_filter_covers_primary_cloud_generators() {
        for name in [*b"cld1", *b"cld2"] {
            assert!(CLOUD_CANOPY_GENERATOR_NAMES.contains(&name));
        }
    }

    // The canopy rides the camera, so bevy's AABB-centre depth sort ranks it at
    // 0 — nearest — and draws it over every other transparent object, washing
    // out any nameplate or particle seen against sky (kuluu-w4jf).
    #[test]
    fn canopy_material_sorts_at_the_sky_depth_not_on_the_camera() {
        use bevy::pbr::Material;
        assert_eq!(
            cloud_material(None, false).depth_bias(),
            crate::skybox::SKY_SORT_DEPTH_CLOUDS
        );
    }

    // The canopy must carry the generator's own fog bit through, not a blanket
    // exemption: retail fogs the overcast cld2 haze sheet and not cld1, and both
    // go through this one constructor.
    #[test]
    fn canopy_material_carries_the_generator_fog_bit() {
        use crate::ffxi_zone_material::{ZONE_FLAG_FOGGED, ZONE_FLAG_UNFOGGED};
        assert_eq!(
            cloud_material(None, true).material_flags.flags.z,
            ZONE_FLAG_FOGGED
        );
        assert_eq!(
            cloud_material(None, false).material_flags.flags.z,
            ZONE_FLAG_UNFOGGED
        );
    }
}
