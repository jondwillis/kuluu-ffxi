#![cfg(not(target_arch = "wasm32"))]

use bevy::prelude::*;
use ffxi_dat::{chunk::walk, generator::Generator, kind::ChunkKind, DatRoot};
use ffxi_viewer_wire::Vec3 as WireVec3;

use crate::components::InGameEntity;
use crate::scene::mzb_to_bevy;
use crate::snapshot::SceneState;

const FAITHFUL_LIGHT_INTENSITY: f32 = 25_000.0;

// Shared point-light model for the FFXI custom materials (zone + skinned). The
// shader computes `nl * (1/(const + lin*d + quad*d²)) * color`, so colour
// carries strength and the quad term is the per-light falloff.
// Constant term of the inverse-square falloff: peak surface factor is 1/const at
// the lamp base. 1.0 keeps the base a gentle wash (the outer 2x overbright in
// zone_ffxi.wgsl still lifts it) rather than the blinding 2x spotlight 0.5 gave.
const SCENE_LIGHT_CONST_ATTEN: f32 = 1.0;
// Widen reach and use a gentle quad falloff so lanterns light a usable pool
// rather than only a tight base, and so a light entering/leaving the nearest-N
// set near the (now larger) range edge contributes little — softening the pop.
const ZONE_LIGHT_REACH_SCALE: f32 = 2.4;
const SCENE_LIGHT_FALLOFF_K: f32 = 3.0;

// Below this night factor the lamps are treated as fully off (skip the feed
// entirely so daytime costs nothing and surfaces go dark).
const LAMP_OFF_EPSILON: f32 = 0.02;

/// Faithful streetlamp/brazier day-night gate: lamps light at dusk and go out at
/// dawn, driven by the Vana'diel sun altitude (radians, +π/2 zenith, −π/2 nadir).
/// Returns 1.0 once the sun is below the twilight band, 0.0 in full daylight, and
/// a smooth ramp through dusk/dawn. This is a client clock behaviour, not an
/// Events/NPC effect.
pub fn lamp_night_factor(sun_altitude: f32) -> f32 {
    // ~±7° around the horizon: full on just after the sun dips, off just after
    // it rises.
    const LO: f32 = -0.12;
    const HI: f32 = 0.12;
    let t = ((HI - sun_altitude) / (HI - LO)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
// `/lights` emitters are Bevy PointLights with lumen intensity; fold intensity
// into colour magnitude against the faithful reference so a default-intensity
// emitter reads like a colour~1 Generator light.
const EMITTER_MIN_INTENSITY: f32 = 1.0;

#[derive(Debug, Clone, Copy)]
pub struct ZonePointLight {
    pub world_pos: Vec3,

    pub color: Vec3,

    pub range: f32,

    pub attenuation: f32,

    pub is_character: bool,
}

#[derive(Resource, Default)]
pub struct ZonePointLights {
    pub file_id: Option<u32>,
    pub lights: Vec<ZonePointLight>,
}

/// Per-frame merge of every dynamic point light that the FFXI custom materials
/// (zone geometry + skinned actors) consume: the faithful Generator lights and
/// the `/lights` over-bright vertex emitters, expressed in the shared shader
/// convention so one nearest-N picker serves both consumers.
#[derive(Resource, Default)]
pub struct ActiveSceneLights {
    pub lights: Vec<ZonePointLight>,
}

pub fn build_active_scene_lights(
    faithful: Res<ZonePointLights>,
    q_emitters: Query<(&GlobalTransform, &PointLight), With<crate::zone_lights::ZoneLightEmitter>>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    mut active: ResMut<ActiveSceneLights>,
) {
    active.lights.clear();
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
    let night = lamp_night_factor(sky.sun_altitude);
    if night <= LAMP_OFF_EPSILON {
        return;
    }
    let reach = ZONE_LIGHT_REACH_SCALE * settings.light_reach_scale();
    for l in &faithful.lights {
        let range = l.range * reach;
        active.lights.push(ZonePointLight {
            world_pos: l.world_pos,
            color: l.color * night,
            range,
            attenuation: SCENE_LIGHT_FALLOFF_K / (range * range),
            is_character: l.is_character,
        });
    }
    for (gt, pl) in &q_emitters {
        if pl.intensity <= EMITTER_MIN_INTENSITY {
            continue;
        }
        let lin = pl.color.to_linear();
        let mag = pl.intensity / FAITHFUL_LIGHT_INTENSITY * night;
        let range = pl.range.max(1e-3) * ZONE_LIGHT_REACH_SCALE;
        active.lights.push(ZonePointLight {
            world_pos: gt.translation(),
            color: Vec3::new(lin.red, lin.green, lin.blue) * mag,
            range,
            attenuation: SCENE_LIGHT_FALLOFF_K / (range * range),
            is_character: false,
        });
    }
}

/// Pack up to four selected lights into the `(point_pos, point_color,
/// point_atten)` arrays of `FfxiLightingUniform`. `point_color.w` carries range
/// (the shader treats slots with range <= 0 as empty); `point_atten` is
/// `(const, linear, quad, _)`.
fn pack_point_light_arrays(selected: &[ZonePointLight]) -> ([Vec4; 4], [Vec4; 4], [Vec4; 4]) {
    let mut point_pos = [Vec4::ZERO; 4];
    let mut point_color = [Vec4::ZERO; 4];
    let mut point_atten = [Vec4::ZERO; 4];
    for (slot, l) in selected.iter().take(4).enumerate() {
        point_pos[slot] = l.world_pos.extend(0.0);
        point_color[slot] = l.color.extend(l.range);
        point_atten[slot] = Vec4::new(SCENE_LIGHT_CONST_ATTEN, 0.0, l.attenuation, 0.0);
    }
    (point_pos, point_color, point_atten)
}

/// Pick the four nearest in-range lights to `pos` and pack them. Used by the
/// per-actor feed, where popping is invisible (actors are small and moving).
pub fn nearest_point_light_arrays(
    pos: Vec3,
    lights: &[ZonePointLight],
) -> ([Vec4; 4], [Vec4; 4], [Vec4; 4]) {
    let mut best: [(f32, usize); 4] = [(f32::INFINITY, usize::MAX); 4];
    for (i, l) in lights.iter().enumerate() {
        let d2 = pos.distance_squared(l.world_pos);
        if d2 > l.range * l.range || d2 >= best[3].0 {
            continue;
        }
        best[3] = (d2, i);
        let mut j = 3;
        while j > 0 && best[j].0 < best[j - 1].0 {
            best.swap(j, j - 1);
            j -= 1;
        }
    }
    let selected: Vec<ZonePointLight> = best
        .iter()
        .filter(|(_, idx)| *idx != usize::MAX)
        .map(|(_, idx)| lights[*idx])
        .collect();
    pack_point_light_arrays(&selected)
}

// A light stays selected until it leaves this multiple of its range, but only
// enters within 1.0× — the gap is the hysteresis band that stops lights from
// flipping in/out of the four-slot set as the viewer crosses a boundary.
const ZONE_LIGHT_KEEP_FACTOR: f32 = 1.35;

/// Like [`nearest_point_light_arrays`] but with hysteresis for the zone-surface
/// feed: lights already in `selected` are kept while still within their (scaled)
/// keep range, and only the remaining slots are filled by the nearest newcomers.
/// `selected` is the caller's persisted set of chosen world positions, updated
/// in place. This keeps the global four-slot set stable as the viewer moves so
/// surfaces don't pop on/off — the affordable stand-in for true fading, which
/// would need per-frame all-material uploads (see update_zone_material_lighting).
pub fn sticky_nearest_point_light_arrays(
    pos: Vec3,
    lights: &[ZonePointLight],
    selected: &mut Vec<Vec3>,
) -> ([Vec4; 4], [Vec4; 4], [Vec4; 4]) {
    let mut chosen: Vec<ZonePointLight> = Vec::with_capacity(4);

    for keep_pos in selected.iter() {
        if chosen.len() >= 4 {
            break;
        }
        if let Some(l) = lights.iter().find(|l| l.world_pos == *keep_pos) {
            let keep = l.range * ZONE_LIGHT_KEEP_FACTOR;
            if pos.distance_squared(l.world_pos) <= keep * keep {
                chosen.push(*l);
            }
        }
    }

    let mut newcomers: Vec<(f32, ZonePointLight)> = lights
        .iter()
        .filter(|l| {
            pos.distance_squared(l.world_pos) <= l.range * l.range
                && !chosen.iter().any(|c| c.world_pos == l.world_pos)
        })
        .map(|l| (pos.distance_squared(l.world_pos), *l))
        .collect();
    newcomers.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, l) in newcomers {
        if chosen.len() >= 4 {
            break;
        }
        chosen.push(l);
    }

    *selected = chosen.iter().map(|l| l.world_pos).collect();
    pack_point_light_arrays(&chosen)
}

fn load_zone_point_lights(scene_state: Res<SceneState>, mut store: ResMut<ZonePointLights>) {
    let current = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    if current == store.file_id {
        return;
    }
    store.file_id = current;
    store.lights.clear();

    let Some(file_id) = current else {
        return;
    };
    let Ok(root) = DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(loc) = root.resolve(file_id) else {
        return;
    };
    let path = loc.path_under(root.root());
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };

    for c in walk(&bytes) {
        let Ok(c) = c else { continue };
        if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Generator) {
            continue;
        }
        let Ok(Some(pl)) = Generator::parse_point_light(c.data) else {
            continue;
        };

        if pl.range <= 0.0 {
            continue;
        }
        let bp = WireVec3 {
            x: pl.base_position[0],
            y: pl.base_position[1],
            z: pl.base_position[2],
        };
        store.lights.push(ZonePointLight {
            world_pos: mzb_to_bevy(bp),
            color: Vec3::new(pl.color[0], pl.color[1], pl.color[2]),
            range: pl.range,
            attenuation: pl.attenuation,
            is_character: c.name.first() == Some(&b'c'),
        });
    }

    info!(
        "zone_point_lights: DAT {file_id} → {} faithful point light(s)",
        store.lights.len()
    );
}

#[derive(Component)]
struct FaithfulZoneLight {
    base_intensity: f32,
    base_range: f32,
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
    for l in &store.lights {
        if l.is_character {
            continue;
        }

        let peak = l.color.max_element().max(1e-3);
        let hue = l.color / peak;
        let base_intensity = FAITHFUL_LIGHT_INTENSITY * peak;
        commands.spawn((
            FaithfulZoneLight {
                base_intensity,
                base_range: l.range,
            },
            InGameEntity,
            PointLight {
                color: Color::srgb(hue.x, hue.y, hue.z),
                intensity: base_intensity,
                range: l.range * ZONE_LIGHT_REACH_SCALE,
                radius: 0.05,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(l.world_pos),
        ));
    }
}

// Faithful Generator lights are real Bevy point lights (they light StandardMaterial
// props and feed clustered lighting); gate their intensity by the same dusk/dawn
// ramp as the custom-material feed so towns light up only at night.
fn animate_faithful_zone_lights(
    vana_clock: Res<crate::vana_time::VanaClock>,
    settings: Res<crate::graphics_settings::GraphicsSettings>,
    mut q: Query<(&FaithfulZoneLight, &mut PointLight)>,
) {
    let sky = crate::sun_moon::vana_sky_from_clock(&vana_clock);
    let night = lamp_night_factor(sky.sun_altitude);
    let reach = ZONE_LIGHT_REACH_SCALE * settings.light_reach_scale();
    for (l, mut pl) in &mut q {
        pl.intensity = l.base_intensity * night;
        pl.range = l.base_range * reach;
    }
}

pub struct ZonePointLightsPlugin;

impl Plugin for ZonePointLightsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZonePointLights>()
            .init_resource::<ActiveSceneLights>()
            .add_systems(
                Update,
                (
                    load_zone_point_lights,
                    sync_faithful_zone_light_entities,
                    animate_faithful_zone_lights,
                    build_active_scene_lights,
                )
                    .chain(),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn light(pos: Vec3, range: f32) -> ZonePointLight {
        ZonePointLight {
            world_pos: pos,
            color: Vec3::splat(1.0),
            range,
            attenuation: 0.25,
            is_character: false,
        }
    }

    #[test]
    fn lamp_night_factor_on_at_night_off_by_day() {
        assert_eq!(lamp_night_factor(-1.0), 1.0, "deep night: lamps full on");
        assert_eq!(lamp_night_factor(1.0), 0.0, "high noon: lamps off");
        let dusk = lamp_night_factor(0.0);
        assert!(
            dusk > 0.0 && dusk < 1.0,
            "horizon (dusk/dawn) is a partial ramp, got {dusk}"
        );
        assert!(
            lamp_night_factor(-0.05) > lamp_night_factor(0.05),
            "ramp rises as the sun sinks"
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
        let (pos, color, atten) = nearest_point_light_arrays(Vec3::ZERO, &lights);

        let xs: Vec<f32> = pos.iter().map(|p| p.x).collect();
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

    #[test]
    fn out_of_range_lights_excluded() {
        let lights = [
            light(Vec3::new(20.0, 0.0, 0.0), 5.0),
            light(Vec3::new(2.0, 0.0, 0.0), 5.0),
        ];
        let (_, color, _) = nearest_point_light_arrays(Vec3::ZERO, &lights);
        assert_eq!(color[0].w, 5.0, "the in-range light fills slot 0");
        assert_eq!(
            color[1].w, 0.0,
            "empty slot stays zero (shader skips range <= 0)"
        );
    }

    #[test]
    fn sticky_keeps_selected_light_inside_keep_band() {
        let lights = [light(Vec3::ZERO, 10.0)];
        let mut selected = vec![Vec3::ZERO];
        let (_, color, _) =
            sticky_nearest_point_light_arrays(Vec3::new(12.0, 0.0, 0.0), &lights, &mut selected);
        assert_eq!(
            color[0].w, 10.0,
            "past range (10) but inside keep band (13.5), an already-selected light stays"
        );
        assert_eq!(selected, vec![Vec3::ZERO]);
    }

    #[test]
    fn sticky_drops_light_past_keep_band() {
        let lights = [light(Vec3::ZERO, 10.0)];
        let mut selected = vec![Vec3::ZERO];
        let (_, color, _) =
            sticky_nearest_point_light_arrays(Vec3::new(15.0, 0.0, 0.0), &lights, &mut selected);
        assert_eq!(
            color[0].w, 0.0,
            "beyond the keep band (13.5) the light drops"
        );
        assert!(selected.is_empty());
    }

    #[test]
    fn sticky_newcomer_only_enters_within_range() {
        let lights = [light(Vec3::ZERO, 10.0)];
        let mut selected = Vec::new();
        let (_, color, _) =
            sticky_nearest_point_light_arrays(Vec3::new(12.0, 0.0, 0.0), &lights, &mut selected);
        assert_eq!(color[0].w, 0.0, "a fresh light past range does not enter");
        assert!(selected.is_empty());

        let (_, color, _) =
            sticky_nearest_point_light_arrays(Vec3::new(5.0, 0.0, 0.0), &lights, &mut selected);
        assert_eq!(color[0].w, 10.0, "within range it enters");
        assert_eq!(selected, vec![Vec3::ZERO]);
    }
}
