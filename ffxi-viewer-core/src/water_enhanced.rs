#![cfg(feature = "enhanced-water")]

use bevy::prelude::*;
use bevy_water::material::WaterMaterial;
use bevy_water::{WaterPlugin, WaterSettings};

pub use bevy_water::material::StandardWaterMaterial;

const POND_AMPLITUDE: f32 = 0.1;
const POND_CLARITY: f32 = 0.18;

pub struct EnhancedWaterPlugin;

impl Plugin for EnhancedWaterPlugin {
    fn build(&self, app: &mut App) {
        // spawn_tiles: None suppresses bevy_water's auto ocean grid — we place
        // our own pond surfaces from MZB water_height. Pond-scale amplitude, not
        // open ocean.
        app.insert_resource(WaterSettings {
            spawn_tiles: None,
            amplitude: POND_AMPLITUDE,
            clarity: POND_CLARITY,
            ..default()
        });
        app.add_plugins(WaterPlugin);
    }
}

// Material for a water footprint mesh (built in dat_mzb, world-XZ UVs over the
// `min..max` bounds). coord_offset/scale invert those UVs back to world XZ so the
// wave field is continuous and world-scaled across differently-sized ponds.
pub fn pond_water_material(
    water_materials: &mut Assets<StandardWaterMaterial>,
    min: Vec3,
    max: Vec3,
) -> MeshMaterial3d<StandardWaterMaterial> {
    let dx = (max.x - min.x).max(0.01);
    let dz = (max.z - min.z).max(0.01);
    MeshMaterial3d(water_materials.add(StandardWaterMaterial {
        base: StandardMaterial {
            perceptual_roughness: 0.2,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            ..default()
        },
        extension: WaterMaterial {
            amplitude: POND_AMPLITUDE,
            clarity: POND_CLARITY,
            coord_offset: Vec2::new(min.x, min.z),
            coord_scale: Vec2::new(dx, dz),
            ..default()
        },
    }))
}
