// Screen-space distortion (haze/smear) pass — retail's 0x22 Distortion element. It samples the
// previous frame's processed output with a horizontal bias so motion leaves a directional ghost,
// composited over the current frame at low alpha (research/xim GLDrawer.kt hazeSwitch +
// XimParticleShader.kt frag_hazePosition; the 75/25 current/previous blend is approximated here by
// sampling last frame's buffer directly).
struct PassUniform {
    offset: vec2<f32>,   // x = sec2 0x32 horizontalOffset, y = unused
    intensity: f32,      // ghost alpha (retail forces ~0.25)
    copy_mode: f32,      // >0.5 => pure capture (output the sampled texel at full alpha)
};

@group(0) @binding(0) var<uniform> u: PassUniform;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// Full-screen triangle in clip space (covers the viewport with three verts, no index buffer).
@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    let p = array<vec2<f32>, 3>(
        vec2(-1.0, -3.0),
        vec2(-1.0, 1.0),
        vec2(3.0, 1.0),
    );
    var out: VsOut;
    out.pos = vec4<f32>(p[vi], 0.0, 1.0);
    out.uv = p[vi] * vec2(0.5, -0.5) + vec2(0.5);
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv + u.offset;
    let c = textureSample(src_tex, src_sampler, clamp(uv, vec2(0.0), vec2(1.0)));
    if (u.copy_mode > 0.5) {
        return vec4<f32>(c.rgb, 1.0);
    }
    return vec4<f32>(c.rgb, c.a * u.intensity);
}
