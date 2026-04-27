use bevy::prelude::*;
use ffxi_viewer_core::components::CameraOccluder;
use ffxi_viewer_core::dat_mzb::{DrawDistance, MzbCollisionGeometry};

const LEAF_THRESHOLD: usize = 16;

#[derive(Resource, Default)]
pub struct ZoneCollisionBvh(pub Option<CollisionBvh>);

#[derive(Component)]
pub struct CollisionBvh {
    nodes: Vec<BvhNode>,

    triangles: Vec<[Vec3; 3]>,
}

impl CollisionBvh {
    pub fn root_aabb(&self) -> Option<(Vec3, Vec3)> {
        self.nodes.first().map(|n| (n.aabb_min, n.aabb_max))
    }

    pub fn tri_count(&self) -> usize {
        self.triangles.len()
    }

    pub fn ray_cast_brute_force(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<f32> {
        let mut hit_t = max_t;
        let mut hit_any = false;
        for tri in &self.triangles {
            if let Some(t) = ray_tri_intersect(origin, dir, tri[0], tri[1], tri[2]) {
                if t < hit_t {
                    hit_t = t;
                    hit_any = true;
                }
            }
        }
        hit_any.then_some(hit_t)
    }
}

struct BvhNode {
    aabb_min: Vec3,
    aabb_max: Vec3,

    left: u32,
    right: u32,

    count: u32,
}

impl CollisionBvh {
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<f32> {
        if self.nodes.is_empty() {
            return None;
        }

        let inv_dir = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);

        let mut stack = [0u32; 64];
        let mut sp = 1usize;
        stack[0] = 0;

        let mut hit_t = max_t;

        while sp > 0 {
            sp -= 1;
            let idx = stack[sp] as usize;
            let node = &self.nodes[idx];
            if !aabb_ray_intersects(node.aabb_min, node.aabb_max, origin, inv_dir, hit_t) {
                continue;
            }
            if node.right == u32::MAX {
                let start = node.left as usize;
                let end = start + node.count as usize;
                for tri in &self.triangles[start..end] {
                    if let Some(t) = ray_tri_intersect(origin, dir, tri[0], tri[1], tri[2]) {
                        if t < hit_t {
                            hit_t = t;
                        }
                    }
                }
            } else {
                if sp + 2 <= stack.len() {
                    stack[sp] = node.left;
                    stack[sp + 1] = node.right;
                    sp += 2;
                }
            }
        }

        (hit_t < max_t).then_some(hit_t)
    }

    pub fn from_world_triangles(triangles: Vec<[Vec3; 3]>) -> Self {
        build_bvh_with_leaf_offsets(triangles)
    }

    fn build(triangles: Vec<[Vec3; 3]>) -> Self {
        if triangles.is_empty() {
            return Self {
                nodes: Vec::new(),
                triangles,
            };
        }

        let mut indices: Vec<u32> = (0..triangles.len() as u32).collect();
        let centroids: Vec<Vec3> = triangles
            .iter()
            .map(|t| (t[0] + t[1] + t[2]) / 3.0)
            .collect();

        let mut nodes: Vec<BvhNode> = Vec::with_capacity(triangles.len() * 2);

        nodes.push(placeholder_node());
        build_recursive(0, &mut nodes, &mut indices, &triangles, &centroids, 0);

        let reordered: Vec<[Vec3; 3]> = indices.iter().map(|&i| triangles[i as usize]).collect();

        Self {
            nodes,
            triangles: reordered,
        }
    }
}

fn placeholder_node() -> BvhNode {
    BvhNode {
        aabb_min: Vec3::ZERO,
        aabb_max: Vec3::ZERO,
        left: 0,
        right: 0,
        count: 0,
    }
}

fn build_recursive(
    node_idx: usize,
    nodes: &mut Vec<BvhNode>,
    indices: &mut [u32],
    triangles: &[[Vec3; 3]],
    centroids: &[Vec3],
    depth: u32,
) {
    let (aabb_min, aabb_max) = compute_aabb(indices, triangles);

    if indices.len() <= LEAF_THRESHOLD || depth >= 32 {
        nodes[node_idx] = BvhNode {
            aabb_min,
            aabb_max,

            left: 0,
            right: u32::MAX,
            count: indices.len() as u32,
        };
        return;
    }

    let (cmin, cmax) = compute_centroid_aabb(indices, centroids);
    let extent = cmax - cmin;
    let axis = if extent.x > extent.y && extent.x > extent.z {
        0
    } else if extent.y > extent.z {
        1
    } else {
        2
    };

    let mid = indices.len() / 2;
    indices.select_nth_unstable_by(mid, |&a, &b| {
        let ca = centroids[a as usize][axis];
        let cb = centroids[b as usize][axis];
        ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
    });

    if extent[axis] < 1e-6 {
        nodes[node_idx] = BvhNode {
            aabb_min,
            aabb_max,
            left: 0,
            right: u32::MAX,
            count: indices.len() as u32,
        };
        return;
    }

    let (left_indices, right_indices) = indices.split_at_mut(mid);

    let left_node_idx = nodes.len();
    nodes.push(placeholder_node());
    let right_node_idx = nodes.len();
    nodes.push(placeholder_node());

    build_recursive(
        left_node_idx,
        nodes,
        left_indices,
        triangles,
        centroids,
        depth + 1,
    );
    build_recursive(
        right_node_idx,
        nodes,
        right_indices,
        triangles,
        centroids,
        depth + 1,
    );

    nodes[node_idx] = BvhNode {
        aabb_min,
        aabb_max,
        left: left_node_idx as u32,
        right: right_node_idx as u32,
        count: 0,
    };
}

fn compute_aabb(indices: &[u32], triangles: &[[Vec3; 3]]) -> (Vec3, Vec3) {
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for &i in indices {
        let tri = &triangles[i as usize];
        for v in tri {
            mn = mn.min(*v);
            mx = mx.max(*v);
        }
    }
    (mn, mx)
}

fn compute_centroid_aabb(indices: &[u32], centroids: &[Vec3]) -> (Vec3, Vec3) {
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for &i in indices {
        let c = centroids[i as usize];
        mn = mn.min(c);
        mx = mx.max(c);
    }
    (mn, mx)
}

fn aabb_ray_intersects(mn: Vec3, mx: Vec3, origin: Vec3, inv_dir: Vec3, max_t: f32) -> bool {
    let t1 = (mn - origin) * inv_dir;
    let t2 = (mx - origin) * inv_dir;
    let tmin = t1.min(t2);
    let tmax = t1.max(t2);
    let near = tmin.x.max(tmin.y).max(tmin.z);
    let far = tmax.x.min(tmax.y).min(tmax.z);
    near <= far && far >= 0.0 && near <= max_t
}

fn ray_tri_intersect(orig: Vec3, dir: Vec3, v0: Vec3, v1: Vec3, v2: Vec3) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let e1 = v1 - v0;
    let e2 = v2 - v0;
    let h = dir.cross(e2);
    let a = e1.dot(h);
    if a.abs() < EPS {
        return None;
    }
    let f = 1.0 / a;
    let s = orig - v0;
    let u = f * s.dot(h);
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = f * dir.dot(q);
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = f * e2.dot(q);
    if t > EPS {
        Some(t)
    } else {
        None
    }
}

pub fn build_collision_bvh_system(
    mut commands: Commands,
    draw: Res<DrawDistance>,
    query: Query<
        (Entity, &Mesh3d, &GlobalTransform),
        (With<CameraOccluder>, Without<CollisionBvh>),
    >,
    meshes: Res<Assets<Mesh>>,
) {
    if !draw.camera_collision_source.uses_mmb() {
        return;
    }
    for (entity, mesh3d, global) in query.iter() {
        let Some(mesh) = meshes.get(mesh3d.0.id()) else {
            continue;
        };
        let Some(positions) = mesh
            .attribute(Mesh::ATTRIBUTE_POSITION)
            .and_then(|a| a.as_float3())
        else {
            continue;
        };
        let Some(indices) = mesh.indices() else {
            continue;
        };

        let xform = global.to_matrix();
        let mut tris: Vec<[Vec3; 3]> = Vec::with_capacity(indices.len() / 3);
        let mut iter = indices.iter();
        while let (Some(i0), Some(i1), Some(i2)) = (iter.next(), iter.next(), iter.next()) {
            let v0 = xform.transform_point3(Vec3::from_array(positions[i0]));
            let v1 = xform.transform_point3(Vec3::from_array(positions[i1]));
            let v2 = xform.transform_point3(Vec3::from_array(positions[i2]));
            tris.push([v0, v1, v2]);
        }

        let tri_count = tris.len();
        if tri_count == 0 {
            continue;
        }
        let bvh = build_bvh_with_leaf_offsets(tris);
        debug!(
            entity = ?entity,
            triangles = tri_count,
            nodes = bvh.nodes.len(),
            "built collision BVH for CameraOccluder"
        );
        commands.entity(entity).insert(bvh);
    }
}

pub fn build_zone_collision_bvh_system(
    geom: Res<MzbCollisionGeometry>,
    mut zone_bvh: ResMut<ZoneCollisionBvh>,
) {
    if !geom.is_changed() {
        return;
    }
    if geom.indices.is_empty() {
        zone_bvh.0 = None;
        return;
    }
    let mut tris: Vec<[Vec3; 3]> = Vec::with_capacity(geom.indices.len() / 3);
    for tri in geom.indices.chunks_exact(3) {
        tris.push([
            geom.positions[tri[0] as usize],
            geom.positions[tri[1] as usize],
            geom.positions[tri[2] as usize],
        ]);
    }
    let tri_count = tris.len();
    let bvh = CollisionBvh::from_world_triangles(tris);
    debug!(
        triangles = tri_count,
        nodes = bvh.nodes.len(),
        "built zone-level MZB collision BVH"
    );
    zone_bvh.0 = Some(bvh);
}

fn build_bvh_with_leaf_offsets(triangles: Vec<[Vec3; 3]>) -> CollisionBvh {
    let mut bvh = CollisionBvh::build(triangles);

    let mut offset: u32 = 0;
    patch_leaf_offsets(&mut bvh.nodes, 0, &mut offset);
    bvh
}

fn patch_leaf_offsets(nodes: &mut [BvhNode], idx: usize, offset: &mut u32) {
    let (right, count) = (nodes[idx].right, nodes[idx].count);
    if right == u32::MAX {
        nodes[idx].left = *offset;
        *offset += count;
    } else {
        let l = nodes[idx].left as usize;
        let r = right as usize;
        patch_leaf_offsets(nodes, l, offset);
        patch_leaf_offsets(nodes, r, offset);
    }
}
