// UV cells keep the dissolve attached to animated surfaces without particle draws.
const REVEAL_CELLS: f32 = 96.0;
const HASH_AXIS: vec2<f32> = vec2<f32>(127.1, 311.7);
const HASH_GAIN: f32 = 43758.5453;
const EDGE_WIDTH: f32 = 0.12;
const EDGE_COLOR: vec3<f32> = vec3<f32>(0.3, 1.2, 1.8);
const THRESHOLD_MARGIN: f32 = 0.001;

fn reveal_threshold(uv: vec2<f32>) -> f32 {
    let cell = floor(uv * REVEAL_CELLS);
    return THRESHOLD_MARGIN + (1.0 - THRESHOLD_MARGIN) * fract(sin(dot(cell, HASH_AXIS)) * HASH_GAIN);
}

fn reveal_edge(progress: f32, threshold: f32) -> vec3<f32> {
    let edge = 1.0 - smoothstep(0.0, EDGE_WIDTH, progress - threshold);
    return EDGE_COLOR * edge * (1.0 - smoothstep(1.0 - EDGE_WIDTH, 1.0, progress));
}
