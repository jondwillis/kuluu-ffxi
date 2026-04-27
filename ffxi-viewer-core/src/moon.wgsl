// Moon billboard shader.
//
// The mesh is a unit Rectangle quad in the XY plane. UV is [0,1]²
// across the quad; we remap to disc-space [-1,1]² and discard pixels
// outside the unit circle so the quad reads as a round moon disc.
//
// Phase mask: for LSB illumination `i ∈ [0,1]`, the terminator is an
// ellipse with semi-minor axis `k = 1 - 2i`. A pixel at (u, v) inside
// the disc is illuminated when `u * waxing_sign > k * sqrt(1 - v²)`.
// `waxing_sign` is +1 when the moon is waxing (illuminated side on
// the +u half), -1 when waning, mirroring LSB's
// `vana_time.h::moon::get_direction`.

#import bevy_pbr::forward_io::VertexOutput

struct MoonUniform {
    // rgb = tint, w = mode: 0 = procedural disc, 2 = retail sprite sheet.
    tint: vec4<f32>,
    /// x = illumination [0,1], y = waxing sign (+1 / -1),
    /// z = intensity multiplier, w = earthshine strength [0,1]
    /// (0 = unlit side stays fully dark; 0.06 = retail-equivalent
    /// floor; higher = brighter Da-Vinci-glow on the dark crescent,
    /// physically peaks near thin crescents and falls to ~0 at full).
    params: vec4<f32>,
    // Current phase frame's sub-rect (u0,v0,u1,v1) in the sprite-sheet texture.
    frame_uv: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> data: MoonUniform;
// Real lunar texture (file 55660). Bound to a default white texture
// until loaded; `data.tint.w` gates whether we actually sample it.
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var surface_tex: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var surface_samp: sampler;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // Retail sprite-sheet mode: the phase is baked per frame, so sample the
    // current frame's sub-rect; the texture's alpha defines the moon's shape.
    if (data.tint.w > 1.5) {
        let f = data.frame_uv;
        let suv = vec2<f32>(mix(f.x, f.z, in.uv.x), mix(f.y, f.w, in.uv.y));
        let texel = textureSample(surface_tex, surface_samp, suv);
        let intensity = data.params.z;
        return vec4<f32>(data.tint.rgb * texel.rgb * intensity, texel.a);
    }

    let uv = in.uv * 2.0 - vec2<f32>(1.0);
    let r2 = dot(uv, uv);
    if (r2 > 1.0) { discard; }
    let r = sqrt(r2);

    // Procedural lunar surface — low-amplitude grey noise plus a few
    // wide darker patches faked as Gaussian blobs ("maria"). Cheap
    // stand-in until/unless we wire a real moon texture.
    let h = hash(uv * 16.0);
    var grey = mix(0.80, 1.05, h);
    let m1 = length(uv - vec2<f32>(-0.25,  0.30));
    let m2 = length(uv - vec2<f32>( 0.30, -0.10));
    let m3 = length(uv - vec2<f32>( 0.10,  0.45));
    grey = grey
        * (1.0 - 0.30 * exp(-(m1 * m1) * 9.0))
        * (1.0 - 0.25 * exp(-(m2 * m2) * 7.0))
        * (1.0 - 0.18 * exp(-(m3 * m3) * 12.0));

    let illumination = data.params.x;
    let waxing_sign = data.params.y;
    let intensity = data.params.z;

    // Retail's moon was a flat, bright textured disc; the Enhanced
    // look adds earthshine + sphere-like limb darkening. The Rust side
    // already drives `earthshine` to 0 in Retail style and >0 in
    // Enhanced, so it doubles as a faithful "flat disc" selector with
    // no extra uniform: `flat ≈ 1` in Retail, `0` in Enhanced.
    let earthshine = data.params.w;
    let flat = 1.0 - smoothstep(0.0, 0.02, earthshine);

    let k = 1.0 - 2.0 * illumination;
    let term_x = k * sqrt(max(0.0, 1.0 - uv.y * uv.y));
    // Anti-alias the terminator over a thin band instead of a hard
    // `step`, so the half-moon edge isn't jagged. Band is narrow so
    // Retail still reads as a crisp terminator.
    let lit = smoothstep(-0.02, 0.02, uv.x * waxing_sign - term_x);
    // Earthshine keeps the unlit side faintly visible (Enhanced only).
    let brightness = mix(earthshine, 1.0, lit);

    // Limb darkening for the sphere illusion on a flat quad — eased
    // toward a flat disc in Retail, and the disc rides a touch brighter
    // there to match retail's luminous moon.
    let limb_min = mix(0.80, 0.96, flat);
    let limb = mix(limb_min, 1.0, sqrt(max(0.0, 1.0 - r2))) * mix(1.0, 1.08, flat);

    // Soft edge antialias against the discard.
    let edge = smoothstep(1.0, 0.97, r);

    // Surface: the real lunar texture when loaded (tint.w ≥ 0.5),
    // otherwise the procedural grey. The texture supplies per-texel
    // colour; the procedural maria are the fallback. The phase
    // terminator, tint, limb and intensity all still ride on top, so
    // the moon waxes/wanes correctly regardless of source.
    let tex = textureSample(surface_tex, surface_samp, in.uv).rgb;
    let has_tex = step(0.5, data.tint.w);
    let surface_rgb = mix(vec3<f32>(grey, grey, grey), tex, has_tex);

    let color = data.tint.rgb * surface_rgb * brightness * limb * intensity;
    return vec4<f32>(color, edge);
}
