#![cfg(feature = "rkyv")]

use recastnavigation_rs::demo::*;
use recastnavigation_rs::detour::*;
use recastnavigation_rs::recast::*;

mod common;
use common::*;

#[test]
fn test_solo_mesh_dungeon() {
    let _ = build_nav_mesh("solo_mesh", "dungeon");
}

#[test]
fn test_solo_mesh_nav_test() {
    let _ = build_nav_mesh("solo_mesh", "nav_test");
}

#[test]
fn test_solo_mesh_undulating() {
    let _ = build_nav_mesh("solo_mesh", "undulating");
}

fn build_nav_mesh(folder: &str, name: &str) -> DtNavMesh {
    let cell_size = 0.3;
    let cell_height = 0.2;
    let angle_max_slope = 45.0;
    let agent_height = 2.0;
    let agent_max_climb = 0.9;
    let agent_radius = 0.6;
    let edge_max_len = 12.0;
    let edge_max_error = 1.3;
    let region_min_size = 8.0;
    let region_merge_size = 20.0;
    let verts_per_poly = 6.0;
    let detail_sample_dist = 6.0;
    let detail_sample_max_error = 1.0;

    //
    // Step 0. Context & object.
    //

    let mut ctx = RcContext::new(true);

    let mut mesh_loader = RcMeshLoaderObj::default();
    mesh_loader.load(&format!("./resource/{}.obj", name));
    mesh_loader.get_file_name();

    //
    // Step 1. Initialize build config.
    //

    let mut cfg = RcConfig::default();
    cfg.cs = cell_size;
    cfg.ch = cell_height;
    cfg.walkable_slope_angle = angle_max_slope;
    cfg.walkable_height = (agent_height / cfg.ch).ceil() as i32;
    cfg.walkable_climb = (agent_max_climb / cfg.ch).floor() as i32;
    cfg.walkable_radius = (agent_radius / cfg.cs).floor() as i32;
    cfg.max_edge_len = (edge_max_len / cell_size) as i32;
    cfg.max_simplification_error = edge_max_error;
    cfg.min_region_area = (region_min_size * region_min_size) as i32;
    cfg.merge_region_area = (region_merge_size * region_merge_size) as i32;
    cfg.max_verts_per_poly = verts_per_poly as i32;
    cfg.detail_sample_dist = if detail_sample_dist < 0.9 {
        0.0
    } else {
        cell_size * detail_sample_dist
    };
    cfg.detail_sample_max_error = cell_height * detail_sample_max_error;

    let (bmin, bmax) = rc_calc_bounds(mesh_loader.get_verts());
    cfg.bmin = bmin;
    cfg.bmax = bmax;
    let (width, height) = rc_calc_grid_size(&bmin, &bmax, cfg.cs);
    cfg.width = width;
    cfg.height = height;

    //
    // Step 2. Rasterize input polygon soup.
    //

    let mut solid = RcHeightfield::new();
    rc_create_heightfield(
        &mut ctx, &mut solid, cfg.width, cfg.height, &cfg.bmin, &cfg.bmax, cfg.cs, cfg.ch,
    )
    .unwrap();

    let mut triareas: Vec<u8> = vec![0; mesh_loader.get_tri_count() as usize];
    rc_mark_walkable_triangles(
        &mut ctx,
        cfg.walkable_slope_angle,
        mesh_loader.get_verts(),
        mesh_loader.get_tris(),
        &mut triareas,
    )
    .unwrap();
    compare_with_rkyv(folder, &format!("{}_triareas", name), &triareas).unwrap();

    rc_rasterize_triangles_1(
        &mut ctx,
        mesh_loader.get_verts(),
        mesh_loader.get_tris(),
        &triareas,
        &mut solid,
        cfg.walkable_climb,
    )
    .unwrap();
    compare_with_rkyv(
        folder,
        &format!("{}_solid_1", name),
        &dump_heightfield_state(&solid),
    )
    .unwrap();

    //
    // Step 3. Filter walkable surfaces.
    //

    rc_filter_low_hanging_walkable_obstacles(&mut ctx, cfg.walkable_climb, &mut solid);
    rc_filter_ledge_spans(&mut ctx, cfg.walkable_height, cfg.walkable_climb, &mut solid);
    rc_filter_walkable_low_height_spans(&mut ctx, cfg.walkable_height, &mut solid);
    compare_with_rkyv(
        folder,
        &format!("{}_solid_2", name),
        &dump_heightfield_state(&solid),
    )
    .unwrap();

    //
    // Step 4. Partition walkable surface to simple regions.
    //

    let mut chf = RcCompactHeightfield::new();
    rc_build_compact_heightfield(&mut ctx, cfg.walkable_height, cfg.walkable_climb, &mut solid, &mut chf).unwrap();
    rc_erode_walkable_area(&mut ctx, cfg.walkable_radius, &mut chf).unwrap();
    rc_build_distance_field(&mut ctx, &mut chf).unwrap();
    rc_build_regions(
        &mut ctx,
        &mut chf,
        cfg.border_size,
        cfg.min_region_area,
        cfg.merge_region_area,
    )
    .unwrap();
    compare_with_rkyv(
        folder,
        &format!("{}_chf", name),
        &dump_compact_heightfield_state(&chf),
    )
    .unwrap();

    //
    // Step 5. Trace and simplify region contours.
    //

    let mut cset = RcContourSet::new();
    rc_build_contours(
        &mut ctx,
        &mut chf,
        cfg.max_simplification_error,
        cfg.max_edge_len,
        &mut cset,
        RcBuildContoursFlags::RC_CONTOUR_TESS_WALL_EDGES,
    )
    .unwrap();
    compare_with_rkyv(
        folder,
        &format!("{}_cset", name),
        &dump_contour_set_state(&cset),
    )
    .unwrap();

    //
    // Step 6. Build polygons mesh from contours.
    //

    let mut pmesh = RcPolyMesh::new();
    rc_build_poly_mesh(&mut ctx, &cset, cfg.max_verts_per_poly, &mut pmesh).unwrap();
    compare_with_rkyv(
        folder,
        &format!("{}_pmesh", name),
        &dump_poly_mesh_state(&pmesh),
    )
    .unwrap();

    //
    // Step 7. Create detail mesh which allows to access approximate height on each polygon.
    //

    let mut dmesh = RcPolyMeshDetail::new();
    rc_build_poly_mesh_detail(
        &mut ctx,
        &pmesh,
        &chf,
        cfg.detail_sample_dist,
        cfg.detail_sample_max_error,
        &mut dmesh,
    )
    .unwrap();
    compare_with_rkyv(
        folder,
        &format!("{}_dmesh", name),
        &dump_poly_mesh_detail_state(&dmesh),
    )
    .unwrap();

    //
    // Step 8. Create Detour data from Recast poly mesh.
    //

    for i in 0..pmesh.npolys() {
        if pmesh.areas()[i] == RC_WALKABLE_AREA {
            pmesh.areas_mut()[i] = SamplePolyAreas::Ground as u8;
        }
        if pmesh.areas()[i] == SamplePolyAreas::Ground as u8
            || pmesh.areas()[i] == SamplePolyAreas::Grass as u8
            || pmesh.areas()[i] == SamplePolyAreas::Road as u8
        {
            pmesh.flags_mut()[i] = SamplePolyFlags::Walk as u16;
        } else if pmesh.areas()[i] == SamplePolyAreas::Water as u8 {
            pmesh.flags_mut()[i] = SamplePolyFlags::Swim as u16;
        } else if pmesh.areas()[i] == SamplePolyAreas::Door as u8 {
            pmesh.flags_mut()[i] = SamplePolyFlags::Walk as u16 | SamplePolyFlags::Door as u16;
        }
    }

    let mut params = DtNavMeshCreateParams::default();
    params.verts = Some(pmesh.verts());
    params.polys = Some(pmesh.polys());
    params.poly_areas = Some(pmesh.areas());
    params.poly_flags = Some(pmesh.flags());
    params.nvp = pmesh.nvp();
    params.detail_meshes = Some(dmesh.meshes());
    params.detail_verts = Some(dmesh.verts());
    params.detail_tris = Some(dmesh.tris());
    params.walkable_height = 2.0;
    params.walkable_radius = 0.6;
    params.walkable_climb = 0.9;
    params.bmin = pmesh.bmin;
    params.bmax = pmesh.bmax;
    params.cs = cfg.cs;
    params.ch = cfg.ch;
    params.build_bv_tree = true;
    let nav_data = dt_create_nav_mesh_data(&mut params).unwrap();

    let nav_mesh = DtNavMesh::with_data(nav_data).unwrap();
    compare_with_cpp_out(&nav_mesh, folder, name).unwrap();

    return nav_mesh;
}
