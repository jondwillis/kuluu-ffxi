//! Live network test for `ffxi_nav_recast::fetch`. Marked `#[ignore]`
//! so CI doesn't depend on github.com or the user's home cache; run
//! with `cargo test -p ffxi-nav-recast -- --ignored`.

use std::fs;

/// Tahrongi_Canyon (zone 133) is one of the three legacy numeric
/// filenames in xiNavmeshes (`133.nav`). Hits the numeric branch.
#[test]
#[ignore = "network: fetches from github.com"]
fn fetch_tahrongi_canyon() {
    let path = ffxi_nav_recast::fetch(133)
        .expect("fetch returned Err")
        .expect("expected a navmesh for zone 133");
    let meta = fs::metadata(&path).expect("nav file should exist");
    assert!(
        meta.len() > 1024,
        "navmesh suspiciously small ({} bytes)",
        meta.len()
    );
    // Idempotent: second call should hit cache, not re-download.
    let path2 = ffxi_nav_recast::fetch(133).unwrap().unwrap();
    assert_eq!(path, path2);
}

/// East_Ronfaure (zone 102) is name-keyed in xiNavmeshes
/// (`East_Ronfaure.nav`). Hits the zone-name fallback branch.
#[test]
#[ignore = "network: fetches from github.com"]
fn fetch_east_ronfaure() {
    let path = ffxi_nav_recast::fetch(102)
        .expect("fetch returned Err")
        .expect("expected a navmesh for zone 102");
    assert!(path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .ends_with(".nav"));
    let meta = fs::metadata(&path).unwrap();
    assert!(meta.len() > 1024);
}

/// End-to-end: download zone 133's navmesh, load it via Recast, and
/// confirm `RecastNavMesh::from_path` succeeds. Doesn't query a path
/// (zone-specific coords would be brittle); just proves the format is
/// what `recastnavigation-rs::load_nav_mesh` accepts.
#[test]
#[ignore = "network: fetches from github.com"]
fn load_recast_navmesh_for_tahrongi_canyon() {
    let path = ffxi_nav_recast::fetch(133)
        .expect("fetch returned Err")
        .expect("expected a navmesh for zone 133");
    let _nav = ffxi_nav_recast::RecastNavMesh::from_path(&path)
        .expect("Recast should accept a real xiNavmeshes file");
}

/// Sanity-check polygon edge extraction. Tahrongi Canyon is a large
/// outdoor zone; a non-trivial navmesh must produce thousands of edges.
/// If we somehow short-circuit the iteration the count will be ~0.
#[test]
#[ignore = "network: fetches from github.com"]
fn polygon_edges_returns_nontrivial_count() {
    let path = ffxi_nav_recast::fetch(133).unwrap().unwrap();
    let nav = ffxi_nav_recast::RecastNavMesh::from_path(&path).unwrap();
    let edges = nav.polygon_edges_detour();
    assert!(
        edges.len() > 100,
        "Tahrongi Canyon navmesh produced only {} edges \u{2014} likely a bug in iteration",
        edges.len()
    );
}
