// FFXI zone prepass — unskinned twin of zone_ffxi.wgsl, modelled on Bevy's
// prepass.wgsl (bevy_pbr 0.18). Serves TWO render passes:
//
//   1. The directional-light SHADOW pass (enable_shadows()=true). No prepass
//      defines are set, so only the depth path runs (+ frag_depth under the
//      Metal/macOS UNCLIPPED_DEPTH_ORTHO_EMULATION emulation).
//   2. The CAMERA depth/normal/motion prepass (enable_prepass()=true), used by
//      SSAO (depth+normal), depth-of-field (depth) and TAA (depth+motion).
//      Bevy sets NORMAL_PREPASS / MOTION_VECTOR_PREPASS / PREPASS_FRAGMENT on
//      the *camera* pass only — never on the shadow pass — so the shadow path
//      below is byte-for-byte unchanged from before the prepass unlock.
//
// The FFXI zone vertex buffer layout (built in dat_mmb.rs / ffxi_zone_material's
// specialize) is FIXED: position/normal/uv/color are always present, so the
// Vertex inputs are declared unconditionally (unlike Bevy's generic prepass_io,
// which gates them). Only the outputs and per-pass computations are #ifdef'd.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
}

#ifdef MOTION_VECTOR_PREPASS
#import bevy_pbr::{
    mesh_view_bindings::view,
    prepass_bindings::previous_view_uniforms,
}
#endif

struct FfxiMaterialFlags {
    flags: vec4<f32>,
};

// Only the bindings this pass needs (lighting@0 is in the layout but unused
// here, so simply not declared).
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

struct PrepassVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    @location(2) world_normal: vec3<f32>,
#endif
#ifdef MOTION_VECTOR_PREPASS
    @location(4) world_position: vec4<f32>,
    @location(5) previous_world_position: vec4<f32>,
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @location(6) unclipped_depth: f32,
#endif
};

@vertex
fn vertex(v: Vertex) -> PrepassVertexOutput {
    var out: PrepassVertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(v.instance_index);
    let world_position = world_from_local * vec4<f32>(v.position, 1.0);
    out.clip_position = position_world_to_clip(world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.clip_position.z;
    out.clip_position.z = min(out.clip_position.z, 1.0); // clamp to avoid clipping
#endif
    out.uv = v.uv;
#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(v.normal, v.instance_index);
#endif
#ifdef MOTION_VECTOR_PREPASS
    out.world_position = world_position;
    // Static zone geometry has previous == current model matrix, so the motion
    // vector is correctly zero (Bevy tracks previous transforms for all meshes).
    let prev_world_from_local = mesh_functions::get_previous_world_from_local(v.instance_index);
    out.previous_world_position = prev_world_from_local * vec4<f32>(v.position, 1.0);
#endif
    return out;
}

// Cut-out alpha test so masked texels don't write solid depth/normals (and don't
// cast solid-block shadows). Threshold in flags.w (0.0 for opaque subs → never
// discards). Mirrors the lit fragment.
fn prepass_alpha_discard(uv: vec2<f32>) {
    let has_texture = material_flags.flags.x > 0.5;
    if (has_texture) {
        let texel = textureSample(base_tex, base_samp, uv);
        if (texel.a < material_flags.flags.w) {
            discard;
        }
    }
}

#ifdef MOTION_VECTOR_PREPASS
// Mirror of bevy_pbr::pbr_prepass_functions::calculate_motion_vector.
fn calculate_motion_vector(world_position: vec4<f32>, previous_world_position: vec4<f32>) -> vec2<f32> {
    let clip_position_t = view.unjittered_clip_from_world * world_position;
    let clip_position = clip_position_t.xy / clip_position_t.w;
    let previous_clip_position_t = previous_view_uniforms.clip_from_world * previous_world_position;
    let previous_clip_position = previous_clip_position_t.xy / previous_clip_position_t.w;
    return (clip_position - previous_clip_position) * vec2(0.5, -0.5);
}
#endif

#ifdef PREPASS_FRAGMENT
// Camera prepass: emit the G-buffer outputs the camera requested. PREPASS_FRAGMENT
// is set when any of NORMAL / MOTION_VECTOR / DEFERRED prepass is active.
struct FragmentOutput {
#ifdef NORMAL_PREPASS
    @location(0) normal: vec4<f32>,
#endif
#ifdef MOTION_VECTOR_PREPASS
    @location(1) motion_vector: vec2<f32>,
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    @builtin(frag_depth) frag_depth: f32,
#endif
}

@fragment
fn fragment(in: PrepassVertexOutput) -> FragmentOutput {
    prepass_alpha_discard(in.uv);
    var out: FragmentOutput;
#ifdef NORMAL_PREPASS
    out.normal = vec4(normalize(in.world_normal) * 0.5 + vec3(0.5), 1.0);
#endif
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif
#ifdef MOTION_VECTOR_PREPASS
    out.motion_vector = calculate_motion_vector(in.world_position, in.previous_world_position);
#endif
    return out;
}
#else // PREPASS_FRAGMENT
// Depth-only / shadow pass. Under ortho-depth emulation we must write frag_depth;
// otherwise the fragment only performs the alpha-test discard (an empty output
// struct is invalid WGSL).
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
#endif // PREPASS_FRAGMENT
