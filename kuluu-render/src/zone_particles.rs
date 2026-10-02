#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashSet;

use bevy::prelude::*;
use ffxi_dat::particle_gen::ParticleGeneratorDef;
use ffxi_dat::ChunkKind;
use kuluu_snapshot::Vec3 as WireVec3;

use crate::particle_sim::{spawn_zone_particle_generator, ParticleSimulator, ZoneGeneratorOptions};
use crate::scene::mzb_to_bevy;
use crate::scheduler_runtime::{parse_action_bytes, GlobalEffectDir};
use crate::snapshot::{effective_zone_file_id, SceneState};

#[derive(Resource, Default)]
pub struct ZoneParticles {
    loaded: Option<(Option<u32>, bool)>,
    entities: Vec<Entity>,
}

// research/XIClient/src/XIClient/source/World/Zone/XiZone.cpp InitWeather;
// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp CYyGenerator.
pub(crate) fn is_zone_static(def: &ParticleGeneratorDef) -> bool {
    def.auto_run
}

pub(crate) struct ZoneStaticDef {
    pub name: [u8; 4],
    pub def: ParticleGeneratorDef,
    // The chunk's byte offset in the zone DAT: the element_sort.rs tie-break.
    pub dat_offset: usize,
}

/// The same emitter is sometimes authored twice (West Ronfaure's effe/fir1 campfire subtree
/// appears in file 200 twice, byte for byte), and a second additive flame on the same spot
/// doubles its brightness, so an exact repeat of (name, mesh, base position) collapses to
/// one. A repeated NAME alone is not a repeat: many sit at distinct positions
/// (Manaclipper's g000 under saki/, sira/ and shik/) and every one of them runs in retail.
fn zone_static_defs(bytes: &[u8]) -> Vec<ZoneStaticDef> {
    fn walk(
        node: &ffxi_dat::chunk::ChunkNode<'_>,
        water: &HashSet<usize>,
        seen: &mut HashSet<([u8; 4], [u8; 4], [u32; 3])>,
        out: &mut Vec<ZoneStaticDef>,
    ) {
        for child in &node.children {
            let c = &child.chunk;
            if !child.children.is_empty() || c.kind == ChunkKind::Rmp as u8 {
                if c.name != crate::weather_particles::WEAT_DIR {
                    walk(child, water, seen, out);
                }
                continue;
            }
            if ChunkKind::from_u8(c.kind) != Some(ChunkKind::Generator) {
                continue;
            }
            let Ok(Some(def)) = ParticleGeneratorDef::parse(c.data) else {
                continue;
            };
            if !is_zone_static(&def) || water.contains(&c.offset) {
                continue;
            }
            if seen.insert((c.name, def.mesh_id, def.base_position.map(f32::to_bits))) {
                out.push(ZoneStaticDef {
                    name: c.name,
                    def,
                    dat_offset: c.offset,
                });
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &ffxi_dat::chunk::walk_tree(bytes),
        &crate::dat_mzb::water_generator_offsets(bytes),
        &mut HashSet::new(),
        &mut out,
    );
    out
}

/// Spawns the zone's static particle generators. The load key carries the global
/// effect dir's arrival (it loads off-thread): a set built before it lands is
/// missing every generator whose mesh ships there (the campfire flame sheet
/// syst/effe/hi12 among them) and has to be rebuilt once. Camera-relative
/// generators start at the origin as a placeholder; track_zone_particles
/// rewrites them from the camera before the first mesh rebuild.
fn sync_zone_particles(
    scene_state: Res<SceneState>,
    global: Option<Res<GlobalEffectDir>>,
    mut store: ResMut<ZoneParticles>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<crate::ffxi_particle_material::FfxiParticleMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
    dat_root: Res<crate::dat_root::SharedDatRoot>,
) {
    let Some(root) = dat_root.get() else {
        return;
    };
    let file_id = effective_zone_file_id(&scene_state.snapshot);
    let key = (file_id, global.is_some());
    if store.loaded == Some(key) {
        return;
    }
    store.loaded = Some(key);

    // OnExit(InGame) does not fire on a zone warp, so despawn the previous zone's
    // generator entities explicitly here; the simulator self-reaps the dangling
    // LiveGenerators once their mesh entity is gone (sync_particle_meshes).
    for e in store.entities.drain(..) {
        commands.entity(e).try_despawn();
    }

    let Some(file_id) = file_id else {
        return;
    };
    let Ok(loc) = root.resolve(file_id) else {
        return;
    };
    let path = loc.path_under(root);
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };

    let (_schedulers, assets, _cameras) = parse_action_bytes(&bytes);
    let global = global.as_ref().map(|g| &g.assets);
    let mut spawned = 0usize;
    let mut unresolved: Vec<String> = Vec::new();
    for ZoneStaticDef {
        name,
        def,
        dat_offset,
    } in zone_static_defs(&bytes)
    {
        let bp = def.base_position;
        let origin = if def.camera_relative {
            Vec3::ZERO
        } else {
            mzb_to_bevy(WireVec3 {
                x: bp[0],
                y: bp[1],
                z: bp[2],
            })
        };
        let opts = ZoneGeneratorOptions {
            camera_relative: def.camera_relative,
            dat_offset,
            ..Default::default()
        };
        let entity = spawn_zone_particle_generator(
            def,
            &assets,
            global,
            origin,
            opts,
            &mut meshes,
            &mut mats,
            &mut images,
            &mut sim,
            &mut commands,
        );
        match entity {
            Some(e) => {
                store.entities.push(e);
                spawned += 1;
            }
            None => unresolved.push(format!(
                "{}<{}>",
                String::from_utf8_lossy(&name).trim_end(),
                String::from_utf8_lossy(&def.mesh_id).trim_end()
            )),
        }
    }

    info!(
        "zone_particles: DAT {file_id} → {spawned} zone-static particle generator(s), {} without a resolvable mesh{}{}",
        unresolved.len(),
        if global.is_none() {
            " (global effect dir not loaded yet)"
        } else {
            ""
        },
        if unresolved.is_empty() {
            String::new()
        } else {
            format!(": {}", unresolved.join(" "))
        },
    );
}

/// Runs before particle_sim::sync_particle_meshes: the simulator bakes each
/// generator's world positions into its mesh, so the camera-relative origins
/// have to land first.
fn track_zone_particles(
    cam: Query<&GlobalTransform, With<crate::camera::OperatorCamera>>,
    mut sim: ResMut<ParticleSimulator>,
) {
    let Some(cam) = cam.iter().next() else {
        return;
    };
    sim.set_camera_relative_origins(cam.translation());
}

pub struct ZoneParticlesPlugin;

impl Plugin for ZoneParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneParticles>().add_systems(
            Update,
            (sync_zone_particles, track_zone_particles)
                .chain()
                .before(crate::particle_sim::sync_particle_meshes),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler_runtime::GLOBAL_EFFECT_DIR_FILE_ID;
    use crate::weather_particles::tests::{weat_generator_names, zone_dat};

    const BATALLIA_DOWNS_ZONE_DAT: u32 = 197;
    const WEST_RONFAURE_ZONE_DAT: u32 = 200;
    const SELBINA_ZONE_DAT: u32 = 348;

    fn names(defs: &[ZoneStaticDef]) -> Vec<String> {
        defs.iter()
            .map(|d| String::from_utf8_lossy(&d.name).trim_end().to_string())
            .collect()
    }

    #[test]
    fn zone_static_is_every_auto_run_generator() {
        let base = ParticleGeneratorDef {
            auto_run: true,
            max_life_frames: 60.0,
            ..Default::default()
        };
        assert!(
            is_zone_static(&base),
            "unplaced, unnamed, meshless: still runs"
        );
        assert!(
            !is_zone_static(&ParticleGeneratorDef {
                auto_run: false,
                ..base
            }),
            "manual-run generator excluded"
        );
        assert!(
            is_zone_static(&ParticleGeneratorDef {
                max_life_frames: 0.0,
                ..base
            }),
            "persistent singleton runs unless another renderer owns it"
        );
    }

    /// Before the weat/ subtree was carved out this path spawned 16 weather emitters as
    /// permanent zone scenery across eight zones — Batallia Downs among them.
    #[test]
    fn real_dat_zone_static_defs_skip_the_weat_subtree() {
        let Some(bytes) = zone_dat(BATALLIA_DOWNS_ZONE_DAT) else {
            return;
        };
        let statics = zone_static_defs(&bytes);
        assert!(!statics.is_empty(), "zone ships static generators");
        let weat = weat_generator_names(&bytes);
        assert!(!weat.is_empty(), "zone ships weat/ generators");
        for d in &statics {
            assert!(
                !weat.contains(&d.name),
                "{} is a weat/ generator",
                String::from_utf8_lossy(&d.name)
            );
        }
    }

    #[test]
    fn real_dat_selbina_lanterns_and_chimneys_are_zone_statics() {
        let Some(bytes) = zone_dat(SELBINA_ZONE_DAT) else {
            return;
        };
        let names = names(&zone_static_defs(&bytes));
        for n in [
            "lt01", "lfr1", "fir1", "sk00", "wi01", "lt15", "lt16", "lt17",
        ] {
            assert!(names.iter().any(|x| x == n), "{n} missing: {names:?}");
        }
        for n in ["sea1", "sea2", "scol", "se01"] {
            assert!(!names.iter().any(|x| x == n), "{n} claimed here");
        }
    }

    #[test]
    fn real_dat_lower_jeuno_monument_persistent_effects_are_owned() {
        const LOWER_JEUNO_DAT: u32 = 345;
        let Some(bytes) = zone_dat(LOWER_JEUNO_DAT) else {
            return;
        };
        let defs = zone_static_defs(&bytes);
        let (_, assets, _) = parse_action_bytes(&bytes);
        for name in [*b"myl7", *b"SPLT"] {
            let d = defs
                .iter()
                .find(|d| d.name == name)
                .expect("monument effect");
            assert_eq!(d.def.max_life_frames, 0.0);
            assert_eq!(d.def.mesh_id, *b"ligh");
        }
        assert!(assets.sprite_sheets.contains_key(b"ligh"));
        assert!(assets.mmbs.contains_key(b"ligh"));
    }

    /// The campfire flame sheet hi12 ships only in the global effect dir
    /// (syst/effe/hi12): the zone-local assets alone cannot spawn fir1, which
    /// is how a Selbina session logged "0 zone-static particle generator(s)"
    /// beside its seven lit point lights.
    #[test]
    fn real_dat_selbina_flame_sheet_lives_in_the_global_effect_dir() {
        let Some(bytes) = zone_dat(SELBINA_ZONE_DAT) else {
            return;
        };
        let Some(global) = zone_dat(GLOBAL_EFFECT_DIR_FILE_ID) else {
            return;
        };
        let fir1 = zone_static_defs(&bytes)
            .into_iter()
            .find(|d| &d.name == b"fir1")
            .expect("fir1");
        assert_eq!(&fir1.def.mesh_id, b"hi12");
        let (_s, local, _) = parse_action_bytes(&bytes);
        let (_s, global, _) = parse_action_bytes(&global);
        assert!(
            !local.sprite_sheets.contains_key(b"hi12") && !local.d3ms.contains_key(b"hi12"),
            "hi12 is not zone-local"
        );
        assert!(
            global.sprite_sheets.contains_key(b"hi12"),
            "the flame sheet resolves only against the global effect dir"
        );
    }

    /// West Ronfaure authors its effe/fir1 and effe/fir2 campfire subtrees twice
    /// each; the exact repeats collapse while the like-named lamps under
    /// mode/ligh/s_li and mode/ligh/taki, at their own positions, all stay.
    #[test]
    fn real_dat_west_ronfaure_repeated_campfire_collapses_to_one() {
        let Some(bytes) = zone_dat(WEST_RONFAURE_ZONE_DAT) else {
            return;
        };
        let fir1: Vec<_> = zone_static_defs(&bytes)
            .into_iter()
            .filter(|d| &d.name == b"fir1")
            .collect();
        let positions: HashSet<[u32; 3]> = fir1
            .iter()
            .map(|d| d.def.base_position.map(f32::to_bits))
            .collect();
        assert_eq!(fir1.len(), 4, "s_li, taki, effe/fir1, effe/fir2");
        assert_eq!(positions.len(), fir1.len(), "no two at one spot");
    }
}
