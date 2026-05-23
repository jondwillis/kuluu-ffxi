use recastnavigation_rs::recast::*;

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcHeightfieldState {
    pub width: i32,
    pub height: i32,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub cs: f32,
    pub ch: f32,
    pub spans: Vec<RcSpanState>,
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcSpanState {
    pub smin: u32,
    pub smax: u32,
    pub area: u32,
}

pub fn dump_heightfield_state(hf: &RcHeightfield) -> RcHeightfieldState {
    let mut state = RcHeightfieldState {
        width: hf.width(),
        height: hf.height(),
        bmin: hf.bmin,
        bmax: hf.bmax,
        cs: hf.cs,
        ch: hf.ch,
        spans: Vec::new(),
    };

    for idx in 0..((hf.width() * hf.height()) as usize) {
        let mut span = hf.get_span(idx);
        while let Some(sp) = span {
            state.spans.push(RcSpanState {
                smin: sp.smin(),
                smax: sp.smax(),
                area: sp.area(),
            });
            span = sp.next();
        }
    }
    return state;
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcCompactHeightfieldState {
    pub width: i32,
    pub height: i32,
    pub walkable_height: i32,
    pub walkable_climb: i32,
    pub border_size: i32,
    pub max_distance: u16,
    pub max_regions: u16,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub cs: f32,
    pub ch: f32,
    pub cells: Vec<RcCompactCellState>,
    pub spans: Vec<RcCompactSpanState>,
    pub dist: Vec<u16>,
    pub areas: Vec<u8>,
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcCompactCellState {
    pub index: u32,
    pub count: u32,
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcCompactSpanState {
    pub y: u16,
    pub reg: u16,
    pub con: u32,
    pub h: u32,
}

pub fn dump_compact_heightfield_state(chf: &RcCompactHeightfield) -> RcCompactHeightfieldState {
    return RcCompactHeightfieldState {
        width: chf.width(),
        height: chf.height(),
        walkable_height: chf.walkable_height,
        walkable_climb: chf.walkable_climb,
        border_size: chf.border_size,
        max_distance: chf.max_distance,
        max_regions: chf.max_regions,
        bmin: chf.bmin,
        bmax: chf.bmax,
        cs: chf.cs,
        ch: chf.ch,
        cells: chf
            .cells()
            .iter()
            .map(|x| RcCompactCellState {
                index: x.index(),
                count: x.count(),
            })
            .collect(),
        spans: chf
            .spans()
            .iter()
            .map(|x| RcCompactSpanState {
                y: x.y,
                reg: x.reg,
                con: x.con(),
                h: x.h(),
            })
            .collect(),
        dist: chf.dist().iter().map(|x| *x).collect(),
        areas: chf.areas().iter().map(|x| *x).collect(),
    };
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcContourSetState {
    pub conts: Vec<RcContourState>,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub cs: f32,
    pub ch: f32,
    pub width: i32,
    pub height: i32,
    pub border_size: i32,
    pub max_error: f32,
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcContourState {
    pub verts: Vec<[i32; 4]>,
    pub rverts: Vec<[i32; 4]>,
    pub reg: u16,
    pub area: u8,
}

pub fn dump_contour_set_state(cs: &RcContourSet) -> RcContourSetState {
    return RcContourSetState {
        conts: cs
            .conts()
            .iter()
            .map(|x| RcContourState {
                verts: x.verts().into(),
                rverts: x.rverts().into(),
                reg: x.reg,
                area: x.area,
            })
            .collect(),
        bmin: cs.bmin,
        bmax: cs.bmax,
        cs: cs.cs,
        ch: cs.ch,
        width: cs.width,
        height: cs.height,
        border_size: cs.border_size,
        max_error: cs.max_error,
    };
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcPolyMeshState {
    pub verts: Vec<[u16; 3]>,
    pub polys: Vec<u16>,
    pub regs: Vec<u16>,
    pub flags: Vec<u16>,
    pub areas: Vec<u8>,
    pub maxpolys: usize,
    pub nvp: usize,
    pub bmin: [f32; 3],
    pub bmax: [f32; 3],
    pub cs: f32,
    pub ch: f32,
    pub border_size: i32,
    pub max_edge_error: f32,
}

pub fn dump_poly_mesh_state(pm: &RcPolyMesh) -> RcPolyMeshState {
    return RcPolyMeshState {
        verts: pm.verts().into(),
        polys: pm.polys().into(),
        regs: pm.regs().into(),
        flags: pm.flags().into(),
        areas: pm.areas().into(),
        maxpolys: pm.maxpolys(),
        nvp: pm.nvp(),
        bmin: pm.bmin,
        bmax: pm.bmax,
        cs: pm.cs,
        ch: pm.ch,
        border_size: pm.border_size,
        max_edge_error: pm.max_edge_error,
    };
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
pub struct RcPolyMeshDetailState {
    pub meshes: Vec<[u32; 4]>,
    pub verts: Vec<[f32; 3]>,
    pub tris: Vec<[u8; 4]>,
}

pub fn dump_poly_mesh_detail_state(pmd: &RcPolyMeshDetail) -> RcPolyMeshDetailState {
    return RcPolyMeshDetailState {
        meshes: pmd.meshes().into(),
        verts: pmd.verts().into(),
        tris: pmd.tris().into(),
    };
}
