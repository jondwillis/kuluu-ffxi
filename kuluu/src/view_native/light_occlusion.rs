//! Occlusion-aware point lights: a lamp only reaches surfaces the zone geometry lets it.
//!
//! The DAT's per-chunk light bindings decide WHICH slots a surface may receive, but nothing in
//! them says whether the lamp is actually visible from that surface — a wall behind a pillar
//! within range still got painted by the full attenuation term. This system closes that gap:
//! for every authored (surface, lamp) pair it raycasts lamp -> surface through the zone
//! collision BVH and zeroes the slot when geometry stands between them. The zeroed binding is
//! written back into the material asset, so `update_zone_point_lighting` repacks the chunk's
//! point-light arrays from the reduced set on its own (no second write path).
//!
//! Zeroing is reversible while an AnimationTest box toggle is in play: before a material loses
//! its first slot, its authored bindings are stashed ([`AuthoredZoneBindings`]), and flipping
//! the toggle to rays-off writes them back. Without a toggle there is nothing to restore *to*,
//! so no stash is kept and zeroing stands for the zone load's lifetime (the pre-existing
//! behaviour).
//!
//! Lamps gated off by time of day carry zero colour (`zone_point_lights::at_time`) and already
//! contribute nothing; their bindings are left intact so they return when the track turns them
//! back on.
//!
//! Actors get the same test against their current position: a person standing in a pillar's
//! shadow stops receiving that lamp. The filter runs before `update_ffxi_actor_point_lights`
//! so the registry write of the frame uses the filtered indices; when that system recomputes
//! a selection (the actor moved past its reselect epsilon) the unfiltered set is visible for at
//! most one frame, which reads as nothing.

use std::collections::HashMap;

use bevy::prelude::*;
use kuluu_render::{
    ffxi_actor_render::FfxiRenderActor,
    ffxi_zone_material::{FfxiZoneMaterial, ZoneLightBindings},
    zone_point_lights::ActiveSceneLights,
};

use super::animation_test_scene::LampRaysOff;
use super::collision_bvh::ZoneCollisionBvh;

// The lamp fixture mesh sits at the light position (the DAT's basePosition is inside the
// lantern), so a ray started there hits its own housing and occludes everything. Start past it.
const LIGHT_OCCLUSION_NEAR_CLIP: f32 = 1.0;

/// True when any zone collision geometry stands between `from` and `to`.
fn bvh_blocks(bvh: &ZoneCollisionBvh, from: Vec3, to: Vec3) -> bool {
    let dir = to - from;
    let dist = dir.length();
    if dist <= LIGHT_OCCLUSION_NEAR_CLIP {
        return false;
    }
    let d = dir / dist;
    let origin = from + d * LIGHT_OCCLUSION_NEAR_CLIP;
    bvh.0.as_ref().is_some_and(|b| {
        b.ray_cast(origin, d, dist - LIGHT_OCCLUSION_NEAR_CLIP)
            .is_some()
    })
}

/// The authored bindings of every material the raycast has zeroed at least one slot in, kept
/// so a rays-off toggle can restore them. Only populated while an AnimationTest box could
/// restore — without the toggle there is no path back, and stash-free zeroing avoids leaking a
/// copy per zone load.
#[derive(Resource, Default)]
pub(crate) struct AuthoredZoneBindings(HashMap<Handle<FfxiZoneMaterial>, ZoneLightBindings>);

/// Zero the authored light slots of every zone material whose lamp is occluded from that
/// surface, and drop the same lamps from actors' cached point-light selections. Runs after the
/// BVH build (so a freshly loaded zone has its geometry) and before the actor registry write;
/// the zone-material repack lands on `update_zone_point_lighting`'s next pass, one frame later —
/// occlusion only changes when a zone loads or an actor walks between lamp and wall, so the lag
/// is not visible.
pub(crate) fn apply_light_occlusion_system(
    bvh: Res<ZoneCollisionBvh>,
    active: Option<Res<ActiveSceneLights>>,
    toggle: Option<Res<LampRaysOff>>,
    q_zone: Query<(Entity, &GlobalTransform, &MeshMaterial3d<FfxiZoneMaterial>)>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
    mut authored: ResMut<AuthoredZoneBindings>,
    mut q_actors: Query<&mut FfxiRenderActor, With<GlobalTransform>>,
) {
    let Some(active) = active else { return };
    if active.lights.is_empty() || bvh.0.is_none() {
        return;
    }

    if toggle.is_some() && !authored.0.is_empty() {
        authored.0.retain(|handle, _| materials.contains(handle));
    }

    let stashable = toggle.is_some();
    for (_entity, gt, mat_handle) in &q_zone {
        let Some(mut mat) = materials.get_mut(&mat_handle.0) else {
            continue;
        };
        // No authored binding: nothing to occlude (the nearest-N fallback path is the
        // no-binding zone's only lighting and stays as-is).
        if !mat.light_bindings.iter().any(Option::is_some) {
            continue;
        }
        let target = gt.translation();
        for slot in 0..mat.light_bindings.len() {
            let Some(id) = mat.light_bindings[slot] else {
                continue;
            };
            // ToD-gated-off lamps carry zero colour and contribute nothing; leave the binding
            // intact so the lamp returns when its track turns it back on.
            let Some(light) = active.lights.iter().find(|l| l.light_id == id) else {
                continue;
            };
            if light.color != Vec3::ZERO && bvh_blocks(&bvh, light.world_pos, target) {
                if stashable {
                    authored
                        .0
                        .entry(mat_handle.0.clone())
                        .or_insert_with(|| mat.light_bindings);
                }
                mat.light_bindings[slot] = None;
            }
        }
    }

    for mut actor in &mut q_actors {
        let Some(eval_pos) = actor.point_light_eval_pos() else {
            continue;
        };
        actor.filter_point_light_selection(|i| {
            active
                .lights
                .get(i as usize)
                .is_some_and(|l| !bvh_blocks(&bvh, l.world_pos, eval_pos))
        });
    }
}

/// The edge of the box's toggle flipping to rays-off: put every stashed authored binding back
/// so switching off returns the *current* zone to its authored lighting, not just freshly loaded
/// ones. Without this the slots zeroed while rays were on persist in the material asset until a
/// zone reload and rays-off reads as a no-op (kuluu lamp-room sessions hit exactly that).
pub(crate) fn restore_light_bindings_on_rays_off(
    rays: Option<Res<LampRaysOff>>,
    mut authored: ResMut<AuthoredZoneBindings>,
    mut materials: ResMut<Assets<FfxiZoneMaterial>>,
) {
    let Some(rays) = rays else { return };
    if !rays.is_changed() || !rays.0 {
        return;
    }
    // Drain everything: handles whose assets are gone would otherwise leak across zone loads,
    // and there is nothing left to restore once a zone unloads anyway.
    for (handle, bindings) in authored.0.drain() {
        if let Some(mut mat) = materials.get_mut(&handle) {
            // The asset mutation dirties the material; update_zone_point_lighting repacks from
            // the restored bindings on its next pass.
            mat.light_bindings = bindings;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_render::{
        dat_mzb::MzbCollisionGeometry,
        ffxi_actor_render::{render_actor_stub, update_ffxi_actor_point_lights},
        graphics_settings::GraphicsSettings,
        skinned_ffxi_material::FfxiSkinRegistry,
        zone_point_lights::{ZonePointLight, UNAUTHORED_LIGHT_ID},
    };

    #[test]
    fn stationary_actor_relights_after_a_daytime_lamp_pause() {
        let mut app = App::new();
        app.init_resource::<Assets<FfxiZoneMaterial>>()
            .init_resource::<AuthoredZoneBindings>()
            .init_resource::<MzbCollisionGeometry>()
            .init_resource::<GraphicsSettings>()
            .init_resource::<kuluu_render::ffxi_zone_material::ZoneGlobalLighting>()
            .init_resource::<FfxiSkinRegistry>()
            .insert_resource(ZoneCollisionBvh(Some(
                super::super::collision_bvh::CollisionBvh::from_world_triangles(Vec::new()),
            )))
            .insert_resource(ActiveSceneLights {
                lights: vec![ZonePointLight {
                    light_id: UNAUTHORED_LIGHT_ID,
                    world_pos: Vec3::new(2.0, 2.0, 0.0),
                    color: Vec3::ONE,
                    range: 10.0,
                    attenuation: 1.0,
                    theta_track: None,
                    theta_multiplier: 1.0,
                }],
            })
            .add_systems(
                Update,
                (apply_light_occlusion_system, update_ffxi_actor_point_lights).chain(),
            );
        let actor = render_actor_stub(1);
        let slot = actor.skin_slot();
        app.world_mut().spawn((actor, GlobalTransform::IDENTITY));
        app.update();
        let lighting = |app: &App| {
            app.world()
                .resource::<FfxiSkinRegistry>()
                .skin(slot)
                .lighting
                .point_color
        };
        let night = lighting(&app);
        assert!(night.iter().any(|c| c.truncate() != Vec3::ZERO));

        app.world_mut().resource_mut::<ActiveSceneLights>().lights[0].color = Vec3::ZERO;
        app.update();
        assert!(lighting(&app).iter().all(|c| c.truncate() == Vec3::ZERO));
        app.world_mut().resource_mut::<ActiveSceneLights>().lights[0].color = Vec3::ONE;
        app.update();
        assert_eq!(lighting(&app), night);
    }
}
