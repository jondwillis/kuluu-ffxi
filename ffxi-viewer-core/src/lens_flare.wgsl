// Screen-space lens flare (Enhanced sky style only).
//
// The mesh is a unit Rectangle quad placed in front of the camera and
// scaled to exactly fill the frustum, so its UV [0,1]² maps 1:1 to the
// screen. The CPU side (`lens_flare_system`) feeds us the sun's
// projected screen UV and an intensity that fades the whole effect out
// when the sun drops below the horizon, goes off-screen, or the user is
// in Retail style.
//
// Additive blend: where the flare contributes nothing the fragment is
// black (adds zero), so the quad can cover the whole screen cheaply.
//
// Three classic flare elements, all keyed off the sun→screen-centre
// axis the way a real compound lens scatters light:
//   * Halo      — bright bloom centred on the sun.
//   * Ghosts    — a chain of dim discs marching through screen centre
//                 to the opposite side (internal lens reflections).
//   * Streak    — a horizontal anamorphic flare bar through the sun.
//
// Aspect correction (params.w) keeps the discs round on wide viewports.

#import bevy_pbr::forward_io::VertexOutput

struct LensFlareUniform {
    // xy = sun screen UV (origin bottom-left), z = intensity [0,1],
    // w = viewport aspect (width / height).
    params: vec4<f32>,
    // rgb = flare tint, a = unused.
    tint: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> data: LensFlareUniform;

// Soft radial disc: 1.0 at centre, smooth falloff to 0 at `radius`.
fn disc(uv: vec2<f32>, centre: vec2<f32>, radius: f32, aspect: f32) -> f32 {
    var d = uv - centre;
    d.x = d.x * aspect; // undo viewport stretch so the disc stays round
    let r = length(d);
    return 1.0 - smoothstep(0.0, radius, r);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let intensity = data.params.z;
    if (intensity <= 0.0) {
        // Premultiplied-alpha "Add" blend (Bevy maps AlphaMode::Add to
        // BLEND_PREMULTIPLIED_ALPHA = src·1 + dst·(1−src.a)). Output
        // alpha MUST be 0 so the destination is preserved (dst·1) and we
        // add nothing; alpha=1 here would paint an opaque black quad
        // over the whole world.
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let uv = in.uv;
    let sun = data.params.xy;
    let aspect = data.params.w;
    let centre = vec2<f32>(0.5, 0.5);
    let tint = data.tint.rgb;

    var col = vec3<f32>(0.0);

    // --- Halo: tight bright core + wide soft bloom around the sun. ---
    let core = disc(uv, sun, 0.06, aspect);
    let bloom = disc(uv, sun, 0.32, aspect);
    col += tint * (pow(core, 2.0) * 1.6 + pow(bloom, 3.0) * 0.45);

    // --- Ghosts: discs spaced along the sun→centre→opposite line. ---
    // Vector from sun toward centre; ghosts sit at multiples beyond it,
    // landing on the far side of the screen — the hallmark "chain".
    let to_centre = centre - sun;
    // (offset multiplier, radius, brightness, colour-shift) per ghost.
    let g0 = disc(uv, sun + to_centre * 1.30, 0.045, aspect);
    let g1 = disc(uv, sun + to_centre * 1.65, 0.085, aspect);
    let g2 = disc(uv, sun + to_centre * 2.00, 0.030, aspect);
    let g3 = disc(uv, sun + to_centre * 2.45, 0.120, aspect);
    let g4 = disc(uv, sun + to_centre * 0.65, 0.025, aspect);
    // Tint ghosts slightly toward complementary hues so the chain reads
    // as glass dispersion, not copies of the sun.
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

    // Fade the whole flare toward the screen edges — a real flare is
    // strongest with the sun framed, washing out as it leaves view.
    let edge = 1.0 - smoothstep(0.35, 0.75, length((sun - centre) * vec2<f32>(aspect, 1.0)));

    // Premultiplied additive: rgb is the light to add, alpha = 0 so the
    // world behind shows through everywhere the flare is dark. (alpha=1
    // would replace the scene with black — the opaque-overlay bug.)
    return vec4<f32>(col * intensity * edge, 0.0);
}
