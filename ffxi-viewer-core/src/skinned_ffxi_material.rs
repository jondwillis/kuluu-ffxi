//! Custom Bevy material for FFXI-faithful skinned characters — the
//! Rust/WGSL port of FFXI's skinned-character shader (cross-referenced
//! against `research/xim`'s `XimSkinnedShader.kt`). See `skinned_ffxi.wgsl`
//! for the shader and [`crate::skeleton_instance`] for the per-frame
//! bone-matrix upload.
//!
//! This exists because FFXI's skinning + shading can't be expressed
//! through Bevy's `StandardMaterial` + `SkinnedMesh`:
//!   * vertices carry **two** bone-local positions/normals, not one;
//!   * bone matrices are **world-space pose** matrices (no inverse bind);
//!   * shading is `2*vertexColor*texel` with an alpha-test discard, not PBR.
//!
//! The mesh feeds a custom vertex layout (the [`ATTR_*`] attributes) and
//! the material carries the per-actor bone matrices inline as a uniform at
//! binding 3 ([`FfxiJointMatrices`]); [`crate::skeleton_instance`] rewrites
//! that uniform each frame from the evaluated pose.

#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::mesh::{Mesh, MeshVertexAttribute, MeshVertexBufferLayoutRef, VertexFormat};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;

/// Maximum bones a single actor skeleton can have (FFXI's `maxNumJoints`).
/// Fixes the size of the bone uniform array; joint indices are a single
/// byte on the wire so 128 is the hard ceiling. 128 × 64 B = 8 KiB, well
/// under the uniform-binding size limit.
pub const MAX_JOINTS: usize = 128;

// Custom vertex attributes. IDs use an "FFXI" ASCII prefix in the high
// bits so they can't collide with Bevy's small built-in attribute ids.
const ATTR_ID_BASE: u64 = 0x4646_5849_0000_0000; // "FFXI" << 32

/// Bone-0-local position (rigid verts store the only position here).
pub const ATTR_POSITION0: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Position0", ATTR_ID_BASE, VertexFormat::Float32x3);
/// Bone-1-local position (zero for rigid verts).
pub const ATTR_POSITION1: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Position1", ATTR_ID_BASE + 1, VertexFormat::Float32x3);
/// Bone-0-local normal.
pub const ATTR_NORMAL0: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Normal0", ATTR_ID_BASE + 2, VertexFormat::Float32x3);
/// Bone-1-local normal.
pub const ATTR_NORMAL1: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Normal1", ATTR_ID_BASE + 3, VertexFormat::Float32x3);
/// Blend weight `w` for bone 0; bone 1's weight is `1 - w`.
pub const ATTR_JOINT_WEIGHT: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_JointWeight", ATTR_ID_BASE + 4, VertexFormat::Float32);
/// Skeleton bone index for slot 0 (index into the bone-matrix buffer).
pub const ATTR_JOINT0: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Joint0", ATTR_ID_BASE + 5, VertexFormat::Uint32);
/// Skeleton bone index for slot 1.
pub const ATTR_JOINT1: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Joint1", ATTR_ID_BASE + 6, VertexFormat::Uint32);
/// Per-vertex RGBA color (white for textured meshes; carries the BGRA
/// color for FFXI's untextured "C"/"CS" meshes once those are wired).
pub const ATTR_COLOR: MeshVertexAttribute =
    MeshVertexAttribute::new("Ffxi_Color", ATTR_ID_BASE + 7, VertexFormat::Float32x4);

/// FFXI light model inputs, uploaded as the material's uniform at binding
/// 0. Mirrors the `FfxiLighting` struct in `skinned_ffxi.wgsl` exactly —
/// keep field order/types in sync. Directions are world-space unit
/// vectors pointing *from* the light; `*_color.w` is intensity (dir) or
/// range (point).
#[derive(Clone, Debug, ShaderType)]
pub struct FfxiLightingUniform {
    pub ambient: Vec4,
    pub dir0_dir: Vec4,
    pub dir0_color: Vec4,
    pub dir1_dir: Vec4,
    pub dir1_color: Vec4,
    pub point_pos: [Vec4; 4],
    /// Per-point RGB color in `xyz`; `w` carries the light's range (and a
    /// `w <= 0` slot is treated as empty by the shader).
    pub point_color: [Vec4; 4],
    /// Per-point attenuation `(const, linear, quad)` in `xyz` (`w` unused) for
    /// XIM's `1/(c + l·d + q·d²)` falloff. Actors set `const = 0.5` (the FFXI
    /// point-lights-affect-actors-less dampen, `GLDrawer.kt:285-290`).
    pub point_atten: [Vec4; 4],
}

impl Default for FfxiLightingUniform {
    fn default() -> Self {
        // Neutral fill until `update_ffxi_lighting_system` (M7) sources the
        // real zone lights: soft ambient + one overhead key light so the
        // character is legible from frame 0.
        Self {
            ambient: Vec4::new(0.5, 0.5, 0.5, 1.0),
            dir0_dir: Vec4::new(0.0, -1.0, 0.0, 0.0),
            dir0_color: Vec4::new(0.6, 0.6, 0.6, 1.0),
            dir1_dir: Vec4::ZERO,
            dir1_color: Vec4::ZERO,
            point_pos: [Vec4::ZERO; 4],
            point_color: [Vec4::ZERO; 4],
            point_atten: [Vec4::ZERO; 4],
        }
    }
}

/// Per-actor bone world-pose matrices, uploaded as the material's uniform
/// at binding 3 — the world-space pose the skinned vertices multiply
/// through (no inverse bind). Mirrors `FfxiJoints` in `skinned_ffxi.wgsl`.
///
/// This is a fixed-size *uniform* array rather than a separately-prepared
/// [`ShaderStorageBuffer`] on purpose. A storage asset reallocates its GPU
/// buffer on every `set_data`, while the material's bind group caches the
/// prior buffer by value — so per-frame rewrites reach the shader only
/// when the bind group happens to rebuild, and the two race frame-to-frame
/// (a frame-rate-coupled jiggle). Inlining the bones as a uniform makes the
/// upload atomic with the material's own bind group (same mechanism as
/// `lighting`), and matches how XIM uploads its bone array.
#[derive(Clone, Debug, ShaderType)]
pub struct FfxiJointMatrices {
    pub matrices: [Mat4; MAX_JOINTS],
}

impl Default for FfxiJointMatrices {
    fn default() -> Self {
        Self {
            matrices: [Mat4::IDENTITY; MAX_JOINTS],
        }
    }
}

impl FfxiJointMatrices {
    /// Overwrite the leading `pose.len()` bones from an evaluated pose;
    /// trailing slots stay as-is (vertex joint indices are bone ids within
    /// `pose.len()`). Clamped to `MAX_JOINTS`.
    pub fn set_from(&mut self, pose: &[Mat4]) {
        let n = pose.len().min(MAX_JOINTS);
        self.matrices[..n].copy_from_slice(&pose[..n]);
    }
}

/// Per-material flags packed into a uniform, one boolean per lane (`> 0.5`
/// = on), filling a 16-byte std140 `Vec4`:
///   * `flags.x` — `has_texture`: 1.0 when `base_color_texture` is real, 0.0
///     for FFXI's untextured vertex-colored meshes (C/CS ops, e.g. bee
///     wings); 0.0 makes the shader skip the texture sample + alpha-test and
///     render the vertex color directly.
///   * `flags.y` — `realistic`: 1.0 selects the Bevy-scene-driven
///     energy-conserving lighting model, 0.0 the FFXI-faithful
///     `2*vertexColor*texel` model (the "Model Lighting" setting).
///   * `flags.z` — `receive_shadows`: 1.0 routes the directional cascade
///     shadow map into the lit fragment so the model receives world / self
///     cast-shadows (the "Model Shadows" setting). The faithful branch dims
///     toward a soft floor; the realistic branch takes the full factor.
///   * `flags.w` — reserved padding.
/// Lanes y/z are stamped each frame by
/// `ffxi_actor_render::update_ffxi_render_actor_lighting`.
#[derive(Clone, Debug, ShaderType)]
pub struct FfxiMaterialFlags {
    pub flags: Vec4,
}

impl Default for FfxiMaterialFlags {
    fn default() -> Self {
        Self {
            flags: Vec4::new(1.0, 0.0, 0.0, 0.0),
        }
    }
}

/// FFXI-faithful skinned character material. One asset per mesh group; the
/// `joints` uniform is rewritten each frame from the actor's evaluated
/// skeleton pose.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct FfxiSkinnedMaterial {
    #[uniform(0)]
    pub lighting: FfxiLightingUniform,
    #[texture(1)]
    #[sampler(2)]
    pub base_color_texture: Option<Handle<Image>>,
    #[uniform(3)]
    pub joints: FfxiJointMatrices,
    #[uniform(4)]
    pub material_flags: FfxiMaterialFlags,
}

impl Material for FfxiSkinnedMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skinned_ffxi.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skinned_ffxi.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        // Alpha-tested: opaque depth-write, no blending, discard at the
        // FFXI threshold (the shader discards manually at 69/255).
        AlphaMode::Mask(0.271)
    }

    // The custom vertex layout (position0/position1/...) has no standard
    // POSITION attribute, so the *default* prepass / shadow vertex shaders can't
    // specialize against it. We supply a BESPOKE depth-only shadow shader
    // (`skinned_ffxi_prepass.wgsl`) that reuses the FFXI dual-bone skin, so
    // characters CAST directional-light shadows.
    //
    // `enable_shadows = true` runs that shader for the shadow pass.
    // `enable_prepass = false` keeps the material OUT of the camera's
    // depth/normal/motion-vector prepass: our prepass module is depth-only (no
    // normal / motion-vector outputs), and a TAA/SSAO camera that runs those
    // prepasses would otherwise mis-pipeline the material. Shadow RECEIVE for the
    // realistic branch reads Bevy's shadow maps directly in the lit fragment, so
    // it needs no camera prepass either.
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        true
    }

    fn prepass_vertex_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skinned_ffxi_prepass.wgsl".into()
    }

    fn prepass_fragment_shader() -> ShaderRef {
        "embedded://ffxi_viewer_core/skinned_ffxi_prepass.wgsl".into()
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        let vertex_layout = layout.0.get_layout(&[
            ATTR_POSITION0.at_shader_location(0),
            ATTR_POSITION1.at_shader_location(1),
            ATTR_NORMAL0.at_shader_location(2),
            ATTR_NORMAL1.at_shader_location(3),
            Mesh::ATTRIBUTE_UV_0.at_shader_location(4),
            ATTR_JOINT_WEIGHT.at_shader_location(5),
            ATTR_JOINT0.at_shader_location(6),
            ATTR_JOINT1.at_shader_location(7),
            ATTR_COLOR.at_shader_location(8),
        ])?;
        descriptor.vertex.buffers = vec![vertex_layout];
        // FFXI never backface-culls character geometry (symmetry/mirror
        // produces both windings); match that.
        descriptor.primitive.cull_mode = None;

        // No `entry_point` override. The lit pass uses `skinned_ffxi.wgsl` and
        // the shadow pass uses `skinned_ffxi_prepass.wgsl`; each module has a
        // single `vertex` / `fragment` entry, so Bevy's `None` (auto-select)
        // resolves the right one per pipeline. The previous all-in-one module
        // needed a manual override keyed off the prepass mesh-key bits, but a TAA
        // / SSAO camera sets those same bits on the MAIN pass — which routed the
        // lit pass through the depth-only fragment and rendered every character
        // pure black.
        Ok(())
    }
}

/// Registers [`FfxiSkinnedMaterial`] and embeds its WGSL.
pub struct FfxiMaterialPlugin;

impl Plugin for FfxiMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "skinned_ffxi.wgsl");
        embedded_asset!(app, "skinned_ffxi_prepass.wgsl");
        app.add_plugins(MaterialPlugin::<FfxiSkinnedMaterial>::default());
    }
}
