// Screen-space sky gradient.
//
// 8 colors paired with 8 altitudes ([-1, 1] dot of ray direction
// with +Y). For each fragment, find the two altitude bands bracketing
// the ray, lerp between their colors. Matches lotus-ffxi's miss
// shader algorithm (cite-only reference) but the math here is our
// own.
//
// The mesh is an inverted UVSphere centered on the camera; that
// turns interior fragments into "sky pixels" whose world-space ray
// direction is what we sample.
//
// Uniform layout: WGSL forbids `var<uniform>` of bare array types —
// the uniform global must be a struct. `SkyboxUniform` mirrors the
// Rust-side `SkyboxUniform` exactly so `AsBindGroup`'s std140 layout
// is byte-compatible with this declaration.
//
// Bind-group index: Bevy 0.17 puts the mesh storage buffer at group 2
// (`bevy_pbr::mesh_bindings::mesh: array<Mesh>`) and material bind
// groups at `MATERIAL_BIND_GROUP_INDEX = 3` — see
// `bevy_pbr/src/material.rs:66`. Using `@group(2)` here collides with
// the mesh-bindings slot and produces a cryptic
// `Storage class Storage doesn't match Uniform` pipeline-layout
// mismatch. The `#{MATERIAL_BIND_GROUP}` placeholder is substituted
// by Bevy's shader processor (`material.rs:473`) to the right group
// even if Bevy reshuffles indices in a future version.

#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::forward_io::VertexOutput

struct SkyboxUniform {
    colors: array<vec4<f32>, 8>,
    altitudes_packed: array<vec4<f32>, 2>,
    // x = coverage [0,1], y = opacity [0,1] (0 disables the layer),
    // zw = UV scroll offset. Procedural clouds, Enhanced style only.
    cloud_params: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> data: SkyboxUniform;

fn get_altitude(i: u32) -> f32 {
    let outer = i / 4u;
    let inner = i % 4u;
    let v = data.altitudes_packed[outer];
    if (inner == 0u) { return v.x; }
    if (inner == 1u) { return v.y; }
    if (inner == 2u) { return v.z; }
    return v.w;
}

// --- Value-noise FBM for the procedural cloud layer. ---
fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var v = 0.0;
    var amp = 0.5;
    var freq = p;
    for (var i = 0; i < 5; i = i + 1) {
        v = v + amp * vnoise(freq);
        freq = freq * 2.0;
        amp = amp * 0.5;
    }
    return v;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // World-space ray direction from camera to this fragment. The
    // skybox sphere is centered on the camera, so the unnormalized
    // direction is just (world_pos - cam_pos).
    let cam_pos = view.world_position;
    let ray = normalize(in.world_position.xyz - cam_pos);
    // Altitude: +1 at zenith (up), -1 at nadir (down). Lotus uses
    // dot(ray, -Y) which flips this; we use +Y for readability so
    // skybox_altitudes[0] is the lowest band (horizon-ish) and
    // skybox_altitudes[7] is the highest (zenith).
    let altitude = ray.y;

    // Walk altitudes to find the bracketing pair. Linear scan over
    // 8 entries; trivial on GPU.
    var lo_idx = 0u;
    for (var i = 1u; i < 8u; i = i + 1u) {
        if (get_altitude(i) <= altitude) {
            lo_idx = i;
        }
    }
    var hi_idx = lo_idx + 1u;
    if (hi_idx > 7u) { hi_idx = 7u; }

    let lo_alt = get_altitude(lo_idx);
    let hi_alt = get_altitude(hi_idx);
    let span = max(hi_alt - lo_alt, 0.0001);
    let t = clamp((altitude - lo_alt) / span, 0.0, 1.0);

    let lo_col = data.colors[lo_idx];
    let hi_col = data.colors[hi_idx];
    var col = mix(lo_col, hi_col, t).rgb;

    // --- Procedural cloud layer (Enhanced style; opacity 0 = off). ---
    let opacity = data.cloud_params.y;
    if (opacity > 0.001 && altitude > 0.02) {
        let coverage = data.cloud_params.x;
        let scroll = data.cloud_params.zw;
        // Stereographic projection of the ray onto a horizontal disc
        // (projection point at the nadir). The naive ray.xz/ray.y planar
        // map diverges as ray.y → 0, smearing the noise into radial
        // streaks along the horizon and forcing a clamp whose frozen
        // region shows up as vertical banding. Stereographic projection
        // is *conformal* — it preserves local angles, so a round cloud
        // puff stays round from zenith to skyline instead of stretching —
        // and bounded (|proj| ≤ 1 for any visible altitude), so there's
        // no singularity and no clamp discontinuity to band.
        let proj = ray.xz / (1.0 + ray.y);
        let density = fbm(proj * 3.5 + scroll);
        // Threshold the noise into cloud shapes: higher coverage lowers
        // the cut so more sky fills in.
        let cut = 1.0 - coverage;
        let shape = smoothstep(cut, cut + 0.35, density);
        // Fade in from the horizon so clouds don't knife-edge at the
        // skyline, and ride on the opacity knob.
        let horizon_fade = smoothstep(0.02, 0.30, altitude);
        let cloud_a = clamp(shape * horizon_fade * opacity, 0.0, 1.0);
        // Tint clouds by the local sky color so they share its hue
        // (warm at dusk, blue at noon) but read brighter — the
        // texColor·skyColor idea from retail, done procedurally.
        let cloud_col = mix(col, vec3<f32>(1.0, 1.0, 1.0), 0.6) * (0.7 + 0.3 * density);
        col = mix(col, cloud_col, cloud_a);
    }

    return vec4<f32>(col, 1.0);
}
