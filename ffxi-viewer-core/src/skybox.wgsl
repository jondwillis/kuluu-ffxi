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
    extra: vec4<f32>,
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
    let col = mix(lo_col, hi_col, t).rgb;

    // Clouds are mesh-rendered from the weat/<type>/ DAT (zone_clouds.rs); the
    // dome is pure gradient. cloud_params is retained in the uniform for layout
    // compatibility but no longer drives a procedural layer here.
    return vec4<f32>(col, 1.0);
}
