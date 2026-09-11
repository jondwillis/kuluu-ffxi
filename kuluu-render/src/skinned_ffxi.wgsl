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
    mesh_view_bindings as view_bindings,
    mesh_view_types,
}

#import kuluu_render::directional_shadow::directional_shadow_factor
#import kuluu_render::point_shadow::point_shadow_factor

// Distance fog — see zone_ffxi.wgsl for the rationale. Applied so a distant
// actor fades into the same horizon backdrop as the terrain behind it; near
// the camera (the usual case) the distance term is ~0, a no-op.
#ifdef DISTANCE_FOG
#import bevy_pbr::fog as fog_fns
#endif

// D3DTOP_MODULATE2X's gain, the actor draw's single texture stage.
const D3D_MODULATE_2X: f32 = 2.0;

// Fraction of the sun term a fully shadowed fragment keeps. Real FFXI PCs are flat-lit
// and receive no world shadow, so any floor above 0 is a deliberate departure; this
// model's ambient is intentionally low (the 2x compositing compensates), so cutting the
// sun term to zero collapses the character to a near-black silhouette. Kept identical to
// zone_ffxi.wgsl's FFXI_SHADOW_FLOOR (guard test `shadow_floor_matches_the_actor_shader`)
// so a character and the terrain under it shade by the same amount.
const FFXI_SHADOW_FLOOR: f32 = 0.45;

// Mirror of `FfxiLightingUniform` in skinned_ffxi_material.rs — keep the
// field order/types identical so AsBindGroup's std140 layout matches.
struct FfxiLighting {
    ambient: vec4<f32>,
    dir0_dir: vec4<f32>,
    dir0_color: vec4<f32>,
    dir1_dir: vec4<f32>,
    dir1_color: vec4<f32>,
    point_pos: array<vec4<f32>, 16>,
    point_color: array<vec4<f32>, 16>,
    // Per-point attenuation coefficients `(const, linear, quad)` in xyz (w
    // unused). XIM's `1/(c + l·d + q·d²)` falloff; actors get `const = 0.5`
    // (the FFXI "point-lights affect actors less" dampen, GLDrawer.kt:285-290).
    point_atten: array<vec4<f32>, 16>,
    // x = elapsed seconds, y = wind strength, z/w reserved.
    time_params: vec4<f32>,
};

// Mirror of `FfxiSkin` in skinned_ffxi_material.rs. 128 = MAX_JOINTS. One
// record per live actor in the shared storage buffer, selected via the
// per-submesh instance record's skin_slot.
struct FfxiSkin {
    joints: array<mat4x4<f32>, 128>,
    lighting: FfxiLighting,
};

// Mirror of `FfxiInstance`. `flags.x` = has_texture (1.0 / 0.0),
// `flags.y` = realistic lighting, `flags.z` = receive_shadows, `flags.w` =
// target-highlight glow added on top of the lit color (0 = no highlight).
// `tint` = per-mesh t_factor modulation (XIM GLDrawer.kt:329-331 uEffectColor),
// neutral at 1.0. One record per submesh, indexed by MeshTag (get_tag).
struct FfxiInstance {
    flags: vec4<f32>,
    tint: vec4<f32>,
    skin_slot: u32,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<storage, read> skins: array<FfxiSkin>;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var base_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var base_samp: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<storage, read> instances: array<FfxiInstance>;

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
    @location(4) @interpolate(flat) inst_idx: u32,
};

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let inst = mesh_functions::get_tag(v.instance_index);
    let si = instances[inst].skin_slot;

    let w = v.joint_weight;
    let m0 = skins[si].joints[v.joint0];
    let m1 = skins[si].joints[v.joint1];

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
    out.inst_idx = inst;
    return out;
}

fn scene_irradiance(si: u32, n: vec3<f32>, p: vec3<f32>, wrap: f32, shadow_scale: vec2<f32>, point_shadows: bool, frag_coord: vec2<f32>) -> vec3<f32> {
    var rgb = skins[si].lighting.ambient.rgb;
    let nl0 = max((dot(n, -skins[si].lighting.dir0_dir.xyz) + wrap) / (1.0 + wrap), 0.0);
    rgb += shadow_scale.x * nl0 * skins[si].lighting.dir0_color.rgb * skins[si].lighting.dir0_color.w;
    let nl1 = max((dot(n, -skins[si].lighting.dir1_dir.xyz) + wrap) / (1.0 + wrap), 0.0);
    rgb += shadow_scale.y * nl1 * skins[si].lighting.dir1_color.rgb * skins[si].lighting.dir1_color.w;
    // 16 = MAX_POINT_LIGHTS (skinned_ffxi_material.rs); empty slots have range 0.
    for (var i = 0u; i < 16u; i = i + 1u) {
        // `.w` of the color carries the light's range; <= 0 means an empty slot.
        let range = skins[si].lighting.point_color[i].w;
        if (range > 0.0) {
            // XIM's `pointLightCalc` (ShaderConstants.kt:186-198): diffuse N·L,
            // `1/(c + l·d + q·d²)` falloff, hard-cut past `range`. Vertex color
            // and the FFXI 2x/texel compositing are applied by the caller, so —
            // unlike XIM, which folds vertexColor into every light term — this
            // returns pure light (matching how the dir0/dir1 terms above work).
            let to_light = skins[si].lighting.point_pos[i].xyz - p;
            let dist = length(to_light);
            if (dist <= range) {
                let a = skins[si].lighting.point_atten[i].xyz; // (const, linear, quad)
                let denom = a.x + a.y * dist + a.z * dist * dist;
                let dist_factor = select(1.0 / denom, 0.0, denom <= 0.0);
                let nl = max(dot(n, to_light / max(dist, 1e-5)), 0.0);
                // Enhanced Dynamic Lights: the slot's light may carry a cube shadow map
                // (zone_point_lights.rs select_shadowed_zone_lights); no floor, matching
                // zone_ffxi.wgsl so the shadow reads the same on the character and the deck.
                var shadow = 1.0;
                if (point_shadows) {
                    shadow = point_shadow_factor(p, n, skins[si].lighting.point_pos[i].xyz, frag_coord);
                }
                rgb += shadow * nl * dist_factor * skins[si].lighting.point_color[i].rgb;
            }
        }
    }
    return rgb;
}

// Fade a lit fragment toward the fog colour by view distance (see
// zone_ffxi.wgsl). No-op when the view carries no DistanceFog.
fn apply_distance_fog(color: vec4<f32>, world_pos: vec3<f32>) -> vec4<f32> {
#ifdef DISTANCE_FOG
    let fog_params = view_bindings::fog;
    let dist = length(world_pos - view_bindings::view.world_position);
    let scattering = vec3<f32>(0.0);
    if (fog_params.mode == mesh_view_types::FOG_MODE_LINEAR) {
        return fog_fns::linear_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_EXPONENTIAL) {
        return fog_fns::exponential_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_EXPONENTIAL_SQUARED) {
        return fog_fns::exponential_squared_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_ATMOSPHERIC) {
        return fog_fns::atmospheric_fog(fog_params, color, dist, scattering);
    }
#endif
    return color;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let rec = instances[in.inst_idx];
    let si = rec.skin_slot;
    // Untextured FFXI meshes (C/CS ops) carry a null TextureLink: treat the
    // texel as white opaque and skip the alpha-test so the vertex color shows.
    let has_texture = rec.flags.x > 0.5;
    // flags.y selects the realistic (Bevy-scene-driven) lighting model.
    let realistic = rec.flags.y > 0.5;
    // flags.z gates shadow RECEIVE from the sun/moon and from Enhanced point lights (the
    // "Model Shadow Receiving" graphics setting). When off, both branches light the
    // model with no shadow attenuation (sun term at full strength).
    let receive_shadows = rec.flags.z > 0.5;
    // flags.w is the target-strobe glow: an additive highlight pulsed onto the
    // currently-selected actor (driven by target_strobe.rs). 0 for everything else.
    let highlight = vec3<f32>(max(rec.flags.w, 0.0));
    var texel = vec4<f32>(1.0);
    if (has_texture) {
        texel = textureSample(base_tex, base_samp, in.uv);
        // Alpha test (XIM discardThreshold = 69/255 — SKINNED_ALPHA_DISCARD in
        // skinned_ffxi_material.rs). Applied manually since a custom fragment
        // shader bypasses Bevy's built-in mask handling.
        if (texel.a < 69.0 / 255.0) {
            discard;
        }
    }

    let n = normalize(in.world_normal);
    var shadow_scale = vec2<f32>(1.0);
    if (receive_shadows) {
        shadow_scale = vec2<f32>(
            directional_shadow_factor(in.world_position, n, -skins[si].lighting.dir0_dir.xyz, in.clip_position.xy),
            directional_shadow_factor(in.world_position, n, -skins[si].lighting.dir1_dir.xyz, in.clip_position.xy),
        );
    }
    if (realistic) {
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
        let albedo = texel.rgb * in.color.rgb * rec.tint.rgb;
        let irr = scene_irradiance(si, n, in.world_position, 0.3, shadow_scale, receive_shadows, in.clip_position.xy);
        let rgb = albedo * (irr * EXPOSURE + vec3<f32>(AMBIENT_FLOOR));
        // Opaque output (AlphaMode::Mask already discarded cut-out texels). A
        // sub-1 alpha here would let the preview camera composite the character
        // see-through over the launcher backdrop.
        return apply_distance_fog(vec4<f32>(rgb + highlight, 1.0), in.world_position);
    }

    // FFXI-faithful: flat per-vertex light * vertex color through the single
    // MODULATE2X texture stage. research/XIClient Rendering/Direct3D8Manager.cpp:
    // 373,390,393,395 — fixed-function T&L sources DIFFUSE/AMBIENT from D3DMCS_COLOR1
    // and emits a D3DCOLOR, so the lit vertex term saturates before the stage.
    //
    shadow_scale = mix(vec2<f32>(FFXI_SHADOW_FLOOR), vec2<f32>(1.0), shadow_scale);
    let lit = saturate(scene_irradiance(si, n, in.world_position, 0.0, shadow_scale, receive_shadows, in.clip_position.xy) * in.color.rgb);
    let rgb = saturate(D3D_MODULATE_2X * lit * texel.rgb * rec.tint.rgb);
    // Opaque output (AlphaMode::Mask already discarded cut-out texels). A sub-1
    // alpha here would let the preview camera composite the character see-
    // through over the launcher backdrop. The depth-only cast-shadow / prepass
    // path lives in the separate skinned_ffxi_prepass.wgsl module.
    return apply_distance_fog(vec4<f32>(rgb + highlight, 1.0), in.world_position);
}
