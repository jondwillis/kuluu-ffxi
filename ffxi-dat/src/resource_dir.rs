use crate::chunk::{walk_tree, ChunkNode};
use crate::cib::Cib;
use crate::datid::DatId;
use crate::kind::ChunkKind;
use crate::scheduler::Scheduler;
use crate::skel::{self, Skeleton};
use crate::skel_anim::{self, SkeletonAnimation};
use crate::skel_mesh::{self, SkelMesh};

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

    pub fn find_animations_matching(&self, query: &DatId) -> Vec<SkeletonAnimation> {
        self.collect_animations()
            .into_iter()
            .filter(|a| a.id.parameterized_match(query))
            .collect()
    }

    pub fn collect_schedulers(&self) -> Vec<Scheduler> {
        let root = self.tree();
        let mut out = Vec::new();
        collect(&root, ChunkKind::Scheduler as u8, &mut |node| {
            if let Ok(s) = Scheduler::parse(node.chunk.name, node.chunk.data) {
                out.push(s);
            }
        });
        out
    }

    pub fn first_cib(&self) -> Option<Cib> {
        let root = self.tree();
        let mut found = None;
        collect(&root, ChunkKind::Cib as u8, &mut |node| {
            if found.is_none() {
                if let Ok(c) = Cib::parse(node.chunk.name, node.chunk.data) {
                    found = Some(c);
                }
            }
        });
        found
    }
}

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
        body[0x02] = 0;
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&0xFFFFu16.to_le_bytes());
        body.extend_from_slice(&0xCDCDCDCDu32.to_le_bytes());
        body
    }

    fn minimal_anim_body() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1.0f32.to_le_bytes());
        b
    }

    fn minimal_mesh_body() -> Vec<u8> {
        let counts = 0x20usize;
        let mapping = 0x28usize;
        let vdata = 0x30usize;
        let instr = 0x40usize;
        let mut b = Vec::new();
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        b.extend_from_slice(&((instr / 2) as u32).to_le_bytes());
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&((counts / 2) as u32).to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&((mapping / 2) as u32).to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&((vdata / 2) as u32).to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.resize(counts, 0);
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.resize(instr, 0);
        b.extend_from_slice(&0xFFFFu16.to_le_bytes());
        b
    }

    fn build_dat() -> Vec<u8> {
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

    fn synth_scheduler_body(motion_clip: &[u8; 4]) -> Vec<u8> {
        let mut b = vec![0u8; SCHEDULER_HEADER_LEN_TEST];

        b.push(0x05);
        b.push(0x0a);
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(motion_clip);

        b.resize(SCHEDULER_HEADER_LEN_TEST + 40, 0);
        b
    }

    const SCHEDULER_HEADER_LEN_TEST: usize = crate::scheduler::SCHEDULER_HEADER_LEN;

    fn build_dat_with_routines() -> Vec<u8> {
        let mut dat = synth_chunk(b"file", ChunkKind::Rmp as u8, &[]);
        dat.extend(synth_chunk(
            b"ati0",
            ChunkKind::Scheduler as u8,
            &synth_scheduler_body(b"at0?"),
        ));

        let cib_body: [u8; crate::cib::CIB_LEN] = [0, 0, 0, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        dat.extend(synth_chunk(b"cib0", ChunkKind::Cib as u8, &cib_body));
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
    fn collect_schedulers_and_cib() {
        let dir = ResourceDir::from_bytes(build_dat_with_routines());
        let scheds = dir.collect_schedulers();
        assert_eq!(scheds.len(), 1);
        assert_eq!(&scheds[0].name, b"ati0");

        let motion = scheds[0]
            .stages
            .iter()
            .find(|t| t.stage.kind == crate::scheduler::StageKind::Motion)
            .expect("motion stage");
        assert_eq!(&motion.stage.id, b"at0?");

        let cib = dir.first_cib().expect("cib");
        assert_eq!(cib.motion_index, 2);
        assert_eq!(cib.motion_option, 1);
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

        let exact = dir.find_animations_matching(&DatId::from_str("idl0"));
        assert_eq!(exact.len(), 1);
        let none = dir.find_animations_matching(&DatId::from_str("idl1"));
        assert!(none.is_empty());
    }
}
