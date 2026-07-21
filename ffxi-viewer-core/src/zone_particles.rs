#![cfg(not(target_arch = "wasm32"))]

use bevy::prelude::*;
use ffxi_dat::particle_gen::ParticleGeneratorDef;
use ffxi_dat::DatRoot;
use ffxi_viewer_wire::Vec3 as WireVec3;

use crate::particle_sim::{spawn_zone_particle_generator, ParticleSimulator};
use crate::scene::mzb_to_bevy;
use crate::scheduler_runtime::parse_action_bytes;
use crate::snapshot::{effective_zone_file_id, SceneState};

#[derive(Resource, Default)]
pub struct ZoneParticles {
    pub file_id: Option<u32>,
    entities: Vec<Entity>,
}

// A zone-static emitter is an auto-run Generator embedded in the zone MZB DAT with
// a real world placement; the ~110 defs at [0,0,0] are action templates reused by
// the effect scheduler, not placed emitters (Bastok Mines DAT 334 keeps exactly 6).
fn is_placed_zone_generator(def: &ParticleGeneratorDef) -> bool {
    def.auto_run && def.base_position != [0.0, 0.0, 0.0]
}

fn sync_zone_particles(
    scene_state: Res<SceneState>,
    mut store: ResMut<ZoneParticles>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    let current = effective_zone_file_id(&scene_state.snapshot);
    if current == store.file_id {
        return;
    }
    store.file_id = current;

    // OnExit(InGame) does not fire on a zone warp, so despawn the previous zone's
    // generator entities explicitly here; the simulator self-reaps the dangling
    // LiveGenerators once their mesh entity is gone (sync_particle_meshes).
    for e in store.entities.drain(..) {
        commands.entity(e).try_despawn();
    }

    let Some(file_id) = current else {
        return;
    };
    let Ok(root) = DatRoot::from_env_or_default() else {
        return;
    };
    let Ok(loc) = root.resolve(file_id) else {
        return;
    };
    let path = loc.path_under(root.root());
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };

    let (_schedulers, assets) = parse_action_bytes(&bytes);
    let mut spawned = 0usize;
    for def in assets.particle_defs.values() {
        if !is_placed_zone_generator(def) {
            continue;
        }
        let bp = def.base_position;
        let origin = mzb_to_bevy(WireVec3 {
            x: bp[0],
            y: bp[1],
            z: bp[2],
        });
        if let Some(entity) = spawn_zone_particle_generator(
            *def,
            &assets,
            origin,
            &mut meshes,
            &mut mats,
            &mut images,
            &mut sim,
            &mut commands,
        ) {
            store.entities.push(entity);
            spawned += 1;
        }
    }

    info!("zone_particles: DAT {file_id} → {spawned} zone-static particle generator(s)");
}

pub struct ZoneParticlesPlugin;

impl Plugin for ZoneParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneParticles>()
            .add_systems(Update, sync_zone_particles);
    }
}
