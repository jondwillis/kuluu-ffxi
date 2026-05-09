//! Zone-line markers — visual cues at the positions where stepping
//! triggers a zone transition.
//!
//! The data lives in `ffxi-nav::zonelines` (scraped from
//! `vendor/server/sql/zonelines.sql` at compile time). This module
//! holds the Bevy-side rendering: it accepts a `ZoneLineResolver`
//! closure (so viewer-core stays decoupled from ffxi-nav) and respawns
//! a set of 3D markers each time `snapshot.zone_id` changes.
//!
//! Markers are flat amber cylinders at the FFXI `from_pos`. They are
//! `Pickable::IGNORE` so they never interfere with click-to-target on
//! mobs, and they sit at y=0.05 so they don't z-fight with the ground.

use bevy::picking::Pickable;
use bevy::prelude::*;

use crate::scene::ffxi_to_bevy;
use crate::snapshot::SceneState;
use ffxi_viewer_wire::Vec3 as WireVec3;

/// Slim, viewer-core-local description of a zone-line. Mirrors the
/// fields we need from `ffxi_nav::ZoneLine`, but kept here so this
/// crate doesn't drag in the heavy nav dep.
#[derive(Debug, Clone, Copy)]
pub struct ZoneLineDescriptor {
    pub line_id: u32,
    /// FFXI-native position of the trigger in `from_zone`.
    pub from_pos: [f32; 3],
    pub to_zone: u16,
}

/// Pluggable zone-id → list-of-zone-lines resolver. The native
/// front-end inserts this with `ffxi_nav::zone_lines_for`; the WASM
/// front-end may inject a stub that returns an empty list.
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

/// Tracks which zone we last spawned markers for so we can detect a
/// zone change without doing a deep diff against the snapshot.
#[derive(Resource, Default, Debug)]
pub struct ZoneLineState {
    pub current_zone: Option<u16>,
}

/// Marker component on every spawned zone-line entity. Used by the
/// despawn pass on zone changes.
#[derive(Component, Debug)]
pub struct ZoneLineMarker {
    pub line_id: u32,
    pub to_zone: u16,
}

/// Cached mesh + material for zone-line markers. Spawning ~10 markers
/// per zone every transition is fine even without caching, but doing
/// so keeps the asset count flat across many zone-hops.
#[derive(Resource)]
pub struct ZoneLineAssets {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
}

pub fn setup_zone_line_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // 4-yalm-radius disc, 0.1 yalm thick. Big enough to read at a
    // glance from chase-camera distance, thin enough to look like a
    // floor decal rather than a column.
    let mesh = meshes.add(Cylinder::new(4.0, 0.1));
    // Warm amber, mildly emissive — reads as "interactable trigger"
    // without competing with the brighter target/aggro materials.
    let material = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.75, 0.20, 0.55),
        emissive: LinearRgba::new(0.6, 0.40, 0.05, 1.0),
        alpha_mode: AlphaMode::Blend,
        perceptual_roughness: 0.6,
        metallic: 0.0,
        ..default()
    });
    commands.insert_resource(ZoneLineAssets { mesh, material });
}

/// Rebuild the set of zone-line markers whenever the snapshot's
/// zone_id changes.
pub fn sync_zone_lines_system(
    mut commands: Commands,
    state: Res<SceneState>,
    mut zl_state: ResMut<ZoneLineState>,
    resolver: Option<Res<ZoneLineResolver>>,
    assets: Option<Res<ZoneLineAssets>>,
    existing: Query<Entity, With<ZoneLineMarker>>,
) {
    let Some(zone_id) = state.snapshot.zone_id else {
        return;
    };
    if zl_state.current_zone == Some(zone_id) {
        return;
    }

    // Whether we have a resolver/assets or not, drop existing markers
    // first — they're stale by definition once the zone changed.
    for e in &existing {
        commands.entity(e).despawn();
    }
    zl_state.current_zone = Some(zone_id);

    let (Some(resolver), Some(assets)) = (resolver, assets) else {
        // No resolver registered (e.g. WASM stub or test harness):
        // markers stay cleared. Still record the zone so we don't
        // re-despawn every frame.
        return;
    };

    let descriptors = (resolver.0)(zone_id);
    for desc in descriptors {
        let world_pos = ffxi_to_bevy(WireVec3 {
            x: desc.from_pos[0],
            y: desc.from_pos[1],
            z: desc.from_pos[2],
        }) + Vec3::new(0.0, 0.05, 0.0);

        commands.spawn((
            ZoneLineMarker {
                line_id: desc.line_id,
                to_zone: desc.to_zone,
            },
            Pickable::IGNORE,
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.material.clone()),
            Transform::from_translation(world_pos),
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
