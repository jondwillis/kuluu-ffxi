use bevy::picking::Pickable;
use bevy::prelude::*;

use crate::graphics_settings::{GraphicsSettings, ZoneLineDisplay};
use crate::scene::ffxi_to_bevy;
use crate::snapshot::SceneState;
use ffxi_viewer_wire::Vec3 as WireVec3;

/// Visible height of a marker (pillar or gate). The real trigger is unbounded
/// vertically; this is just a pleasant on-screen extent centered on the ground.
const MARKER_HEIGHT: f32 = 12.0;

#[derive(Debug, Clone, Copy)]
pub struct ZoneLineDescriptor {
    pub line_id: u32,

    pub from_pos: [f32; 3],
    pub to_zone: u16,

    pub scale_x: f32,
    pub scale_z: f32,
    pub rotation: f32,
}

#[derive(Resource)]
pub struct ZoneLineResolver(pub Box<dyn Fn(u16) -> Vec<ZoneLineDescriptor> + Send + Sync>);

impl ZoneLineResolver {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(u16) -> Vec<ZoneLineDescriptor> + Send + Sync + 'static,
    {
        Self(Box::new(f))
    }
}

#[derive(Resource, Default, Debug)]
pub struct ZoneLineState {
    pub current_zone: Option<u16>,
    pub current_mode: Option<ZoneLineDisplay>,
}

#[derive(Component, Debug)]
pub struct ZoneLineMarker {
    pub line_id: u32,
    pub to_zone: u16,
}

#[derive(Resource)]
pub struct ZoneLineAssets {
    pub pillar_mesh: Handle<Mesh>,
    pub gate_mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
}

pub fn setup_zone_line_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut zl_state: ResMut<ZoneLineState>,
) {
    // Markers carry InGameEntity and are despawned on OnExit(InGame); clear the
    // tracker on re-entry so the sync system respawns even into the same zone.
    *zl_state = ZoneLineState::default();

    let pillar_mesh = meshes.add(Cylinder::new(1.5, MARKER_HEIGHT));
    // Unit cube, scaled per zone line to the real trigger footprint at spawn.
    let gate_mesh = meshes.add(Cuboid::from_size(Vec3::ONE));

    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.75, 0.20, 0.55),
        emissive: LinearRgba::new(0.6, 0.40, 0.05, 1.0),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.6,
        metallic: 0.0,
        ..default()
    });
    commands.insert_resource(ZoneLineAssets {
        pillar_mesh,
        gate_mesh,
        material,
    });
}

pub fn sync_zone_lines_system(
    mut commands: Commands,
    state: Res<SceneState>,
    settings: Res<GraphicsSettings>,
    mut zl_state: ResMut<ZoneLineState>,
    resolver: Option<Res<ZoneLineResolver>>,
    assets: Option<Res<ZoneLineAssets>>,
    existing: Query<Entity, With<ZoneLineMarker>>,
) {
    let Some(zone_id) = state.snapshot.zone_id else {
        return;
    };
    let mode = settings.zone_line_display;
    if zl_state.current_zone == Some(zone_id) && zl_state.current_mode == Some(mode) {
        return;
    }

    for e in &existing {
        commands.entity(e).despawn();
    }
    zl_state.current_zone = Some(zone_id);
    zl_state.current_mode = Some(mode);

    if mode == ZoneLineDisplay::Off {
        return;
    }

    let (Some(resolver), Some(assets)) = (resolver, assets) else {
        return;
    };

    let descriptors = (resolver.0)(zone_id);
    for desc in descriptors {
        let ground_pos = ffxi_to_bevy(WireVec3 {
            x: desc.from_pos[0],
            y: desc.from_pos[1],
            z: desc.from_pos[2],
        });
        let center = ground_pos + Vec3::new(0.0, MARKER_HEIGHT / 2.0, 0.0);

        let (mesh, transform) = match mode {
            ZoneLineDisplay::Off => unreachable!("handled above"),
            ZoneLineDisplay::Pillar => (
                assets.pillar_mesh.clone(),
                Transform::from_translation(center),
            ),
            ZoneLineDisplay::Gate => {
                // The detector (reactor::is_inside_trigger_box) treats
                // from_pos[0]/[1] as the horizontal plane and yaws by `rotation`
                // about vertical; ffxi_to_bevy sends that plane to Bevy's XZ, so
                // a Y-rotation by `rotation` matches the trigger footprint.
                let transform = Transform {
                    translation: center,
                    rotation: Quat::from_rotation_y(desc.rotation),
                    scale: Vec3::new(desc.scale_x.max(0.1), MARKER_HEIGHT, desc.scale_z.max(0.1)),
                };
                (assets.gate_mesh.clone(), transform)
            }
        };

        commands.spawn((
            crate::components::InGameEntity,
            ZoneLineMarker {
                line_id: desc.line_id,
                to_zone: desc.to_zone,
            },
            Pickable::IGNORE,
            Mesh3d(mesh),
            MeshMaterial3d(assets.material.clone()),
            transform,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_roundtrip() {
        let d = ZoneLineDescriptor {
            line_id: 808595578,
            from_pos: [0.786, -10.312, -819.851],
            to_zone: 104,
            scale_x: 1.0,
            scale_z: 4.0,
            rotation: 4.18879,
        };
        assert_eq!(d.line_id, 808595578);
        assert_eq!(d.to_zone, 104);
    }

    #[test]
    fn resolver_can_be_constructed() {
        let r = ZoneLineResolver::new(|_| {
            vec![ZoneLineDescriptor {
                line_id: 1,
                from_pos: [0.0, 0.0, 0.0],
                to_zone: 2,
                scale_x: 1.0,
                scale_z: 1.0,
                rotation: 0.0,
            }]
        });
        let result = (r.0)(0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].to_zone, 2);
    }

    #[test]
    fn resolver_default_returns_empty_for_unknown() {
        let r = ZoneLineResolver::new(|_| Vec::new());
        assert!((r.0)(0xFFFF).is_empty());
    }

    #[test]
    fn zone_line_state_default_is_none() {
        let s = ZoneLineState::default();
        assert!(s.current_zone.is_none());
    }
}
