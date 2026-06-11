// FFXI skinned-character DEPTH/SHADOW prepass shader.
//
// A SEPARATE module from `skinned_ffxi.wgsl` on purpose: Bevy auto-selects a
// shader's entry point only when the module has a single `vertex` / `fragment`
// entry per stage. Keeping the prepass entries here (and the lit entries there)
// means each module has exactly one of each, so the main pipeline and the
// shadow/prepass pipeline each resolve their entry point WITHOUT a manual
// `entry_point` override in `specialize` — the override was fragile because a
// TAA / SSAO camera sets the prepass key bits on the MAIN pass too, which made
// the override mis-route the main pass through this depth path (everything
// rendered black).
//
// Bevy's stock prepass vertex shader reads a standard `POSITION` at
// @location(0); FFXI's dual-position layout has none, so this reuses the exact
// dual-bone skin from the lit shader to emit clip position. The material sets
// `enable_shadows() = true` (so this runs for the directional-light shadow
// pass) and `enable_prepass() = false` (so it does NOT join the camera's
// depth/normal/motion-vector prepass — this module is depth-only and carries no
// motion-vector / normal outputs).
//
// For directional-light shadow views Bevy needs UNCLIPPED depth. On GPUs that
// lack DEPTH_CLIP_CONTROL (e.g. Metal/macOS) it sets
// `UNCLIPPED_DEPTH_ORTHO_EMULATION` and expects the fragment to write
// `@builtin(frag_depth)` — mirrored here exactly as Bevy's own prepass.wgsl.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
}

// Mirror of `FfxiJointMatrices` in skinned_ffxi_material.rs. 128 = MAX_JOINTS.
struct FfxiJoints {
    matrices: array<mat4x4<f32>, 128>,
};

// Mirror of `FfxiMaterialFlags`. `flags.x` = has_texture (1.0 / 0.0).
struct FfxiMaterialFlags {
    flags: vec4<f32>,
};

// Only the bindings this depth pass needs (the lighting uniform at binding 0 is
// part of the bind-group layout but unused here, so it is simply not declared).
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> joints: FfxiJoints;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var<uniform> material_flags: FfxiMaterialFlags;

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

struct PrepassVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(1) unclipped_depth: f32,
#endif
};

@vertex
fn vertex(v: Vertex) -> PrepassVertexOutput {
    var out: PrepassVertexOutput;

    // Same FFXI dual-position skin as the lit shader (see skinned_ffxi.wgsl).
    let w = v.joint_weight;
    let m0 = joints.matrices[v.joint0];
    let m1 = joints.matrices[v.joint1];
    let model_pos = m0 * vec4<f32>(v.position0, w)
                  + m1 * vec4<f32>(v.position1, 1.0 - w);

    let world_from_local = mesh_functions::get_world_from_local(v.instance_index);
    let world_position = world_from_local * vec4<f32>(model_pos.xyz, 1.0);

    out.clip_position = position_world_to_clip(world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.clip_position.z;
    out.clip_position.z = min(out.clip_position.z, 1.0); // clamp to avoid clipping
#endif
    out.uv = v.uv;
    return out;
}

// Cut-out alpha test so transparent texels (hair edges, fabric fringe) don't
// cast solid-block shadows. Mirrors the lit fragment's discard threshold
// (69/255). Untextured C/CS meshes (has_texture == 0) are solid, never discard.
fn prepass_alpha_discard(uv: vec2<f32>) {
    let has_texture = material_flags.flags.x > 0.5;
    if (has_texture) {
        let texel = textureSample(base_tex, base_samp, uv);
        if (texel.a < 0.271) {
            discard;
        }
    }
}

// Two fragment variants by `UNCLIPPED_DEPTH_ORTHO_EMULATION`. The emulation case
// (GPUs lacking DEPTH_CLIP_CONTROL) MUST write `@builtin(frag_depth)`. Otherwise
// the depth-only fragment writes no output (an empty output struct is invalid
// WGSL); it only performs the alpha-test discard.
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
struct PrepassFragmentOutput {
    @builtin(frag_depth) frag_depth: f32,
};

@fragment
fn fragment(in: PrepassVertexOutput) -> PrepassFragmentOutput {
    prepass_alpha_discard(in.uv);
    var out: PrepassFragmentOutput;
    out.frag_depth = in.unclipped_depth;
    return out;
}
#else
@fragment
fn fragment(in: PrepassVertexOutput) {
    prepass_alpha_discard(in.uv);
}
#endif
