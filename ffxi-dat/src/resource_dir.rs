//! Minimal recursive resource directory over a DAT chunk tree — the Rust
//! analog of XIM `DirectoryResource`, scoped to the skinned-character types
//! (skeletons 0x29, skeleton meshes 0x2A, animations 0x2B).
//!
//! Built from a raw DAT byte buffer via [`crate::chunk::walk_tree`]. The
//! runtime layer uses [`ResourceDir::find_animations_matching`] to fetch
//! animations by a parameterized [`DatId`] (e.g. `idl?`, `run?`).

use crate::chunk::{walk_tree, ChunkNode};
use crate::datid::DatId;
use crate::kind::ChunkKind;
use crate::skel::{self, Skeleton};
use crate::skel_anim::{self, SkeletonAnimation};
use crate::skel_mesh::{self, SkelMesh};

/// Owns a chunk tree built from a DAT buffer and exposes recursive
/// collection of the skinned-character resource types.
pub struct ResourceDir {
    bytes: Vec<u8>,
}

impl ResourceDir {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        ResourceDir { bytes }
    }

    fn tree(&self) -> ChunkNode<'_> {
        walk_tree(&self.bytes)
    }

    pub fn collect_skeletons(&self) -> Vec<Skeleton> {
        let root = self.tree();
        let mut out = Vec::new();
        collect(&root, ChunkKind::Bone as u8, &mut |node| {
            out.push(skel::parse(
                DatId::from_name(&node.chunk.name),
                node.chunk.data,
            ));
        });
        out
    }

    pub fn collect_skel_meshes(&self) -> Vec<SkelMesh> {
        let root = self.tree();
        let mut out = Vec::new();
        collect(&root, ChunkKind::VertexOs2 as u8, &mut |node| {
            out.push(skel_mesh::parse(
                DatId::from_name(&node.chunk.name),
                node.chunk.data,
            ));
        });
        out
    }

    pub fn collect_animations(&self) -> Vec<SkeletonAnimation> {
        let root = self.tree();
        let mut out = Vec::new();
        collect(&root, ChunkKind::AnimMo2 as u8, &mut |node| {
            out.push(skel_anim::parse(
                DatId::from_name(&node.chunk.name),
                node.chunk.data,
            ));
        });
        out
    }

    /// All animations whose id `parameterized_match`es `query` (e.g. `idl?`).
    pub fn find_animations_matching(&self, query: &DatId) -> Vec<SkeletonAnimation> {
        self.collect_animations()
            .into_iter()
            .filter(|a| a.id.parameterized_match(query))
            .collect()
    }
}

/// Recursively visit every node of `kind` in the tree.
fn collect<'a>(node: &ChunkNode<'a>, kind: u8, visit: &mut dyn FnMut(&ChunkNode<'a>)) {
    if node.chunk.kind == kind {
        visit(node);
    }
    for child in &node.children {
        collect(child, kind, visit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic chunk with a 16-byte header (matching `chunk.rs`).
    fn synth_chunk(name: &[u8; 4], kind: u8, body: &[u8]) -> Vec<u8> {
        let total = 16 + body.len();
        let padded = total.div_ceil(16) * 16;
        let pad = padded - total;
        let size_units = (padded / 16) as u32;
        let value = (size_units << 7) | (kind as u32 & 0x7F);
        let mut out = Vec::with_capacity(padded);
        out.extend_from_slice(name);
        out.extend_from_slice(&value.to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, 8));
        out.extend_from_slice(body);
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    fn minimal_skeleton_body() -> Vec<u8> {
        let mut body = vec![0u8; 0x04];
        body[0x02] = 0; // no joints
        body.extend_from_slice(&0u16.to_le_bytes()); // numReferences
        body.extend_from_slice(&0xFFFFu16.to_le_bytes()); // unk
        body.extend_from_slice(&0xCDCDCDCDu32.to_le_bytes()); // bbox sentinel
        body
    }

    fn minimal_anim_body() -> Vec<u8> {
        // 0 joints; just the 10-byte header.
        let mut b = Vec::new();
        b.extend_from_slice(&0u16.to_le_bytes()); // unk0
        b.extend_from_slice(&0u16.to_le_bytes()); // numJoints
        b.extend_from_slice(&1u16.to_le_bytes()); // numFrames
        b.extend_from_slice(&1.0f32.to_le_bytes()); // duration
        b
    }

    fn minimal_mesh_body() -> Vec<u8> {
        // Header pointing all sections to empty/zero, single+double = 0,
        // instructions = immediate stop. Lay out: counts @ 0x20,
        // mapping @ 0x28, vertexData @ 0x30, instructions @ 0x40.
        let counts = 0x20usize;
        let mapping = 0x28usize;
        let vdata = 0x30usize;
        let instr = 0x40usize;
        let mut b = Vec::new();
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // f1..f6
        b.extend_from_slice(&((instr / 2) as u32).to_le_bytes());
        b.extend_from_slice(&[0, 0]); // mesh/instr count
        b.extend_from_slice(&0u32.to_le_bytes()); // jointArray offset
        b.extend_from_slice(&0u16.to_le_bytes()); // numJoints
        b.extend_from_slice(&((counts / 2) as u32).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes()); // numVertexCounts
        b.extend_from_slice(&((mapping / 2) as u32).to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&((vdata / 2) as u32).to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // end offset
        b.extend_from_slice(&0u16.to_le_bytes()); // end size
        b.resize(counts, 0);
        b.extend_from_slice(&0u16.to_le_bytes()); // single = 0
        b.extend_from_slice(&0u16.to_le_bytes()); // double = 0
        b.resize(instr, 0);
        b.extend_from_slice(&0xFFFFu16.to_le_bytes()); // immediate stop
        b
    }

    fn build_dat() -> Vec<u8> {
        // Rmp container, then a skeleton, a mesh, and two animations.
        let mut dat = synth_chunk(b"file", ChunkKind::Rmp as u8, &[]);
        dat.extend(synth_chunk(
            b"skel",
            ChunkKind::Bone as u8,
            &minimal_skeleton_body(),
        ));
        dat.extend(synth_chunk(
            b"mesh",
            ChunkKind::VertexOs2 as u8,
            &minimal_mesh_body(),
        ));
        dat.extend(synth_chunk(
            b"idl0",
            ChunkKind::AnimMo2 as u8,
            &minimal_anim_body(),
        ));
        dat.extend(synth_chunk(
            b"run0",
            ChunkKind::AnimMo2 as u8,
            &minimal_anim_body(),
        ));
        dat.extend(synth_chunk(b"end\0", ChunkKind::Terminate as u8, &[]));
        dat
    }

    #[test]
    fn collects_all_types() {
        let dir = ResourceDir::from_bytes(build_dat());
        assert_eq!(dir.collect_skeletons().len(), 1);
        assert_eq!(dir.collect_skel_meshes().len(), 1);
        assert_eq!(dir.collect_animations().len(), 2);
    }

    #[test]
    fn find_animations_matching_parameterized() {
        let dir = ResourceDir::from_bytes(build_dat());
        let idl = dir.find_animations_matching(&DatId::from_str("idl?"));
        assert_eq!(idl.len(), 1);
        assert_eq!(idl[0].id.as_str(), "idl0");

        let run = dir.find_animations_matching(&DatId::from_str("run?"));
        assert_eq!(run.len(), 1);
        assert_eq!(run[0].id.as_str(), "run0");

        // Exact (non-parameterized) match.
        let exact = dir.find_animations_matching(&DatId::from_str("idl0"));
        assert_eq!(exact.len(), 1);
        let none = dir.find_animations_matching(&DatId::from_str("idl1"));
        assert!(none.is_empty());
    }
}
