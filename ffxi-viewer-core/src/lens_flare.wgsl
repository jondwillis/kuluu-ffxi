// Screen-space lens flare (Vanilla sky style — the FFXI-faithful sun glare;
// Enhanced mode uses bloom on the sun disc instead).
//
// The mesh is a unit Rectangle placed in front of the camera and scaled to
// (over)fill the frustum, so its UV [0,1]² maps to the screen. Unlike the old
// CPU path, the sun's screen position is projected HERE, against the live view
// matrix the renderer is using this frame — so the flare can't lag the camera.
// Occlusion is sampled from the depth prepass, so the flare fades behind terrain
// instead of shining through it.
//
// Additive blend: where the flare contributes nothing the fragment is black
// (adds zero), so the quad can cover the whole screen cheaply.
//
// When the zone ships an lf0x lens-flare sprite sheet (flare_params.y), the chain is
// data-driven: each element is an additive textured quad placed along the
// sun→screen-centre axis at sun*(1-offset)+opposite*offset, sized viewport/32
// (research/xim ZoneDrawer.kt:233-236). Without a sheet it falls back to three
// analytic elements keyed off the same axis: a halo bloom, a chain of ghost discs,
// and a horizontal anamorphic streak.

#import bevy_pbr::forward_io::VertexOutput
#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::prepass_utils

const MAX_FLARE_ELEMENTS: u32 = 16u;

struct LensFlareUniform {
    // xyz = normalized world-space sun direction, w = intensity [0,1].
    sun_dir_intensity: vec4<f32>,
    // rgb = flare tint, a = unused.
    tint: vec4<f32>,
    // x = element count, y = 1.0 when the lf0x sheet is loaded (data-driven chain).
    flare_params: vec4<f32>,
    // x = per-element offset fraction along sun->opposite.
    offsets: array<vec4<f32>, 16>,
    // (u0,v0,u1,v1) sub-rect of each element in the lf0x texture.
    frame_uv: array<vec4<f32>, 16>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> data: LensFlareUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var flare_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var flare_samp: sampler;

// Distance used to synthesize a world point in the sun's direction. Matched to
// the skybox radius (sun_moon::SKY_RADIUS) so the sun's depth equals the sky's
// depth — clear-sky pixels read as "not occluded", only real geometry occludes.
const SUN_SKY_RADIUS: f32 = 4000.0;

fn fully_transparent() -> vec4<f32> {
    // Premultiplied-alpha "Add" blend (Bevy maps AlphaMode::Add to
    // BLEND_PREMULTIPLIED_ALPHA = src·1 + dst·(1−src.a)). Output alpha MUST be 0
    // so the destination is preserved and we add nothing.
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}

// Soft radial disc: 1.0 at centre, smooth falloff to 0 at `radius`.
fn disc(uv: vec2<f32>, centre: vec2<f32>, radius: f32, aspect: f32) -> f32 {
    var d = uv - centre;
    d.x = d.x * aspect; // undo viewport stretch so the disc stays round
    let r = length(d);
    return 1.0 - smoothstep(0.0, radius, r);
}

// View-space Z of a clip-space point (negative in front of the camera; more
// negative = farther). Used to compare scene vs sun depth in linear units, which
// has uniform precision (NDC reverse-Z does not, far away).
fn view_z(clip: vec4<f32>) -> f32 {
    let v = view.view_from_clip * clip;
    return v.z / v.w;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let intensity = data.sun_dir_intensity.w;
    if (intensity <= 0.0) {
        return fully_transparent();
    }

    // --- Project the sun to screen space against the live view matrix. ---
    let sun_dir = data.sun_dir_intensity.xyz;
    let sun_world = view.world_position + sun_dir * SUN_SKY_RADIUS;
    let sun_clip = view.clip_from_world * vec4<f32>(sun_world, 1.0);
    if (sun_clip.w <= 0.0) {
        return fully_transparent(); // sun behind the camera
    }
    let sun_ndc = sun_clip.xy / sun_clip.w;
    var sun = sun_ndc * 0.5 + vec2<f32>(0.5);
    sun.y = 1.0 - sun.y; // NDC (y up) → UV (y down), matching in.uv

    let aspect = view.viewport.z / max(view.viewport.w, 1.0);

    // --- Soft occlusion from the depth prepass (geometry in front of the sun). ---
    var visibility = 1.0;
#ifdef DEPTH_PREPASS
    let sun_z = view_z(sun_clip);
    let sun_px = sun * view.viewport.zw + view.viewport.xy;
    let taps = array<vec2<f32>, 5>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(6.0, 0.0),
        vec2<f32>(-6.0, 0.0),
        vec2<f32>(0.0, 6.0),
        vec2<f32>(0.0, -6.0),
    );
    var vis_sum = 0.0;
    for (var i = 0; i < 5; i = i + 1) {
        let px = sun_px + taps[i];
        let scene_depth = bevy_pbr::prepass_utils::prepass_depth(vec4<f32>(px, 0.0, 0.0), 0u);
        let scene_z = view_z(vec4<f32>(sun_ndc, scene_depth, 1.0));
        // scene_z, sun_z are negative; occluded when scene sits at least ~5%
        // nearer than the sky distance. The skybox itself (≈ sun_z) stays visible.
        let occluded = scene_z > sun_z * 0.95;
        vis_sum = vis_sum + select(1.0, 0.0, occluded);
    }
    visibility = vis_sum / 5.0;
    if (visibility <= 0.0) {
        return fully_transparent();
    }
#endif

    // Screen UV from the framebuffer coord, NOT in.uv: the quad is oversized by
    // FLARE_OVERSCAN, so its [0,1] UV spills past the frustum and would drift the
    // flare off the sun by overscan·(uv−0.5). frag coord ÷ viewport is the true
    // screen position, matching the projected `sun` regardless of overscan.
    let uv = (in.position.xy - view.viewport.xy) / max(view.viewport.zw, vec2<f32>(1.0));
    let centre = vec2<f32>(0.5, 0.5);
    let tint = data.tint.rgb;

    var col = vec3<f32>(0.0);

    // --- Data-driven lf0x chain (research/xim ZoneDrawer.kt:233-236). ---
    // Each lens-flare mesh is an additive textured quad placed at
    // sun*(1-offset) + opposite*offset along the sun->screen-centre axis (opposite =
    // sun + 2*to_centre), sized from its native sprite texels. Intensity rides the
    // depth-prepass occlusion `visibility` instead of an analytic halo.
    if (data.flare_params.y > 0.5) {
        let count = u32(data.flare_params.x);
        let to_centre_d = centre - sun;
        let opposite = sun + to_centre_d * 2.0;
        let scale = data.flare_params.z;
        for (var i = 0u; i < count && i < MAX_FLARE_ELEMENTS; i = i + 1u) {
            let offset = data.offsets[i].x;
            let pos = sun * (1.0 - offset) + opposite * offset;
            // Per-element half-extent: native sprite texels * scale, in screen-UV.
            let half = data.offsets[i].yz * scale / max(view.viewport.zw, vec2<f32>(1.0));
            let local = (uv - pos) / half; // [-1,1] inside the quad
            // Clamp + mask rather than `continue`, so the textureSample stays in
            // uniform control flow (WGSL requires implicit-LOD sampling there).
            let inside = step(abs(local.x), 1.0) * step(abs(local.y), 1.0);
            let quad_uv = clamp(local * 0.5 + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
            let f = data.frame_uv[i];
            let suv = vec2<f32>(mix(f.x, f.z, quad_uv.x), mix(f.y, f.w, quad_uv.y));
            let texel = textureSample(flare_tex, flare_samp, suv);
            col += tint * texel.rgb * texel.a * inside;
        }
        let edge_d = 1.0 - smoothstep(0.35, 0.75, length((sun - centre) * vec2<f32>(aspect, 1.0)));
        return vec4<f32>(col * intensity * visibility * edge_d, 0.0);
    }

    // --- Halo: tight bright core + wide soft bloom around the sun. ---
    let core = disc(uv, sun, 0.06, aspect);
    let bloom = disc(uv, sun, 0.32, aspect);
    col += tint * (pow(core, 2.0) * 1.6 + pow(bloom, 3.0) * 0.45);

    // --- Ghosts: discs spaced along the sun→centre→opposite line. ---
    let to_centre = centre - sun;
    let g0 = disc(uv, sun + to_centre * 1.30, 0.045, aspect);
    let g1 = disc(uv, sun + to_centre * 1.65, 0.085, aspect);
    let g2 = disc(uv, sun + to_centre * 2.00, 0.030, aspect);
    let g3 = disc(uv, sun + to_centre * 2.45, 0.120, aspect);
    let g4 = disc(uv, sun + to_centre * 0.65, 0.025, aspect);
    col += vec3<f32>(0.55, 0.70, 1.00) * pow(g0, 2.0) * 0.22;
    col += vec3<f32>(0.90, 0.55, 0.45) * pow(g1, 2.0) * 0.16;
    col += vec3<f32>(0.50, 1.00, 0.65) * pow(g2, 2.0) * 0.20;
    col += vec3<f32>(0.70, 0.55, 1.00) * pow(g3, 2.0) * 0.10;
    col += vec3<f32>(1.00, 0.85, 0.55) * pow(g4, 2.0) * 0.18;

    // --- Anamorphic streak: thin horizontal bar through the sun. ---
    let dy = abs(uv.y - sun.y);
    let dx = abs(uv.x - sun.x) * aspect;
    let streak = (1.0 - smoothstep(0.0, 0.004, dy)) * (1.0 - smoothstep(0.0, 0.6, dx));
    col += vec3<f32>(0.6, 0.75, 1.0) * streak * 0.5;

    // Fade toward the screen edges — a real flare is strongest with the sun
    // framed, washing out as it leaves view.
    let edge = 1.0 - smoothstep(0.35, 0.75, length((sun - centre) * vec2<f32>(aspect, 1.0)));

    // Premultiplied additive: rgb is the light to add, alpha = 0.
    return vec4<f32>(col * intensity * visibility * edge, 0.0);
}
