import kuluu_render::actor_reveal::{reveal_threshold, reveal_edge};
import bevy_pbr::render::{
    mesh_functions,
    view_transformations::position_world_to_clip,
};
@if(MOTION_VECTOR_PREPASS)
import bevy_pbr::prepass::bindings::previous_view_uniforms;
@if(MOTION_VECTOR_PREPASS)
import bevy_pbr::render::{
    mesh_view_bindings::view,
};

// FFXI skinned-character prepass shader. Twin of skinned_ffxi.wesl, modelled on
// Bevy's prepass.wgsl (bevy_pbr 0.18). Serves TWO render passes:
//
//   1. The directional-light SHADOW pass (enable_shadows()=true): no prepass
//      defines set → depth-only path (+ frag_depth under the Metal/macOS
//      UNCLIPPED_DEPTH_ORTHO_EMULATION emulation). UNCHANGED by the unlock.
//   2. The CAMERA depth/normal/motion prepass (enable_prepass()=true): SSAO
//      (depth+normal), depth-of-field (depth) and TAA (depth+motion). Bevy sets
//      NORMAL_PREPASS / MOTION_VECTOR_PREPASS / PREPASS_FRAGMENT on the camera
//      pass only.
//
// A SEPARATE module from skinned_ffxi.wesl on purpose: Bevy auto-selects a
// shader's entry point only when the module has a single vertex/fragment entry
// per stage. Keeping the prepass entries here (lit entries there) lets each
// pipeline resolve its entry point WITHOUT a manual `entry_point` override —
// that override mis-routed the main pass through this depth path once a TAA/SSAO
// camera set the prepass key bits, rendering everything black.
//
// Bevy's stock prepass reads a standard POSITION at @location(0); FFXI's
// dual-position layout has none, so this reuses the exact dual-bone skin from
// the lit shader to emit clip position, world normal and motion vectors.
//
// MOTION-VECTOR LIMITATION: the FFXI joint palette (FfxiJoints) is single-
// buffered — there are no previous-frame bone matrices. So the previous world
// position is approximated as `previous_entity_transform * current_skinned_pos`.
// This captures whole-actor (rigid-body) motion for TAA/motion-blur but NOT
// per-limb articulation, so fast-moving limbs may still ghost slightly. This is
// strictly better than before (actors were absent from the camera prepass
// entirely, so TAA had zero motion data for them). A faithful fix needs a
// double-buffered joint palette.

// Mirror of `FfxiLightingUniform` in skinned_ffxi_material.rs — declared only
// because `FfxiSkin` embeds it (this pass reads joints, never lighting).
struct FfxiLighting {
    ambient: vec4<f32>,
    dir0_dir: vec4<f32>,
    dir0_color: vec4<f32>,
    dir1_dir: vec4<f32>,
    dir1_color: vec4<f32>,
    point_pos: array<vec4<f32>, 16>,
    point_color: array<vec4<f32>, 16>,
    point_atten: array<vec4<f32>, 16>,
    time_params: vec4<f32>,
};

// Mirror of `FfxiSkin` in skinned_ffxi_material.rs. 128 = MAX_JOINTS.
struct FfxiSkin {
    joints: array<mat4x4<f32>, 128>,
    lighting: FfxiLighting,
};

// Mirror of `FfxiInstance`. `flags.x` = has_texture (1.0 / 0.0).
struct FfxiInstance {
    flags: vec4<f32>,
    tint: vec4<f32>,
    skin_slot: u32,
    reveal: f32,
    opacity: f32,
};

@group(constants::MATERIAL_BIND_GROUP) @binding(0) var<storage, read> skins: array<FfxiSkin>;
@group(constants::MATERIAL_BIND_GROUP) @binding(1) var base_tex: texture_2d<f32>;
@group(constants::MATERIAL_BIND_GROUP) @binding(2) var base_samp: sampler;
@group(constants::MATERIAL_BIND_GROUP) @binding(3) var<storage, read> instances: array<FfxiInstance>;

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
    @location(1) @interpolate(flat) inst_idx: u32,
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)
    @location(2) world_normal: vec3<f32>,
    @if(MOTION_VECTOR_PREPASS)
    @location(4) world_position: vec4<f32>,
    @if(MOTION_VECTOR_PREPASS)
    @location(5) previous_world_position: vec4<f32>,
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    @location(6) unclipped_depth: f32,
};

@vertex
fn vertex(v: Vertex) -> PrepassVertexOutput {
    var out: PrepassVertexOutput;

    // Same FFXI dual-position/dual-normal skin as the lit shader. The position
    // is NOT weighted on position0 (FFXI's asymmetry); the normal IS weighted.
    let inst = mesh_functions::get_tag(v.instance_index);
    let si = instances[inst].skin_slot;
    let w = v.joint_weight;
    let m0 = skins[si].joints[v.joint0];
    let m1 = skins[si].joints[v.joint1];
    let model_pos = m0 * vec4<f32>(v.position0, w)
                  + m1 * vec4<f32>(v.position1, 1.0 - w);

    let world_from_local = mesh_functions::get_world_from_local(v.instance_index);
    let world_position = world_from_local * vec4<f32>(model_pos.xyz, 1.0);

    out.clip_position = position_world_to_clip(world_position.xyz);
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    out.unclipped_depth = out.clip_position.z;
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    out.clip_position.z = min(out.clip_position.z, 1.0); // clamp to avoid clipping
    out.uv = v.uv;
    out.inst_idx = inst;

    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)
    let model_norm = w * (m0 * vec4<f32>(v.normal0, 0.0)).xyz
                   + (1.0 - w) * (m1 * vec4<f32>(v.normal1, 0.0)).xyz;
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)
    out.world_normal = normalize(mesh_functions::mesh_normal_local_to_world(model_norm, v.instance_index));

    @if(MOTION_VECTOR_PREPASS)
    out.world_position = world_position;
    // Approximation: previous *entity* transform applied to the CURRENT skinned
    // model position (no previous-frame joint palette exists). See header note.
    @if(MOTION_VECTOR_PREPASS)
    let prev_world_from_local = mesh_functions::get_previous_world_from_local(v.instance_index);
    @if(MOTION_VECTOR_PREPASS)
    out.previous_world_position = prev_world_from_local * vec4<f32>(model_pos.xyz, 1.0);

    return out;
}

// Cut-out alpha test so transparent texels (hair edges, fabric fringe) don't
// write solid depth/normals or cast solid-block shadows. Mirrors the lit
// fragment's discard threshold (69/255). Untextured C/CS meshes never discard.
fn prepass_alpha_discard(uv: vec2<f32>, inst_idx: u32) {
    let coverage = min(instances[inst_idx].reveal, instances[inst_idx].opacity);
    if (coverage < 1.0) {
        if (coverage < reveal_threshold(uv)) { discard; }
    }
    let has_texture = instances[inst_idx].flags.x > 0.5;
    if (has_texture) {
        let texel = textureSample(base_tex, base_samp, uv);
        if (texel.a < 69.0 / 255.0) {
            discard;
        }
    }
}

// Mirror of bevy_pbr::pbr_prepass_functions::calculate_motion_vector.
@if(MOTION_VECTOR_PREPASS)
fn calculate_motion_vector(world_position: vec4<f32>, previous_world_position: vec4<f32>) -> vec2<f32> {
    let clip_position_t = view.unjittered_clip_from_world * world_position;
    let clip_position = clip_position_t.xy / clip_position_t.w;
    let previous_clip_position_t = previous_view_uniforms.clip_from_world * previous_world_position;
    let previous_clip_position = previous_clip_position_t.xy / previous_clip_position_t.w;
    return (clip_position - previous_clip_position) * vec2(0.5, -0.5);
}

// Camera prepass: emit the G-buffer outputs the camera requested. PREPASS_FRAGMENT
// is set when any of NORMAL / MOTION_VECTOR / DEFERRED prepass is active.
@if(PREPASS_FRAGMENT)
struct FragmentOutput {
    @if(PREPASS_FRAGMENT && NORMAL_PREPASS)
    @location(0) normal: vec4<f32>,
    @if(PREPASS_FRAGMENT && MOTION_VECTOR_PREPASS)
    @location(1) motion_vector: vec2<f32>,
    @if(PREPASS_FRAGMENT && UNCLIPPED_DEPTH_ORTHO_EMULATION)
    @builtin(frag_depth) frag_depth: f32,
}

@if(PREPASS_FRAGMENT)
@fragment
fn fragment(in: PrepassVertexOutput) -> FragmentOutput {
    prepass_alpha_discard(in.uv, in.inst_idx);
    var out: FragmentOutput;
    @if(PREPASS_FRAGMENT && NORMAL_PREPASS)
    out.normal = vec4(normalize(in.world_normal) * 0.5 + vec3(0.5), 1.0);
    @if(PREPASS_FRAGMENT && UNCLIPPED_DEPTH_ORTHO_EMULATION)
    out.frag_depth = in.unclipped_depth;
    @if(PREPASS_FRAGMENT && MOTION_VECTOR_PREPASS)
    out.motion_vector = calculate_motion_vector(in.world_position, in.previous_world_position);
    return out;
}
// Depth-only / shadow pass. Under ortho-depth emulation we must write frag_depth;
// otherwise the fragment only performs the alpha-test discard (an empty output
// struct is invalid WGSL).
@if(!PREPASS_FRAGMENT && UNCLIPPED_DEPTH_ORTHO_EMULATION)
struct PrepassFragmentOutput {
    @builtin(frag_depth) frag_depth: f32,
};

@if(!PREPASS_FRAGMENT && UNCLIPPED_DEPTH_ORTHO_EMULATION)
@fragment
fn fragment(in: PrepassVertexOutput) -> PrepassFragmentOutput {
    prepass_alpha_discard(in.uv, in.inst_idx);
    var out: PrepassFragmentOutput;
    out.frag_depth = in.unclipped_depth;
    return out;
}
@if(!PREPASS_FRAGMENT && !UNCLIPPED_DEPTH_ORTHO_EMULATION)
@fragment
fn fragment(in: PrepassVertexOutput) {
    prepass_alpha_discard(in.uv, in.inst_idx);
}
