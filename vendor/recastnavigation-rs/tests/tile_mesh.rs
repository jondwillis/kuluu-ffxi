#![cfg(feature = "rkyv")]

use recastnavigation_rs::demo::*;
use recastnavigation_rs::detour::*;
use recastnavigation_rs::recast::*;

mod common;
use common::*;

#[derive(Debug, Clone, Copy)]
enum PartitionType {
    // Watershed,
    Monotone,
    Layers,
}

#[test]
fn test_tile_mesh_dungeon() {
    let _ = build_nav_mesh("tile_mesh", "dungeon", 64.0, PartitionType::Monotone);
}

#[test]
fn test_tile_mesh_nav_test() {
    let _ = build_nav_mesh("tile_mesh", "nav_test", 64.0, PartitionType::Monotone);
}

#[test]
fn test_tile_mesh_undulating() {
    let _ = build_nav_mesh("tile_mesh", "undulating", 96.0, PartitionType::Layers);
}

fn build_nav_mesh(folder: &str, name: &str, tile_size: f32, part: PartitionType) -> DtNavMesh {
    let cell_size = 0.3;

    let mut mesh_loader = RcMeshLoaderObj::default();
    mesh_loader.load(&format!("./resource/{}.obj", name));
    mesh_loader.get_file_name();

    let mut chunky_mesh = RcChunkyTriMesh::new();
    rc_create_chunky_tri_mesh(&mut chunky_mesh, mesh_loader.get_verts(), mesh_loader.get_tris(), 256).unwrap();

    let (bmin, bmax) = rc_calc_bounds(mesh_loader.get_verts());
    let (grid_width, grid_height) = rc_calc_grid_size(&bmin, &bmax, cell_size);
    let ts = tile_size as i32;
    let tw = (grid_width + ts - 1) / ts;
    let th = (grid_height + ts - 1) / ts;
    let tcs = tile_size * cell_size;
    let tile_bits = i32::min(ilog2(next_pow2(tw * th)), 14);
    let poly_bits = 22 - tile_bits;
    let params = DtNavMeshParams {
        orig: bmin,
        tile_width: tile_size * cell_size,
        tile_height: tile_size * cell_size,
        max_tiles: 1 << tile_bits,
        max_polys: 1 << poly_bits,
    };
    let mut nav_mesh = DtNavMesh::with_params(&params).unwrap();

    for y in 0..th {
        for x in 0..tw {
            let tile_bmin = [bmin[0] + x as f32 * tcs, bmin[1], bmin[2] + y as f32 * tcs];
            let tile_bmax = [bmin[0] + (x + 1) as f32 * tcs, bmax[1], bmin[2] + (y + 1) as f32 * tcs];

            let data = build_nav_mesh_tile(
                &mesh_loader,
                &chunky_mesh,
                tile_size,
                part,
                x,
                y,
                &tile_bmin,
                &tile_bmax,
            );
            if let Some(data) = data {
                nav_mesh.add_tile(data, DtTileRef::default()).unwrap();
            }
        }
    }

    compare_with_cpp_out(&nav_mesh, folder, name).unwrap();

    return nav_mesh;
}

fn build_nav_mesh_tile(
    mesh_loader: &RcMeshLoaderObj,
    chunky_mesh: &RcChunkyTriMesh,
    tile_size: f32,
    part: PartitionType,
    tx: i32,
    ty: i32,
    bmin: &[f32; 3],
    bmax: &[f32; 3],
) -> Option<DtBuf> {
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

    let mut ctx = RcContext::new(true);

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
    cfg.tile_size = tile_size as i32;
    cfg.border_size = cfg.walkable_radius + 3;
    cfg.width = cfg.tile_size + cfg.border_size * 2;
    cfg.height = cfg.tile_size + cfg.border_size * 2;
    cfg.detail_sample_dist = if detail_sample_dist < 0.9 {
        0.0
    } else {
        cell_size * detail_sample_dist
    };
    cfg.detail_sample_max_error = cell_height * detail_sample_max_error;

    // Expand the heighfield bounding box by border size to find the extents of geometry we need to build this tile.
    //
    // This is done in order to make sure that the navmesh tiles connect correctly at the borders,
    // and the obstacles close to the border work correctly with the dilation process.
    // No polygons (or contours) will be created on the border area.
    //
    // IMPORTANT!
    //
    //   :''''''''':
    //   : +-----+ :
    //   : |     | :
    //   : |     |<--- tile to build
    //   : |     | :
    //   : +-----+ :<-- geometry needed
    //   :.........:
    //
    // You should use this bounding box to query your input geometry.
    //
    // For example if you build a navmesh for terrain, and want the navmesh tiles to match the terrain tile size
    // you will need to pass in data from neighbour terrain tiles too! In a simple case, just pass in all the 8 neighbours,
    // or use the bounding box below to only pass in a sliver of each of the 8 neighbours.

    cfg.bmin = *bmin;
    cfg.bmax = *bmax;
    cfg.bmin[0] -= (cfg.border_size as f32) * cfg.cs;
    cfg.bmin[2] -= (cfg.border_size as f32) * cfg.cs;
    cfg.bmax[0] += (cfg.border_size as f32) * cfg.cs;
    cfg.bmax[2] += (cfg.border_size as f32) * cfg.cs;

    // Allocate voxel heightfield where we rasterize our input data to.

    let mut solid = RcHeightfield::new();
    rc_create_heightfield(
        &mut ctx, &mut solid, cfg.width, cfg.height, &cfg.bmin, &cfg.bmax, cfg.cs, cfg.ch,
    )
    .unwrap();

    // Allocate array that can hold triangle flags.
    // If you have multiple meshes you need to process, allocate
    // and array which can hold the max number of triangles you need to process.

    let mut triareas = vec![0; chunky_mesh.max_tris_per_chunk()];

    let mut cid = [0; 64];
    let ncid = rc_get_chunks_overlapping_rect(
        &chunky_mesh,
        &[cfg.bmin[0], cfg.bmin[2]],
        &mut [cfg.bmax[0], cfg.bmax[2]],
        &mut cid,
    );
    if ncid == 0 {
        return None;
    }

    for i in 0..ncid {
        let node = &chunky_mesh.nodes()[cid[i] as usize];

        let tris = &chunky_mesh.tris()[node.i as usize..(node.i + node.n) as usize];
        triareas[0..node.n as usize].fill(0);
        rc_mark_walkable_triangles(
            &mut ctx,
            cfg.walkable_slope_angle,
            mesh_loader.get_verts(),
            tris,
            &mut triareas,
        )
        .unwrap();

        rc_rasterize_triangles_1(
            &mut ctx,
            mesh_loader.get_verts(),
            tris,
            &triareas[0..node.n as usize],
            &mut solid,
            cfg.walkable_climb,
        )
        .unwrap();
    }

    // Once all geometry is rasterized, we do initial pass of filtering to
    // remove unwanted overhangs caused by the conservative rasterization
    // as well as filter spans where the character cannot possibly stand.

    rc_filter_low_hanging_walkable_obstacles(&mut ctx, cfg.walkable_climb, &mut solid);
    rc_filter_ledge_spans(&mut ctx, cfg.walkable_height, cfg.walkable_climb, &mut solid);
    rc_filter_walkable_low_height_spans(&mut ctx, cfg.walkable_height, &mut solid);

    // Compact the heightfield so that it is faster to handle from now on.
    // This will result more cache coherent data as well as the neighbours
    // between walkable cells will be calculated.

    let mut chf = RcCompactHeightfield::new();
    rc_build_compact_heightfield(&mut ctx, cfg.walkable_height, cfg.walkable_climb, &mut solid, &mut chf).unwrap();

    // Erode the walkable area by agent radius.
    rc_erode_walkable_area(&mut ctx, cfg.walkable_radius, &mut chf).unwrap();

    // Partition the heightfield so that we can use simple algorithm later to triangulate the walkable areas.
    match part {
        PartitionType::Monotone => rc_build_regions_monotone(
            &mut ctx,
            &mut chf,
            cfg.border_size,
            cfg.min_region_area,
            cfg.merge_region_area,
        ),
        PartitionType::Layers => rc_build_layer_regions(&mut ctx, &mut chf, cfg.border_size, cfg.min_region_area),
    }
    .unwrap();

    // Create contours.
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
    if cset.conts().is_empty() {
        return None;
    }

    // Build polygon navmesh from the contours.

    let mut pmesh = RcPolyMesh::new();
    rc_build_poly_mesh(&mut ctx, &cset, cfg.max_verts_per_poly, &mut pmesh).unwrap();

    // Build detail mesh.
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

    // Create Detour data from Recast poly mesh.

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
    params.tile_x = tx;
    params.tile_y = ty;
    params.tile_layer = 0;
    params.bmin = pmesh.bmin;
    params.bmax = pmesh.bmax;
    params.cs = cfg.cs;
    params.ch = cfg.ch;
    params.build_bv_tree = true;
    let nav_data = dt_create_nav_mesh_data(&mut params).unwrap();
    return Some(nav_data);
}

fn next_pow2(mut v: i32) -> i32 {
    v -= 1;
    v |= v >> 1;
    v |= v >> 2;
    v |= v >> 4;
    v |= v >> 8;
    v |= v >> 16;
    v += 1;
    return v;
}

fn ilog2(mut v: i32) -> i32 {
    let mut r: i32;
    let mut shift: i32;

    r = ((v > 0xffff) as i32) << 4;
    v >>= r;

    shift = ((v > 0xff) as i32) << 3;
    v >>= shift;
    r |= shift;

    shift = ((v > 0xf) as i32) << 2;
    v >>= shift;
    r |= shift;

    shift = ((v > 0x3) as i32) << 1;
    v >>= shift;
    r |= shift;

    r |= v >> 1;
    return r;
}
