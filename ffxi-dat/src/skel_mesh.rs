use crate::datid::DatId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshType {
    Strip,
    Mesh,
}

#[derive(Debug, Clone)]
pub struct RenderProperties {
    pub t_factor: [u8; 4],
    pub specular_highlight_enabled: bool,
    pub specular_highlight_power: f32,
    pub display_type_flag: u8,
    pub ambient_multiplier: f32,
    // Undecoded (research/xim SkeletonMeshSection.kt:363-366: flag0, displayType, flag2, flag3).
    pub flag0: u8,
    pub flag2: u8,
    pub flag3: u8,
}

impl Default for RenderProperties {
    fn default() -> Self {
        RenderProperties {
            t_factor: [0x80, 0x80, 0x80, 0x80],
            specular_highlight_enabled: false,
            specular_highlight_power: 0.0,
            display_type_flag: 0,
            ambient_multiplier: 1.0,
            flag0: 0,
            flag2: 0,
            flag3: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct JointRef {
    index: usize,
    flipped_index: usize,
    flip_axis: u8,
}

fn unpack_joint_ref(data: u16) -> JointRef {
    JointRef {
        index: (data & 0x7F) as usize,
        flipped_index: ((data >> 7) & 0x7F) as usize,
        flip_axis: ((data >> 14) & 0x3) as u8,
    }
}

#[derive(Debug, Clone)]
struct Vertex {
    p0: [f32; 3],
    p1: [f32; 3],
    n0: [f32; 3],
    n1: [f32; 3],
    joint0_weight: f32,
    joint1_weight: f32,
    joint_index0: u16,
    joint_index1: u16,
    joint_ref0: JointRef,
    joint_ref1: JointRef,
}

#[derive(Debug, Clone)]
pub struct SkinVertex {
    pub p0: [f32; 3],
    pub p1: [f32; 3],
    pub n0: [f32; 3],
    pub n1: [f32; 3],
    pub u: f32,
    pub v: f32,
    pub joint0_weight: f32,
    pub joint1_weight: f32,
    pub joint_index0: u16,
    pub joint_index1: u16,
    pub color: [u8; 4],
}

#[derive(Debug, Clone)]
pub struct MeshBuffer {
    pub mesh_type: MeshType,
    pub texture_name: String,
    pub render_properties: RenderProperties,
    pub vertices: Vec<SkinVertex>,
}

#[derive(Debug, Clone)]
pub struct SkelMesh {
    pub id: DatId,
    pub meshes: Vec<MeshBuffer>,
    pub occlude_type: u8,
}

const HALF_COLOR: [u8; 4] = [0x80, 0x80, 0x80, 0x80];

struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }
    fn next8(&mut self) -> u8 {
        let b = self.data[self.pos];
        self.pos += 1;
        b
    }
    fn next16(&mut self) -> u16 {
        let v = u16::from_le_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        v
    }
    fn next32(&mut self) -> u32 {
        let v = u32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        v
    }
    fn next_f32(&mut self) -> f32 {
        f32::from_bits(self.next32())
    }
    fn next_vec3(&mut self) -> [f32; 3] {
        [self.next_f32(), self.next_f32(), self.next_f32()]
    }

    fn next_bgra(&mut self) -> [u8; 4] {
        let b = self.next8();
        let g = self.next8();
        let r = self.next8();
        let a = self.next8();
        [r, g, b, a]
    }

    fn next_string(&mut self, len: usize) -> String {
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            s.push(self.next8() as char);
        }
        s
    }
}

struct MeshVertex {
    src: usize,
    u: f32,
    v: f32,
    color: [u8; 4],
}

pub fn parse(id: DatId, data: &[u8]) -> SkelMesh {
    let mut c = Cursor::new(data);

    let _f1 = c.next8();
    let _f2 = c.next8();
    let f3 = c.next8();
    let cloth_effect = (f3 & 0x01) != 0;
    let use_joint_array = (f3 & 0x80) != 0;
    let has_normals = !cloth_effect;

    let f4 = c.next8();
    let occlude_type = f4;
    let f5 = c.next8();
    let symmetric = f5 == 0x01;
    let _f6 = c.next8();

    let instruction_offset = 2 * c.next32() as usize;
    let _mesh_count = c.next8();
    let _instr_count = c.next8();

    let joint_array_offset = 2 * c.next32() as usize;
    let num_joints = c.next16() as usize;

    let vertex_counts_offset = 2 * c.next32() as usize;
    let num_vertex_counts = c.next16();

    let vertex_joint_mapping_offset = 2 * c.next32() as usize;
    let _vertex_joint_mapping_count = c.next16();

    let vertex_data_offset = 2 * c.next32() as usize;
    let _vertex_data_size = c.next16();

    let _end_offset = 2 * c.next32() as usize;
    let _end_size = c.next16();

    if cloth_effect {
        parse_cloth_data(&mut c);
    }

    c.pos = joint_array_offset;
    let mut palette = Vec::with_capacity(num_joints);
    for _ in 0..num_joints {
        palette.push(c.next16());
    }

    let resolve = |r: &JointRef| -> u16 {
        if use_joint_array {
            palette[r.index]
        } else {
            r.index as u16
        }
    };
    let resolve_flipped = |r: &JointRef| -> u16 {
        if use_joint_array {
            palette[r.flipped_index]
        } else {
            r.flipped_index as u16
        }
    };

    c.pos = vertex_counts_offset;
    debug_assert_eq!(num_vertex_counts, 2, "expected only 2 types of counts");
    let single_count = c.next16() as usize;
    let double_count = c.next16() as usize;
    let total = single_count + double_count;

    c.pos = vertex_joint_mapping_offset;
    let mut vertices: Vec<Vertex> = Vec::with_capacity(total);
    for _ in 0..single_count {
        let ref0 = unpack_joint_ref(c.next16());
        let joint_index0 = resolve(&ref0);
        let ref1 = unpack_joint_ref(c.next16());
        vertices.push(Vertex {
            p0: [0.0; 3],
            p1: [0.0; 3],
            n0: [0.0; 3],
            n1: [0.0; 3],
            joint0_weight: 1.0,
            joint1_weight: 0.0,
            joint_index0,
            joint_index1: 0,
            joint_ref0: ref0,
            joint_ref1: ref1,
        });
    }
    for _ in 0..double_count {
        let ref0 = unpack_joint_ref(c.next16());
        let joint_index0 = resolve(&ref0);
        let ref1 = unpack_joint_ref(c.next16());
        let joint_index1 = resolve(&ref1);
        vertices.push(Vertex {
            p0: [0.0; 3],
            p1: [0.0; 3],
            n0: [0.0; 3],
            n1: [0.0; 3],
            joint0_weight: 1.0,
            joint1_weight: 0.0,
            joint_index0,
            joint_index1,
            joint_ref0: ref0,
            joint_ref1: ref1,
        });
    }

    c.pos = vertex_data_offset;
    for v in vertices.iter_mut().take(single_count) {
        v.p0 = c.next_vec3();
        if has_normals {
            v.n0 = c.next_vec3();
        }
    }
    for v in vertices.iter_mut().skip(single_count).take(double_count) {
        v.p0[0] = c.next_f32();
        v.p1[0] = c.next_f32();
        v.p0[1] = c.next_f32();
        v.p1[1] = c.next_f32();
        v.p0[2] = c.next_f32();
        v.p1[2] = c.next_f32();
        v.joint0_weight = c.next_f32();
        v.joint1_weight = c.next_f32();
        if has_normals {
            v.n0[0] = c.next_f32();
            v.n1[0] = c.next_f32();
            v.n0[1] = c.next_f32();
            v.n1[1] = c.next_f32();
            v.n0[2] = c.next_f32();
            v.n1[2] = c.next_f32();
        }
    }

    let build = |mesh_type: MeshType,
                 texture_name: &str,
                 render_properties: &RenderProperties,
                 records: &[MeshVertex],
                 out: &mut Vec<MeshBuffer>| {
        let normal: Vec<SkinVertex> = records
            .iter()
            .map(|mv| skin_vertex(&vertices[mv.src], mv))
            .collect();
        out.push(MeshBuffer {
            mesh_type,
            texture_name: texture_name.to_string(),
            render_properties: render_properties.clone(),
            vertices: normal,
        });
        if symmetric {
            let mirrored: Vec<SkinVertex> = records
                .iter()
                .map(|mv| flip_vertex(&vertices[mv.src], mv, &resolve_flipped))
                .collect();
            out.push(MeshBuffer {
                mesh_type,
                texture_name: texture_name.to_string(),
                render_properties: render_properties.clone(),
                vertices: mirrored,
            });
        }
    };

    c.pos = instruction_offset;
    let mut meshes: Vec<MeshBuffer> = Vec::new();
    let mut texture_name = String::new();
    let mut render_properties = RenderProperties::default();

    loop {
        let opcode = c.next16();
        match opcode {
            0xFFFF => break,
            0x8010 => render_properties = read_render_properties(&mut c),
            0x8000 => texture_name = c.next_string(0x10),
            0x5453 => {
                let mv = parse_tri_strip(&mut c);
                build(
                    MeshType::Strip,
                    &texture_name,
                    &render_properties,
                    &mv,
                    &mut meshes,
                );
            }
            0x0054 => {
                let mv = parse_tri_mesh(&mut c);
                build(
                    MeshType::Mesh,
                    &texture_name,
                    &render_properties,
                    &mv,
                    &mut meshes,
                );
            }
            0x0043 => {
                let mv = parse_untextured_tri_mesh(&mut c);
                build(
                    MeshType::Mesh,
                    &texture_name,
                    &render_properties,
                    &mv,
                    &mut meshes,
                );
            }
            0x4353 => {
                let mv = parse_single_color_untextured_tri_strip(&mut c);
                build(
                    MeshType::Strip,
                    &texture_name,
                    &render_properties,
                    &mv,
                    &mut meshes,
                );
            }
            other => panic!("Unknown op-code [{other:#x}] @ {}", c.pos),
        }
    }

    SkelMesh {
        id,
        meshes,
        occlude_type,
    }
}

fn skin_vertex(src: &Vertex, mv: &MeshVertex) -> SkinVertex {
    SkinVertex {
        p0: src.p0,
        p1: src.p1,
        n0: src.n0,
        n1: src.n1,
        u: mv.u,
        v: mv.v,
        joint0_weight: src.joint0_weight,
        joint1_weight: src.joint1_weight,
        joint_index0: src.joint_index0,
        joint_index1: src.joint_index1,
        color: mv.color,
    }
}

fn flip_vertex<F: Fn(&JointRef) -> u16>(
    src: &Vertex,
    mv: &MeshVertex,
    resolve_flipped: &F,
) -> SkinVertex {
    SkinVertex {
        p0: flip_vector(src.p0, src.joint_ref0.flip_axis),
        p1: flip_vector(src.p1, src.joint_ref1.flip_axis),
        n0: flip_vector(src.n0, src.joint_ref0.flip_axis),
        n1: flip_vector(src.n1, src.joint_ref1.flip_axis),
        u: mv.u,
        v: mv.v,
        joint0_weight: src.joint0_weight,
        joint1_weight: src.joint1_weight,
        joint_index0: resolve_flipped(&src.joint_ref0),
        joint_index1: resolve_flipped(&src.joint_ref1),
        color: mv.color,
    }
}

fn flip_vector(orig: [f32; 3], flip_axis: u8) -> [f32; 3] {
    let mut v = orig;
    match flip_axis {
        1 => v[0] = -v[0],
        2 => v[1] = -v[1],
        3 => v[2] = -v[2],
        _ => {}
    }
    v
}

fn parse_cloth_data(c: &mut Cursor) {
    let _locked_single = c.next16();
    let _locked_double = c.next16();
    let _cloth_link_offset = 2 * c.next32() as usize;
    let _cloth_link_size = c.next16();
    let _adjacent_size = c.next16();
    let _diagonal_size = c.next16();
    let _adjacent_spring = c.next_f32();
    let _diagonal_spring = c.next_f32();
    let _exponential_spring = c.next_f32();
    let _gravity = c.next_f32();
    let _movement = c.next_f32();
}

fn read_render_properties(c: &mut Cursor) -> RenderProperties {
    let t_factor = c.next_bgra();
    let _f0 = c.next_f32();
    let _f1 = c.next_f32();
    let flag0 = c.next8();
    let display_type = c.next8();
    let flag2 = c.next8();
    let flag3 = c.next8();
    let ambient_multiplier = c.next_f32();
    let _unk0 = c.next32();
    let _unk1 = c.next32();
    let _unk2 = c.next16();
    let _f4 = c.next_f32();
    let _unk3 = c.next16();
    let specular_highlight_power = c.next_f32();
    let specular_highlight_enabled = c.next_f32() == 1.0;
    RenderProperties {
        t_factor,
        specular_highlight_enabled,
        specular_highlight_power,
        display_type_flag: display_type,
        ambient_multiplier,
        flag0,
        flag2,
        flag3,
    }
}

fn parse_tri_strip(c: &mut Cursor) -> Vec<MeshVertex> {
    let num_tri = c.next16() as usize;
    let mut out = Vec::with_capacity(num_tri + 2);

    let i0 = c.next16() as usize;
    let i1 = c.next16() as usize;
    let i2 = c.next16() as usize;
    let u0 = c.next_f32();
    let v0 = c.next_f32();
    let u1 = c.next_f32();
    let v1 = c.next_f32();
    let u2 = c.next_f32();
    let v2 = c.next_f32();
    out.push(MeshVertex {
        src: i0,
        u: u0,
        v: v0,
        color: HALF_COLOR,
    });
    out.push(MeshVertex {
        src: i1,
        u: u1,
        v: v1,
        color: HALF_COLOR,
    });
    out.push(MeshVertex {
        src: i2,
        u: u2,
        v: v2,
        color: HALF_COLOR,
    });

    for _ in 1..num_tri {
        let idx = c.next16() as usize;
        let u = c.next_f32();
        let v = c.next_f32();
        out.push(MeshVertex {
            src: idx,
            u,
            v,
            color: HALF_COLOR,
        });
    }
    out
}

fn parse_tri_mesh(c: &mut Cursor) -> Vec<MeshVertex> {
    let num_tri = c.next16() as usize;
    let mut out = Vec::with_capacity(num_tri * 3);
    for _ in 0..num_tri {
        let i0 = c.next16() as usize;
        let i1 = c.next16() as usize;
        let i2 = c.next16() as usize;
        let u0 = c.next_f32();
        let v0 = c.next_f32();
        let u1 = c.next_f32();
        let v1 = c.next_f32();
        let u2 = c.next_f32();
        let v2 = c.next_f32();
        out.push(MeshVertex {
            src: i0,
            u: u0,
            v: v0,
            color: HALF_COLOR,
        });
        out.push(MeshVertex {
            src: i1,
            u: u1,
            v: v1,
            color: HALF_COLOR,
        });
        out.push(MeshVertex {
            src: i2,
            u: u2,
            v: v2,
            color: HALF_COLOR,
        });
    }
    out
}

fn parse_untextured_tri_mesh(c: &mut Cursor) -> Vec<MeshVertex> {
    let num_tri = c.next16() as usize;
    let mut out = Vec::with_capacity(num_tri * 3);
    for _ in 0..num_tri {
        let i0 = c.next16() as usize;
        let i1 = c.next16() as usize;
        let i2 = c.next16() as usize;
        let color = c.next_bgra();
        out.push(MeshVertex {
            src: i0,
            u: 0.0,
            v: 0.0,
            color,
        });
        out.push(MeshVertex {
            src: i1,
            u: 0.0,
            v: 0.0,
            color,
        });
        out.push(MeshVertex {
            src: i2,
            u: 0.0,
            v: 0.0,
            color,
        });
    }
    out
}

fn parse_single_color_untextured_tri_strip(c: &mut Cursor) -> Vec<MeshVertex> {
    let num_tri = c.next16() as usize;
    let mut out = Vec::with_capacity(num_tri + 2);
    let i0 = c.next16() as usize;
    let i1 = c.next16() as usize;
    let i2 = c.next16() as usize;
    let color = c.next_bgra();
    out.push(MeshVertex {
        src: i0,
        u: 0.0,
        v: 0.0,
        color,
    });
    out.push(MeshVertex {
        src: i1,
        u: 0.0,
        v: 0.0,
        color,
    });
    out.push(MeshVertex {
        src: i2,
        u: 0.0,
        v: 0.0,
        color,
    });
    for _ in 1..num_tri {
        let idx = c.next16() as usize;
        out.push(MeshVertex {
            src: idx,
            u: 0.0,
            v: 0.0,
            color,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    struct HeaderSpec {
        f3: u8,
        f4: u8,
        f5: u8,
        instruction_words: u32,
        joint_array_words: u32,
        num_joints: u16,
        vertex_counts_words: u32,
        vertex_joint_mapping_words: u32,
        vertex_data_words: u32,
    }

    fn write_header(h: &HeaderSpec) -> Vec<u8> {
        let mut b = vec![0, 0, h.f3, h.f4, h.f5, 0];
        b.extend_from_slice(&h.instruction_words.to_le_bytes());
        b.push(0);
        b.push(0);
        b.extend_from_slice(&h.joint_array_words.to_le_bytes());
        b.extend_from_slice(&h.num_joints.to_le_bytes());
        b.extend_from_slice(&h.vertex_counts_words.to_le_bytes());
        b.extend_from_slice(&2u16.to_le_bytes());
        b.extend_from_slice(&h.vertex_joint_mapping_words.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&h.vertex_data_words.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b
    }

    fn pf(buf: &mut Vec<u8>, v: f32) {
        buf.extend_from_slice(&v.to_le_bytes());
    }
    fn p16(buf: &mut Vec<u8>, v: u16) {
        buf.extend_from_slice(&v.to_le_bytes());
    }

    fn build_basic_body(f5_symmetric: u8, opcode: u16) -> Vec<u8> {
        let counts_off = 0x40usize;
        let mapping_off = 0x48usize;
        let vertex_data_off = 0x60usize;
        let instr_off = 0xC0usize;

        let h = HeaderSpec {
            f3: 0,
            f4: 7,
            f5: f5_symmetric,
            instruction_words: (instr_off / 2) as u32,
            joint_array_words: 0,
            num_joints: 0,
            vertex_counts_words: (counts_off / 2) as u32,
            vertex_joint_mapping_words: (mapping_off / 2) as u32,
            vertex_data_words: (vertex_data_off / 2) as u32,
        };
        let mut body = write_header(&h);
        body.resize(counts_off, 0);

        p16(&mut body, 1);
        p16(&mut body, 1);

        body.resize(mapping_off, 0);

        let single_ref0: u16 = (1u16 << 14) | (5u16 << 7) | 3u16;
        p16(&mut body, single_ref0);
        p16(&mut body, 0);

        let double_ref0: u16 = (2u16 << 14) | (6u16 << 7) | 2u16;
        let double_ref1: u16 = (3u16 << 14) | (8u16 << 7) | 4u16;
        p16(&mut body, double_ref0);
        p16(&mut body, double_ref1);

        body.resize(vertex_data_off, 0);

        for v in [1.0f32, 2.0, 3.0] {
            pf(&mut body, v);
        }
        for v in [0.0f32, 1.0, 0.0] {
            pf(&mut body, v);
        }

        for v in [10.0f32, 11.0, 20.0, 21.0, 30.0, 31.0] {
            pf(&mut body, v);
        }
        pf(&mut body, 0.6);
        pf(&mut body, 0.4);
        for v in [1.0f32, -1.0, 0.0, 0.0, 0.0, 0.0] {
            pf(&mut body, v);
        }

        body.resize(instr_off, 0);
        match opcode {
            0x5453 => {
                p16(&mut body, 0x5453);
                p16(&mut body, 1);
                p16(&mut body, 0);
                p16(&mut body, 1);
                p16(&mut body, 0);
                for _ in 0..6 {
                    pf(&mut body, 0.5);
                }
            }
            0x0054 => {
                p16(&mut body, 0x0054);
                p16(&mut body, 1);
                p16(&mut body, 0);
                p16(&mut body, 1);
                p16(&mut body, 0);
                for _ in 0..6 {
                    pf(&mut body, 0.25);
                }
            }
            0x0043 => {
                p16(&mut body, 0x0043);
                p16(&mut body, 1);
                p16(&mut body, 0);
                p16(&mut body, 1);
                p16(&mut body, 0);
                body.extend_from_slice(&[0x11, 0x22, 0x33, 0x44]);
            }
            0x4353 => {
                p16(&mut body, 0x4353);
                p16(&mut body, 1);
                p16(&mut body, 0);
                p16(&mut body, 1);
                p16(&mut body, 0);
                body.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
            }
            _ => unreachable!(),
        }
        p16(&mut body, 0xFFFF);
        body
    }

    #[test]
    fn next_bgra_returns_rgba() {
        let mut c = Cursor::new(&[0x11, 0x22, 0x33, 0x44]);
        assert_eq!(c.next_bgra(), [0x33, 0x22, 0x11, 0x44]);
    }

    #[test]
    fn next_string_keeps_nul_and_padding() {
        let mut bytes = b"tex\0".to_vec();
        bytes.resize(0x10, 0);
        let mut c = Cursor::new(&bytes);
        let s = c.next_string(0x10);
        assert_eq!(s.len(), 0x10);
        assert_eq!(&s[..3], "tex");
        assert_eq!(s.as_bytes()[3], 0);
        assert_eq!(c.pos, 0x10);
    }

    #[test]
    fn unpack_joint_ref_fields() {
        let data: u16 = (2 << 14) | (0x0A << 7) | 0x05;
        let r = unpack_joint_ref(data);
        assert_eq!(r.index, 0x05);
        assert_eq!(r.flipped_index, 0x0A);
        assert_eq!(r.flip_axis, 2);
    }

    #[test]
    fn single_and_double_vertex_decode() {
        let body = build_basic_body(0, 0x0054);
        let m = parse(DatId::from_str("0000"), &body);
        assert_eq!(m.occlude_type, 7);
        assert_eq!(m.meshes.len(), 1);
        let verts = &m.meshes[0].vertices;
        assert_eq!(verts.len(), 3);

        assert_eq!(verts[0].p0, [1.0, 2.0, 3.0]);
        assert_eq!(verts[0].joint0_weight, 1.0);
        assert_eq!(verts[0].joint1_weight, 0.0);
        assert_eq!(verts[0].joint_index0, 3);

        assert_eq!(verts[1].p0, [10.0, 20.0, 30.0]);
        assert_eq!(verts[1].p1, [11.0, 21.0, 31.0]);
        assert_eq!(verts[1].joint0_weight, 0.6);
        assert_eq!(verts[1].joint1_weight, 0.4);
        assert_eq!(verts[1].joint_index0, 2);
        assert_eq!(verts[1].joint_index1, 4);
    }

    #[test]
    fn symmetric_doubles_buffers_and_mirrors() {
        let body = build_basic_body(1, 0x0054);
        let m = parse(DatId::from_str("0000"), &body);
        assert_eq!(m.meshes.len(), 2);

        let normal = &m.meshes[0].vertices;
        let mirror = &m.meshes[1].vertices;

        assert_eq!(normal[0].p0, [1.0, 2.0, 3.0]);
        assert_eq!(mirror[0].p0, [-1.0, 2.0, 3.0]);
        assert_eq!(mirror[0].joint_index0, 5);

        assert_eq!(normal[1].p0, [10.0, 20.0, 30.0]);
        assert_eq!(mirror[1].p0, [10.0, -20.0, 30.0]);
        assert_eq!(mirror[1].p1, [11.0, 21.0, -31.0]);
        assert_eq!(mirror[1].joint_index0, 6);
        assert_eq!(mirror[1].joint_index1, 8);

        assert_eq!(mirror[1].joint0_weight, 0.6);
        assert_eq!(mirror[1].joint1_weight, 0.4);
    }

    #[test]
    fn tri_strip_count() {
        let body = build_basic_body(0, 0x5453);
        let m = parse(DatId::from_str("0000"), &body);

        assert_eq!(m.meshes[0].mesh_type, MeshType::Strip);
        assert_eq!(m.meshes[0].vertices.len(), 3);
        assert_eq!(m.meshes[0].vertices[0].u, 0.5);
    }

    #[test]
    fn tri_mesh_count() {
        let body = build_basic_body(0, 0x0054);
        let m = parse(DatId::from_str("0000"), &body);

        assert_eq!(m.meshes[0].mesh_type, MeshType::Mesh);
        assert_eq!(m.meshes[0].vertices.len(), 3);
    }

    #[test]
    fn untextured_tri_mesh_carries_bgra() {
        let body = build_basic_body(0, 0x0043);
        let m = parse(DatId::from_str("0000"), &body);
        assert_eq!(m.meshes[0].mesh_type, MeshType::Mesh);
        assert_eq!(m.meshes[0].vertices.len(), 3);

        assert_eq!(m.meshes[0].vertices[0].color, [0x33, 0x22, 0x11, 0x44]);
        assert_eq!(m.meshes[0].vertices[0].u, 0.0);
    }

    #[test]
    fn single_color_untextured_tri_strip_carries_bgra() {
        let body = build_basic_body(0, 0x4353);
        let m = parse(DatId::from_str("0000"), &body);
        assert_eq!(m.meshes[0].mesh_type, MeshType::Strip);
        assert_eq!(m.meshes[0].vertices.len(), 3);

        for v in &m.meshes[0].vertices {
            assert_eq!(v.color, [0xCC, 0xBB, 0xAA, 0xDD]);
        }
    }
}
