use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::ViewVisibility;
use bevy::math::Affine3A;
use bevy::prelude::*;
use kuluu_render::camera::OperatorCamera;
use kuluu_render::dat_mzb::{DrawDistance, MMB_LOAD_DISTANCE_MARGIN};
use kuluu_render::ffxi_actor_render::FfxiActorMeshChild;
use kuluu_render::lens_flare::SunOcclusion;
use kuluu_render::sun_moon::{sun_angular_radius, sun_direction, VanaSky, SKY_RADIUS};
use kuluu_render::weather::ZoneWeather;

use super::collision_bvh::{CollisionBvh, ZoneCollisionBvh};

// research/xim src/jsMain/kotlin/xim/poc/ParticleDrawer.kt queryLensFlare - retail's flare occlusion
// draws a screen-space quad the size of the sun particle with colour writes off and takes the
// PERCENTAGE of pixels that pass, so the sampled set is the sun's own disc, not a jitter around
// its centre. Equal-area rings make the unoccluded tap count an estimate of that area fraction;
// a point sample is a 0/1 result that flips frame to frame when a grazing ray crosses a ridge.
const SUN_OCCLUSION_RINGS: usize = 3;
const SUN_OCCLUSION_RING_TAPS: usize = 8;
const SUN_OCCLUSION_TAP_COUNT: usize = 1 + SUN_OCCLUSION_RINGS * SUN_OCCLUSION_RING_TAPS;

// research/xim ParticleDrawer.kt renderHazeTexture - retail consumes the occlusion query one frame after
// issuing it, so a short lag is retail-shaped; ours additionally smooths the 1/TAP_COUNT
// quantization of the disc sample (~150ms time constant).
const SUN_VISIBILITY_FADE_PER_SEC: f32 = 6.0;

// Exponential smoothing never reaches its target; snapping within this
// sub-perceptual band lets visibility hit exactly 0.0 (the Hidden draw skip in
// lens_flare_system) and 1.0.
const SUN_VISIBILITY_SNAP_EPS: f32 = 1e-3;

fn occlusion_ray_dirs(sun_dir: Vec3) -> [Vec3; SUN_OCCLUSION_TAP_COUNT] {
    let right = sun_dir.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
    let up = right.cross(sun_dir);
    let disc = sun_angular_radius();
    let mut dirs = [sun_dir; SUN_OCCLUSION_TAP_COUNT];
    let mut n = 1;
    for ring in 0..SUN_OCCLUSION_RINGS {
        // sqrt spacing puts equal disc area between successive rings, so every tap stands for
        // the same share of the sun's screen coverage.
        let r = disc * ((ring + 1) as f32 / SUN_OCCLUSION_RINGS as f32).sqrt();
        // Half-step stagger per ring so taps do not line up radially and read one thin occluder
        // as a whole spoke.
        let phase = ring as f32 * std::f32::consts::TAU / (2.0 * SUN_OCCLUSION_RING_TAPS as f32);
        for tap in 0..SUN_OCCLUSION_RING_TAPS {
            let a = phase + tap as f32 * std::f32::consts::TAU / SUN_OCCLUSION_RING_TAPS as f32;
            dirs[n] = (sun_dir + right * (r * a.cos()) + up * (r * a.sin())).normalize();
            n += 1;
        }
    }
    dirs
}

// research/xim ParticleDrawer.kt queryLensFlare - retail's occlusion query is against the
// depth buffer, so every drawn actor blocks the flare the same as terrain. The zone BVH
// holds no actors; their stand-in is the pose-tracked submesh Aabbs update_actor_mesh_aabbs
// already maintains for frustum culling, ray-tested as OBBs in each mesh's local space.
struct ActorOccluder {
    local_from_world: Affine3A,
    min: Vec3,
    max: Vec3,
}

fn actor_sun_occluder(
    world_from_local: &Affine3A,
    aabb: &Aabb,
    origin: Vec3,
    sun_dir: Vec3,
    reach: f32,
) -> Option<ActorOccluder> {
    let m = world_from_local.matrix3;
    let max_scale = m
        .x_axis
        .length()
        .max(m.y_axis.length())
        .max(m.z_axis.length());
    let radius = max_scale * Vec3::from(aabb.half_extents).length();
    let to_center = world_from_local.transform_point3(aabb.center.into()) - origin;
    let along = to_center.dot(sun_dir);
    if along + radius <= 0.0 || along - radius >= reach {
        return None;
    }
    let perp = (to_center - sun_dir * along).length();
    let corridor = along.max(0.0) * sun_angular_radius().tan() + radius;
    (perp <= corridor).then(|| ActorOccluder {
        local_from_world: world_from_local.inverse(),
        min: aabb.min().into(),
        max: aabb.max().into(),
    })
}

// Slab test; dir is unit-length in world space and stays unnormalized in local
// space, so t is in world units and comparable to reach.
fn ray_hits_actor(occ: &ActorOccluder, origin: Vec3, dir: Vec3, max_t: f32) -> bool {
    let o = occ.local_from_world.transform_point3(origin);
    let d = occ.local_from_world.transform_vector3(dir);
    let inv_d = d.recip();
    let t1 = (occ.min - o) * inv_d;
    let t2 = (occ.max - o) * inv_d;
    let t_enter = t1.min(t2).max_element().max(0.0);
    let t_exit = t1.max(t2).min_element().min(max_t);
    t_enter <= t_exit
}

fn sun_visibility_target(
    bvh: Option<&CollisionBvh>,
    actors: &[ActorOccluder],
    origin: Vec3,
    sun_dir: Vec3,
    reach: f32,
) -> f32 {
    let dirs = occlusion_ray_dirs(sun_dir);
    let unoccluded = dirs
        .iter()
        .filter(|dir| {
            bvh.is_none_or(|bvh| bvh.ray_cast(origin, **dir, reach).is_none())
                && !actors
                    .iter()
                    .any(|a| ray_hits_actor(a, origin, **dir, reach))
        })
        .count();
    unoccluded as f32 / dirs.len() as f32
}

// research/xim ParticleDrawer.kt renderHazeTexture queryLensFlare: retail's flare visibility is a
// depth-buffer occlusion query, so only geometry actually drawn that frame occludes, while our
// collision BVH holds the whole zone block. Three bounds decide what the player can see: zone
// placements are only spawned inside the zone draw distance * MMB_LOAD_DISTANCE_MARGIN
// (dat_mmb.rs process_load_mmb_requests) and culled at the draw distance itself
// (dat_mzb.rs select_zone_mmb_lod), DAT distance fog leaves no contrast past its visibility
// distance while the sky dome is drawn unfogged (weather.rs apply_zone_weather), and the sun
// billboard itself sits at SKY_RADIUS so anything past it is behind the sun.
fn occlusion_reach(draw_distance: f32, fog_visibility: Option<f32>) -> f32 {
    (draw_distance * MMB_LOAD_DISTANCE_MARGIN)
        .min(fog_visibility.unwrap_or(f32::INFINITY))
        .min(SKY_RADIUS)
}

fn smoothed_visibility(current: f32, target: f32, dt_secs: f32) -> f32 {
    let blend = 1.0 - (-SUN_VISIBILITY_FADE_PER_SEC * dt_secs).exp();
    let next = current + (target - current) * blend;
    if (next - target).abs() < SUN_VISIBILITY_SNAP_EPS {
        target
    } else {
        next
    }
}

pub fn update_sun_occlusion_system(
    sky: Res<VanaSky>,
    zone_bvh: Res<ZoneCollisionBvh>,
    settings: Res<kuluu_render::graphics_settings::GraphicsSettings>,
    draw: Res<DrawDistance>,
    zone_weather: Res<ZoneWeather>,
    cam_q: Query<&GlobalTransform, With<OperatorCamera>>,
    actor_q: Query<(&GlobalTransform, &Aabb, &ViewVisibility), With<FfxiActorMeshChild>>,
    time: Res<Time>,
    mut occlusion: ResMut<SunOcclusion>,
) {
    let reach = occlusion_reach(
        draw.world,
        zone_weather.fog_visibility_dist(settings.draw_distance_scale),
    );
    let sun_up = sky.sun_altitude > 0.0;
    let target = match (sun_up, cam_q.single()) {
        (true, Ok(cam)) => {
            let origin = cam.translation();
            let sun_dir = sun_direction(sky.hour);
            // ViewVisibility mirrors retail's depth query: only actors actually drawn
            // last frame occlude (culled, hidden, and first-person-self meshes do not).
            let actors: Vec<ActorOccluder> = actor_q
                .iter()
                .filter(|(_, _, vis)| vis.get())
                .filter_map(|(gt, aabb, _)| {
                    actor_sun_occluder(&gt.affine(), aabb, origin, sun_dir, reach)
                })
                .collect();
            sun_visibility_target(zone_bvh.0.as_ref(), &actors, origin, sun_dir, reach)
        }
        _ => 1.0,
    };
    let next = smoothed_visibility(occlusion.visibility, target, time.delta_secs());
    if next != occlusion.visibility {
        occlusion.visibility = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::f32::consts::TAU;

    const NOON_HOUR: f32 = 12.0;
    const TEST_DT_SECS: f32 = 1.0 / 60.0;

    // A mid-range DAT max_fog_dist_landscape, standing in for the outdoor zone the strobe was
    // reported in.
    const TEST_FOG_VISIBILITY: f32 = 1200.0;

    /// A zone authored to draw past its own fog, as Lower Jeuno is at noon.
    const TEST_ZONE_DRAW_DISTANCE: f32 = 1000.0;

    /// What update_sun_occlusion_system passes in that zone at the vanilla multiplier.
    fn default_reach() -> f32 {
        occlusion_reach(TEST_ZONE_DRAW_DISTANCE, Some(TEST_FOG_VISIBILITY))
    }

    // The point-sample cone this fix replaced (~6px at a 1080p-tall 60-degree FoV viewport).
    const LEGACY_TAP_ANGLE_RAD: f32 = 0.006;

    // Cut-line orientation for the half-coverage case, chosen so no ring tap angle lands on the
    // half-plane boundary (ring taps sit on multiples of TAU/16).
    const HALF_PLANE_CUT_ANGLE_RAD: f32 = TAU / 32.0;

    fn plane_half_cover(
        origin: Vec3,
        sun_dir: Vec3,
        dist: f32,
        cut_angle: f32,
        cut_offset: f32,
        size: f32,
    ) -> Vec<[Vec3; 3]> {
        let right = sun_dir.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
        let up = right.cross(sun_dir);
        let n = up * cut_angle.cos() + right * cut_angle.sin();
        let t = right * cut_angle.cos() - up * cut_angle.sin();
        let centre = origin + sun_dir * dist - n * (size + cut_offset);
        let a = centre + (-n - t) * size;
        let b = centre + (n - t) * size;
        let c = centre + (n + t) * size;
        let d = centre + (-n + t) * size;
        vec![[a, b, c], [a, c, d]]
    }

    fn wall_quad(centre: Vec3, facing: Vec3, half_extent: f32) -> Vec<[Vec3; 3]> {
        let right = facing.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
        let up = right.cross(facing);
        // Asymmetric nudge so no ray lands exactly on the shared diagonal edge
        // of the two triangles.
        let centre = centre + (right * 0.05 + up * 0.03) * half_extent;
        let a = centre + (-right - up) * half_extent;
        let b = centre + (right - up) * half_extent;
        let c = centre + (right + up) * half_extent;
        let d = centre + (-right + up) * half_extent;
        vec![[a, b, c], [a, c, d]]
    }

    #[test]
    fn wall_between_camera_and_sun_fully_occludes() {
        let origin = Vec3::new(0.0, 1.0, 0.0);
        let sun_dir = sun_direction(NOON_HOUR);
        let bvh =
            CollisionBvh::from_world_triangles(wall_quad(origin + sun_dir * 10.0, sun_dir, 50.0));
        assert_eq!(
            sun_visibility_target(Some(&bvh), &[], origin, sun_dir, default_reach()),
            0.0
        );
    }

    #[test]
    fn wall_away_from_the_sun_path_is_fully_visible() {
        let origin = Vec3::new(0.0, 1.0, 0.0);
        let sun_dir = sun_direction(NOON_HOUR);
        let bvh =
            CollisionBvh::from_world_triangles(wall_quad(origin - sun_dir * 10.0, sun_dir, 50.0));
        assert_eq!(
            sun_visibility_target(Some(&bvh), &[], origin, sun_dir, default_reach()),
            1.0
        );
    }

    // research/xim ParticleDrawer.kt renderHazeTexture: retail's flare visibility is a depth-buffer query,
    // so only what was drawn this frame occludes. Collision the player cannot see is invisible
    // to the query. Both walls sit inside SKY_RADIUS so the sun's own sphere is not what
    // rejects the far one.
    #[test]
    fn geometry_beyond_the_rendered_draw_distance_does_not_occlude() {
        let origin = Vec3::new(0.0, 1.0, 0.0);
        let sun_dir = sun_direction(NOON_HOUR);
        let far_dist = TEST_FOG_VISIBILITY * 2.0;
        assert!(far_dist < SKY_RADIUS);
        let far = CollisionBvh::from_world_triangles(wall_quad(
            origin + sun_dir * far_dist,
            sun_dir,
            200.0,
        ));
        assert_eq!(
            sun_visibility_target(Some(&far), &[], origin, sun_dir, default_reach()),
            1.0
        );

        let near = CollisionBvh::from_world_triangles(wall_quad(
            origin + sun_dir * (TEST_FOG_VISIBILITY * 0.5),
            sun_dir,
            200.0,
        ));
        assert_eq!(
            sun_visibility_target(Some(&near), &[], origin, sun_dir, default_reach()),
            0.0
        );
    }

    #[test]
    fn the_reach_is_bounded_by_what_is_drawn() {
        assert_eq!(
            default_reach(),
            TEST_FOG_VISIBILITY,
            "fog must bound the ray in a zone drawn past it"
        );
        assert!(
            default_reach() < SKY_RADIUS,
            "the reach must be shorter than the sun's own sphere"
        );
        assert!(
            occlusion_reach(DrawDistance::default().world, Some(TEST_FOG_VISIBILITY))
                < TEST_FOG_VISIBILITY,
            "before a zone record resolves, the fallback draw distance bounds the ray"
        );

        let cave_draw_distance = 120.0;
        assert_eq!(
            occlusion_reach(cave_draw_distance, None),
            cave_draw_distance * MMB_LOAD_DISTANCE_MARGIN,
            "with no weather record the spawn radius bounds the ray"
        );
        assert_eq!(
            occlusion_reach(f32::MAX, None),
            SKY_RADIUS,
            "geometry past the sun billboard cannot occlude it"
        );
    }

    // Pins the fog contract: the reach a shipping session uses is the distance the emitter
    // (weather.rs apply_zone_weather) hands FogFalloff, not a re-derived copy.
    #[test]
    fn the_reach_fog_bound_is_the_emitters_visibility_distance() {
        let rec = ffxi_dat::weather::WeatherRecord {
            max_fog_dist_landscape: TEST_FOG_VISIBILITY,
            ..Default::default()
        };
        assert_eq!(
            occlusion_reach(
                TEST_ZONE_DRAW_DISTANCE,
                Some(kuluu_render::weather::fog_visibility_dist(
                    &rec,
                    ffxi_dat::mzb::RETAIL_DRAW_DISTANCE_SCALE,
                )),
            ),
            default_reach(),
            "the fog bound must be the emitter's visibility distance, not a local copy"
        );
    }

    #[test]
    fn the_sample_cone_is_the_sun_disc_angular_radius() {
        let sun_dir = sun_direction(NOON_HOUR);
        let dirs = occlusion_ray_dirs(sun_dir);
        assert_eq!(dirs.len(), SUN_OCCLUSION_TAP_COUNT);
        assert!(dirs[0].abs_diff_eq(sun_dir, 1e-6), "centre tap is the axis");
        let mut widest = 0.0f32;
        for dir in dirs {
            assert!((dir.length() - 1.0).abs() < 1e-5, "tap is not unit length");
            widest = widest.max(dir.angle_between(sun_dir));
        }
        assert!(
            (widest - sun_angular_radius()).abs() < 1e-5,
            "outer ring should sit on the sun's own rim, got {widest}"
        );
    }

    // research/xim ParticleDrawer.kt queryLensFlare: the flare's opacity is the percentage of the sun
    // quad's pixels that pass the depth test, so half the disc covered is half visibility.
    #[test]
    fn a_wall_covering_half_the_disc_gives_a_half_area_fraction() {
        let origin = Vec3::ZERO;
        let sun_dir = sun_direction(NOON_HOUR);
        let wall_dist = 100.0;
        let disc_extent = wall_dist * sun_angular_radius();
        let bvh = CollisionBvh::from_world_triangles(plane_half_cover(
            origin,
            sun_dir,
            wall_dist,
            HALF_PLANE_CUT_ANGLE_RAD,
            disc_extent * 0.01,
            disc_extent * 10.0,
        ));
        let vis = sun_visibility_target(Some(&bvh), &[], origin, sun_dir, default_reach());
        let tolerance = 1.5 / SUN_OCCLUSION_TAP_COUNT as f32;
        assert!(
            (vis - 0.5).abs() <= tolerance,
            "half-covered disc should read ~0.5, got {vis}"
        );
    }

    // The direct anti-strobe guard: an occluder narrower than the sun disc dims the flare
    // instead of hard-hiding it, so a grazing ray crossing a ridge slides rather than flips.
    #[test]
    fn a_wall_smaller_than_the_sun_disc_only_dims_the_flare() {
        let origin = Vec3::ZERO;
        let sun_dir = sun_direction(NOON_HOUR);
        let wall_dist = 100.0;
        let bvh = CollisionBvh::from_world_triangles(wall_quad(
            origin + sun_dir * wall_dist,
            sun_dir,
            wall_dist * LEGACY_TAP_ANGLE_RAD,
        ));
        let vis = sun_visibility_target(Some(&bvh), &[], origin, sun_dir, default_reach());
        assert!(
            vis > 0.5 && vis < 1.0,
            "sub-disc occluder should dim, not hide, got {vis}"
        );
    }

    fn cube_occluder(
        center: Vec3,
        half: f32,
        rot: Quat,
        origin: Vec3,
        sun_dir: Vec3,
    ) -> Option<ActorOccluder> {
        actor_sun_occluder(
            &Affine3A::from_scale_rotation_translation(Vec3::ONE, rot, center),
            &Aabb::from_min_max(Vec3::splat(-half), Vec3::splat(half)),
            origin,
            sun_dir,
            default_reach(),
        )
    }

    // Occlusion must hold with no zone BVH (kuluu-6ef7); the rotation exercises the OBB
    // local-space slab path.
    #[test]
    fn an_actor_covering_the_sun_disc_fully_occludes() {
        let origin = Vec3::new(0.0, 1.0, 0.0);
        let sun_dir = sun_direction(NOON_HOUR);
        let rot = Quat::from_axis_angle(sun_dir, TAU / 10.0) * Quat::from_rotation_y(TAU / 12.0);
        let occ = cube_occluder(origin + sun_dir * 5.0, 1.0, rot, origin, sun_dir)
            .expect("a box on the sun ray must survive the corridor prefilter");
        assert_eq!(
            sun_visibility_target(None, &[occ], origin, sun_dir, default_reach()),
            0.0
        );
    }

    #[test]
    fn an_actor_behind_the_camera_is_prefiltered_out() {
        let origin = Vec3::new(0.0, 1.0, 0.0);
        let sun_dir = sun_direction(NOON_HOUR);
        assert!(
            cube_occluder(origin - sun_dir * 5.0, 1.0, Quat::IDENTITY, origin, sun_dir).is_none()
        );
    }

    #[test]
    fn an_actor_off_the_sun_corridor_is_prefiltered_out() {
        let origin = Vec3::new(0.0, 1.0, 0.0);
        let sun_dir = sun_direction(NOON_HOUR);
        let right = sun_dir.cross(Vec3::Y).normalize();
        assert!(cube_occluder(
            origin + sun_dir * 5.0 + right * 10.0,
            1.0,
            Quat::IDENTITY,
            origin,
            sun_dir
        )
        .is_none());
    }

    // A weapon-sized part narrower than the sun disc dims the flare instead of
    // hard-hiding it, same anti-strobe contract as the sub-disc wall test above.
    #[test]
    fn an_actor_part_smaller_than_the_sun_disc_only_dims_the_flare() {
        let origin = Vec3::ZERO;
        let sun_dir = sun_direction(NOON_HOUR);
        let dist = 100.0;
        let sub_disc_half = dist * sun_angular_radius().tan() * 0.2;
        let occ = cube_occluder(
            origin + sun_dir * dist,
            sub_disc_half,
            Quat::IDENTITY,
            origin,
            sun_dir,
        )
        .expect("a sub-disc box on the sun ray must survive the corridor prefilter");
        let vis = sun_visibility_target(None, &[occ], origin, sun_dir, default_reach());
        assert!(
            vis > 0.5 && vis < 1.0,
            "sub-disc actor part should dim, not hide, got {vis}"
        );
    }

    #[test]
    fn smoothing_is_monotone_and_snaps_to_target() {
        for (start, target) in [(1.0_f32, 0.0_f32), (0.0, 1.0)] {
            let mut v = start;
            let mut prev = v;
            for _ in 0..600 {
                v = smoothed_visibility(v, target, TEST_DT_SECS);
                let toward = (v - prev) * (target - start) >= 0.0;
                assert!(toward, "stepped away from target: {prev} -> {v}");
                let overshot = (v - target) * (start - target) < 0.0;
                assert!(!overshot, "overshot target: {v}");
                prev = v;
            }
            assert_eq!(v, target, "did not snap to target");
        }
    }
}
