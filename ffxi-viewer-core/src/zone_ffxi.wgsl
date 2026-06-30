// FFXI faithful zone/scenery shader — an UNSKINNED port of the character
// shader (skinned_ffxi.wgsl), reproducing FFXI's zone-mesh lighting model
// (cross-referenced against research/xim's poc/gl/XimShader.kt:179-187):
//
//   out = 2 * (vertexColor * (ambient + 2 directional + 4 point)) * texel
//
// The baked per-vertex colour is the PRIMARY illumination. FFXI stores it as
// byte/128 (dat_mmb.rs), so a baked value of 255 maps to ~2.0 — "overbright".
// That overbright vertex colour, times the ambient floor, times the final 2x
// boost, is what makes lamps/braziers glow at night with no dynamic light.
//
// Bevy's StandardMaterial (which this replaces for zone meshes) treats vertex
// colour as albedo clamped to [0,1] and requires a live light to be visible,
// so at night the whole scene — lamps included — went dark. This shader keeps
// the overbright term and the 2x compositing, matching the actor path.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::{position_world_to_clip, position_world_to_view},
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    shadows,
}

// Mirror of `FfxiLightingUniform` in skinned_ffxi_material.rs — field
// order/types must stay identical so AsBindGroup's std140 layout matches.
struct FfxiLighting {
    ambient: vec4<f32>,
    dir0_dir: vec4<f32>,
    dir0_color: vec4<f32>,
    dir1_dir: vec4<f32>,
    dir1_color: vec4<f32>,
    point_pos: array<vec4<f32>, 4>,
    point_color: array<vec4<f32>, 4>,
    point_atten: array<vec4<f32>, 4>,
};

// Mirror of `FfxiMaterialFlags`. `flags.x` = has_texture (1.0 / 0.0);
// `flags.y` = blend mode (1.0 = translucent water/glass sub, emit real alpha);
// `flags.w` = alpha discard threshold (0.0 = no discard, e.g. opaque subs).
struct FfxiMaterialFlags {
    flags: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> lighting: FfxiLighting;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> material_flags: FfxiMaterialFlags;

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
    return out;
}

// Pure scene light (ambient sky fill + 2 directional + 4 point), no vertex
// colour folded in — the caller multiplies by vertex colour, matching how
// skinned_ffxi.wgsl::scene_irradiance works. Point slots with `point_color.w`
// (range) <= 0 are empty and skipped. v1 zone lighting feeds only ambient +
// the two directionals; the point loop is here for a later per-zone feed.
fn scene_irradiance(n: vec3<f32>, p: vec3<f32>, sun_scale: f32) -> vec3<f32> {
    var rgb = lighting.ambient.rgb;
    let nl0 = max(dot(n, -lighting.dir0_dir.xyz), 0.0);
    rgb += sun_scale * nl0 * lighting.dir0_color.rgb * lighting.dir0_color.w;
    let nl1 = max(dot(n, -lighting.dir1_dir.xyz), 0.0);
    rgb += nl1 * lighting.dir1_color.rgb * lighting.dir1_color.w;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let range = lighting.point_color[i].w;
        if (range > 0.0) {
            let to_light = lighting.point_pos[i].xyz - p;
            let dist = length(to_light);
            if (dist <= range) {
                let a = lighting.point_atten[i].xyz; // (const, linear, quad)
                let denom = a.x + a.y * dist + a.z * dist * dist;
                let inv_sq = select(1.0 / denom, 0.0, denom <= 0.0);
                // Windowed so the contribution reaches exactly 0 at the range
                // edge; a light leaving the 4-slot set was already dark there,
                // so the swap is invisible (no pop-in) and wide ranges are free.
                let t = dist / range;
                let window = (1.0 - t * t);
                let dist_factor = inv_sq * window * window;
                let nl = max(dot(n, to_light / max(dist, 1e-5)), 0.0);
                rgb += nl * dist_factor * lighting.point_color[i].rgb;
            }
        }
    }
    return rgb;
}

// Directional cast-shadow factor for the sun term (dir0). Bevy owns the real
// directional lights + cascade shadow maps at group(0); take the min shadow
// factor over the shadow-enabled ones (1 = lit, 0 = occluded). Mirrors the
// actor shader's sun_shadow_factor. No shadow-enabled light → returns 1.0.
fn sun_shadow_factor(world_pos: vec3<f32>, world_normal: vec3<f32>) -> f32 {
    let view_z = position_world_to_view(world_pos).z;
    let n = view_bindings::lights.n_directional_lights;
    var factor = 1.0;
    for (var i = 0u; i < n; i = i + 1u) {
        let lflags = view_bindings::lights.directional_lights[i].flags;
        if ((lflags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u) {
            continue;
        }
        factor = min(factor, shadows::fetch_directional_shadow(
            i, vec4<f32>(world_pos, 1.0), world_normal, view_z));
    }
    return factor;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let has_texture = material_flags.flags.x > 0.5;
    var texel = vec4<f32>(1.0);
    if (has_texture) {
        texel = textureSample(base_tex, base_samp, in.uv);
        // Alpha test for Mask subs (threshold in flags.w; 0.0 for opaque subs
        // means this never fires). A custom fragment bypasses Bevy's built-in
        // mask handling, so do it manually.
        if (texel.a < material_flags.flags.w) {
            discard;
        }
    }
    let n = normalize(in.world_normal);
    // Cast-shadow attenuation on the sun term only (ambient/point fill the rest,
    // so a shadowed fragment darkens without crushing to black).
    let sun = sun_shadow_factor(in.world_position, n);
    // XIM's `2 * vertexColor * texel`, with vertexColor modulating the scene
    // light. Vertex colour is overbright (can exceed 1) — do NOT clamp it.
    let lit = scene_irradiance(n, in.world_position, sun) * in.color.rgb;
    let rgb = 2.0 * lit * texel.rgb;
    // XIM `ZoneMeshSection` 0x8000 subs blend (`coloredPixel.a = vertexColor.a *
    // texel.a`, clamped). Our vertex alpha is pre-scaled /128 and texel alpha
    // remapped, so the raw product matches XIM's 4·(va/255)·(ta/255). Opaque
    // subs keep alpha 1.0 so ground/wall textures with incidental alpha stay solid.
    var out_alpha = 1.0;
    if (material_flags.flags.y > 0.5) {
        out_alpha = clamp(in.color.a, 0.0, 1.0) * texel.a;
    }
    return vec4<f32>(rgb, out_alpha);
}
