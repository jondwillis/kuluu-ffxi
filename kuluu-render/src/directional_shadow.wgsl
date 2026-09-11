#define_import_path kuluu_render::directional_shadow

#import bevy_pbr::{
    mesh_view_bindings as view_bindings,
    mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT,
    shadows::fetch_directional_shadow,
    view_transformations::position_world_to_view,
}

// Allow transform roundoff, but never use another light's occlusion for a blended model light.
const SHADOW_DIRECTION_DOT_TOLERANCE: f32 = 0.0001;

fn directional_shadow_factor(
    world_pos: vec3<f32>,
    world_normal: vec3<f32>,
    to_light: vec3<f32>,
    frag_coord_xy: vec2<f32>,
) -> f32 {
    let view_z = position_world_to_view(world_pos).z;
    var factor = 1.0;
    for (var i = 0u; i < view_bindings::lights.n_directional_lights; i = i + 1u) {
        let light = &view_bindings::lights.directional_lights[i];
        if (((*light).flags & DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u
            || !any((*light).color.rgb > vec3<f32>(0.0))
            || dot(to_light, (*light).direction_to_light) < 1.0 - SHADOW_DIRECTION_DOT_TOLERANCE) {
            continue;
        }
        factor = min(factor, fetch_directional_shadow(
            i, vec4<f32>(world_pos, 1.0), world_normal, view_z, frag_coord_xy));
    }
    return factor;
}
