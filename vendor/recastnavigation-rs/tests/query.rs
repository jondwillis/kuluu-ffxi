#![cfg(feature = "rkyv")]

use recastnavigation_rs::demo::*;
use recastnavigation_rs::detour::*;
use recastnavigation_rs::detour_crowd::*;
use recastnavigation_rs::XError;
use rkyv::{Archive, Deserialize, Serialize};

mod common;
use common::*;

const POLY_PICK_EXT: [f32; 3] = [2.0, 4.0, 2.0];
const STEP_SIZE: f32 = 0.5;
const SLOP: f32 = 0.01;

const MAX_POLYS: usize = 256;
const MAX_SMOOTH: usize = 768;

const DUNGEON_POINTS: [([f32; 3], [f32; 3]); 15] = [
    ([37.81, 10.00, -7.43], [42.07, 10.00, 7.89]),
    ([48.20, 10.00, -1.75], [3.82, 10.00, -7.82]),
    ([36.90, 10.00, 7.47], [12.02, 9.99, -20.05]),
    ([12.45, 10.00, 3.22], [-2.77, -0.00, 3.25]),
    ([0.24, 10.00, -14.15], [-0.78, 10.00, -24.71]),
    ([-15.50, 0.86, 2.55], [12.47, 10.00, 2.71]),
    ([9.77, 10.00, -20.07], [12.47, 10.00, 2.71]),
    ([-2.16, 10.00, -26.11], [18.85, 19.63, -80.35]),
    ([47.69, 10.00, 3.47], [21.00, 14.95, -88.66]),
    ([24.53, 15.48, -79.02], [12.89, 24.75, -82.77]),
    ([23.05, 24.24, -84.66], [12.89, 24.75, -82.77]),
    ([-3.34, -0.00, 3.90], [-7.99, -0.00, 1.48]),
    ([24.08, 15.46, -83.25], [-4.17, 10.00, -27.01]),
    ([24.08, 15.46, -83.25], [45.09, 10.00, 6.15]),
    ([26.55, 15.85, -82.11], [-7.50, -0.00, 3.96]),
];

const NAV_TEST_POINTS: [([f32; 3], [f32; 3]); 15] = [
    ([-12.60, 7.93, 3.36], [-22.24, 7.93, 4.78]),
    ([-3.41, 8.24, -8.21], [-0.81, 15.24, 10.78]),
    ([-1.73, 15.24, 12.26], [-20.17, 7.93, 5.35]),
    ([-27.23, -2.37, -18.16], [-18.68, -2.37, 28.57]),
    ([9.29, 10.02, 16.58], [6.59, 9.53, 17.68]),
    ([-26.75, -2.37, -16.65], [19.47, -2.37, -7.85]),
    ([10.93, -2.37, 25.63], [13.55, -2.37, -24.04]),
    ([22.17, -2.37, 22.43], [-16.99, -2.37, -14.67]),
    ([28.23, -0.37, -13.76], [41.08, -0.43, -29.71]),
    ([52.35, -3.73, -42.95], [18.15, -2.37, -22.43]),
    ([60.51, 0.96, 19.20], [29.61, -2.37, -3.50]),
    ([41.73, 13.51, 6.08], [42.75, 13.51, 12.69]),
    ([13.52, 10.26, 17.73], [42.26, 13.51, 8.60]),
    ([38.54, -0.74, 22.11], [46.86, 5.98, 9.61]),
    ([-18.57, 7.93, 4.55], [43.95, 7.80, 20.48]),
];

const UNDULATING_POINTS: [([f32; 3], [f32; 3]); 10] = [
    ([5080.56, 1.99, 5054.29], [5094.18, -0.53, 5094.80]),
    ([5074.76, 0.32, 5090.94], [5059.79, 2.66, 5075.49]),
    ([5005.16, 3.62, 5050.62], [5078.16, 1.60, 5097.15]),
    ([5063.58, -0.31, 5046.30], [5063.58, -0.31, 5046.30]),
    ([5041.36, -2.57, 5052.66], [5046.02, 3.37, 5063.29]),
    ([5005.34, 3.16, 5002.29], [5069.44, -3.76, 5024.77]),
    ([5061.66, 0.72, 5053.60], [5026.51, 0.89, 5078.89]),
    ([5083.74, 3.21, 5001.02], [5038.33, -0.57, 5001.72]),
    ([5091.20, 0.57, 5068.43], [5033.56, -2.64, 5090.44]),
    ([5084.40, 2.44, 5047.61], [5032.15, 1.58, 5029.68]),
];

macro_rules! test {
    ($func:ident, $folder:expr, $name:expr, $points:expr) => {
        let bin_path = format!("./expected/{}/{}_cpp_out.bin", $folder, $name);
        let nav_mesh = load_nav_mesh(&bin_path).unwrap();
        let mut all_data = vec![];
        for (start, end) in &$points {
            all_data.push($func(&nav_mesh, *start, *end));
        }
        compare_with_rkyv($folder, &format!("{}_{}", $name, stringify!($func)), &all_data).unwrap();
    };
}

#[test]
fn test_path_find_straight() {
    test!(path_find_straight, "solo_mesh", "dungeon", DUNGEON_POINTS);
    test!(path_find_straight, "solo_mesh", "nav_test", NAV_TEST_POINTS);
    test!(path_find_straight, "solo_mesh", "undulating", UNDULATING_POINTS);

    test!(path_find_straight, "tile_mesh", "dungeon", DUNGEON_POINTS);
    test!(path_find_straight, "tile_mesh", "nav_test", NAV_TEST_POINTS);
    test!(path_find_straight, "tile_mesh", "undulating", UNDULATING_POINTS);
}

#[derive(Debug, Default, PartialEq, Archive, Serialize, Deserialize)]
struct PathFindStaightData {
    start: [f32; 3],
    end: [f32; 3],
    polys: Vec<DtPolyRef>,
    straight_path: Vec<[f32; 3]>,
    straight_path_flags: Vec<u8>,
    straight_path_refs: Vec<DtPolyRef>,
}

fn path_find_straight(nav_mesh: &DtNavMesh, start: [f32; 3], end: [f32; 3]) -> PathFindStaightData {
    let query = DtNavMeshQuery::with_mesh(&nav_mesh, 2048).unwrap();
    let filter = DtQueryFilter::default();

    let mut data = PathFindStaightData::default();
    data.start = start;
    data.end = end;
    let (start_ref, _) = query.find_nearest_poly_1(&start, &POLY_PICK_EXT, &filter).unwrap();
    let (end_ref, _) = query.find_nearest_poly_1(&end, &POLY_PICK_EXT, &filter).unwrap();

    let mut polys = [DtPolyRef::default(); MAX_POLYS];
    let npolys = query
        .find_path(start_ref, end_ref, &start, &end, &filter, &mut polys)
        .unwrap();
    data.polys = polys[0..npolys].to_vec();

    let mut real_end = end;
    if polys[npolys - 1] != end_ref {
        (real_end, _) = query.closest_point_on_poly(polys[npolys - 1], &end).unwrap();
    }

    let mut straight_path = vec![[0.0; 3]; MAX_POLYS];
    let mut straight_path_flags = vec![0; MAX_POLYS];
    let mut straight_path_refs = vec![DtPolyRef::default(); MAX_POLYS];
    let straight_size = query
        .find_straight_path(
            &start,
            &real_end,
            &polys[0..npolys],
            &mut straight_path,
            Some(&mut straight_path_flags),
            Some(&mut straight_path_refs),
            0,
        )
        .unwrap();
    data.straight_path = straight_path[0..straight_size].to_vec();
    data.straight_path_flags = straight_path_flags[0..straight_size].to_vec();
    data.straight_path_refs = straight_path_refs[0..straight_size].to_vec();
    return data;
}

#[test]
fn test_path_find_sliced() {
    test!(path_find_sliced, "solo_mesh", "dungeon", DUNGEON_POINTS);
    test!(path_find_sliced, "solo_mesh", "nav_test", NAV_TEST_POINTS);
    test!(path_find_sliced, "solo_mesh", "undulating", UNDULATING_POINTS);

    test!(path_find_sliced, "tile_mesh", "dungeon", DUNGEON_POINTS);
    test!(path_find_sliced, "tile_mesh", "nav_test", NAV_TEST_POINTS);
    test!(path_find_sliced, "tile_mesh", "undulating", UNDULATING_POINTS);
}

#[derive(Debug, Default, PartialEq, Archive, Serialize, Deserialize)]
struct PathFindSlicedData {
    start: [f32; 3],
    end: [f32; 3],
    polys: Vec<DtPolyRef>,
    straight_path: Vec<[f32; 3]>,
    straight_path_flags: Vec<u8>,
    straight_path_refs: Vec<DtPolyRef>,
}

fn path_find_sliced(nav_mesh: &DtNavMesh, start: [f32; 3], end: [f32; 3]) -> PathFindSlicedData {
    let mut query = DtNavMeshQuery::with_mesh(&nav_mesh, 2048).unwrap();
    let filter = DtQueryFilter::default();

    let mut data = PathFindSlicedData::default();
    data.start = start;
    data.end = end;
    let (start_ref, _) = query.find_nearest_poly_1(&start, &POLY_PICK_EXT, &filter).unwrap();
    let (end_ref, _) = query.find_nearest_poly_1(&end, &POLY_PICK_EXT, &filter).unwrap();

    let mut polys = [DtPolyRef::default(); MAX_POLYS];
    let npolys;
    let mut result = query.init_sliced_find_path(start_ref, end_ref, &start, &end, &filter, true);
    loop {
        match &result {
            Err(XError::InProgress) => {
                result = query.update_sliced_find_path(1).map(|_| ());
            }
            Ok(_) => {
                npolys = query.finalize_sliced_find_path(&mut polys).unwrap();
                break;
            }
            err @ _ => err.unwrap(),
        }
    }
    let mut real_end = end;
    if polys[npolys - 1] != end_ref {
        (real_end, _) = query.closest_point_on_poly(polys[npolys - 1], &end).unwrap();
    }
    data.polys = polys[0..npolys].to_vec();

    let mut straight_path = vec![[0.0; 3]; MAX_POLYS];
    let mut straight_path_flags = vec![0; MAX_POLYS];
    let mut straight_path_refs = vec![DtPolyRef::default(); MAX_POLYS];
    let straight_size = query
        .find_straight_path(
            &start,
            &real_end,
            &polys[0..npolys],
            &mut straight_path,
            Some(&mut straight_path_flags),
            Some(&mut straight_path_refs),
            DT_STRAIGHTPATH_ALL_CROSSINGS,
        )
        .unwrap();
    data.straight_path = straight_path[0..straight_size].to_vec();
    data.straight_path_flags = straight_path_flags[0..straight_size].to_vec();
    data.straight_path_refs = straight_path_refs[0..straight_size].to_vec();
    return data;
}

#[test]
fn test_raycast() {
    test!(raycast, "solo_mesh", "dungeon", DUNGEON_POINTS);
    test!(raycast, "solo_mesh", "nav_test", NAV_TEST_POINTS);
    test!(raycast, "solo_mesh", "undulating", UNDULATING_POINTS);

    test!(raycast, "tile_mesh", "dungeon", DUNGEON_POINTS);
    test!(raycast, "tile_mesh", "nav_test", NAV_TEST_POINTS);
    test!(raycast, "tile_mesh", "undulating", UNDULATING_POINTS);
}

#[derive(Debug, Default, PartialEq, Archive, Serialize, Deserialize)]
struct RaycastData {
    start: [f32; 3],
    end: [f32; 3],
    hit_t: f32,
    hit_normal: [f32; 3],
    hit_path: Vec<DtPolyRef>,
    hit_pos: [f32; 3],
    hit_res: bool,
}

fn raycast(nav_mesh: &DtNavMesh, start: [f32; 3], end: [f32; 3]) -> RaycastData {
    let query = DtNavMeshQuery::with_mesh(&nav_mesh, 2048).unwrap();
    let filter = DtQueryFilter::default();

    let mut data = RaycastData::default();
    data.start = start;
    data.end = end;
    let (start_ref, _) = query.find_nearest_poly_1(&start, &POLY_PICK_EXT, &filter).unwrap();

    let mut path = vec![DtPolyRef::default(); MAX_POLYS];
    let (t, hit_normal, count) = query
        .raycast_1(start_ref, &start, &end, &filter, Some(&mut path))
        .unwrap();
    data.hit_t = t;
    data.hit_normal = hit_normal;
    data.hit_path = path[0..count].to_vec();

    let mut hit_pos: [f32; 3];
    let hit_res: bool;
    if t > 1.0 {
        hit_pos = end;
        hit_res = false;
    } else {
        hit_pos = dt_vlerp(&start, &end, t);
        hit_res = true;
    }
    if count > 0 {
        if let Ok(height) = query.get_poly_height(path[count - 1], &hit_pos) {
            hit_pos[1] = height;
        }
    }
    data.hit_pos = hit_pos;
    data.hit_res = hit_res;
    return data;
}

#[test]
fn test_distance_to_wall() {
    test!(distance_to_wall, "solo_mesh", "dungeon", DUNGEON_POINTS);
    test!(distance_to_wall, "solo_mesh", "nav_test", NAV_TEST_POINTS);
    test!(distance_to_wall, "solo_mesh", "undulating", UNDULATING_POINTS);

    test!(distance_to_wall, "tile_mesh", "dungeon", DUNGEON_POINTS);
    test!(distance_to_wall, "tile_mesh", "nav_test", NAV_TEST_POINTS);
    test!(distance_to_wall, "tile_mesh", "undulating", UNDULATING_POINTS);
}

#[derive(Debug, Default, PartialEq, Archive, Serialize, Deserialize)]
struct DistanceToWallData {
    point: [f32; 3],
    hit_distance: f32,
    hit_pos: [f32; 3],
    hit_normal: [f32; 3],
}

fn distance_to_wall(nav_mesh: &DtNavMesh, point: [f32; 3], _: [f32; 3]) -> DistanceToWallData {
    let query = DtNavMeshQuery::with_mesh(&nav_mesh, 2048).unwrap();
    let filter = DtQueryFilter::default();

    let mut data = DistanceToWallData::default();
    data.point = point;
    let (point_ref, _) = query.find_nearest_poly_1(&point, &POLY_PICK_EXT, &filter).unwrap();

    let (hit_distance, hit_pos, hit_normal) = query.find_distance_to_wall(point_ref, &point, 100.0, &filter).unwrap();
    data.hit_distance = hit_distance;
    data.hit_pos = hit_pos;
    data.hit_normal = hit_normal;
    return data;
}

#[test]
fn test_find_polys_in_circle() {
    test!(find_polys_in_circle, "solo_mesh", "dungeon", DUNGEON_POINTS);
    test!(find_polys_in_circle, "solo_mesh", "nav_test", NAV_TEST_POINTS);
    test!(find_polys_in_circle, "solo_mesh", "undulating", UNDULATING_POINTS);

    test!(find_polys_in_circle, "tile_mesh", "dungeon", DUNGEON_POINTS);
    test!(find_polys_in_circle, "tile_mesh", "nav_test", NAV_TEST_POINTS);
    test!(find_polys_in_circle, "tile_mesh", "undulating", UNDULATING_POINTS);
}

#[derive(Debug, Default, PartialEq, Archive, Serialize, Deserialize)]
struct FindPolysInCircleData {
    start: [f32; 3],
    end: [f32; 3],
    polys: Vec<DtPolyRef>,
    parents: Vec<DtPolyRef>,
    costs: Vec<f32>,
}

fn find_polys_in_circle(nav_mesh: &DtNavMesh, start: [f32; 3], end: [f32; 3]) -> FindPolysInCircleData {
    let query = DtNavMeshQuery::with_mesh(&nav_mesh, 2048).unwrap();
    let filter = DtQueryFilter::default();

    let mut data = FindPolysInCircleData::default();
    data.start = start;
    data.end = end;
    let (start_ref, _) = query.find_nearest_poly_1(&start, &POLY_PICK_EXT, &filter).unwrap();

    let dx = end[0] - start[0];
    let dz = end[2] - start[2];
    let distance = (dx * dx + dz * dz).sqrt();

    let mut polys = vec![DtPolyRef::default(); MAX_POLYS];
    let mut parents = vec![DtPolyRef::default(); MAX_POLYS];
    let mut costs = vec![0.0; MAX_POLYS];
    let count = query
        .find_polys_around_circle(
            start_ref,
            &start,
            distance,
            &filter,
            Some(&mut polys),
            Some(&mut parents),
            Some(&mut costs),
        )
        .unwrap();
    data.polys = polys[0..count].to_vec();
    data.parents = parents[0..count].to_vec();
    data.costs = costs[0..count].to_vec();
    return data;
}

#[test]
fn test_find_polys_in_shape() {
    test!(find_polys_in_shape, "solo_mesh", "dungeon", DUNGEON_POINTS);
    test!(find_polys_in_shape, "solo_mesh", "nav_test", NAV_TEST_POINTS);
    test!(find_polys_in_shape, "solo_mesh", "undulating", UNDULATING_POINTS);

    test!(find_polys_in_shape, "tile_mesh", "dungeon", DUNGEON_POINTS);
    test!(find_polys_in_shape, "tile_mesh", "nav_test", NAV_TEST_POINTS);
    test!(find_polys_in_shape, "tile_mesh", "undulating", UNDULATING_POINTS);
}

#[derive(Debug, Default, PartialEq, Archive, Serialize, Deserialize)]
struct FindPolysInShapeData {
    start: [f32; 3],
    end: [f32; 3],
    query_poly: Vec<[f32; 3]>,
    polys: Vec<DtPolyRef>,
    parents: Vec<DtPolyRef>,
    costs: Vec<f32>,
}

fn find_polys_in_shape(nav_mesh: &DtNavMesh, start: [f32; 3], end: [f32; 3]) -> FindPolysInShapeData {
    const ANGLE_HEIGHT: f32 = 2.0;

    let query = DtNavMeshQuery::with_mesh(&nav_mesh, 2048).unwrap();
    let filter = DtQueryFilter::default();

    let mut data = FindPolysInShapeData::default();
    data.start = start;
    data.end = end;
    let (start_ref, _) = query.find_nearest_poly_1(&start, &POLY_PICK_EXT, &filter).unwrap();

    let nx = (end[2] - start[2]) * 0.25;
    let nz = -(end[0] - start[0]) * 0.25;
    let query_poly = [
        [start[0] + nx * 1.2, start[1] + ANGLE_HEIGHT / 2.0, start[2] + nz * 1.2],
        [start[0] - nx * 1.3, start[1] + ANGLE_HEIGHT / 2.0, start[2] - nz * 1.3],
        [end[0] - nx * 0.8, end[1] + ANGLE_HEIGHT / 2.0, end[2] - nz * 0.8],
        [end[0] + nx, end[1] + ANGLE_HEIGHT / 2.0, end[2] + nz],
    ];
    data.query_poly = query_poly.to_vec();

    let mut polys = vec![DtPolyRef::default(); MAX_POLYS];
    let mut parents = vec![DtPolyRef::default(); MAX_POLYS];
    let mut costs = vec![0.0; MAX_POLYS];
    let count = query
        .find_polys_around_shape(
            start_ref,
            &query_poly,
            &filter,
            Some(&mut polys),
            Some(&mut parents),
            Some(&mut costs),
        )
        .unwrap();
    data.polys = polys[0..count].to_vec();
    data.parents = parents[0..count].to_vec();
    data.costs = costs[0..count].to_vec();
    return data;
}

#[test]
fn test_find_local_neighbourhood() {
    test!(find_local_neighbourhood, "solo_mesh", "dungeon", DUNGEON_POINTS);
    test!(find_local_neighbourhood, "solo_mesh", "nav_test", NAV_TEST_POINTS);
    test!(find_local_neighbourhood, "solo_mesh", "undulating", UNDULATING_POINTS);

    test!(find_local_neighbourhood, "tile_mesh", "dungeon", DUNGEON_POINTS);
    test!(find_local_neighbourhood, "tile_mesh", "nav_test", NAV_TEST_POINTS);
    test!(find_local_neighbourhood, "tile_mesh", "undulating", UNDULATING_POINTS);
}

#[derive(Debug, Default, PartialEq, Archive, Serialize, Deserialize)]
struct FindLocalNeighbourhoodData {
    point: [f32; 3],
    query_poly: Vec<[f32; 3]>,
    polys: Vec<DtPolyRef>,
    parents: Vec<DtPolyRef>,
}

fn find_local_neighbourhood(nav_mesh: &DtNavMesh, point: [f32; 3], _: [f32; 3]) -> FindLocalNeighbourhoodData {
    const RADIUS: f32 = 12.0;

    let query = DtNavMeshQuery::with_mesh(&nav_mesh, 2048).unwrap();
    let filter = DtQueryFilter::default();

    let mut data = FindLocalNeighbourhoodData::default();
    data.point = point;
    let (point_ref, _) = query.find_nearest_poly_1(&point, &POLY_PICK_EXT, &filter).unwrap();

    let mut polys = vec![DtPolyRef::default(); MAX_POLYS];
    let mut parents = vec![DtPolyRef::default(); MAX_POLYS];
    let count = query
        .find_local_neighbourhood(point_ref, &point, RADIUS, &filter, &mut polys, Some(&mut parents))
        .unwrap();
    data.polys = polys[0..count].to_vec();
    data.parents = parents[0..count].to_vec();
    return data;
}

#[test]
fn test_deterministic_path_find_follow() {
    test!(path_find_follow, "solo_mesh", "dungeon", DUNGEON_POINTS);
    test!(path_find_follow, "solo_mesh", "nav_test", NAV_TEST_POINTS);
    test!(path_find_follow, "solo_mesh", "undulating", UNDULATING_POINTS);

    test!(path_find_follow, "tile_mesh", "dungeon", DUNGEON_POINTS);
    test!(path_find_follow, "tile_mesh", "nav_test", NAV_TEST_POINTS);
    test!(path_find_follow, "tile_mesh", "undulating", UNDULATING_POINTS);
}

#[derive(Debug, Default, PartialEq, Archive, Serialize, Deserialize)]
struct PathFindFollowData {
    start: [f32; 3],
    end: [f32; 3],
    polys: Vec<DtPolyRef>,
    smooth_path: Vec<[f32; 3]>,
    // temp results
    get_steer_target: Vec<SteerTargetOut>,
    move_along_surface: Vec<([f32; 3], usize)>,
    fixup_shortcuts: Vec<usize>,
}

fn path_find_follow(nav_mesh: &DtNavMesh, start: [f32; 3], end: [f32; 3]) -> PathFindFollowData {
    let mut data = PathFindFollowData::default();
    data.start = start;
    data.end = end;

    let query = DtNavMeshQuery::with_mesh(&nav_mesh, 2048).unwrap();
    let filter = DtQueryFilter::default();

    let (start_ref, _) = query.find_nearest_poly_1(&start, &POLY_PICK_EXT, &filter).unwrap();
    let (end_ref, _) = query.find_nearest_poly_1(&end, &POLY_PICK_EXT, &filter).unwrap();

    let mut polys = [DtPolyRef::default(); MAX_POLYS];
    let mut npolys = query
        .find_path(start_ref, end_ref, &start, &end, &filter, &mut polys)
        .unwrap();
    data.polys = polys[0..npolys].to_vec();

    if npolys <= 0 {
        return data;
    }

    let (mut iter_pos, _) = query.closest_point_on_poly(start_ref, &start).unwrap();
    let (target_pos, _) = query.closest_point_on_poly(polys[npolys - 1], &end).unwrap();

    let mut smooth_path = [[0.0; 3]; MAX_SMOOTH];
    let mut smooth_count = 0;
    smooth_path[smooth_count] = iter_pos;
    smooth_count += 1;

    while npolys > 0 && smooth_count < MAX_SMOOTH {
        let steer = match get_steer_target(&query, iter_pos, target_pos, SLOP, &mut polys[0..npolys]) {
            Some(steer) => steer,
            None => break,
        };
        data.get_steer_target.push(steer.clone());

        let end_of_path = steer.steer_pos_flag & DT_STRAIGHTPATH_END != 0;
        let off_mesh_connection = steer.steer_pos_flag & DT_STRAIGHTPATH_OFFMESH_CONNECTION != 0;

        let delta = dt_vsub(&steer.steer_pos, &iter_pos);
        let mut len = dt_vlen(&delta);
        if (end_of_path || off_mesh_connection) && (len < STEP_SIZE) {
            len = 1.0;
        } else {
            len = STEP_SIZE / len;
        }
        let move_tgt = dt_vmad(&iter_pos, &delta, len);

        let mut visited = [DtPolyRef::default(); 16];
        let (mut result, nvisited) = query
            .move_along_surface(polys[0], &iter_pos, &move_tgt, &filter, &mut visited)
            .unwrap();
        data.move_along_surface.push((result, nvisited));

        npolys = merge_corridor_start_moved(&mut polys, npolys, &visited[0..nvisited]);
        npolys = fixup_shortcuts(&nav_mesh, &mut polys[0..npolys]);
        data.fixup_shortcuts.push(npolys);

        result[1] = query.get_poly_height(polys[0], &result).unwrap_or(result[1]);
        iter_pos = result;

        if end_of_path && in_range(&iter_pos, &steer.steer_pos, SLOP, 1.0) {
            iter_pos = target_pos;
            if smooth_count < MAX_SMOOTH {
                smooth_path[smooth_count] = iter_pos;
                smooth_count += 1;
            }
            break;
        } else if off_mesh_connection && in_range(&iter_pos, &steer.steer_pos, SLOP, 1.0) {
            let mut npos = 0;
            let mut prev_ref = DtPolyRef::default();
            let mut poly_ref = polys[0];
            while npos < npolys && poly_ref != steer.steer_pos_ref {
                prev_ref = poly_ref;
                poly_ref = polys[npos];
                npos += 1;
            }
            for i in npos..npolys {
                polys[i - npos] = polys[i];
            }
            npolys -= npos;

            if let Ok((start_pos, end_pos)) = nav_mesh.get_off_mesh_connection_poly_end_points(prev_ref, poly_ref) {
                if smooth_count < MAX_SMOOTH {
                    smooth_path[smooth_count] = start_pos;
                    smooth_count += 1;
                    if smooth_count & 1 != 0 {
                        smooth_path[smooth_count] = start_pos;
                        smooth_count += 1;
                    }
                }
                iter_pos = end_pos;
                iter_pos[1] = query.get_poly_height(polys[0], &iter_pos).unwrap_or(iter_pos[1]);
            }
        }

        if smooth_count < MAX_SMOOTH {
            smooth_path[smooth_count] = iter_pos;
            smooth_count += 1;
        }
    }

    data.smooth_path = smooth_path[0..smooth_count].to_vec();
    return data;
}

#[derive(Debug, Default, Clone, PartialEq, Archive, Serialize, Deserialize)]
struct SteerTargetOut {
    points: Vec<[f32; 3]>,
    steer_pos: [f32; 3],
    steer_pos_flag: DtStraightPathFlags,
    steer_pos_ref: DtPolyRef,
}

fn get_steer_target(
    query: &DtNavMeshQuery,
    start: [f32; 3],
    end: [f32; 3],
    min_target_dist: f32,
    path: &[DtPolyRef],
) -> Option<SteerTargetOut> {
    const MAX_STEER_POINTS: usize = 3;

    let mut out = SteerTargetOut::default();
    let mut steer_path = [[0.0; 3]; MAX_STEER_POINTS];
    let mut steer_path_flags = [0; MAX_STEER_POINTS];
    let mut steer_path_polys = [DtPolyRef::default(); MAX_STEER_POINTS];

    let steer_count = query
        .find_straight_path(
            &start,
            &end,
            path,
            &mut steer_path,
            Some(&mut steer_path_flags),
            Some(&mut steer_path_polys),
            0,
        )
        .ok()?;
    if steer_count == 0 {
        return None;
    }
    out.points = steer_path[0..steer_count].to_vec();

    let mut ns = 0;
    while ns < steer_count {
        if (steer_path_flags[ns] & DT_STRAIGHTPATH_OFFMESH_CONNECTION != 0)
            || !in_range(&steer_path[ns], &start, min_target_dist, 1000.0)
        {
            break;
        }
        ns += 1;
    }
    if ns > steer_count {
        return None;
    }

    out.steer_pos = steer_path[ns];
    out.steer_pos[1] = start[1];
    out.steer_pos_flag = steer_path_flags[ns];
    out.steer_pos_ref = steer_path_polys[ns];

    return Some(out);
}

fn in_range(v1: &[f32; 3], v2: &[f32; 3], r: f32, h: f32) -> bool {
    let dx = v2[0] - v1[0];
    let dy = v2[1] - v1[1];
    let dz = v2[2] - v1[2];
    return (dx * dx + dz * dz) < r * r && dy.abs() < h;
}

fn fixup_shortcuts(nav_mesh: &DtNavMesh, path: &mut [DtPolyRef]) -> usize {
    const MAX_NEIS: usize = 16;
    const MAX_LOOK_AHEAD: usize = 6;

    if path.len() < 3 {
        return path.len();
    }

    let mut neis = [DtPolyRef::default(); MAX_NEIS];
    let mut nneis = 0;

    let (tile, poly) = match nav_mesh.get_tile_and_poly_by_ref(path[0]) {
        Ok(res) => res,
        Err(_) => return path.len(),
    };

    let mut k = poly.first_link;
    while k != DT_NULL_LINK {
        let link = &tile.links()[k as usize];
        if !link.re.is_null() {
            if nneis < MAX_NEIS {
                neis[nneis] = link.re;
                nneis += 1;
            }
        }
        k = link.next;
    }

    let mut cut = 0;
    let mut i = usize::min(MAX_LOOK_AHEAD, path.len()) - 1;
    while i > 1 && cut == 0 {
        for j in 0..nneis {
            if path[i] == neis[j] {
                cut = i;
                break;
            }
        }
        i -= 1;
    }

    let mut npath = path.len();
    if cut > 1 {
        let offset = cut - 1;
        npath -= offset;
        for i in 0..npath {
            path[i] = path[i + offset];
        }
    }

    return npath;
}
