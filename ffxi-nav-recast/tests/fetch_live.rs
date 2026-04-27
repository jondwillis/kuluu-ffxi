use std::fs;

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

    let path2 = ffxi_nav_recast::fetch(133).unwrap().unwrap();
    assert_eq!(path, path2);
}

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

#[test]
#[ignore = "network: fetches from github.com"]
fn load_recast_navmesh_for_tahrongi_canyon() {
    let path = ffxi_nav_recast::fetch(133)
        .expect("fetch returned Err")
        .expect("expected a navmesh for zone 133");
    let _nav = ffxi_nav_recast::RecastNavMesh::from_path(&path)
        .expect("Recast should accept a real xiNavmeshes file");
}

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
