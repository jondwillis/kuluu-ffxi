// FFXI faithful skinned-character shader — a WGSL port of FFXI's
// skinned-character shader (cross-referenced against research/xim's
// poc/gl/XimSkinnedShader.kt).
//
// Unlike Bevy's built-in `SkinnedMesh` (single position, inverse-bind
// linear-blend skinning) this reproduces FFXI's actual scheme:
//
//   * Each vertex carries TWO bone-local positions/normals (`position0`
//     / `position1`) and a single blend weight `joint_weight = w`.
//   * `joints[]` are WORLD-SPACE POSE matrices (no inverse bind) — the
//     CPU side (`skeleton_instance.rs`) composes them each frame.
//   * Position: `M0 * vec4(p0, w) + M1 * vec4(p1, 1-w)`. The weight rides
//     in the `w` slot so it scales only each bone's translation column;
//     both rotated positions add at full strength. This matches lotus's
//     `animation_skin.slang` (`R0*p0 + R1*p1 + t0*w + t1*(1-w)`) and is
//     what the known-good CPU bake already does. Rigid (1-bone) verts
//     set `position1 = 0`, `w = 1`, so the second term vanishes.
//   * Normal: weighted blend `w*(M0*n0) + (1-w)*(M1*n1)` (note: the
//     normal IS weighted, the position is not — FFXI's asymmetry).
//   * Shading: `out = 2 * frag_color * texel`, alpha-test discard at
//     ~0.271 (69/255), no backface cull. `frag_color` = FFXI light model
//     (ambient + 2 directional + 4 point) modulated by vertex color.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
}

// Mirror of `FfxiLightingUniform` in skinned_ffxi_material.rs — keep the
// field order/types identical so AsBindGroup's std140 layout matches.
struct FfxiLighting {
    ambient: vec4<f32>,
    dir0_dir: vec4<f32>,
    dir0_color: vec4<f32>,
    dir1_dir: vec4<f32>,
    dir1_color: vec4<f32>,
    point_pos: array<vec4<f32>, 4>,
    point_color: array<vec4<f32>, 4>,
};

// Mirror of `FfxiJointMatrices` in skinned_ffxi_material.rs. 128 = MAX_JOINTS.
struct FfxiJoints {
    matrices: array<mat4x4<f32>, 128>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> lighting: FfxiLighting;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_samp: sampler;
// Per-actor bone world-pose matrices uploaded inline as a uniform (see the
// `FfxiJointMatrices` doc for why this is a uniform, not a storage buffer).
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> joints: FfxiJoints;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position0: vec3<f32>,
    @location(1) position1: vec3<f32>,
    @location(2) normal0: vec3<f32>,
    @location(3) normal1: vec3<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) joint_weight: f32,
    @location(6) joint0: u32,
    @location(7) joint1: u32,
    @location(8) color: vec4<f32>,
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

    let w = v.joint_weight;
    let m0 = joints.matrices[v.joint0];
    let m1 = joints.matrices[v.joint1];

    // FFXI faithful dual-position skinning (see header).
    let model_pos = m0 * vec4<f32>(v.position0, w)
                  + m1 * vec4<f32>(v.position1, 1.0 - w);
    let model_norm = w * (m0 * vec4<f32>(v.normal0, 0.0)).xyz
                   + (1.0 - w) * (m1 * vec4<f32>(v.normal1, 0.0)).xyz;

    // `world_from_local` (the actor's pivot/placement transform) carries
    // the FFXI-engine -> Bevy axis change + heading + feet-on-ground.
    let world_from_local = mesh_functions::get_world_from_local(v.instance_index);
    let world_position = world_from_local * vec4<f32>(model_pos.xyz, 1.0);

    out.world_position = world_position.xyz;
    out.clip_position = position_world_to_clip(world_position.xyz);
    out.world_normal = normalize(mesh_functions::mesh_normal_local_to_world(model_norm, v.instance_index));
    out.uv = v.uv;
    out.color = v.color;
    return out;
}

// FFXI light model: flat ambient + 2 directional (N·L) + 4 point,
// modulated by the per-vertex color. Mirrors XIM's per-vertex lighting
// (we evaluate per-fragment for smoother gradients; the math is the same).
fn ffxi_light(n: vec3<f32>, p: vec3<f32>, vc: vec4<f32>) -> vec4<f32> {
    var rgb = lighting.ambient.rgb;
    rgb += max(dot(n, -lighting.dir0_dir.xyz), 0.0) * lighting.dir0_color.rgb * lighting.dir0_color.w;
    rgb += max(dot(n, -lighting.dir1_dir.xyz), 0.0) * lighting.dir1_color.rgb * lighting.dir1_color.w;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let range = lighting.point_color[i].w;
        if (range > 0.0) {
            let d = p - lighting.point_pos[i].xyz;
            let atten = 1.0 / (1.0 + dot(d, d) / (range * range));
            rgb += atten * lighting.point_color[i].rgb;
        }
    }
    return vec4<f32>(rgb * vc.rgb, vc.a);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(base_tex, base_samp, in.uv);
    // Alpha test (XIM discardThreshold = 69/255). Applied manually since
    // a custom fragment shader bypasses Bevy's built-in mask handling.
    if (texel.a < 0.271) {
        discard;
    }
    let frag_color = ffxi_light(normalize(in.world_normal), in.world_position, in.color);
    // FFXI composites baked-diffuse textures at 2x the lit vertex color.
    let rgb = 2.0 * frag_color.rgb * texel.rgb;
    return vec4<f32>(rgb, frag_color.a * texel.a);
}
