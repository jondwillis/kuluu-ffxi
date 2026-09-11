#define_import_path kuluu_render::point_shadow

#import bevy_pbr::{
    mesh_view_bindings as view_bindings,
    mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    clustered_forward as clustering,
    shadows::fetch_point_shadow,
    view_transformations::position_world_to_view,
}

// A per-model light slot names its light only by world position; the shadow map is keyed
// by Bevy's clusterable id. Both are packed from the same ZonePointLight.world_pos
// (zone_point_lights.rs pack_point_light_arrays / sync_faithful_zone_light_entities), so
// the match is bit-exact and this only absorbs roundoff.
const SHADOW_POSITION_TOLERANCE: f32 = 0.01;

fn point_shadow_factor(
    world_pos: vec3<f32>,
    world_normal: vec3<f32>,
    light_pos: vec3<f32>,
    frag_coord_xy: vec2<f32>,
) -> f32 {
    let view_z = position_world_to_view(world_pos).z;
    let is_orthographic = view_bindings::view.clip_from_view[3].w == 1.0;
    let cluster_index = clustering::view_fragment_cluster_index(frag_coord_xy, view_z, is_orthographic);
    let ranges = clustering::unpack_clusterable_object_index_ranges(cluster_index);
    for (var i = ranges.first_point_light_index_offset;
            i < ranges.first_spot_light_index_offset; i = i + 1u) {
        let light_id = clustering::get_clusterable_object_id(i);
        let light = &view_bindings::clustered_lights.data[light_id];
        if (((*light).flags & POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u
            || distance((*light).position_radius.xyz, light_pos) > SHADOW_POSITION_TOLERANCE) {
            continue;
        }
        return fetch_point_shadow(light_id, vec4<f32>(world_pos, 1.0), world_normal, frag_coord_xy);
    }
    return 1.0;
}
