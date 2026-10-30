// Exact texel-for-texel scene copy taken before any haze field writes, so a field's displaced
// sample comes from untouched pixels of THIS frame.

@group(0) @binding(0) var src_tex: texture_2d<f32>;

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
    return textureLoad(src_tex, vec2<i32>(in.pos.xy), 0i);
}
