//! Bounding-volume hierarchy over a single [`CameraOccluder`] entity's
//! triangles. Built once (lazily, when the mesh asset is ready) and
//! attached to the entity as a [`CollisionBvh`] component. The camera
//! collision system queries this BVH instead of brute-forcing every
//! triangle every frame.
//!
//! Why we have this: the prior brute-force ray cast walked every
//! triangle of every collision mesh on every frame and tanked FPS in
//! triangle-dense zones (Jeuno, Aht Urhgan). With a median-split BVH
//! the per-frame cost drops from O(N) to roughly O(log N) plus a small
//! constant for the leaf scan.
//!
//! Triangles are baked in **world space** at build time. Zone collision
//! meshes are static — they don't move once spawned — so transforming
//! the ray into mesh-local space per cast would just add cost.
//!
//! Build-once, query-many is a deliberate choice: the build system
//! runs every frame but skips entities that already have a
//! [`CollisionBvh`], so a ~ms-scale BVH build is amortized over the
//! lifetime of the zone.

use bevy::prelude::*;
use ffxi_viewer_core::components::CameraOccluder;

/// Maximum triangles per leaf. Below this we stop subdividing — the
/// per-leaf scan is faster than the AABB tests on smaller groups.
/// 16 was picked by feel; tune with a bench if it ever matters.
const LEAF_THRESHOLD: usize = 16;

/// Per-entity BVH. Inserted by [`build_collision_bvh_system`] once the
/// underlying [`Mesh3d`] asset is loaded; never mutated after build.
#[derive(Component)]
pub struct CollisionBvh {
    nodes: Vec<BvhNode>,
    /// World-space triangles, ordered so each leaf node addresses a
    /// contiguous index range.
    triangles: Vec<[Vec3; 3]>,
}

impl CollisionBvh {
    /// Diagnostic: world-space AABB of the whole BVH (the root node's
    /// bounds). Used by the camera-collision probe so we can see which
    /// BVHs actually live where the geometry is.
    pub fn root_aabb(&self) -> Option<(Vec3, Vec3)> {
        self.nodes.first().map(|n| (n.aabb_min, n.aabb_max))
    }

    /// Diagnostic: total triangle count.
    pub fn tri_count(&self) -> usize {
        self.triangles.len()
    }

    /// Diagnostic: brute-force ray cast over every triangle, ignoring
    /// the BVH structure. Slow (O(N)) — for probe / correctness-check
    /// use only. If the BVH `ray_cast` and this disagree, the BVH
    /// structure or traversal has a bug.
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
    /// Leaf if `right == u32::MAX`. Then `left` is the start index into
    /// `triangles`, and `count` (encoded in the slot below) gives the
    /// length. Interior otherwise: `left` and `right` are child node
    /// indices, `count` unused.
    left: u32,
    right: u32,
    /// For leaves: triangle count. For interior nodes: 0 (unused).
    count: u32,
}

impl CollisionBvh {
    /// Closest forward hit along `dir` (must be unit length for `t` to
    /// equal world distance), bounded by `max_t`. `None` if nothing in
    /// the BVH is closer than `max_t`.
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3, max_t: f32) -> Option<f32> {
        if self.nodes.is_empty() {
            return None;
        }
        // 1.0 / 0.0 = +inf, which is fine for the slab test — branches
        // collapse to "always include" when a ray axis is parallel to a
        // slab. Avoids a per-axis branch in the hot loop.
        let inv_dir = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);

        // 32-deep stack covers any reasonable BVH (2^32 leaves). Local
        // array beats Vec to avoid allocation per cast.
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
                // Leaf — scan its triangles.
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
                // Interior — push both children if there's room.
                if sp + 2 <= stack.len() {
                    stack[sp] = node.left;
                    stack[sp + 1] = node.right;
                    sp += 2;
                }
            }
        }

        (hit_t < max_t).then_some(hit_t)
    }

    fn build(triangles: Vec<[Vec3; 3]>) -> Self {
        if triangles.is_empty() {
            return Self {
                nodes: Vec::new(),
                triangles,
            };
        }

        // Working set: index list we'll permute as we partition. After
        // build, we re-order `triangles` into this index order so each
        // leaf's range is contiguous.
        let mut indices: Vec<u32> = (0..triangles.len() as u32).collect();
        let centroids: Vec<Vec3> = triangles
            .iter()
            .map(|t| (t[0] + t[1] + t[2]) / 3.0)
            .collect();

        let mut nodes: Vec<BvhNode> = Vec::with_capacity(triangles.len() * 2);
        // Reserve root.
        nodes.push(placeholder_node());
        build_recursive(0, &mut nodes, &mut indices, &triangles, &centroids, 0);

        // Reorder triangles to match the partition produced by the build.
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

/// Recursive median-split builder. Operates on `indices[range]` — the
/// slice is mutated in place to reflect the partition.
fn build_recursive(
    node_idx: usize,
    nodes: &mut Vec<BvhNode>,
    indices: &mut [u32],
    triangles: &[[Vec3; 3]],
    centroids: &[Vec3],
    depth: u32,
) {
    // AABB over the triangles in this range.
    let (aabb_min, aabb_max) = compute_aabb(indices, triangles);

    // Leaf: small enough or recursion bounded out.
    if indices.len() <= LEAF_THRESHOLD || depth >= 32 {
        // The `indices` slice we got here is guaranteed contiguous in
        // the parent's working buffer because we always split with
        // `split_at_mut`. The caller embedded the start offset of this
        // slice via the parent's recursion — but we need it here to
        // record the leaf range. Encode it through the addresses: by
        // the time we get here `indices` is a sub-slice of the original
        // `Vec<u32>`, and we don't have direct access to its start
        // offset. Solve by recording start in the closure caller; see
        // [`build_with_offset`].
        // -> Handled by reordering after build (see `build`).
        nodes[node_idx] = BvhNode {
            aabb_min,
            aabb_max,
            // `left` = start offset (set later by the reorder pass —
            // we encode it as the position-within-slice after we
            // finish building. Trick: we track it through `centroids`
            // ordering. Simpler approach below.)
            left: 0, // patched below
            right: u32::MAX,
            count: indices.len() as u32,
        };
        return;
    }

    // Pick split axis = longest extent of the centroid bbox.
    let (cmin, cmax) = compute_centroid_aabb(indices, centroids);
    let extent = cmax - cmin;
    let axis = if extent.x > extent.y && extent.x > extent.z {
        0
    } else if extent.y > extent.z {
        1
    } else {
        2
    };

    // Median split on the chosen axis. `select_nth_unstable_by` is O(N)
    // expected — faster than a full sort.
    let mid = indices.len() / 2;
    indices.select_nth_unstable_by(mid, |&a, &b| {
        let ca = centroids[a as usize][axis];
        let cb = centroids[b as usize][axis];
        ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
    });

    // If centroids all coincide on the axis (degenerate group), force
    // a leaf — avoids infinite recursion when many triangles share a
    // centroid coordinate.
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

    // Reserve children.
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

/// Slab AABB-ray test. Returns true if the ray (origin + t*dir, t in
/// [0, max_t]) hits the box. `inv_dir` is precomputed by the caller
/// to amortize across many node tests.
fn aabb_ray_intersects(mn: Vec3, mx: Vec3, origin: Vec3, inv_dir: Vec3, max_t: f32) -> bool {
    let t1 = (mn - origin) * inv_dir;
    let t2 = (mx - origin) * inv_dir;
    let tmin = t1.min(t2);
    let tmax = t1.max(t2);
    let near = tmin.x.max(tmin.y).max(tmin.z);
    let far = tmax.x.min(tmax.y).min(tmax.z);
    near <= far && far >= 0.0 && near <= max_t
}

/// Möller–Trumbore. Same body as the dedup in `debug_heights.rs` /
/// `camera_collision.rs` — kept local for inlining and to keep this
/// module self-contained.
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

/// Build BVHs for any [`CameraOccluder`] entity that has a loaded
/// mesh and a [`GlobalTransform`] but no [`CollisionBvh`] yet. Runs
/// every frame; cheap when there's nothing to do (the `Without` filter
/// makes it a no-op once every entity is processed).
pub fn build_collision_bvh_system(
    mut commands: Commands,
    query: Query<
        (Entity, &Mesh3d, &GlobalTransform),
        (With<CameraOccluder>, Without<CollisionBvh>),
    >,
    meshes: Res<Assets<Mesh>>,
) {
    for (entity, mesh3d, global) in query.iter() {
        let Some(mesh) = meshes.get(mesh3d.0.id()) else {
            // Asset not loaded yet — try again next frame.
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
            // Mesh has no triangles — skip BVH construction. Without
            // this guard, `build_bvh_with_leaf_offsets` would index
            // `nodes[0]` on an empty node list and panic. An empty
            // occluder mesh can't occlude anything anyway, so a
            // missing BVH component is the correct outcome.
            continue;
        }
        let bvh = build_bvh_with_leaf_offsets(tris);
        info!(
            entity = ?entity,
            triangles = tri_count,
            nodes = bvh.nodes.len(),
            "built collision BVH for CameraOccluder"
        );
        commands.entity(entity).insert(bvh);
    }
}

/// Wrapper around [`CollisionBvh::build`] that also patches each leaf
/// node's `left` field to be the start offset into the reordered
/// `triangles` vector. The recursive builder can't know its position
/// in the global index buffer because it operates on slices, so we
/// fix it up here in a single pass after the structure is known.
fn build_bvh_with_leaf_offsets(triangles: Vec<[Vec3; 3]>) -> CollisionBvh {
    let mut bvh = CollisionBvh::build(triangles);
    // Walk the tree depth-first; whenever we hit a leaf, assign it the
    // running offset and bump the offset by its triangle count. This
    // mirrors exactly the order the build's reorder pass produced.
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
