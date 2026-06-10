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
//! the material binds a per-actor [`ShaderStorageBuffer`] of bone matrices
//! at binding 3. One material asset (and one storage buffer) is allocated
//! per actor; [`crate::skeleton_instance`] rewrites the buffer each frame.

#![cfg(not(target_arch = "wasm32"))]

use bevy::asset::embedded_asset;
use bevy::mesh::{Mesh, MeshVertexAttribute, MeshVertexBufferLayoutRef, VertexFormat};
use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
};
use bevy::render::storage::ShaderStorageBuffer;
use bevy::shader::ShaderRef;

/// Maximum bones a single actor skeleton can have (FFXI's `maxNumJoints`).
/// The storage buffer is runtime-sized, but joint indices are a single
/// byte on the wire so 128 is the hard ceiling.
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
    pub point_color: [Vec4; 4],
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
        }
    }
}

/// FFXI-faithful skinned character material. One asset per actor; the
/// `joint_matrices` buffer is rewritten each frame from the actor's
/// evaluated skeleton pose.
#[derive(Asset, AsBindGroup, TypePath, Clone, Debug)]
pub struct FfxiSkinnedMaterial {
    #[uniform(0)]
    pub lighting: FfxiLightingUniform,
    #[texture(1)]
    #[sampler(2)]
    pub base_color_texture: Option<Handle<Image>>,
    #[storage(3, read_only)]
    pub joint_matrices: Handle<ShaderStorageBuffer>,
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
    // POSITION attribute, so the default prepass / shadow vertex shaders
    // can't specialize against it. Disable both rather than author bespoke
    // prepass shaders; characters not writing the depth prepass or casting
    // shadows is an acceptable first cut (revisit if shadows are wanted).
    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
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
        Ok(())
    }
}

/// Registers [`FfxiSkinnedMaterial`] and embeds its WGSL.
pub struct FfxiMaterialPlugin;

impl Plugin for FfxiMaterialPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "skinned_ffxi.wgsl");
        app.add_plugins(MaterialPlugin::<FfxiSkinnedMaterial>::default());
    }
}
