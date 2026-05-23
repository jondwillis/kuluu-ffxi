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
    tint: vec4<f32>,
    /// x = illumination [0,1], y = waxing sign (+1 / -1),
    /// z = intensity multiplier, w = earthshine strength [0,1]
    /// (0 = unlit side stays fully dark; 0.06 = retail-equivalent
    /// floor; higher = brighter Da-Vinci-glow on the dark crescent,
    /// physically peaks near thin crescents and falls to ~0 at full).
    params: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> data: MoonUniform;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
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

    let k = 1.0 - 2.0 * illumination;
    let term_x = k * sqrt(max(0.0, 1.0 - uv.y * uv.y));
    let lit = step(term_x, uv.x * waxing_sign);
    // Earthshine: keep the unlit side faintly visible so the disc
    // doesn't snap to a hard half-moon edge. Strength is supplied
    // by the Rust side so we can ramp it by phase (peaks at thin
    // crescent, fades to 0 near full).
    let earthshine = data.params.w;
    let brightness = mix(earthshine, 1.0, lit);

    // Limb darkening for the sphere illusion on a flat quad.
    let limb = mix(0.80, 1.0, sqrt(max(0.0, 1.0 - r2)));

    // Soft edge antialias against the discard.
    let edge = smoothstep(1.0, 0.97, r);

    let color = data.tint.rgb * grey * brightness * limb * intensity;
    return vec4<f32>(color, edge);
}
