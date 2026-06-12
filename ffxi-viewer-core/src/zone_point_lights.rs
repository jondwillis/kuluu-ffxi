//! Faithful FFXI dynamic point lights (braziers, lamps, torches, magic
//! glows) — the retail-accurate source for the character shader's 4 point
//! slots, distinct from `zone_lights.rs`'s Enhanced-only over-bright vertex
//! heuristic.
//!
//! # Source of truth
//!
//! FFXI's only discrete real-time lights are particle/effect *generators*
//! (`Generator` / chunk `0x05`) whose particle is a `PointLight`
//! (`linkedDataType == 0x47`). Cross-referenced against `research/xim`
//! (`Particle.kt:418-426`, `ParticleInitializers.kt`) and **validated against
//! the retail install**: these generators live in the *same* zone DAT the
//! viewer already loads for MZB/weather (the file
//! [`ffxi_dat::zone_dat::zone_id_to_mzb_file_id`] resolves), and each carries
//! its placement in `base_position` as **zone-world coordinates** — so static
//! lights can be placed directly, without XIM's heavyweight `ZoneDefParser`
//! (`pointLightIndex` → `pointLightLinks`).
//!
//! [`ffxi_dat::generator::Generator::parse_point_light`] pre-composes XIM's
//! runtime form: `range = range·rangeMult`, quadratic attenuation
//! `1/(theta·thetaMult)`, `color = particleColor·2`.
//!
//! # What this does
//!
//! On a zone change, [`load_zone_point_lights`] scans the zone DAT for those
//! generators, transforms each `base_position` into Bevy world space via
//! [`crate::scene::ffxi_to_bevy`], and stores them in [`ZonePointLights`].
//! `ffxi_actor_render::update_ffxi_actor_point_lights` then feeds the nearest
//! in-range lights into each character's faithful light uniform.
//!
//! The DAT read is synchronous on the zone-change frame (a one-time hitch on
//! zone-in; the OS file cache makes it cheap since the MZB loader just read
//! the same file). If it ever shows up as a stall it can move onto the MZB
//! background task, which already reads these bytes.
#![cfg(not(target_arch = "wasm32"))]

use bevy::prelude::*;
use ffxi_dat::{chunk::walk, generator::Generator, kind::ChunkKind, zone_dat, DatRoot};
use ffxi_viewer_wire::Vec3 as WireVec3;

use crate::scene::ffxi_to_bevy;
use crate::snapshot::SceneState;

/// One placed zone point light, ready to feed the shader's point slots.
#[derive(Debug, Clone, Copy)]
pub struct ZonePointLight {
    /// Bevy world-space position (the generator's zone-world `base_position`
    /// run through [`ffxi_to_bevy`]).
    pub world_pos: Vec3,
    /// RGB color, already `·2` per XIM's `withMultiplied(2f)`.
    pub color: Vec3,
    /// Hard cutoff radius; the shader skips fragments past this.
    pub range: f32,
    /// Quadratic attenuation coefficient (`1/(theta·thetaMult)`).
    pub attenuation: f32,
    /// `'c'`-prefixed generators are character lights (spell/ability glows)
    /// rather than static zone fixtures. Retained for future zone-object
    /// lighting, which excludes them; actors receive both (XIM characterMode).
    pub is_character: bool,
}

/// All faithful point lights for the current zone. Rebuilt on zone change.
#[derive(Resource, Default)]
pub struct ZonePointLights {
    /// The zone these lights were parsed for, so the loader only re-scans on
    /// an actual zone change (and clears to empty when the zone is unknown).
    pub zone_id: Option<u16>,
    pub lights: Vec<ZonePointLight>,
}

/// Rescan the zone DAT for point-light generators when the zone changes.
fn load_zone_point_lights(scene_state: Res<SceneState>, mut store: ResMut<ZonePointLights>) {
    let current = scene_state.snapshot.zone_id;
    if current == store.zone_id {
        return;
    }
    store.zone_id = current;
    store.lights.clear();

    let Some(zone_id) = current else {
        return;
    };
    let Some(file_id) = zone_dat::zone_id_to_mzb_file_id(zone_id) else {
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
        // `range <= 0` marks a disabled light (e.g. XIM's Delkfutt
        // maxLifeSpan==1 hack); the shader skips a zeroed range anyway.
        if pl.range <= 0.0 {
            continue;
        }
        let bp = WireVec3 {
            x: pl.base_position[0],
            y: pl.base_position[1],
            z: pl.base_position[2],
        };
        store.lights.push(ZonePointLight {
            world_pos: ffxi_to_bevy(bp),
            color: Vec3::new(pl.color[0], pl.color[1], pl.color[2]),
            range: pl.range,
            attenuation: pl.attenuation,
            is_character: c.name.first() == Some(&b'c'),
        });
    }

    info!(
        "zone_point_lights: zone {zone_id} → {} faithful point light(s)",
        store.lights.len()
    );
}

pub struct ZonePointLightsPlugin;

impl Plugin for ZonePointLightsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZonePointLights>()
            .add_systems(Update, load_zone_point_lights);
    }
}
