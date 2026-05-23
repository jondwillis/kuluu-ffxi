use std::path::Path;
use std::{env, fs};

fn main() {
    let is_arch_64 = env::var("CARGO_CFG_TARGET_POINTER_WIDTH").unwrap() == "64";

    cxx_build::bridges([
        "src/recast/recast.rs",
        "src/detour/base.rs",
        "src/detour/builder.rs",
        "src/detour/mesh.rs",
        "src/detour/query.rs",
        "src/detour_crowd/local_boundary.rs",
        "src/detour_crowd/path_corridor.rs",
        "src/detour_crowd/obstacle_avoidance.rs",
        "src/detour_crowd/crowd.rs",
        "src/demo/demo.rs",
    ])
    .flag_if_supported("-std=c++14")
    .flag_if_supported("/fp:precise")
    .flag_if_supported("-ffp-model=precise")
    .flag_if_supported("-ffp-contract=off")
    .define("_CRT_SECURE_NO_WARNINGS", "1")
    .define("IS_ARCH_64", if is_arch_64 { "1" } else { "0" })
    .include("./recastnavigation/Deterministic/Include")
    .files(list_cpp_files("./recastnavigation/Deterministic/Source"))
    .include("./recastnavigation/Recast/Include")
    .files(list_cpp_files("./recastnavigation/Recast/Source"))
    .include("./recastnavigation/Detour/Include")
    .files(list_cpp_files("./recastnavigation/Detour/Source"))
    .include("./recastnavigation/DetourCrowd/Include")
    .files(list_cpp_files("./recastnavigation/DetourCrowd/Source"))
    .include("./recastnavigation/RecastDemo/Include")
    .files([
        "./recastnavigation/RecastDemo/Source/MeshLoaderObj.cpp",
        "./recastnavigation/RecastDemo/Source/ChunkyTriMesh.cpp",
    ])
    .files(["./src/demo/demo-ffi.cpp"])
    .compile("recastnavigation");

    println!("cargo:rerun-if-changed=src/utils.h");

    println!("cargo:rerun-if-changed=src/recast/recast.rs");
    println!("cargo:rerun-if-changed=src/recast/recast-ffi.h");

    println!("cargo:rerun-if-changed=src/detour/base.rs");
    println!("cargo:rerun-if-changed=src/detour/builder.rs");
    println!("cargo:rerun-if-changed=src/detour/mesh.rs");
    println!("cargo:rerun-if-changed=src/detour/query.rs");
    println!("cargo:rerun-if-changed=src/detour/detour-ffi.h");

    println!("cargo:rerun-if-changed=src/detour_crowd/path_corridor.rs");
    println!("cargo:rerun-if-changed=src/detour_crowd/path_corridor.rs");
    println!("cargo:rerun-if-changed=src/detour_crowd/obstacle_avoidance.rs");
    println!("cargo:rerun-if-changed=src/detour_crowd/crowd.rs");
    println!("cargo:rerun-if-changed=src/detour_crowd/crowd-ffi.h");

    println!("cargo:rerun-if-changed=src/demo/demo.rs");
    println!("cargo:rerun-if-changed=src/demo/demo-ffi.h");
    println!("cargo:rerun-if-changed=src/demo/demo-ffi.cpp");
}

fn list_cpp_files<P: AsRef<Path>>(dir: P) -> Vec<String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            let path = path.to_str().unwrap().to_string();
            if path.ends_with(".cpp") || path.ends_with(".cc") {
                files.push(path);
            }
        }
    }
    return files;
}
