// Nameplates, drawn as a final pass inside the operator view (see
// nameplate_final_pass.rs for the scheduling story). The vertex layout is the
// bevy_mesh unit Rectangle verbatim (src/primitives/dim2.rs): positions are the
// z=0 square corners in order (+y,-y), and uvs run (1,0),(0,0),(0,1),(1,1) —
// replicate both exactly or every plate flips/mirrors relative to retail.

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

// Per-plate data, rewritten by the CPU each frame before the pass. Byte layout
// (80 total): model mat4 at 0, fade alpha f32 at 64, pad to 80.
struct PlateUniform {
    model: mat4x4<f32>,
    fade_alpha: f32,
};

// Per-view data for this frame's run of the pass: the clip matrix of the view
// currently being drawn (the operator camera's), its projection near plane, and
// the render-res/full-res scale that maps a full-res fragment onto the depth
// sub-rect the main pass actually wrote under an upscaler. Byte layout:
// mat4 @ 0, near f32 @ 64, subrect_scale vec2 @ 72 (total stays 80).
struct ViewUniform {
    clip_from_world: mat4x4<f32>,
    near: f32,
    subrect_scale: vec2<f32>,
};

@group(0) @binding(0) var<uniform> plate: PlateUniform;
@group(0) @binding(1) var<uniform> view_u: ViewUniform;
@group(0) @binding(2) var plate_tex: texture_2d<f32>;
@group(0) @binding(3) var plate_smp: sampler;

// Scene depth of the view this pass draws into. The Hardware variant tests it
// as an ATTACHMENT instead and never touches these bindings (the group is
// simply left unbound). Only referenced when MANUAL_DEPTH_TEST is compiled in:
// - MSAA on: the multi-sample depth buffer cannot sit beside the 1-sample
//   processed color image in one pass, so per-sample textureLoad reads this
//   pixel's sub-sampled scene depths (WGSL forbids textureGather on
//   multisampled depth — the only legal read is textureLoad(tex, coord,
//   sample), so each sample gets its own load).
// - Upscaler active: single-sample depth bound as a plain texture; one nearest
//   load at the render-res sub-rect pixel (SUBRECT_SCALE + SINGLE_SAMPLE_DEPTH).
#ifdef MANUAL_DEPTH_TEST
#ifdef SINGLE_SAMPLE_DEPTH
@group(1) @binding(0) var scene_depth: texture_depth_2d;
#else
@group(1) @binding(0) var scene_depth: texture_depth_multisampled_2d;
#endif
#endif

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    // View distance d = clip.w of this point (> 0 in front of the camera).
    // Perspective-correct delivery makes it exact at every pixel of this flat
    // camera-facing quad — for any planar patch, 1/d is affine in screen space,
    // so interpolating d and dividing by it recovers true per-pixel distance.
    // The depth value the main pass stores is near/d (bevy's reversed-Z infinite
    // projection: near -> 1, far -> 0, closer = LARGER); a pre-computed ratio
    // would NOT survive interpolation (~12% off mid-quad), so only d travels.
    @location(2) view_dist: f32,
};

@vertex
fn vs(v: VsIn) -> VsOut {
    let world = plate.model * vec4<f32>(v.position.xyz, 1.0);
    var out: VsOut;
    out.clip = view_u.clip_from_world * world;
    out.uv = v.uv;
    out.alpha = plate.fade_alpha;
    out.view_dist = out.clip.w;
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    #ifdef MANUAL_DEPTH_TEST
    // Behind the camera (clip.w <= 0): nothing meaningful to compare against.
    // The billboard system already gates these upstream — this is the belt.
    if (in.clip.w <= 0.0) {
        discard;
    }
    // Pixel of scene depth this fragment maps to. @builtin(position) is the
    // fragment centre in pixel units, so truncation yields the texel index.
    var pix = vec2<f32>(in.clip.xy);
    #ifdef SUBRECT_SCALE
    // Under an upscaler the main pass rendered into a top-left sub-rect at
    // render resolution: scale full-res fragment coords down into that sub-rect
    // so every fragment lands on depth the scene actually wrote (the hardware
    // attachment test can't do this — it would read stale texels past the
    // sub-rect and occlude plates against garbage).
    pix *= view_u.subrect_scale;
    #endif
    let pix_i = vec2<i32>(pix);

    #ifdef SINGLE_SAMPLE_DEPTH
    // Single-sample scene depth: one nearest load, same GreaterEqual distance
    // test the attachment variant uses. Compare distances (near/d) instead of
    // raw depth values: both sides divide by the same near (cancels), and a 0
    // sample at the far plane would blow up the division.
    let d = textureLoad(scene_depth, pix_i, 0);
    if (d > 1e-6 && view_u.near / d < in.view_dist) {
        discard;
    }
    #else
    // MSAA sub-sample depths of that pixel. WGSL forbids textureGather on
    // multisampled textures ("Unable to operate on image class Depth {
    // multi: true }") — the only legal read is textureLoad(tex, coord, sample),
    // so read each sub-sample explicitly. Bevy's MSAA count (2/4/8) is baked
    // into the pipeline via the MSAA_SAMPLES_N shader def by the CPU side, so
    // each variant reads exactly its own samples — no wasted loads, no missing
    // ones. textureLoad needs no sampler, so the depth group is a single
    // texture binding.

    // Sample-count constant, set by exactly one shader def per pipeline
    // variant. Falls back to 4 (the game's default preset) so a missing def
    // still compiles into a plausible shader instead of silently killing the
    // MSAA path.
    #ifdef MSAA_SAMPLES_8
    let sample_count: i32 = 8;
    #else ifdef MSAA_SAMPLES_2
    let sample_count: i32 = 2;
    #else
    let sample_count: i32 = 4;
    #endif

    // Any sub-sample nearer than the plate means opaque geometry occupies
    // part of this pixel — hide the plate behind it (same GreaterEqual test
    // the attachment variant uses).
    var occluded: bool = false;
    for (var i: i32 = 0; i < sample_count; i = i + 1) {
        let d = textureLoad(scene_depth, pix_i, i);
        if (d > 1e-6) {
            let scene_dist = view_u.near / d;
            if (scene_dist < in.view_dist) {
                occluded = true;
                break;
            }
        }
    }
    if (occluded) {
        discard;
    }
    #endif
    #endif

    // Replicates core_3d's PBR unlit + AlphaMode::Premultiplied pixel math over the
    // processed scene (see module docs in nameplate_final_pass.rs): sample gives the
    // linearized premultiplied texel (raster is Rgba8UnormSrgb, so wgpu converts on
    // sample; premultiplied BEFORE mips per kuluu-zxxb), then color and coverage
    // scale together by the fade alpha — so a target pulse can't turn additive.
    let texel = textureSample(plate_tex, plate_smp, in.uv);
    return vec4<f32>(texel.rgb * in.alpha, texel.a * in.alpha);
}
