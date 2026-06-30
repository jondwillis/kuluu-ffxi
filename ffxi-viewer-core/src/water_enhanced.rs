#![cfg(feature = "enhanced-water")]

use bevy::mesh::{MeshBuilder, PlaneMeshBuilder};
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

pub fn build_pond_water(
    meshes: &mut Assets<Mesh>,
    water_materials: &mut Assets<StandardWaterMaterial>,
    min: Vec3,
    max: Vec3,
    height: f32,
) -> (Mesh3d, MeshMaterial3d<StandardWaterMaterial>, Transform) {
    let dx = (max.x - min.x).max(0.01);
    let dz = (max.z - min.z).max(0.01);
    let cx = 0.5 * (min.x + max.x);
    let cz = 0.5 * (min.z + max.z);

    let subdivisions = ((dx.max(dz) * 0.5) as u32).clamp(8, 64);
    let mesh = meshes.add(
        PlaneMeshBuilder::from_size(Vec2::new(dx, dz))
            .subdivisions(subdivisions)
            .build(),
    );

    // coord_offset/scale map the plane's 0..1 UVs onto world XZ so the wave
    // function is continuous and world-scaled across differently-sized ponds.
    let mat = water_materials.add(StandardWaterMaterial {
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
    });

    (
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        Transform::from_xyz(cx, height, cz),
    )
}
