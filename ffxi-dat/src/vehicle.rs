use crate::{walk, ChunkKind};

pub const POINT_LIST_KIND: u8 = 0x3E;
pub const VOYAGE_ROUTE_KIND: u8 = 6;

const SHIP_DIRECTORY: [u8; 4] = *b"ship";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoyageLayout {
    pub scenery_mzb: usize,
    pub ship_mzb: usize,
}

// Retail voyage DATs place the passenger hull under mode/ship, separately from the scenery MZB.
pub fn voyage_layout(bytes: &[u8]) -> Option<VoyageLayout> {
    let mut directories = Vec::new();
    let mut scenery_mzb = None;
    let mut ship_mzb = None;
    for (index, chunk) in walk(bytes).filter_map(Result::ok).enumerate() {
        match ChunkKind::from_u8(chunk.kind) {
            Some(ChunkKind::Rmp) => directories.push(chunk.name),
            Some(ChunkKind::Terminate) => {
                directories.pop();
            }
            Some(ChunkKind::Mzb) if directories.last() == Some(&SHIP_DIRECTORY) => {
                ship_mzb = Some(index)
            }
            Some(ChunkKind::Mzb) if scenery_mzb.is_none() => scenery_mzb = Some(index),
            _ => {}
        }
    }
    Some(VoyageLayout {
        scenery_mzb: scenery_mzb?,
        ship_mzb: ship_mzb?,
    })
}

pub fn collision_mzb_index(bytes: &[u8]) -> Option<usize> {
    voyage_layout(bytes)
        .map(|layout| layout.ship_mzb)
        .or_else(|| {
            walk(bytes)
                .filter_map(Result::ok)
                .position(|chunk| chunk.kind == ChunkKind::Mzb as u8)
        })
}

// FFXiMain.dll SHA-256 f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c.
// Transport resource loader RVA 0xC3330.
const TRANSPORT_FILE_BASE: u32 = 0x791C;
pub fn transport_file_id(selector: u32) -> Option<u32> {
    TRANSPORT_FILE_BASE.checked_add(selector)
}

#[derive(Debug, Clone)]
pub struct PointList(pub Vec<[f32; 3]>);

impl PointList {
    pub fn parse(body: &[u8]) -> Option<Self> {
        const HEADER_LEN: usize = 16;
        const POINT_LEN: usize = 16;
        let count = u32::from_le_bytes(body.get(..4)?.try_into().ok()?) as usize;
        if count == 0 || count > body.len().saturating_sub(HEADER_LEN) / POINT_LEN {
            return None;
        }
        let points = body
            .get(HEADER_LEN..HEADER_LEN + count * POINT_LEN)?
            .chunks_exact(POINT_LEN)
            .map(|p| {
                std::array::from_fn(|axis| {
                    f32::from_le_bytes(p[axis * 4..axis * 4 + 4].try_into().unwrap())
                })
            })
            .collect::<Vec<[f32; 3]>>();
        points
            .iter()
            .flatten()
            .all(|v| v.is_finite())
            .then_some(Self(points))
    }
}

#[derive(Debug, Clone)]
pub struct Spline {
    points: Vec<[f32; 3]>,
    lengths: Vec<f32>,
    total: f32,
}

impl Spline {
    // research/XIClient/src/XIClient/source/Common/Math/Spline.cpp Spline::PrecomputeSpline
    pub fn new(points: Vec<[f32; 3]>) -> Option<Self> {
        const MIN_CHORD: f32 = 0.01;
        if points.len() < 2 || !points.iter().flatten().all(|v| v.is_finite()) {
            return None;
        }
        let lengths: Vec<f32> = points
            .windows(2)
            .map(|p| {
                (0..3)
                    .map(|axis| (p[1][axis] - p[0][axis]).powi(2))
                    .sum::<f32>()
                    .sqrt()
                    .max(MIN_CHORD)
            })
            .collect();
        let total = lengths.iter().sum();
        Some(Self {
            points,
            lengths,
            total,
        })
    }

    pub fn sample(&self, progress: f32) -> [f32; 3] {
        self.evaluate(progress).0
    }
    pub fn tangent(&self, progress: f32) -> [f32; 3] {
        self.evaluate(progress).1
    }
    fn evaluate(&self, progress: f32) -> ([f32; 3], [f32; 3]) {
        let mut distance = progress.clamp(0.0, 1.0) * self.total;
        let mut index = 0;
        while index + 1 < self.lengths.len() && distance > self.lengths[index] {
            distance -= self.lengths[index];
            index += 1;
        }
        let length = self.lengths[index];
        let t = (distance / length).clamp(0.0, 1.0);
        let a = self.points[index];
        let b = self.points[index + 1];
        let prev = if index == 0 {
            std::array::from_fn(|axis| 2.0 * a[axis] - b[axis])
        } else {
            self.points[index - 1]
        };
        let next = self
            .points
            .get(index + 2)
            .copied()
            .unwrap_or_else(|| std::array::from_fn(|axis| 2.0 * b[axis] - a[axis]));
        let before = self.lengths[index.saturating_sub(1)];
        let after = self.lengths.get(index + 1).copied().unwrap_or(length);
        let h0 = 2.0 * t.powi(3) - 3.0 * t.powi(2) + 1.0;
        let h1 = -2.0 * t.powi(3) + 3.0 * t.powi(2);
        let h2 = t.powi(3) - 2.0 * t.powi(2) + t;
        let h3 = t.powi(3) - t.powi(2);
        let tangents: [(f32, f32); 3] = std::array::from_fn(|axis| {
            let entry = (length * length / before * (a[axis] - prev[axis])
                + before * (b[axis] - a[axis]))
                / (before + length);
            let exit = (after * (b[axis] - a[axis])
                + length * length / after * (next[axis] - b[axis]))
                / (length + after);
            (entry, exit)
        });
        (
            std::array::from_fn(|axis| {
                h0 * a[axis] + h1 * b[axis] + h2 * tangents[axis].0 + h3 * tangents[axis].1
            }),
            std::array::from_fn(|axis| {
                (6.0 * t * t - 6.0 * t) * a[axis]
                    + (-6.0 * t * t + 6.0 * t) * b[axis]
                    + (3.0 * t * t - 4.0 * t + 1.0) * tangents[axis].0
                    + (3.0 * t * t - 2.0 * t) * tangents[axis].1
            }),
        )
    }
}

#[derive(Debug, Clone)]
pub struct VoyageRoute {
    pub position: Spline,
    pub facing: Spline,
}

impl VoyageRoute {
    // FFXiMain.dll SHA-256 f4f90fbd080c05448aab3f866b127d7c1675b3cc15c8beaa57bfc584064b7e7c.
    // RVA 0x54BB0 builds position and facing splines.
    pub fn parse(body: &[u8]) -> Option<Self> {
        const HEADER_LEN: usize = 32;
        const COUNT_OFFSET: usize = 16;
        const ROW_LEN: usize = 48;
        const FACING_OFFSET: usize = 16;
        const COUNT_MASK: u32 = 0xFF;
        const CAMERA_ENDPOINT_FLAGS: u32 = (1 << 19) | (1 << 20);
        let count_flags =
            u32::from_le_bytes(body.get(COUNT_OFFSET..COUNT_OFFSET + 4)?.try_into().ok()?);
        if count_flags & CAMERA_ENDPOINT_FLAGS != 0 {
            return None;
        }
        let count = (count_flags & COUNT_MASK) as usize;
        if count > body.len().saturating_sub(HEADER_LEN) / ROW_LEN {
            return None;
        }
        let mut position = Vec::new();
        let mut facing = Vec::new();
        for row in body
            .get(HEADER_LEN..HEADER_LEN + count * ROW_LEN)?
            .chunks_exact(ROW_LEN)
        {
            let vector = |offset| {
                std::array::from_fn(|axis| {
                    f32::from_le_bytes(
                        row[offset + axis * 4..offset + axis * 4 + 4]
                            .try_into()
                            .unwrap(),
                    )
                })
            };
            position.push(vector(0));
            facing.push(vector(FACING_OFFSET));
        }
        Some(Self {
            position: Spline::new(position)?,
            facing: Spline::new(facing)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 0.00001;
    const CHUNK_HEADER_BYTES: usize = 16;
    const ROUTE_HEADER_BYTES: usize = 32;
    const ROUTE_ROW_FLOATS: usize = 12;
    const ROUTE_COUNT_OFFSET: usize = 16;
    const UNUSED_ROUTE_SCALAR: usize = 8;

    fn near(actual: [f32; 3], expected: [f32; 3]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() < EPSILON,
                "{actual} != {expected}"
            );
        }
    }

    #[test]
    fn nonuniform_spline_matches_retail_weighted_basis_and_endpoint_extension() {
        let spline = Spline::new(vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 2.0, 0.0]]).unwrap();
        near(spline.sample(0.0), [0.0, 0.0, 0.0]);
        near(spline.sample(1.0 / 6.0), [13.0 / 24.0, -1.0 / 24.0, 0.0]);
        near(spline.sample(1.0 / 3.0), [1.0, 0.0, 0.0]);
        near(spline.sample(2.0 / 3.0), [7.0 / 6.0, 5.0 / 6.0, 0.0]);
        near(spline.tangent(2.0 / 3.0), [-1.0 / 3.0, 7.0 / 3.0, 0.0]);
        near(spline.sample(1.0), [1.0, 2.0, 0.0]);
        near(spline.sample(-1.0), spline.sample(0.0));
        near(spline.sample(2.0), spline.sample(1.0));
    }

    #[test]
    fn spline_repeated_points_stay_finite_and_invalid_controls_are_rejected() {
        let repeated = Spline::new(vec![[2.0, 3.0, 4.0]; 3]).unwrap();
        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
            near(repeated.sample(progress), [2.0, 3.0, 4.0]);
            near(repeated.tangent(progress), [0.0; 3]);
        }
        assert!(Spline::new(vec![]).is_none());
        assert!(Spline::new(vec![[0.0; 3]]).is_none());
        assert!(Spline::new(vec![[0.0; 3], [f32::NAN, 0.0, 0.0]]).is_none());
    }

    fn route_bytes() -> Vec<u8> {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 2.0, 0.0]];
        let targets = [[10.0, 0.0, 0.0], [10.0, 4.0, 0.0], [13.0, 4.0, 0.0]];
        let mut body = vec![0; ROUTE_HEADER_BYTES];
        body[ROUTE_COUNT_OFFSET..ROUTE_COUNT_OFFSET + 4]
            .copy_from_slice(&(positions.len() as u32).to_le_bytes());
        for (position, target) in positions.into_iter().zip(targets) {
            let mut row = [0.0f32; ROUTE_ROW_FLOATS];
            row[..3].copy_from_slice(&position);
            row[4..7].copy_from_slice(&target);
            row[UNUSED_ROUTE_SCALAR] = f32::NAN;
            body.extend(row.into_iter().flat_map(f32::to_le_bytes));
        }
        body
    }

    #[test]
    fn voyage_route_uses_independent_position_and_facing_splines_not_scalar_time() {
        let route = VoyageRoute::parse(&route_bytes()).unwrap();
        near(route.position.sample(1.0 / 3.0), [1.0, 0.0, 0.0]);
        near(route.facing.sample(4.0 / 7.0), [10.0, 4.0, 0.0]);
        near(route.position.sample(1.0), [1.0, 2.0, 0.0]);
        near(route.facing.sample(1.0), [13.0, 4.0, 0.0]);
    }

    #[test]
    fn voyage_route_separates_count_from_camera_endpoint_flags() {
        let mut body = route_bytes();
        body[ROUTE_COUNT_OFFSET..ROUTE_COUNT_OFFSET + 4]
            .copy_from_slice(&(3u32 | (1 << 8)).to_le_bytes());
        near(
            VoyageRoute::parse(&body).unwrap().position.sample(1.0),
            [1.0, 2.0, 0.0],
        );
        for flag in [1u32 << 19, 1 << 20] {
            body[ROUTE_COUNT_OFFSET..ROUTE_COUNT_OFFSET + 4]
                .copy_from_slice(&(3 | flag).to_le_bytes());
            assert!(
                VoyageRoute::parse(&body).is_none(),
                "camera endpoints need runtime camera context"
            );
        }
    }

    #[test]
    fn voyage_route_rejects_short_rows_impossible_counts_and_nonfinite_vectors() {
        let valid = route_bytes();
        assert!(VoyageRoute::parse(&valid[..ROUTE_COUNT_OFFSET]).is_none());
        assert!(VoyageRoute::parse(&valid[..valid.len() - 1]).is_none());
        for count in [0u32, 1, u32::MAX] {
            let mut body = valid.clone();
            body[ROUTE_COUNT_OFFSET..ROUTE_COUNT_OFFSET + 4].copy_from_slice(&count.to_le_bytes());
            assert!(VoyageRoute::parse(&body).is_none());
        }
        let mut nonfinite = valid;
        nonfinite[ROUTE_HEADER_BYTES..ROUTE_HEADER_BYTES + 4]
            .copy_from_slice(&f32::INFINITY.to_le_bytes());
        assert!(VoyageRoute::parse(&nonfinite).is_none());
    }

    #[test]
    fn point_list_bounds_count_and_rejects_nonfinite_positions() {
        let mut body = vec![0; CHUNK_HEADER_BYTES];
        body[..4].copy_from_slice(&1u32.to_le_bytes());
        body.extend(
            [2.0f32, 3.0, 4.0, 1.0]
                .into_iter()
                .flat_map(f32::to_le_bytes),
        );
        assert_eq!(PointList::parse(&body).unwrap().0, [[2.0, 3.0, 4.0]]);
        assert!(PointList::parse(&body[..body.len() - 1]).is_none());
        for count in [0u32, 2, u32::MAX] {
            body[..4].copy_from_slice(&count.to_le_bytes());
            assert!(PointList::parse(&body).is_none());
        }
        body[..4].copy_from_slice(&1u32.to_le_bytes());
        body[CHUNK_HEADER_BYTES..CHUNK_HEADER_BYTES + 4].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(PointList::parse(&body).is_none());
    }

    fn chunk(bytes: &mut Vec<u8>, name: [u8; 4], kind: ChunkKind) {
        const CHUNK_SIZE_SHIFT: u32 = 7;
        let mut header = [0; CHUNK_HEADER_BYTES];
        header[..4].copy_from_slice(&name);
        header[4..8].copy_from_slice(&((1u32 << CHUNK_SIZE_SHIFT) | kind as u32).to_le_bytes());
        bytes.extend(header);
    }

    #[test]
    fn voyage_collision_selects_passenger_hull_after_scenery_and_normal_zones_keep_first() {
        let mut dat = Vec::new();
        chunk(&mut dat, *b"main", ChunkKind::Rmp);
        chunk(&mut dat, *b"land", ChunkKind::Mzb);
        chunk(&mut dat, *b"end\0", ChunkKind::Terminate);
        assert_eq!(collision_mzb_index(&dat), Some(1));
        assert!(voyage_layout(&dat).is_none());
        chunk(&mut dat, *b"mode", ChunkKind::Rmp);
        chunk(&mut dat, *b"ship", ChunkKind::Rmp);
        chunk(&mut dat, *b"hull", ChunkKind::Mzb);
        chunk(&mut dat, *b"end\0", ChunkKind::Terminate);
        chunk(&mut dat, *b"end\0", ChunkKind::Terminate);
        assert_eq!(
            voyage_layout(&dat),
            Some(VoyageLayout {
                scenery_mzb: 1,
                ship_mzb: 5
            })
        );
        assert_eq!(collision_mzb_index(&dat), Some(5));
        assert_eq!(collision_mzb_index(&[]), None);
    }
}
