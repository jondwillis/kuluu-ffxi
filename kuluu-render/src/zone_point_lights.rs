#![cfg(not(target_arch = "wasm32"))]

use bevy::prelude::*;
use ffxi_dat::{chunk::walk, generator::Generator, kind::ChunkKind, mzb};
use kuluu_snapshot::Vec3 as WireVec3;

use crate::components::InGameEntity;
use crate::scene::mzb_to_bevy;
use crate::snapshot::SceneState;

const FAITHFUL_LIGHT_INTENSITY: f32 = 25_000.0;

// FFXiMain.dll retail-2026-09 RVA 0x178610 InitLight zeroes Attenuation0/1.
const SCENE_LIGHT_CONST_ATTEN: f32 = 0.0;

// Retail lamps/braziers visibly waver (2026-07-19 MH capture); the DAT ships no
// flicker keyframes to scrape, so the shape is hand-tuned to the footage: a slow
// deep wave plus a faster shimmer, peaking at 1.0 so the mean sits just under
// the steady level.
const LAMP_FLICKER_BASE: f32 = 0.90;
const LAMP_FLICKER_SLOW_AMP: f32 = 0.07;
const LAMP_FLICKER_SLOW_RATE: f32 = 7.3;
const LAMP_FLICKER_FAST_AMP: f32 = 0.03;
const LAMP_FLICKER_FAST_RATE: f32 = 23.3;
// De-syncs neighbouring lamps so a room full of lights doesn't pulse in unison.
const LAMP_FLICKER_PHASE_STRIDE: f32 = 1.7;

pub fn lamp_flicker(t: f32, seed: f32) -> f32 {
    LAMP_FLICKER_BASE
        + LAMP_FLICKER_SLOW_AMP * (t * LAMP_FLICKER_SLOW_RATE + seed).sin()
        + LAMP_FLICKER_FAST_AMP
            * (t * LAMP_FLICKER_FAST_RATE + seed * LAMP_FLICKER_PHASE_STRIDE).sin()
}
/// No Generator chunk defines this light, so no MZB chunk can bind it. Zone
/// FourCCs are never 0 (`LightID == 0` is retail's empty pool slot,
/// ZoneRenderer.cpp ZoneRenderer::GetOrAllocateLight).
pub const UNAUTHORED_LIGHT_ID: mzb::LightId = 0;

#[derive(Debug, Clone)]
pub struct ZonePointLight {
    /// FourCC of the Generator chunk that defines this light — the `LightID` an
    /// MZB chunk binding names; [`UNAUTHORED_LIGHT_ID`] for a light no zone
    /// authors.
    pub light_id: mzb::LightId,

    pub world_pos: Vec3,

    pub color: Vec3,

    pub range: f32,

    pub attenuation: f32,
    pub theta_track: Option<std::sync::Arc<ffxi_dat::particle_gen::KeyFrameTrack>>,
    pub theta_multiplier: f32,
}

#[derive(Resource, Default)]
pub struct ZonePointLights {
    pub file_id: Option<u32>,
    pub sub_area_file_id: Option<u32>,
    pub lights: Vec<ZonePointLight>,
}

/// Per-frame feed of the faithful Generator lights the FFXI custom materials
/// (zone geometry + skinned actors) consume, in the shared shader convention;
/// each keeps its `light_id`, so a chunk's authored binding resolves against
/// this list.
#[derive(Resource, Default)]
pub struct ActiveSceneLights {
    pub lights: Vec<ZonePointLight>,
}

pub fn build_active_scene_lights(
    faithful: Res<ZonePointLights>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    mut active: ResMut<ActiveSceneLights>,
) {
    let day = crate::vana_time::full_day_fraction(vana_clock.earth_unix_secs_now());
    let enabled = settings.dynamic_lights.faithful_enabled();
    let source = if enabled {
        faithful.lights.as_slice()
    } else {
        &[]
    };
    let mut changed = active.lights.len() != source.len();
    {
        let target = &mut active.bypass_change_detection().lights;
        target.truncate(source.len());
        for (index, light) in source.iter().enumerate() {
            let evaluated = light.at_time(day);
            if let Some(old) = target.get_mut(index) {
                changed |= old.light_id != evaluated.light_id
                    || old.world_pos != evaluated.world_pos
                    || old.color != evaluated.color
                    || old.range != evaluated.range
                    || old.attenuation != evaluated.attenuation;
                *old = evaluated;
            } else {
                target.push(evaluated);
            }
        }
    }
    if changed {
        active.set_changed();
    }
}

impl ZonePointLight {
    fn at_time(&self, day: f32) -> Self {
        let mut light = self.clone();
        if let Some(track) = &self.theta_track {
            let theta = (track.sample(day).max(0.0) * self.theta_multiplier).max(0.0);
            if theta > 0.0 {
                light.attenuation = theta.recip();
            } else {
                light.color = Vec3::ZERO;
                light.attenuation = 1.0;
            }
        }
        light
    }
}

use crate::skinned_ffxi_material::MAX_POINT_LIGHTS;

pub type PointLightArrays = (
    [Vec4; MAX_POINT_LIGHTS],
    [Vec4; MAX_POINT_LIGHTS],
    [Vec4; MAX_POINT_LIGHTS],
);

/// Pack the selected lights into the `(point_pos, point_color, point_atten)`
/// arrays of `FfxiLightingUniform`. `point_color.w` carries range (the shader
/// treats zero-range slots as empty); `point_atten` is
/// `(const, linear, quad, _)`. Excess beyond `MAX_POINT_LIGHTS` is dropped.
pub(crate) fn pack_point_light_arrays<'a>(
    selected: impl Iterator<Item = &'a ZonePointLight>,
) -> PointLightArrays {
    let mut point_pos = [Vec4::ZERO; MAX_POINT_LIGHTS];
    let mut point_color = [Vec4::ZERO; MAX_POINT_LIGHTS];
    let mut point_atten = [Vec4::ZERO; MAX_POINT_LIGHTS];
    for (slot, l) in selected.take(MAX_POINT_LIGHTS).enumerate() {
        point_pos[slot] = l.world_pos.extend(0.0);
        point_color[slot] = l.color.extend(l.range);
        point_atten[slot] = Vec4::new(SCENE_LIGHT_CONST_ATTEN, 0.0, l.attenuation, 0.0);
    }
    (point_pos, point_color, point_atten)
}

/// The chunk's authored light slots as indices into `lights`, in binding order.
///
/// Retail's chunk binding names a `LightID`, and a slot whose light the zone
/// never defines is left disabled (ZoneRenderer.cpp ZoneRenderer::UpdateBlockLightSettings `managedLight ==
/// nullptr`), so an unmatched FourCC drops out rather than shifting the rest.
/// Never yields more than [`mzb::LIGHT_REFERENCE_COUNT`] — retail's four D3D
/// slots — however many slots the shader uniform carries.
pub fn authored_point_light_indices(
    lights: &[ZonePointLight],
    authored: &[Option<mzb::LightId>; mzb::LIGHT_REFERENCE_COUNT],
) -> Vec<u32> {
    authored
        .iter()
        .flatten()
        .filter(|id| **id != UNAUTHORED_LIGHT_ID)
        .filter_map(|id| {
            lights
                .iter()
                .position(|l| l.light_id == *id)
                .map(|i| i as u32)
        })
        .collect()
}

// research/XIClient/src/XIClient/source/Rendering/ZoneRenderer.cpp UpdateBlockLightSettings.
pub fn terrain_point_light_indices(
    lights: &[ZonePointLight],
    authored: &[Option<mzb::LightId>; mzb::LIGHT_REFERENCE_COUNT],
) -> Vec<u32> {
    const ACTOR_ONLY_PREFIX: u8 = b'c';
    let eligible = authored.map(|id| id.filter(|id| id.to_le_bytes()[0] != ACTOR_ONLY_PREFIX));
    authored_point_light_indices(lights, &eligible)
}

/// Pick the `count` nearest in-range lights to `pos` (`count` clamped to
/// `MAX_POINT_LIGHTS`), as indices into `lights`. The fallback for zones that
/// ship no authored binding table, and for the `//lights` emitters no zone
/// authors; the caller may cache the selection while the light set and the actor
/// hold still, repacking live colors per frame via [`point_light_arrays_for`].
pub fn nearest_point_light_indices(pos: Vec3, lights: &[ZonePointLight], count: usize) -> Vec<u32> {
    let count = count.min(MAX_POINT_LIGHTS);
    let mut nearest = [(f32::INFINITY, 0u32); MAX_POINT_LIGHTS];
    let mut len = 0;
    for (i, l) in lights.iter().enumerate() {
        let d2 = pos.distance_squared(l.world_pos);
        if d2 > l.range * l.range {
            continue;
        }
        if len < count {
            nearest[len] = (d2, i as u32);
            len += 1;
        } else if count == 0 || d2 >= nearest[count - 1].0 {
            continue;
        } else {
            nearest[count - 1] = (d2, i as u32);
        }
        let mut slot = len - 1;
        while slot > 0 && nearest[slot].0 < nearest[slot - 1].0 {
            nearest.swap(slot, slot - 1);
            slot -= 1;
        }
    }
    nearest[..len].iter().map(|&(_, i)| i).collect()
}

pub fn point_light_arrays_for(lights: &[ZonePointLight], indices: &[u32]) -> PointLightArrays {
    pack_point_light_arrays(indices.iter().filter_map(|&i| lights.get(i as usize)))
}

// FFXiMain.dll retail-2026-09 RVA 0xCB698 keeps one point light; RVA 0xCB845 converts it to directional.
pub fn actor_directional_point_light(
    pos: Vec3,
    lights: &[ZonePointLight],
    indices: &[u32],
) -> PointLightArrays {
    const MIN_ATTENUATION_DENOMINATOR: f32 = 0.0001;
    const DIRECTIONAL_RANGE_MARKER: f32 = -1.0;
    let mut strongest = None;
    let mut strength = 0.0;
    for &index in indices {
        let Some(light) = lights.get(index as usize) else {
            continue;
        };
        let offset = light.world_pos - pos;
        let distance_sq = offset.length_squared();
        if light.range <= 0.0 || distance_sq > light.range * light.range {
            continue;
        }
        let intensity = (distance_sq * light.attenuation)
            .max(MIN_ATTENUATION_DENOMINATOR)
            .recip();
        if intensity > strength {
            strength = intensity;
            strongest = Some((light, offset.normalize_or_zero()));
        }
    }
    let mut positions = [Vec4::ZERO; MAX_POINT_LIGHTS];
    let mut colors = [Vec4::ZERO; MAX_POINT_LIGHTS];
    if let Some((light, direction)) = strongest {
        positions[0] = direction.extend(0.0);
        colors[0] = (light.color * strength).extend(DIRECTIONAL_RANGE_MARKER);
    }
    (positions, colors, [Vec4::ZERO; MAX_POINT_LIGHTS])
}

pub fn nearest_point_light_arrays(
    pos: Vec3,
    lights: &[ZonePointLight],
    count: usize,
) -> PointLightArrays {
    point_light_arrays_for(lights, &nearest_point_light_indices(pos, lights, count))
}

impl ZonePointLights {
    fn refresh(
        &mut self,
        main: Option<u32>,
        active_sub_area: Option<u32>,
        mut load: impl FnMut(u32) -> Vec<ZonePointLight>,
    ) {
        let interior = main.and(active_sub_area.map(ffxi_dat::sub_area::sub_area_file_id));
        if self.file_id == main && self.sub_area_file_id == interior {
            return;
        }
        self.file_id = main;
        self.sub_area_file_id = interior;
        self.lights.clear();
        for file_id in [main, interior].into_iter().flatten() {
            self.lights.extend(load(file_id));
        }
    }
}

fn point_lights_from_dat(bytes: &[u8]) -> Vec<ZonePointLight> {
    let tracks: std::collections::HashMap<_, _> = walk(bytes)
        .flatten()
        .filter(|c| c.kind == ChunkKind::KeyFrame as u8)
        .map(|c| {
            (
                c.name,
                std::sync::Arc::new(ffxi_dat::particle_gen::KeyFrameTrack::parse(c.data)),
            )
        })
        .collect();
    walk(bytes)
        .flatten()
        .filter(|c| ChunkKind::from_u8(c.kind) == Some(ChunkKind::Generator))
        .filter_map(|c| {
            let pl = Generator::parse_point_light(c.data).ok()??;
            if pl.range <= 0.0 {
                return None;
            }
            let world_pos = mzb_to_bevy(WireVec3 {
                x: pl.base_position[0],
                y: pl.base_position[1],
                z: pl.base_position[2],
            });
            Some(ZonePointLight {
                light_id: u32::from_le_bytes(c.name),
                world_pos,
                color: Vec3::new(pl.color[0], pl.color[1], pl.color[2]),
                range: pl.range,
                attenuation: pl.attenuation,
                theta_track: pl.theta_track.and_then(|id| tracks.get(&id).cloned()),
                theta_multiplier: pl.theta_multiplier,
            })
        })
        .collect()
}

fn load_zone_point_lights(
    scene_state: Res<SceneState>,
    activation: Option<Res<crate::sub_area_activation::SubAreaActivation>>,
    mut store: ResMut<ZonePointLights>,
    dat_root: Res<crate::dat_root::SharedDatRoot>,
) {
    let Some(root) = dat_root.get() else {
        return;
    };
    let current = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    let interior = activation.as_deref().and_then(|a| a.active());
    let interior_file = current.and(interior.map(ffxi_dat::sub_area::sub_area_file_id));
    if store.file_id == current && store.sub_area_file_id == interior_file {
        return;
    }
    store.refresh(current, interior, |file_id| {
        let Ok(loc) = root.resolve(file_id) else {
            return Vec::new();
        };
        let Ok(bytes) = std::fs::read(loc.path_under(root)) else {
            return Vec::new();
        };
        let lights = point_lights_from_dat(&bytes);
        info!(file_id, count = lights.len(), "loaded zone point lights");
        lights
    });
}

#[derive(Component)]
struct FaithfulZoneLight {
    index: usize,
}

fn sync_faithful_zone_light_entities(
    mut commands: Commands,
    store: Res<ZonePointLights>,
    existing: Query<Entity, With<FaithfulZoneLight>>,
) {
    if !store.is_changed() {
        return;
    }
    for e in &existing {
        commands.entity(e).try_despawn();
    }
    for (index, l) in store.lights.iter().enumerate() {
        commands.spawn((
            FaithfulZoneLight { index },
            InGameEntity,
            PointLight {
                range: l.range,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(l.world_pos),
            Visibility::Hidden,
        ));
    }
}

fn animate_faithful_zone_lights(
    active: Res<ActiveSceneLights>,
    time: Res<bevy::time::Time>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    mut q: Query<(&FaithfulZoneLight, &mut PointLight, &mut Visibility)>,
) {
    let enhanced = settings.dynamic_lights.point_shadows_enabled();
    for (source, mut pl, mut vis) in &mut q {
        let Some(light) = active.lights.get(source.index) else {
            *vis = Visibility::Hidden;
            continue;
        };
        let peak = light.color.max_element();
        let hue = if peak > 0.0 {
            light.color / peak
        } else {
            Vec3::ZERO
        };
        let flicker = if enhanced && settings.light_flicker {
            lamp_flicker(time.elapsed_secs_wrapped(), source.index as f32)
        } else {
            1.0
        };
        pl.color = Color::linear_rgb(hue.x, hue.y, hue.z);
        pl.intensity = FAITHFUL_LIGHT_INTENSITY * peak * flicker;
        pl.range = light.range;
        *vis = if enhanced && peak > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

// Retail casts no shadow map at all (graphics/settings.rs `zone_shadow_cast`), so this is the
// Enhanced half of Dynamic Lights: the lit lights nearest the camera render Bevy cube shadow
// maps. A member keeps its map until an outsider is closer by this margin, so a lamp on the
// boundary does not flap six cube faces on and off as the camera drifts.
const SHADOW_HANDOVER_MARGIN: f32 = 1.5;

pub(crate) fn pick_shadowed(
    candidates: &mut [(Entity, f32, bool)],
    count: usize,
    margin: f32,
) -> Vec<Entity> {
    let key = |c: &(Entity, f32, bool)| c.1 - if c.2 { margin } else { 0.0 };
    candidates.sort_by(|a, b| key(a).total_cmp(&key(b)));
    candidates.iter().take(count).map(|c| c.0).collect()
}

fn select_shadowed_zone_lights(
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    cam: Query<&GlobalTransform, With<crate::camera::OperatorCamera>>,
    mut q: Query<(Entity, &GlobalTransform, &Visibility, &mut PointLight), With<FaithfulZoneLight>>,
) {
    let count = if settings.dynamic_lights.point_shadows_enabled() {
        settings.shadowed_lights as usize
    } else {
        0
    };
    let mut candidates: Vec<(Entity, f32, bool)> = Vec::new();
    if let Some(cam_pos) = cam
        .iter()
        .next()
        .map(|c| c.translation())
        .filter(|_| count > 0)
    {
        for (e, gt, vis, pl) in &q {
            if *vis == Visibility::Hidden {
                continue;
            }
            candidates.push((
                e,
                gt.translation().distance(cam_pos),
                pl.shadow_maps_enabled,
            ));
        }
    }
    let chosen = pick_shadowed(&mut candidates, count, SHADOW_HANDOVER_MARGIN);
    let mut switched = 0usize;
    for (e, _, _, mut pl) in &mut q {
        let want = chosen.contains(&e);
        if pl.shadow_maps_enabled != want {
            pl.shadow_maps_enabled = want;
            switched += 1;
        }
    }
    if switched > 0 {
        info!(
            "zone_point_lights: {} of {} lit light(s) carry shadow maps",
            chosen.len(),
            candidates.len()
        );
    }
}

pub struct ZonePointLightsPlugin;

impl Plugin for ZonePointLightsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, select_shadowed_zone_lights);
        app.init_resource::<ZonePointLights>()
            .init_resource::<ActiveSceneLights>()
            .add_systems(
                Update,
                (
                    load_zone_point_lights
                        .after(crate::sub_area_activation::drive_sub_area_activation),
                    sync_faithful_zone_light_entities,
                    build_active_scene_lights,
                    animate_faithful_zone_lights,
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_samples_the_strongest_point_once_at_its_origin() {
        const RANGE: f32 = 10.0;
        const NEAR: f32 = 2.0;
        const FAR: f32 = 4.0;
        let make = |height| ZonePointLight {
            light_id: UNAUTHORED_LIGHT_ID,
            world_pos: Vec3::Y * height,
            color: Vec3::ONE,
            range: RANGE,
            attenuation: 1.0,
            theta_track: None,
            theta_multiplier: 1.0,
        };
        let lights = [make(FAR), make(NEAR)];
        let (positions, colors, attenuation) =
            actor_directional_point_light(Vec3::ZERO, &lights, &[0, 1]);
        assert_eq!(positions[0].truncate(), Vec3::Y);
        assert!(colors[0].w < 0.0);
        assert_eq!(colors[0].truncate(), Vec3::splat((NEAR * NEAR).recip()));
        assert!(colors[1..].iter().all(|color| *color == Vec4::ZERO));
        assert!(attenuation.iter().all(|value| *value == Vec4::ZERO));
        let (_, absent, _) =
            actor_directional_point_light(Vec3::X * (RANGE + FAR), &lights, &[0, 1]);
        assert!(absent.iter().all(|color| *color == Vec4::ZERO));
    }

    #[test]
    fn active_interior_lights_join_main_and_leave_on_deactivation_or_disconnect() {
        const MAIN_FILE: u32 = 348;
        const FERRY_SUB_AREA: u32 = 485;
        let interior_file = ffxi_dat::sub_area::sub_area_file_id(FERRY_SUB_AREA);
        let mut store = ZonePointLights::default();
        let source = |file_id| {
            vec![ZonePointLight {
                light_id: file_id,
                ..light(Vec3::ZERO, 10.0)
            }]
        };
        store.refresh(Some(MAIN_FILE), None, source);
        assert_eq!(store.lights.len(), 1);
        store.refresh(Some(MAIN_FILE), Some(FERRY_SUB_AREA), source);
        assert_eq!(
            store.lights.iter().map(|l| l.light_id).collect::<Vec<_>>(),
            [MAIN_FILE, interior_file]
        );
        store.refresh(Some(MAIN_FILE), Some(FERRY_SUB_AREA), |_| {
            panic!("unchanged active sources must not reload")
        });
        store.refresh(Some(MAIN_FILE), None, source);
        assert_eq!(store.sub_area_file_id, None);
        assert_eq!(store.lights.len(), 1);
        assert_eq!(store.lights[0].light_id, MAIN_FILE);
        store.refresh(Some(MAIN_FILE), Some(FERRY_SUB_AREA), source);
        store.refresh(None, Some(FERRY_SUB_AREA), |_| {
            panic!("a stale activation must not load an interior without a zone")
        });
        assert_eq!(store.file_id, None);
        assert_eq!(store.sub_area_file_id, None);
        assert!(store.lights.is_empty());
    }

    #[test]
    fn unchanged_sources_do_not_mark_lights_changed() {
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut app = App::new();
        app.init_resource::<SceneState>()
            .init_resource::<ZonePointLights>()
            .insert_resource(crate::dat_root::SharedDatRoot(Some(std::sync::Arc::new(
                root,
            ))))
            .add_systems(Update, load_zone_point_lights);
        app.update();
        app.world_mut().clear_trackers();
        app.update();
        assert!(!app.world().resource_ref::<ZonePointLights>().is_changed());
    }

    #[test]
    fn selbina_ferry_dat_supplies_interior_lamps() {
        const SELBINA_FILE: u32 = 348;
        const FERRY_SUB_AREA: u32 = 485;
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let read =
            |file_id| std::fs::read(root.resolve(file_id).unwrap().path_under(&root)).unwrap();
        let main = point_lights_from_dat(&read(SELBINA_FILE));
        let interior =
            point_lights_from_dat(&read(ffxi_dat::sub_area::sub_area_file_id(FERRY_SUB_AREA)));
        let interior_ids = interior.iter().map(|l| l.light_id).collect::<Vec<_>>();
        assert_eq!(
            interior_ids,
            [u32::from_le_bytes(*b"l_01"), u32::from_le_bytes(*b"l_02")]
        );
        assert!(interior_ids
            .iter()
            .all(|id| main.iter().all(|l| l.light_id != *id)));
        assert!(interior
            .iter()
            .all(|l| l.range > 0.0 && l.color.max_element() > 0.0));
    }

    #[test]
    fn vanilla_feed_preserves_monument_light_and_uses_outdoor_clock_track() {
        const LOWER_JEUNO_DAT: u32 = 345;
        let Some(bytes) = crate::weather_particles::tests::zone_dat(LOWER_JEUNO_DAT) else {
            return;
        };
        let lights = point_lights_from_dat(&bytes);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ActiveSceneLights>()
            .init_resource::<crate::graphics_settings::GraphicsSettings>()
            .insert_resource(crate::vana_time::VanaClock::anchored_at_hour(12.0))
            .insert_resource(ZonePointLights {
                file_id: Some(LOWER_JEUNO_DAT),
                sub_area_file_id: None,
                lights,
            })
            .add_systems(Update, build_active_scene_lights);
        app.update();
        let active = app.world().resource::<ActiveSceneLights>();
        let indoor = active
            .lights
            .iter()
            .find(|l| l.light_id == u32::from_le_bytes(*b"c14\0"))
            .unwrap();
        assert_eq!(indoor.range, 18.0);
        assert_eq!(indoor.attenuation, 0.05);
        assert_eq!(indoor.color, Vec3::new(99.0, 74.0, 42.0) / 128.0);
        let outdoor = active
            .lights
            .iter()
            .find(|l| l.light_id == u32::from_le_bytes(*b"pl00"))
            .unwrap();
        assert_eq!(outdoor.color, Vec3::ZERO);
        let night = outdoor.at_time(0.0);
        assert_eq!(night.attenuation, 1.0 / 3.5);
        let (pos, colors, atten) =
            nearest_point_light_arrays(indoor.world_pos, std::slice::from_ref(indoor), 1);
        assert_eq!(colors[0].w, 18.0);
        assert_eq!(atten[0], Vec4::new(0.0, 0.0, 0.05, 0.0));
        assert_eq!(pos[0].xyz(), indoor.world_pos);
    }

    #[test]
    fn terrain_bindings_exclude_character_lights_and_unbound_neighbors() {
        let lights = [
            authored_light(b"pl00", Vec3::ZERO),
            authored_light(b"c14\0", Vec3::ZERO),
            authored_light(b"pl01", Vec3::ZERO),
        ];
        let bindings = [
            Some(lights[2].light_id),
            Some(lights[1].light_id),
            None,
            None,
        ];
        assert_eq!(terrain_point_light_indices(&lights, &bindings), [2]);
        assert_eq!(authored_point_light_indices(&lights, &bindings), [2, 1]);
        assert!(
            terrain_point_light_indices(&lights, &[None; mzb::LIGHT_REFERENCE_COUNT]).is_empty()
        );
    }

    fn light(pos: Vec3, range: f32) -> ZonePointLight {
        ZonePointLight {
            light_id: UNAUTHORED_LIGHT_ID,
            world_pos: pos,
            color: Vec3::splat(1.0),
            range,
            attenuation: 0.25,
            theta_track: None,
            theta_multiplier: 1.0,
        }
    }

    fn authored_light(id: &[u8; 4], pos: Vec3) -> ZonePointLight {
        ZonePointLight {
            light_id: u32::from_le_bytes(*id),
            ..light(pos, 10.0)
        }
    }

    fn slots(ids: &[Option<&[u8; 4]>]) -> [Option<mzb::LightId>; mzb::LIGHT_REFERENCE_COUNT] {
        let mut out = [None; mzb::LIGHT_REFERENCE_COUNT];
        for (slot, id) in ids.iter().enumerate() {
            out[slot] = id.map(|id| u32::from_le_bytes(*id));
        }
        out
    }

    // The binding is by LightID and static per chunk, so the far light stays in
    // and the near unbound one stays out — the property the distance pick cannot
    // have.
    #[test]
    fn authored_pick_takes_the_chunk_s_lights_not_the_nearest() {
        let lights = [
            authored_light(b"li12", Vec3::new(90.0, 0.0, 0.0)),
            authored_light(b"lt01", Vec3::new(1.0, 0.0, 0.0)),
            authored_light(b"l421", Vec3::new(40.0, 0.0, 0.0)),
        ];
        let picked = authored_point_light_indices(&lights, &slots(&[Some(b"l421"), Some(b"li12")]));
        assert_eq!(picked, vec![2, 0], "binding order, not distance order");
        assert_eq!(
            nearest_point_light_indices(Vec3::ZERO, &lights, 4),
            vec![1],
            "the distance pick would have taken the unbound lamp instead"
        );
    }

    #[test]
    fn shadowed_pick_holds_a_member_until_an_outsider_beats_the_margin() {
        let mut world = World::new();
        let (a, b, c) = (
            world.spawn_empty().id(),
            world.spawn_empty().id(),
            world.spawn_empty().id(),
        );
        let margin = 1.5;
        let mut cands = [(a, 10.0, true), (b, 9.0, false), (c, 30.0, false)];
        assert_eq!(
            pick_shadowed(&mut cands, 1, margin),
            vec![a],
            "9 does not beat a member at 10 - 1.5"
        );
        let mut cands = [(a, 10.0, true), (b, 8.0, false), (c, 30.0, false)];
        assert_eq!(pick_shadowed(&mut cands, 1, margin), vec![b], "8 beats 8.5");
        assert_eq!(pick_shadowed(&mut cands, 0, margin), Vec::<Entity>::new());
        assert_eq!(
            pick_shadowed(&mut cands, 5, margin).len(),
            3,
            "the count clamps to the lit set"
        );
    }

    #[test]
    fn authored_slot_whose_light_the_zone_never_defines_drops_out() {
        let lights = [authored_light(b"lt01", Vec3::ZERO)];
        assert_eq!(
            authored_point_light_indices(&lights, &slots(&[Some(b"lt09"), Some(b"lt01")])),
            vec![0],
            "no Generator defines lt09, so retail leaves that slot disabled"
        );
        assert!(authored_point_light_indices(&lights, &slots(&[])).is_empty());
    }

    /// `//lights` emitters carry UNAUTHORED_LIGHT_ID; a chunk binding does not
    /// resolve onto one.
    #[test]
    fn emitters_are_never_bound_by_a_chunk() {
        let lights = [light(Vec3::ZERO, 10.0)];
        let mut all_slots = [Some(UNAUTHORED_LIGHT_ID); mzb::LIGHT_REFERENCE_COUNT];
        all_slots[0] = None;
        assert!(authored_point_light_indices(&lights, &all_slots).is_empty());
    }

    // kuluu-2dzl: the uniform carries MAX_POINT_LIGHTS slots, retail's chunk
    // binding four. The authored feed must fit without truncation.
    #[test]
    fn authored_slots_fit_the_shader_uniform() {
        const { assert!(mzb::LIGHT_REFERENCE_COUNT <= MAX_POINT_LIGHTS) };
        let lights: Vec<ZonePointLight> = [b"li12", b"lt01", b"l421", b"lmb0"]
            .iter()
            .map(|id| authored_light(id, Vec3::ZERO))
            .collect();
        let picked = authored_point_light_indices(
            &lights,
            &slots(&[Some(b"li12"), Some(b"lt01"), Some(b"l421"), Some(b"lmb0")]),
        );
        assert_eq!(picked.len(), mzb::LIGHT_REFERENCE_COUNT);
        let (_, color, _) = point_light_arrays_for(&lights, &picked);
        assert!(
            color[..mzb::LIGHT_REFERENCE_COUNT]
                .iter()
                .all(|c| c.w > 0.0),
            "every authored slot reaches the shader"
        );
    }

    /// point_shadow.wgsl resolves a per-actor slot to its shadow map by matching the
    /// slot's position against the clustered light's, so the uniform pack and the
    /// PointLight entity carry the same f32s for the same light.
    #[test]
    fn packed_slot_position_is_the_spawned_light_s_translation() {
        const NIGHT_VANA_HOUR: f32 = 22.0;
        let pos = Vec3::new(123.456_79, -7.891_011, 0.123_456_79);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<ActiveSceneLights>()
            .init_resource::<crate::graphics_settings::GraphicsSettings>()
            .insert_resource(crate::vana_time::VanaClock::anchored_at_hour(
                NIGHT_VANA_HOUR,
            ))
            .insert_resource(ZonePointLights {
                file_id: None,
                sub_area_file_id: None,
                lights: vec![light(pos, 10.0)],
            })
            .add_systems(
                Update,
                (sync_faithful_zone_light_entities, build_active_scene_lights),
            );
        app.update();

        let spawned: Vec<[u32; 3]> = app
            .world_mut()
            .query_filtered::<&Transform, With<FaithfulZoneLight>>()
            .iter(app.world())
            .map(|t| t.translation.to_array().map(f32::to_bits))
            .collect();
        let active = app.world().resource::<ActiveSceneLights>();
        let (point_pos, _, _) = point_light_arrays_for(&active.lights, &[0]);
        assert_eq!(
            spawned,
            vec![point_pos[0].xyz().to_array().map(f32::to_bits)]
        );
    }

    #[test]
    fn lamp_flicker_bounded_and_never_dark() {
        for i in 0..400 {
            let t = i as f32 * 0.037;
            for seed in 0..8 {
                let f = lamp_flicker(t, seed as f32);
                assert!((0.7..=1.0 + 1e-5).contains(&f), "flicker {f} out of band");
            }
        }
    }

    // The per-actor feed caches the index selection and repacks live colors
    // each frame; the split must reproduce the one-shot picker exactly.
    #[test]
    fn cached_indices_repack_matches_one_shot_pick() {
        let lights = [
            light(Vec3::new(1.0, 0.0, 0.0), 10.0),
            light(Vec3::new(50.0, 0.0, 0.0), 10.0),
            light(Vec3::new(2.0, 0.0, 0.0), 10.0),
            light(Vec3::new(3.0, 0.0, 0.0), 10.0),
        ];
        let indices = nearest_point_light_indices(Vec3::ZERO, &lights, 2);
        assert_eq!(indices, vec![0, 2]);
        assert_eq!(
            point_light_arrays_for(&lights, &indices),
            nearest_point_light_arrays(Vec3::ZERO, &lights, 2)
        );
    }

    #[test]
    fn nearest_picks_four_closest_in_range() {
        let lights = [
            light(Vec3::new(1.0, 0.0, 0.0), 10.0),
            light(Vec3::new(5.0, 0.0, 0.0), 10.0),
            light(Vec3::new(2.0, 0.0, 0.0), 10.0),
            light(Vec3::new(9.0, 0.0, 0.0), 10.0),
            light(Vec3::new(3.0, 0.0, 0.0), 10.0),
        ];
        let (pos, color, atten) = nearest_point_light_arrays(Vec3::ZERO, &lights, 4);

        let xs: Vec<f32> = pos.iter().take(4).map(|p| p.x).collect();
        assert_eq!(
            xs,
            vec![1.0, 2.0, 3.0, 5.0],
            "four nearest, sorted by distance"
        );
        for slot in 0..4 {
            assert_eq!(color[slot].w, 10.0, "point_color.w carries range");
            assert_eq!(
                atten[slot].x, SCENE_LIGHT_CONST_ATTEN,
                "const attenuation term"
            );
            assert_eq!(
                atten[slot].z, 0.25,
                "quad attenuation term = light.attenuation"
            );
        }
    }

    // The bounded insertion pick must match the old collect-then-sort semantics:
    // with more in-range lights than slots, the MAX_POINT_LIGHTS nearest come
    // back in ascending distance order.
    #[test]
    fn overfull_zone_yields_the_nearest_max_in_ascending_order() {
        // Distances 1..=n in scrambled order, so the pick must both evict far
        // lights and insert mid-list.
        let n = MAX_POINT_LIGHTS + 9;
        let lights: Vec<ZonePointLight> = (0..n)
            .map(|i| {
                let d = (i * 7) % n + 1;
                let x = if i % 2 == 0 { d as f32 } else { -(d as f32) };
                light(Vec3::new(x, 0.0, 0.0), 1000.0)
            })
            .collect();
        let mut all: Vec<usize> = lights
            .iter()
            .map(|l| l.world_pos.x.abs() as usize)
            .collect();
        all.sort_unstable();
        assert_eq!(
            all,
            (1..=n).collect::<Vec<_>>(),
            "scramble is a permutation"
        );
        let picked = nearest_point_light_indices(Vec3::ZERO, &lights, MAX_POINT_LIGHTS);
        assert_eq!(picked.len(), MAX_POINT_LIGHTS);
        let dists: Vec<f32> = picked
            .iter()
            .map(|&i| lights[i as usize].world_pos.x.abs())
            .collect();
        let expected: Vec<f32> = (1..=MAX_POINT_LIGHTS as i32).map(|d| d as f32).collect();
        assert_eq!(dists, expected, "nearest MAX_POINT_LIGHTS, ascending");
    }

    #[test]
    fn out_of_range_lights_excluded() {
        let lights = [
            light(Vec3::new(20.0, 0.0, 0.0), 5.0),
            light(Vec3::new(2.0, 0.0, 0.0), 5.0),
        ];
        let (_, color, _) = nearest_point_light_arrays(Vec3::ZERO, &lights, 4);
        assert_eq!(color[0].w, 5.0, "the in-range light fills slot 0");
        assert_eq!(
            color[1].w, 0.0,
            "empty slot stays zero (shader skips range <= 0)"
        );
    }
}
