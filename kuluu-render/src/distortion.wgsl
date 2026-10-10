// One haze field from a 0x22 `Distortion` generator element, drawn the way retail draws it: a four
// triangle fan (center plus four ordered rim points) whose texture is this frame's scene copy. No
// authored image is involved anywhere — see
// `.agents/skills/retail-observe/references/2026-10-07-procedural-distortion.md`.
//
// Every vertex carries its own unshifted sampling coordinate, so the haze translation lives entirely
// in the drawn position and never moves a sample: what lands inside the fan is scene content from
// the original footprint, displaced. Vertex alpha (center = the element's current alpha, rim =
// transparent) rasterizes into the coverage gradient — there is no gradient texture.

struct FanVertex {
    // Projected fan corner with the haze translation applied along both draw axes.
    @location(0) pos_shifted: vec2f,
    // The same corner projected WITHOUT the haze translation; the sample comes from here.
    @location(1) sample_ndc: vec2f,
    // Top-left of the field's projected bounding box (flat across the fan).
    @location(2) foot_origin: vec2f,
    // One texel of the intermediate capture in NDC on each axis (flat across the fan). Retail holds
    // the capture to at most 255 texels per axis, so a big field samples coarse.
    @location(3) capture_step: vec2f,
    @location(4) vertex_alpha: f32,
    @location(5) vertex_rgb: f32,
};

// Same-frame scene (a copy taken before any field wrote), filterable.
@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;

const TEXTURE_FACTOR_ALPHA: f32 = 0.50196078431372549; // the copy pass's [128,128,128,128] factor / 255
const COVERAGE_MULTIPLIER: f32 = 4.0;                  // final fan's source-alpha multiplier

struct VsOut {
    @builtin(position) clip: vec4f,
    @location(1) sample_ndc: vec2f,
    @location(2) foot_origin: vec2f,
    @location(3) capture_step: vec2f,
    @location(4) vertex_alpha: f32,
    @location(5) vertex_rgb: f32,
};

@vertex
fn vs(in: FanVertex) -> VsOut {
    var out: VsOut;
    out.clip = vec4f(in.pos_shifted, 0.0, 1.0);
    out.sample_ndc = in.sample_ndc;
    out.foot_origin = in.foot_origin;
    out.capture_step = in.capture_step;
    out.vertex_alpha = in.vertex_alpha;
    out.vertex_rgb = in.vertex_rgb;
    return out;
}

fn scene_uv(ndc: vec2f) -> vec2f {
    // NDC is y-up, the texture is y-down. Clamped so filtering cannot reach past the frame edge.
    let uv = (ndc - vec2f(-1.0, 1.0)) / vec2f(2.0, -2.0);
    return clamp(uv, vec2f(0.0), vec2f(1.0));
}

@fragment
fn fs(
    @location(1) sample_ndc: vec2f,
    @location(2) foot_origin: vec2f,
    @location(3) capture_step: vec2f,
    @location(4) vertex_alpha: f32,
    @location(5) vertex_rgb: f32,
) -> @location(0) vec4f {
    // Coverage is 4 x interpolated vertex alpha x the copy pass's texture factor, clamped by the
    // pipeline. A rim texel has no alpha and costs nothing.
    let coverage = clamp(COVERAGE_MULTIPLIER * vertex_alpha * TEXTURE_FACTOR_ALPHA, 0.0, 1.0);
    if (coverage <= 0.0) {
        discard;
    }

    // Sample the centre of a capture texel, never between two of them: retail's intermediate is at
    // most 255 texels across, and it is magnified back over the footprint from there.
    let grid = floor((sample_ndc - foot_origin) / max(capture_step, vec2f(1e-6))) + vec2f(0.5);
    let warped = textureSampleLevel(
        scene,
        scene_sampler,
        scene_uv(foot_origin + grid * capture_step),
        0.0,
    );
    // The fan's colour op is a doubled modulate of texture and vertex colour, so a corner carrying
    // [128,128,128] returns the scene unmodified and only the coverage ramp decides how much of it
    // shows (`.agents/skills/retail-observe/references/2026-10-07-procedural-distortion.md`,
    // "Coverage, color and time").
    return vec4f(clamp(2.0 * warped.rgb * vertex_rgb, vec3f(0.0), vec3f(1.0)), coverage);
}
