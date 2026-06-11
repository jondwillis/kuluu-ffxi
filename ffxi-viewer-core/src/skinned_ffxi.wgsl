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
    view_transformations::{position_world_to_clip, position_world_to_view},
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    shadows,
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

// Mirror of `FfxiMaterialFlags`. `flags.x` = has_texture (1.0 / 0.0).
struct FfxiMaterialFlags {
    flags: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> lighting: FfxiLighting;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_samp: sampler;
// Per-actor bone world-pose matrices uploaded inline as a uniform (see the
// `FfxiJointMatrices` doc for why this is a uniform, not a storage buffer).
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

// Scene irradiance at a surface: ambient sky fill + 2 directional (sun/moon)
// + 4 point lights, all sourced from the live zone lighting uniform. `wrap`
// softens the N·L terminator (0 = hard Lambert). `sun_scale` attenuates ONLY
// the primary directional (sun/dir0) term so a cast-shadow factor can darken
// the sun contribution while leaving the moon/ambient/point fill intact (1.0 =
// fully lit). Shared by both shading models below.
fn scene_irradiance(n: vec3<f32>, p: vec3<f32>, wrap: f32, sun_scale: f32) -> vec3<f32> {
    var rgb = lighting.ambient.rgb;
    let nl0 = max((dot(n, -lighting.dir0_dir.xyz) + wrap) / (1.0 + wrap), 0.0);
    rgb += sun_scale * nl0 * lighting.dir0_color.rgb * lighting.dir0_color.w;
    let nl1 = max((dot(n, -lighting.dir1_dir.xyz) + wrap) / (1.0 + wrap), 0.0);
    rgb += nl1 * lighting.dir1_color.rgb * lighting.dir1_color.w;
    for (var i = 0u; i < 4u; i = i + 1u) {
        let range = lighting.point_color[i].w;
        if (range > 0.0) {
            let d = p - lighting.point_pos[i].xyz;
            let atten = 1.0 / (1.0 + dot(d, d) / (range * range));
            rgb += atten * lighting.point_color[i].rgb;
        }
    }
    return rgb;
}

// Directional cast-shadow factor for the primary sun term (dir0). Bevy owns the
// real directional lights + cascade shadow maps at group(0) (the mesh-view bind
// group the main material pass binds); this loops them, takes the minimum
// shadow factor over the shadow-enabled ones, and returns it (1 = lit, 0 = fully
// occluded). Matches StandardMaterial's usage in pbr_functions.wgsl. We do NOT
// gate on a per-mesh SHADOW_RECEIVER flag (our VertexOutput carries no mesh
// flags) — every FFXI character receives. When the scene has no shadow-enabled
// directional light (e.g. the live FfxiLighting-only path, or a headless capture
// without `--shadowtest`), the loop finds none and this returns 1.0, a no-op.
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
    // Untextured FFXI meshes (C/CS ops) carry a null TextureLink: treat the
    // texel as white opaque and skip the alpha-test so the vertex color shows.
    let has_texture = material_flags.flags.x > 0.5;
    // flags.y selects the realistic (Bevy-scene-driven) lighting model.
    let realistic = material_flags.flags.y > 0.5;
    var texel = vec4<f32>(1.0);
    if (has_texture) {
        texel = textureSample(base_tex, base_samp, in.uv);
        // Alpha test (XIM discardThreshold = 69/255). Applied manually since
        // a custom fragment shader bypasses Bevy's built-in mask handling.
        if (texel.a < 0.271) {
            discard;
        }
    }

    let n = normalize(in.world_normal);
    if (realistic) {
        // Cast-shadow attenuation for the sun term — ONLY in the realistic
        // branch. The realistic model is energy-conserving (AMBIENT_FLOOR +
        // EXPOSURE), so a fully-shadowed fragment fades to the ambient floor,
        // never pure black. (Receive is deliberately absent from the faithful
        // branch below — see there.)
        let sun = sun_shadow_factor(in.world_position, n);
        // Energy-conserving: albedo (texture * vertex color) lit ONCE by the
        // live scene sun/moon/ambient (+ point lights), with a soft wrap so
        // the unshadowed back side fades instead of clamping to hard black.
        // No FFXI 2x doubling, so characters sit naturally in the PBR-lit
        // world. Bevy's post-process tonemap compresses the HDR result.
        //
        // The driven irradiance lands in a 0..~1 band, which tonemaps DARKER
        // than the zone's PBR meshes (lit from the full-lux HDR sun). EXPOSURE
        // lifts entities to sit at the zone's brightness; the small additive
        // floor keeps the ambient-only (shadowed) side off pure black. Raise
        // EXPOSURE if models still read dark against the zone; lower it if they
        // blow out. (Exact parity needs true PBR + shadow receiving — a larger
        // change; this is the tunable approximation.)
        let EXPOSURE = 1.7;
        let AMBIENT_FLOOR = 0.10;
        let albedo = texel.rgb * in.color.rgb;
        let irr = scene_irradiance(n, in.world_position, 0.3, sun);
        let rgb = albedo * (irr * EXPOSURE + vec3<f32>(AMBIENT_FLOOR));
        // Opaque output (AlphaMode::Mask already discarded cut-out texels). A
        // sub-1 alpha here would let the preview camera composite the character
        // see-through over the launcher backdrop.
        return vec4<f32>(rgb, 1.0);
    }

    // FFXI-faithful: flat per-vertex light * vertex color, composited at 2x the
    // baked-diffuse texel (XIM's `2 * vertexColor * texel`). NO dynamic
    // shadow-receive here (`sun_scale = 1.0`): real FFXI PCs are flat-lit and do
    // not receive the world's shadows, and — critically — this model's ambient
    // is intentionally LOW (the 2x doubling compensates), so attenuating the
    // dominant sun term to zero on a shadowed fragment collapses the character
    // to near-black. Receive lives only in the realistic branch above; PCs still
    // CAST shadows (the depth prepass writes the shadow map regardless of this
    // branch).
    let lit = scene_irradiance(n, in.world_position, 0.0, 1.0) * in.color.rgb;
    let rgb = 2.0 * lit * texel.rgb;
    // Opaque output (AlphaMode::Mask already discarded cut-out texels). A sub-1
    // alpha here would let the preview camera composite the character see-
    // through over the launcher backdrop. The depth-only cast-shadow / prepass
    // path lives in the separate skinned_ffxi_prepass.wgsl module.
    return vec4<f32>(rgb, 1.0);
}
