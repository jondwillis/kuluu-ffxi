use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use ffxi_dat::mmb::{MmbHeader, MmbSubRecord};
use ffxi_dat::{mmb, walk, ChunkKind, DatRoot};
use std::env;
use std::fs;

#[derive(Resource)]
struct LoadRequest {
    file_id: u32,
    chunk_idx: usize,
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let file_id: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(115);
    let chunk_idx: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(18);

    let dry_run = args.iter().any(|a| a == "--dry-run");
    eprintln!("mmb-view: file_id={file_id} chunk={chunk_idx} dry_run={dry_run}");
    if dry_run {
        match dry_run_parse(file_id, chunk_idx) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("dry-run failed: {e}");
                std::process::exit(1);
            }
        }
    }

    App::new()
        .insert_resource(LoadRequest { file_id, chunk_idx })
        .insert_resource(ClearColor(Color::srgb(0.06, 0.06, 0.08)))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: format!("ffxi-mmb-view: file_id={file_id} chunk={chunk_idx}"),
                resolution: (1280u32, 720u32).into(),
                ..default()
            }),
            ..default()
        }))
        .add_systems(Startup, (setup_scene, load_and_spawn_mmb))
        .add_systems(Update, orbit_camera)
        .run();
}

fn dry_run_parse(file_id: u32, chunk_idx: usize) -> Result<(), Box<dyn std::error::Error>> {
    let root = DatRoot::from_env()?;
    let location = root.resolve(file_id)?;
    let path = location.path_under(root.root());
    let bytes = fs::read(&path)?;
    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let chunk = chunks.get(chunk_idx).ok_or("chunk_idx out of range")?;
    if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Mmb) {
        return Err(format!("chunk {} is not an MMB (kind={:#x})", chunk_idx, chunk.kind).into());
    }
    let decrypted = mmb::decrypt(chunk.data)?;
    let header = MmbHeader::parse(&decrypted)?;
    let subs = MmbSubRecord::find_all(header.payload);

    eprintln!("MMB asset: {:?}", header.asset_name_str());
    eprintln!("MMB sub-records: {}", subs.len());
    for (i, sub) in subs.iter().enumerate() {
        let verts = sub.parse_vertices().map(|v| v.len()).unwrap_or(0);
        let tris = sub.parse_triangle_list().len();
        eprintln!(
            "  [{i}] variant={:?} count={} verts={} tris={}",
            sub.variant_name_str(),
            sub.count,
            verts,
            tris
        );
    }
    Ok(())
}

fn setup_scene(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 0.0, 150.0).looking_at(Vec3::ZERO, Vec3::Y),
        OrbitCam {
            distance: 150.0,
            yaw: 0.0,
            pitch: 0.0,
        },
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 9000.0,
            ..default()
        },
        Transform::from_xyz(50.0, 100.0, 80.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    commands.insert_resource(GlobalAmbientLight {
        color: Color::WHITE,
        brightness: 200.0,
        ..default()
    });
}

#[derive(Component)]
struct OrbitCam {
    distance: f32,
    yaw: f32,
    pitch: f32,
}

fn orbit_camera(time: Res<Time>, mut q: Query<(&mut Transform, &mut OrbitCam)>) {
    for (mut t, mut cam) in &mut q {
        cam.yaw += time.delta_secs() * 0.3;
        let (sy, cy) = cam.yaw.sin_cos();
        let (sp, cp) = cam.pitch.sin_cos();
        t.translation = Vec3::new(
            cam.distance * cp * sy,
            cam.distance * sp + 30.0,
            cam.distance * cp * cy,
        );
        t.look_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y);
    }
}

fn load_and_spawn_mmb(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    req: Res<LoadRequest>,
) {
    let root = match DatRoot::from_env() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("DatRoot::from_env: {e}");
            return;
        }
    };

    let location = match root.resolve(req.file_id) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("resolve({}): {e}", req.file_id);
            return;
        }
    };

    let path = location.path_under(root.root());
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("read {}: {e}", path.display());
            return;
        }
    };

    let chunks: Vec<_> = walk(&bytes).filter_map(Result::ok).collect();
    let Some(chunk) = chunks.get(req.chunk_idx) else {
        eprintln!(
            "file has {} chunks, idx {} out of range",
            chunks.len(),
            req.chunk_idx
        );
        return;
    };
    if ChunkKind::from_u8(chunk.kind) != Some(ChunkKind::Mmb) {
        eprintln!(
            "chunk {} is kind {:#x} ({:?}), not an MMB",
            req.chunk_idx,
            chunk.kind,
            ChunkKind::label(chunk.kind),
        );
        return;
    }

    let decrypted = match mmb::decrypt(chunk.data) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("decrypt failed: {e}");
            return;
        }
    };

    let header = match MmbHeader::parse(&decrypted) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("MMB header parse failed: {e}");
            return;
        }
    };

    println!("loaded MMB: {:?}", header.asset_name_str());
    let sub_records = MmbSubRecord::find_all(header.payload);
    println!("  {} sub-records", sub_records.len());

    let palette = [
        Color::srgb(0.9, 0.4, 0.4),
        Color::srgb(0.4, 0.9, 0.4),
        Color::srgb(0.4, 0.4, 0.9),
        Color::srgb(0.9, 0.9, 0.4),
        Color::srgb(0.4, 0.9, 0.9),
        Color::srgb(0.9, 0.4, 0.9),
    ];

    for (i, sub) in sub_records.iter().enumerate() {
        let Some(vertices) = sub.parse_vertices() else {
            println!(
                "  sub[{i}] {:?} count={}: skipped (vertices won't fit body)",
                sub.variant_name_str(),
                sub.count
            );
            continue;
        };
        let triangles = sub.parse_triangle_list();
        if triangles.is_empty() {
            println!(
                "  sub[{i}] {:?} count={}: 0 triangles after strip decode",
                sub.variant_name_str(),
                sub.count
            );
            continue;
        }
        println!(
            "  sub[{i}] {:?}: {} verts, {} tris",
            sub.variant_name_str(),
            vertices.len(),
            triangles.len()
        );

        let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.pos).collect();
        let normals: Vec<[f32; 3]> = vertices.iter().map(|v| v.normal).collect();
        let uvs: Vec<[f32; 2]> = vertices.iter().map(|v| v.uv).collect();
        let colors: Vec<[f32; 4]> = vertices
            .iter()
            .map(|v| {
                [
                    v.rgba[0] as f32 / 255.0,
                    v.rgba[1] as f32 / 255.0,
                    v.rgba[2] as f32 / 255.0,
                    v.rgba[3] as f32 / 255.0,
                ]
            })
            .collect();
        let indices: Vec<u32> = triangles
            .iter()
            .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
            .collect();

        let mut mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        );
        mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
        mesh.insert_indices(Indices::U32(indices));

        let mesh_handle = meshes.add(mesh);
        let mat_handle = materials.add(StandardMaterial {
            base_color: palette[i % palette.len()],
            perceptual_roughness: 1.0,

            cull_mode: None,

            ..default()
        });

        commands.spawn((
            Mesh3d(mesh_handle),
            MeshMaterial3d(mat_handle),
            Transform::default(),
        ));
    }
}
