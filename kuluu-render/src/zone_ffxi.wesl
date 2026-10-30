import bevy_pbr::render::{
    mesh_functions,
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    pbr_lighting::getRangeFalloff,
    view_transformations::position_world_to_clip,
};
import kuluu_render::directional_shadow::directional_shadow_factor;
import kuluu_render::point_shadow::point_shadow_factor;
@if(DISTANCE_FOG)
import bevy_pbr::render::fog as fog_fns;

// FFXI faithful zone/scenery shader — an UNSKINNED port of the character
// shader (skinned_ffxi.wesl), reproducing FFXI's zone-mesh texture-stage chain.
// The baked per-vertex colour is the PRIMARY illumination; it is what makes
// lamps/braziers glow at night with no dynamic light.
//
// research/XIClient Rendering/Direct3D8Manager.cpp:373,390,393,395 — zone draws run
// fixed-function T&L with COLORVERTEX on and both DIFFUSE and AMBIENT sourced from
// D3DMCS_COLOR1, so the lit vertex term is emitted as a D3DCOLOR and is saturated
// before it reaches any texture stage. D3D then saturates each stage result in turn.
// Two stage setups consume that term, selected by FFXI_GENERATOR_STAGE_CHAIN:
//
//   terrain   (ZoneRenderer.cpp:2453-2455 ApplyPreRenderState)
//     out = saturate(2 * texel * saturate(vertexColor * light))
//   generator (CMoD3m.cpp:53-70 NonZeroTwoTSS, via CMoD3mElem.cpp:57-63 DoMMBDraw)
//     out = saturate(2 * tint * saturate(2 * vertexColor * light * texel))
//
// `tint` is the generator's D3DRS_TEXTUREFACTOR; on the terrain chain it is white.

// D3DTOP_MODULATE2X's gain. Both retail stage chains are built out of it.
const D3D_MODULATE_2X: f32 = 2.0;

// Fraction of the sun term a fully shadowed fragment keeps. A tuning, not a retail
// value: retail casts no shadow map over zone geometry at all, so any floor above 0
// is already a deliberate departure. Kept identical to skinned_ffxi.wesl's
// FFXI_SHADOW_FLOOR (guard test `shadow_floor_matches_the_actor_shader`) so terrain
// and the characters standing on it shade by the same amount.
const FFXI_SHADOW_FLOOR: f32 = 0.45;

// Distance fog. FFXI fades distant terrain/water into the horizon backdrop
// (the weather-DAT `fog_landscape` colour, also painted as ClearColor). Bevy
// sets the `DistanceFog` on the camera (weather.rs), but this custom material
// bypasses the StandardMaterial fragment that would apply it — without this the
// far terrain falls straight to the void behind the sky dome. The `fog` binding
// (mesh_view_bindings group 0 @binding 13) and this def exist only when the
// view carries a DistanceFog (mesh.rs pushes the DISTANCE_FOG shaderdef).

// Mirror of `FfxiLightingUniform` in skinned_ffxi_material.rs — field
// order/types must stay identical so AsBindGroup's std140 layout matches.
struct FfxiLighting {
    ambient: vec4<f32>,
    dir0_dir: vec4<f32>,
    dir0_color: vec4<f32>,
    dir1_dir: vec4<f32>,
    dir1_color: vec4<f32>,
    point_pos: array<vec4<f32>, 16>,
    point_color: array<vec4<f32>, 16>,
    point_atten: array<vec4<f32>, 16>,
    // x = elapsed seconds, y = wind strength, z/w reserved.
    time_params: vec4<f32>,
};

// Mirror of `FfxiMaterialFlags`. `flags.x` = has_texture (1.0 / 0.0);
// `flags.y` = blend mode (1.0 = translucent water/glass sub, emit real alpha);
// `flags.z` = skip distance fog (ffxi_zone_material.rs ZONE_FLAG_UNFOGGED);
// `flags.w` = alpha discard threshold (0.0 = no discard, e.g. opaque subs).
struct FfxiMaterialFlags {
    flags: vec4<f32>,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<uniform> lighting: FfxiLighting;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var<uniform> material_flags: FfxiMaterialFlags;
@group(constants::MATERIAL_BIND_GROUP) @binding(5) var<uniform> uv_offset: vec4<f32>;
// Per-mesh ToD tint: rgb is the cloud/sun-mesh color setter, w an alpha multiplier.
// White (1,1,1,1) for every non-cloud zone mesh, so this is a no-op there.
@group(constants::MATERIAL_BIND_GROUP) @binding(4) var<uniform> tint: vec4<f32>;

struct ZonePointLighting {
    positions: array<vec4<f32>, 4>,
    colors: array<vec4<f32>, 4>,
    attenuation: array<vec4<f32>, 4>,
};
@group(constants::MATERIAL_BIND_GROUP) @binding(6) var<uniform> points: ZonePointLighting;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) color: vec4<f32>,
    @location(4) point_light: vec3<f32>,
};

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;
    // Standard (unskinned) mesh placement: the MMB instance's world transform.
    let world_from_local = mesh_functions::get_world_from_local(v.instance_index);
    let world_position = world_from_local * vec4<f32>(v.position, 1.0);
    out.world_position = world_position.xyz;
    out.clip_position = position_world_to_clip(world_position.xyz);
    out.world_normal = normalize(mesh_functions::mesh_normal_local_to_world(v.normal, v.instance_index));
    out.uv = v.uv;
    out.color = v.color;
    out.point_light = authored_point_irradiance(out.world_normal, out.world_position);
    return out;
}

fn authored_point_irradiance(n: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    var rgb = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i += 1u) {
        let range = points.colors[i].w;
        if (range <= 0.0) { continue; }
        let to_light = points.positions[i].xyz - p;
        let dist = length(to_light);
        if (dist > range) { continue; }
        let a = points.attenuation[i].xyz;
        let denom = a.x + a.y * dist + a.z * dist * dist;
        let nl = max(dot(n, to_light / max(dist, 1e-5)), 0.0);
        rgb += nl * points.colors[i].rgb / max(denom, 1e-5);
    }
    return rgb;
}

fn shadowed_point_irradiance(n: vec3<f32>, p: vec3<f32>, frag: vec2<f32>) -> vec3<f32> {
    var rgb = vec3<f32>(0.0);
    for (var i = 0u; i < 4u; i += 1u) {
        let range = points.colors[i].w;
        if (range <= 0.0) { continue; }
        let to_light = points.positions[i].xyz - p;
        let dist = length(to_light);
        if (dist > range) { continue; }
        let a = points.attenuation[i].xyz;
        let denom = a.x + a.y * dist + a.z * dist * dist;
        let nl = max(dot(n, to_light / max(dist, 1e-5)), 0.0);
        let shadow = point_shadow_factor(p, n, points.positions[i].xyz, frag);
        let range_fade = getRangeFalloff(dist * dist, 1.0 / (range * range));
        rgb += range_fade * shadow * nl * points.colors[i].rgb / max(denom, 1e-5);
    }
    return rgb;
}

fn scene_irradiance(n: vec3<f32>, p: vec3<f32>, shadow_scale: vec2<f32>, frag_coord: vec2<f32>) -> vec3<f32> {
    var rgb = lighting.ambient.rgb;
    let nl0 = max(dot(n, -lighting.dir0_dir.xyz), 0.0);
    rgb += shadow_scale.x * nl0 * lighting.dir0_color.rgb * lighting.dir0_color.w;
    let nl1 = max(dot(n, -lighting.dir1_dir.xyz), 0.0);
    rgb += shadow_scale.y * nl1 * lighting.dir1_color.rgb * lighting.dir1_color.w;
    return rgb;
}

// Fade a lit fragment toward the fog colour by view distance. Scattering is
// left at zero (the weather DAT drives a flat horizon colour, not sun-inscatter
// fog), so this is the plain distance blend Bevy's `apply_fog` does. No-op when
// the view has no DistanceFog (the whole body compiles out).
fn apply_distance_fog(color: vec4<f32>, world_pos: vec3<f32>) -> vec4<f32> {
    @if(DISTANCE_FOG)
    let fog_params = view_bindings::fog;
    @if(DISTANCE_FOG)
    let dist = length(world_pos - view_bindings::view.world_position);
    @if(DISTANCE_FOG)
    let scattering = vec3<f32>(0.0);
    @if(DISTANCE_FOG)
    if (fog_params.mode == mesh_view_types::FOG_MODE_LINEAR) {
        return fog_fns::linear_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_EXPONENTIAL) {
        return fog_fns::exponential_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_EXPONENTIAL_SQUARED) {
        return fog_fns::exponential_squared_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_ATMOSPHERIC) {
        return fog_fns::atmospheric_fog(fog_params, color, dist, scattering);
    }
    return color;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let has_texture = material_flags.flags.x > 0.5;
    var texel = vec4<f32>(1.0);
    if (has_texture) {
        texel = textureSample(base_tex, base_samp, in.uv + uv_offset.xy);
    }
    // XIM `coloredPixel.a = vertexColor.a * texel.a` (XIM's 4·(va/255)·(ta/255)
    // matches our /128 vertex alpha × remapped texel alpha). Vertex alpha is a
    // second alpha-clip layer FFXI leans on for river/water edges, so fold it
    // into the cutout discard — not just the blend output. Opaque subs carry
    // flags.w = 0, so the test never fires and ground/walls stay solid. A custom
    // fragment bypasses Bevy's built-in mask handling, so do the test manually.
    let combined_a = clamp(in.color.a, 0.0, 1.0) * texel.a;
    if (combined_a < material_flags.flags.w) {
        discard;
    }
    let n = normalize(in.world_normal);
    let shadow_scale = mix(
        vec2<f32>(FFXI_SHADOW_FLOOR),
        vec2<f32>(1.0),
        vec2<f32>(
            directional_shadow_factor(in.world_position, n, -lighting.dir0_dir.xyz, in.clip_position.xy),
            directional_shadow_factor(in.world_position, n, -lighting.dir1_dir.xyz, in.clip_position.xy),
        ),
    );
    var point_light = in.point_light;
    if (lighting.time_params.z > 0.0) {
        point_light = shadowed_point_irradiance(n, in.world_position, in.clip_position.xy);
    }
    let lit = (scene_irradiance(n, in.world_position, shadow_scale, in.clip_position.xy) + point_light) * in.color.rgb;
    // research/xim ParticleGeneratorParser.kt:431-434: ToD color.rgb is a setter folded
    // over the lit texel; color multiplier (.w) scales the emitted alpha.
    @if(FFXI_GENERATOR_STAGE_CHAIN)
    let stage0 = saturate(D3D_MODULATE_2X * saturate(lit) * texel.rgb);
    @if(FFXI_GENERATOR_STAGE_CHAIN)
    let rgb = saturate(D3D_MODULATE_2X * stage0 * tint.rgb);
    @if(!FFXI_GENERATOR_STAGE_CHAIN)
    let rgb = saturate(D3D_MODULATE_2X * saturate(lit) * texel.rgb * tint.rgb);
    // 0x8000 subs (water/glass) emit the blended alpha; everything else opaque.
    var out_alpha = 1.0;
    if (material_flags.flags.y > 0.5) {
        out_alpha = combined_a;
    }
    var out_color = vec4<f32>(rgb, out_alpha * tint.w);
    // research/XIClient CMoElem.cpp:542-543: fog is a per-generator render state. The
    // weat/ sky layers that clear it sit past every 0x2F fog distance, so fogging them
    // would swap their colour for the horizon tint outright.
    if (material_flags.flags.z < 0.5) {
        out_color = apply_distance_fog(out_color, in.world_position);
    }
    return out_color;
}
