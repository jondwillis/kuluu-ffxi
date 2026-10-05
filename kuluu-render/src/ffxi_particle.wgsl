// Particle element shader — reproduces retail's fixed-function texture-stage tables exactly,
// per stage, with D3D8 saturation after every op. The vertex colour attribute carries the
// template's authored diffuse (D), the factor attribute carries the particle's current
// TEXTUREFACTOR (F); the sampler supplies T. Nothing is pre-scaled on the CPU: each table's
// MODULATE2X/MODULATE4X gains live here and only here.
//
// Tables, decoded from the D3DTSS state blocks with d3d8types.h op/arg values
// (D3DTOP_MODULATE=4, MODULATE2X=5, MODULATE4X=6; D3DTA_DIFFUSE=0, CURRENT=1, TEXTURE=2,
// TFACTOR=3):
//
// research/XIClient/src/XIClient/source/Resource/Derived/CMoD3m.cpp NonZeroTwoTSS — the
// textured default: stage 0 MODULATE2X(CURRENT=D, TEXTURE=T) on both channels, stage 1
// MODULATE2X(CURRENT, TFACTOR) rgb / MODULATE4X(CURRENT, TFACTOR) alpha. D is the doubled
// upload colour (CMoD3m.cpp PrepDX: `2 * byte`, clamped at 0xFF). in.color carries the D3m/sheet
// byte/128 (ffxi_dat::d3m::VERTEX_COLOR_DIVISOR) unclamped, and fragment() saturates it to
// reproduce that clamp.
// NonZeroOneTSS — same with stage 0 alpha SELECTARG1(DIFFUSE): the texture alpha drops out.
// ZeroOneTSS — untextured (CMoD3m.cpp Draw: `data[0x04] == 0` -> SetTexture(0, nullptr)):
// single stage MODULATE2X(CURRENT=D, TFACTOR) rgb / MODULATE4X(DIFFUSE, TFACTOR) alpha.
//
// research/XIClient/src/XIClient/source/Rendering/ZoneRenderer.cpp DoD3mDraw — the MMB table
// (CMoD3mElem.cpp DoMMBDraw routes there), a4 = renderStateFlags bit 12: textured stage 0
// MODULATE(TEXTURE, CURRENT) with a4 set, MODULATE(TEXTURE, TFACTOR) with it clear; stage 1
// MODULATE4X(CURRENT, TFACTOR) / MODULATE4X(CURRENT, DIFFUSE) to match. Alpha per a4: set ->
// MODULATE4X(DIFFUSE, TFACTOR), clear -> MODULATE2X(TEXTURE, TFACTOR) then MODULATE4X(CURRENT,
// DIFFUSE). Untextured: MODULATE2X(CURRENT, TFACTOR) rgb / MODULATE4X(CURRENT, TFACTOR) alpha.
// MMB colours are the raw byte/255 (mmb::VERTEX_COLOR_DIVISOR), no upload doubling — so the
// same two-stage form below with D un-doubled yields DoD3mDraw's totals exactly:
// textured rgb 4*D*T*F, alpha a4 ? 4*Da*Fa : min(1,2TaFa)*4Da; untextured rgb 2*D*F,
// alpha 4*Da*Fa.

#import bevy_pbr::mesh_functions
#import bevy_pbr::view_transformations::position_world_to_clip
#import bevy_pbr::mesh_view_bindings as view_bindings
#import bevy_pbr::mesh_view_types
#ifdef DISTANCE_FOG
#import bevy_pbr::fog as fog_fns
#endif

struct ParticleUniform {
    // x = the premultiply the fixed-function blend state expects, mirroring
    // bevy_pbr::pbr_functions::premultiply_alpha. A custom fragment shader bypasses that
    // function, so an Add-mode particle that does not premultiply here erases the
    // background by (1 - a) instead of adding to it.
    // y = the element's fog selector (FOG_*), from ffxi_particle_material.rs ParticleFog.
    // z = 1.0 when the table drops the texture alpha: NonZeroOneTSS for D3m, DoD3mDraw a4
    //     set for MMB (ffxi_particle_material.rs).
    // w = the stage table selector (PATH_*), from ffxi_particle_material.rs.
    params: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> data: ParticleUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var particle_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var particle_sampler: sampler;

const PREMULTIPLY_NONE: f32 = 0.0;
const PREMULTIPLY_ADD: f32 = 1.0;
const PREMULTIPLY_MULTIPLY: f32 = 2.0;

const FOG_OFF: f32 = 0.0;
const FOG_ZONE: f32 = 1.0;
const FOG_BLACK: f32 = 2.0;

// data.params.w selectors (ffxi_particle_material.rs). Both untextured tables — ZeroOneTSS
// and DoD3mDraw's — are the same MODULATE2X/MODULATE4X against TFACTOR, so they share one.
const PATH_D3M_TEXTURED: f32 = 0.0;
const PATH_D3M_UNTEXTURED: f32 = 1.0;
const PATH_MMB_TEXTURED: f32 = 2.0;
// Lamp halo alpha map: neutral light only — adds no authored colour, lifts the pixel
// underneath along the sheet's alpha pattern.
const PATH_LAMP_ALPHAMAP: f32 = 3.0;

// d3d8types.h D3DTOP_MODULATE2X / MODULATE4X — the per-stage gains of every table above.
const STAGE_MODULATE_2X: f32 = 2.0;
const STAGE_MODULATE_4X: f32 = 4.0;

// Bevy's material pipeline exposes standard attributes at fixed shader locations
// (bevy_pbr render/mesh.rs MeshPipeline::specialize): position 0, normal 1, uv 2,
// uv_b 3, tangent 4, color 5. The per-particle factor rides the TANGENT slot —
// stage 1's TFACTOR argument.
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(4) factor: vec4<f32>,
    @location(5) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) factor: vec4<f32>,
};

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world = mesh_functions::mesh_position_local_to_world(
        mesh_functions::get_world_from_local(v.instance_index),
        vec4<f32>(v.position, 1.0),
    );
    out.world_position = world.xyz;
    out.clip_position = position_world_to_clip(world.xyz);
    out.uv = v.uv;
    out.color = v.color;
    out.factor = v.factor;
    return out;
}

// research/XIClient/src/XIClient/source/World/Generator/Effects/CMoElem.cpp CMoElem::PrepDX —
// a fogged element takes the area's linear fog like terrain (zone_ffxi.wgsl apply_distance_fog),
// with the colour forced to black for FOG_BLACK. Fixed-function fog lands on the fragment
// before the blend, so this runs before the premultiply below.
fn apply_element_fog(color: vec4<f32>, world_pos: vec3<f32>, selector: f32) -> vec4<f32> {
#ifdef DISTANCE_FOG
    if selector == FOG_OFF {
        return color;
    }
    var fog_params = view_bindings::fog;
    if selector == FOG_BLACK {
        fog_params.base_color = vec4<f32>(0.0, 0.0, 0.0, fog_params.base_color.a);
    }
    let dist = length(world_pos - view_bindings::view.world_position);
    let scattering = vec3<f32>(0.0);
    if (fog_params.mode == mesh_view_types::FOG_MODE_LINEAR) {
        return fog_fns::linear_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_EXPONENTIAL) {
        return fog_fns::exponential_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_EXPONENTIAL_SQUARED) {
        return fog_fns::exponential_squared_fog(fog_params, color, dist, scattering);
    } else if (fog_params.mode == mesh_view_types::FOG_MODE_ATMOSPHERIC) {
        return fog_fns::atmospheric_fog(fog_params, color, dist, scattering);
    }
#endif
    return color;
}

// Stage 0 of the selected table: D3D saturates after every op, so each min() is a stage
// boundary, not a final clamp. Returns (stage0 rgb, stage0 alpha) with the texture argument
// already in for the textured tables; `in_factor` is needed only by DoD3mDraw's a4-clear
// alpha lane, which modulates TEXTURE against TFACTOR.
fn stage0(d: vec4<f32>, texel: vec4<f32>, in_factor: vec4<f32>) -> vec4<f32> {
    if (data.params.w == PATH_D3M_TEXTURED) {
        // NonZeroTwoTSS / NonZeroOneTSS stage 0: MODULATE2X(CURRENT, TEXTURE); the One table
        // selects DIFFUSE for alpha (SELECTARG1 — no doubling).
        let rgb = min(STAGE_MODULATE_2X * d.rgb * texel.rgb, vec3<f32>(1.0));
        if (data.params.z > 0.5) {
            return vec4<f32>(rgb, d.a);
        }
        return vec4<f32>(rgb, min(STAGE_MODULATE_2X * d.a * texel.a, 1.0));
    }
    if (data.params.w == PATH_MMB_TEXTURED) {
        // DoD3mDraw textured stage 0: MODULATE(TEXTURE, CURRENT) with a4 set; with it clear
        // the texture modulates TFACTOR and stage 1 takes DIFFUSE. No doubling — MMB colours
        // are raw byte/255 so every product here is already <= 1.
        if (data.params.z > 0.5) {
            return vec4<f32>(d.rgb * texel.rgb, d.a);
        }
        return vec4<f32>(
            texel.rgb * in_factor.rgb,
            min(STAGE_MODULATE_2X * in_factor.a * texel.a, 1.0),
        );
    }
    // ZeroOneTSS / DoD3mDraw untextured: one stage against TFACTOR — in the shared form the
    // stage-0 output is D itself and stage 1 supplies both gains.
    return vec4<f32>(d.rgb, d.a);
}

// Stage 1 of every table: MODULATE2X(CURRENT, TFACTOR) rgb / MODULATE4X(CURRENT, TFACTOR)
// alpha — except DoD3mDraw's textured stage 1 is MODULATE4X on both channels (its stage 0
// carried no doubling), with the second argument TFACTOR when a4 is set and DIFFUSE when it
// is clear.
fn stage1(s0: vec4<f32>, d: vec4<f32>, f: vec4<f32>) -> vec4<f32> {
    if (data.params.w == PATH_MMB_TEXTURED) {
        var arg = f;
        if (data.params.z <= 0.5) {
            arg = d;
        }
        return vec4<f32>(
            min(STAGE_MODULATE_4X * s0.rgb * arg.rgb, vec3<f32>(1.0)),
            min(STAGE_MODULATE_4X * s0.a * arg.a, 1.0),
        );
    }
    return vec4<f32>(
        min(STAGE_MODULATE_2X * s0.rgb * f.rgb, vec3<f32>(1.0)),
        min(STAGE_MODULATE_4X * s0.a * f.a, 1.0),
    );
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    // CMoD3m.cpp PrepDX clamps the doubled upload byte at 0xFF; the D3m/sheet templates store
    // byte/128 unclamped, so a >128 byte reads above 1.0 and must saturate here. MMB colours
    // are already <= 1.0 (byte/255).
    let d = min(in.color, vec4<f32>(1.0));
    var texel = vec4<f32>(1.0);
    if (data.params.w == PATH_D3M_TEXTURED
        || data.params.w == PATH_MMB_TEXTURED
        || data.params.w == PATH_LAMP_ALPHAMAP)
    {
        texel = textureSample(particle_texture, particle_sampler, in.uv);
    }
    var staged = stage1(stage0(d, texel, in.factor), d, in.factor);
    if (data.params.w == PATH_LAMP_ALPHAMAP) {
        staged = vec4<f32>(in.factor.rgb, in.factor.a * texel.a);
    }

    let core = clamp(staged, vec4<f32>(0.0), vec4<f32>(1.0));

    let color = apply_element_fog(core, in.world_position.xyz, data.params.y);

    if data.params.x == PREMULTIPLY_ADD {
        return vec4<f32>(color.rgb * color.a, 0.0);
    }
    if data.params.x == PREMULTIPLY_MULTIPLY {
        return vec4<f32>(color.rgb * color.a, color.a);
    }
    return color;
}
