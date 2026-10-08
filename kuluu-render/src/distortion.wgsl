// One haze field from a 0x22 `Distortion` generator element. The field never draws pixels of
// its own: inside the footprint (the linked texture quad drawn at authored scale) it reads this
// frame's scene at a horizontally shifted position and writes the shifted sample back, so there
// is no second image of anything — displacement only, where the haze texture says so.

struct FieldUniform {
    rect_min: vec2f, // field footprint as a screen-space NDC rect
    rect_max: vec2f,
    haze: f32,       // sec2 0x32 HazeOffsetInitializer authored horizontal offset
    env: f32,        // sec2 0x2D KeyFrameValueSetup envelope sampled at this frame
};

@group(0) @binding(0) var<uniform> u_field: FieldUniform;
// Same-frame scene (a copy taken before any field wrote), filterable.
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var scene_sampler: sampler;
// The element's own linked texture, RGBA alpha = where inside the footprint haze acts.
@group(1) @binding(0) var haze_map: texture_2d<f32>;
@group(1) @binding(1) var map_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4f,
};

const TRIANGLE: array<vec2f, 3> = array<vec2f, 3>(
    vec2f(-1.0, -1.0),
    vec2f(3.0, -1.0),
    vec2f(-1.0, 3.0),
);

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    out.pos = vec4f(TRIANGLE[vi], 0.0, 1.0);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4f {
    if (u_field.env <= 0.0) {
        discard;
    }
    let res = textureDimensions(scene, 0i);
    let uv = in.pos.xy / vec2f(f32(res.x), f32(res.y));
    let ndc = uv * vec2f(2.0, -2.0) + vec2f(-1.0, 1.0);
    if (ndc.x < u_field.rect_min.x || ndc.x > u_field.rect_max.x
        || ndc.y < u_field.rect_min.y || ndc.y > u_field.rect_max.y) {
        discard;
    }

    let half = max((u_field.rect_max - u_field.rect_min) * 0.5, vec2f(1e-4));
    let q = (ndc - (u_field.rect_min + u_field.rect_max) * 0.5) / half; // [-1,1] over the quad
    let map_uv = clamp(q * vec2f(0.5, -0.5) + 0.5, vec2f(0.0), vec2f(1.0));

    let texel_x = 1.0 / f32(textureDimensions(haze_map, 0i).x);
    let m = textureSampleLevel(haze_map, map_sampler, map_uv, 0.0).a;
    if (m <= 0.0) {
        discard; // outside the texture's silhouette: the pixel was and stays untouched
    }

    // Displacement follows the map alpha's horizontal gradient — bend at the edges of whatever
    // the texture paints, zero across flat regions, so a shift can never read as a copy.
    let g = textureSampleLevel(haze_map, map_sampler, map_uv + vec2f(texel_x, 0.0), 0.0).a
          - textureSampleLevel(haze_map, map_sampler, map_uv - vec2f(texel_x, 0.0), 0.0).a;
    let dx = u_field.haze * u_field.env * g;

    let warped = textureSampleLevel(scene, scene_sampler, uv + vec2f(dx, 0.0), 0.0);
    return vec4f(warped.rgb, clamp(m * u_field.env, 0.0, 1.0));
}
