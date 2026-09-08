#![cfg(not(target_arch = "wasm32"))]

use bevy::prelude::*;
use ffxi_dat::chunk::ChunkNode;
use ffxi_dat::particle_gen::{AttachType, ParticleGeneratorDef};
use ffxi_dat::weather::WeatherTypeId;
use ffxi_dat::ChunkKind;
use ffxi_dat::DatRoot;
use kuluu_snapshot::Vec3 as WireVec3;

use crate::particle_sim::{spawn_zone_particle_generator, ParticleSimulator, ZoneGeneratorOptions};
use crate::scene::mzb_to_bevy;
use crate::scheduler_runtime::parse_action_tree;
use crate::snapshot::{effective_zone_file_id, SceneState};
use crate::zone_clouds::{find_weat_type, CLOUD_CANOPY_GENERATOR_NAMES};

// The directory holding a zone's per-weather environment subtrees.
pub const WEAT_DIR: WeatherTypeId = *b"weat";

// research/XIClient/src/XIClient/source/World/Generator/CYyGenerator.cpp:418-434 — Open() walks up
// to the `taew` (weat) container and ORs field_DE with 0x83, which arms the per-emission count
// scale in the unbatched arm of the emit loop at :2817-2831 (`v161 *= GetSomeGeneratorScalar() *
// 0.30000001`; the scalar defaults to 1.0, RegistryConfig.cpp:25).
//
// Retail only reaches that arm when CheckFlag29 is clear (:2814); a batched generator — which the
// precipitation curtains are — instead calls ElemGenerate once and lets the batched elem draw its
// own sub-particles, and that element is the one retail's reimplementation leaves as
// SPDLOG_ERROR("0x11"), so its population is not transcribable. `Particle` models the
// sub-particle, so this scalar stands in for the batched draw with retail's own weat/ thinning
// factor rather than a number of our own invention (same disclosure as
// particle_sim::particle_orientation).
const WEATHER_EMIT_SCALE: f32 = 0.3;

// Subtrees under weat/<tag>/ that another module already draws: the lens-flare chain is projected
// in screen space by lens_flare.rs, the star dome and the cloud canopies by zone_clouds.rs, and
// the sun/moon billboards by celestial_particles.rs.
const LENS_FLARE_SUBDIR_PREFIX: &[u8] = b"lf";
const OTHER_MODULE_SUBDIRS: [WeatherTypeId; 3] = [*b"lens", *b"star", *b"moon"];

fn owned_elsewhere(subdir: WeatherTypeId) -> bool {
    subdir.starts_with(LENS_FLARE_SUBDIR_PREFIX) || OTHER_MODULE_SUBDIRS.contains(&subdir)
}

// research/XIClient/src/XIClient/source/World/Weather/WeatherTransition.cpp:22,95 — activation
// walks the weat/<tag> container for Generator resources and takes every one whose
// `flags & 0x1000` is set, which is the bit we parse as `auto_run`. The rest is ours, not retail's:
// it keeps this module off surfaces another one already draws. A life of 0 marks a persistent
// billboard (the cloud canopies, the sky sheets) rather than a timed emitter — the same split
// zone_particles.rs draws for the zone's own scenery.
fn is_precipitation(name: [u8; 4], def: &ParticleGeneratorDef) -> bool {
    def.auto_run
        && def.max_life_frames > 0.0
        && !matches!(def.attach_type, AttachType::Sun | AttachType::Moon)
        && !CLOUD_CANOPY_GENERATOR_NAMES.contains(&name)
}

// research/XIClient/src/XIClient/source/Resource/FileResource.cpp:578-624 — the container walk is
// sequential over everything nested below the starting container, so weat/rain/kino and
// weat/rain/hamo/kawa are in scope, not just the tag's direct children. An excluded subtree stays
// excluded all the way down: the lens-flare dirs hold their generators one level further in.
fn collect_precipitation(node: &ChunkNode<'_>) -> Vec<([u8; 4], ParticleGeneratorDef)> {
    fn walk(node: &ChunkNode<'_>, owned: bool, out: &mut Vec<([u8; 4], ParticleGeneratorDef)>) {
        for child in &node.children {
            let c = &child.chunk;
            if !child.children.is_empty() || c.kind == ChunkKind::Rmp as u8 {
                walk(child, owned || owned_elsewhere(c.name), out);
                continue;
            }
            if owned
                || ffxi_dat::kind::ChunkKind::from_u8(c.kind)
                    != Some(ffxi_dat::kind::ChunkKind::Generator)
            {
                continue;
            }
            if let Ok(Some(def)) = ParticleGeneratorDef::parse(c.data) {
                if is_precipitation(c.name, &def) {
                    out.push((c.name, def));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(node, false, &mut out);
    out
}

#[derive(Resource, Default)]
pub struct WeatherParticles {
    loaded: Option<(Option<u32>, WeatherTypeId, bool)>,
    entities: Vec<Entity>,
}

impl WeatherParticles {
    /// The launcher backdrop mirrors its own zone into `SceneState`, so the key does not
    /// reliably pass through `None` between logout and the next zone-in; without this the
    /// guard can still match after `OnExit(InGame)` despawned every entity it names.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

fn sync_weather_particles(
    scene_state: Res<SceneState>,
    zone_weather: Res<crate::weather::ZoneWeather>,
    panels: Res<crate::hud::HudPanels>,
    mut store: ResMut<WeatherParticles>,
    global: Option<Res<crate::scheduler_runtime::GlobalEffectDir>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<crate::ffxi_particle_material::FfxiParticleMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sim: ResMut<ParticleSimulator>,
    mut commands: Commands,
) {
    // Debug gate (Debug menu Weather row): while off, despawn any live
    // precipitation generators and drop the load key so re-enabling rebuilds
    // the set for whatever weather is active then.
    if panels.weather_off {
        for e in store.entities.drain(..) {
            commands.entity(e).try_despawn();
        }
        store.clear();
        return;
    }

    let file_id = effective_zone_file_id(&scene_state.snapshot);
    let weather = zone_weather
        .active_weather_type()
        .unwrap_or(ffxi_dat::weather::WEATHER_TYPE_FALLBACK);
    // The global effect dir loads off-thread, so the key carries its arrival: a set built before
    // it lands is missing every generator whose sheet lives there and has to be rebuilt once.
    let key = (file_id, weather, global.is_some());
    if store.loaded == Some(key) {
        return;
    }
    store.loaded = Some(key);

    // OnExit(InGame) does not fire on a zone warp, and a weather change swaps the whole set, so
    // the previous one is despawned explicitly here.
    for e in store.entities.drain(..) {
        commands.entity(e).try_despawn();
    }

    let Some(bytes) = file_id
        .zip(DatRoot::from_env_or_default().ok())
        .and_then(|(id, root)| {
            root.resolve(id)
                .ok()
                .and_then(|loc| std::fs::read(loc.path_under(&root)).ok())
        })
    else {
        return;
    };

    let tree = ffxi_dat::chunk::walk_tree(&bytes);
    let Some(weat) = find_weat_type(&tree, weather) else {
        return;
    };
    let defs = collect_precipitation(weat);
    if defs.is_empty() {
        return;
    }
    let (_schedulers, assets) = parse_action_tree(weat);
    let global = global.as_ref().map(|g| &g.assets);

    for (name, def) in &defs {
        let bp = def.base_position;
        let origin = if def.camera_relative {
            // Placeholder: track_weather_particles rewrites it from the camera before the first
            // mesh rebuild.
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
            emit_scale: WEATHER_EMIT_SCALE,
        };
        let entity = spawn_zone_particle_generator(
            *def,
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
            Some(e) => store.entities.push(e),
            None => debug!(
                "weather_particles: {} has no resolvable mesh {}",
                String::from_utf8_lossy(name),
                String::from_utf8_lossy(&def.mesh_id)
            ),
        }
    }

    info!(
        "weather_particles: DAT {file_id:?} weat/{} → {}/{} generator(s)",
        String::from_utf8_lossy(&weather),
        store.entities.len(),
        defs.len(),
    );
}

fn track_weather_particles(
    cam: Query<&GlobalTransform, With<crate::camera::OperatorCamera>>,
    mut sim: ResMut<ParticleSimulator>,
) {
    let Some(cam) = cam.iter().next() else {
        return;
    };
    sim.set_camera_relative_origins(cam.translation());
}

pub struct WeatherParticlesPlugin;

impl Plugin for WeatherParticlesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WeatherParticles>().add_systems(
            Update,
            (sync_weather_particles, track_weather_particles)
                .chain()
                // The weather key comes from the set sample_zone_weather selects; unordered,
                // zone-in reads the whole DAT once against the `suny` fallback and again the
                // next frame.
                .after(crate::weather::WeatherSampleSet)
                // The simulator bakes each generator's world positions into its mesh, so the
                // camera-relative origins have to land before it rebuilds.
                .before(crate::particle_sim::sync_particle_meshes),
        );
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ffxi_dat::zone_dat::ZONE_DAT_TABLE;

    // La Theine Plateau. Its weat/rain is the reference set: one camera-following curtain
    // (`~1ra`), one camera-following mist puff (`rai2`), six placed mist patches (`~1r1..6`) and
    // five placed ground splashes (`~1h1..5`) — plus a cld2/~4cl canopy pair that belongs to
    // zone_clouds and must not appear here.
    const LA_THEINE_ZONE_DAT: u32 = 202;

    pub(crate) fn zone_dat(file_id: u32) -> Option<Vec<u8>> {
        let root = DatRoot::from_env_or_default().ok()?;
        let loc = root.resolve(file_id).ok()?;
        std::fs::read(loc.path_under(&root)).ok()
    }

    // Every Generator chunk anywhere under weat/, whatever its weather tag — the set no other
    // module may claim.
    pub(crate) fn weat_generator_names(bytes: &[u8]) -> Vec<[u8; 4]> {
        fn walk(node: &ChunkNode<'_>, in_weat: bool, out: &mut Vec<[u8; 4]>) {
            for child in &node.children {
                let c = &child.chunk;
                if !child.children.is_empty() || c.kind == ChunkKind::Rmp as u8 {
                    walk(child, in_weat || c.name == WEAT_DIR, out);
                    continue;
                }
                if in_weat
                    && ffxi_dat::kind::ChunkKind::from_u8(c.kind)
                        == Some(ffxi_dat::kind::ChunkKind::Generator)
                {
                    out.push(c.name);
                }
            }
        }
        let mut out = Vec::new();
        walk(&ffxi_dat::chunk::walk_tree(bytes), false, &mut out);
        out
    }

    #[test]
    fn real_dat_la_theine_rain_set() {
        let Some(bytes) = zone_dat(LA_THEINE_ZONE_DAT) else {
            return;
        };
        let tree = ffxi_dat::chunk::walk_tree(&bytes);
        let weat = find_weat_type(&tree, *b"rain").expect("La Theine ships weat/rain");
        let defs = collect_precipitation(weat);
        let names: Vec<String> = defs
            .iter()
            .map(|(n, _)| String::from_utf8_lossy(n).into_owned())
            .collect();

        assert!(
            names.iter().any(|n| n == "~1ra"),
            "curtain missing: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "~1h1"),
            "splash missing: {names:?}"
        );
        assert!(names.iter().any(|n| n == "~1r6"), "mist missing: {names:?}");
        assert_eq!(defs.len(), 13, "{names:?}");

        // The two life==0 sheets in the same directory are canopies, not precipitation:
        // cld2 is zone_clouds', and ~4cl is drawn by nobody yet (kuluu-zi3t).
        for canopy in ["cld2", "~4cl"] {
            assert!(!names.iter().any(|n| n == canopy), "{canopy} claimed here");
        }
    }

    // Valkurm Dunes carries the dust storm: a single camera-attached batched sheet whose `hit3`
    // sprite sheet ships nowhere in the zone DAT at all — it lives in the global effect dir at
    // syst/effe/hit3. The zone-local assets alone drop the generator and the storm renders with
    // sound and no particles.
    const VALKURM_DUNES_ZONE_DAT: u32 = 203;

    #[test]
    fn real_dat_dust_storm_sheet_lives_in_the_global_effect_dir() {
        let Some(bytes) = zone_dat(VALKURM_DUNES_ZONE_DAT) else {
            return;
        };
        let tree = ffxi_dat::chunk::walk_tree(&bytes);
        let Some(weat) = find_weat_type(&tree, *b"dust") else {
            return;
        };
        if weat.chunk.name != *b"dust" {
            return;
        }
        let defs = collect_precipitation(weat);
        let names: Vec<String> = defs
            .iter()
            .map(|(n, _)| String::from_utf8_lossy(n).into_owned())
            .collect();
        assert_eq!(names, ["~1du"], "{names:?}");
        let dust = defs[0].1;
        assert!(dust.camera_attached_base && !dust.follow_camera);
        assert_eq!(&dust.mesh_id, b"hit3");

        let (_s, scoped) = parse_action_tree(weat);
        let (_s, whole) = crate::scheduler_runtime::parse_action_bytes(&bytes);
        for zone_tier in [&scoped, &whole] {
            assert!(!zone_tier.sprite_sheets.contains_key(b"hit3"));
            assert!(!zone_tier.mmbs.contains_key(b"hit3"));
        }

        let Some(global) = zone_dat(crate::scheduler_runtime::GLOBAL_EFFECT_DIR_FILE_ID) else {
            return;
        };
        let (_s, global) = crate::scheduler_runtime::parse_action_bytes(&global);
        assert!(
            global.sprite_sheets.contains_key(b"hit3"),
            "the dust sheet resolves only against the global effect dir"
        );
        // The alpha and time-of-day tint curves stay zone-local, so the two tiers have to be
        // searched per link, not picked once for the whole generator.
        assert!(scoped.keyframes.contains_key(b"kdus"));
        assert!(!global.keyframes.contains_key(b"kdus"));
    }

    // Chunk ids repeat across weat/<tag> subtrees inside one DAT, so resolving assets against the
    // whole file binds the wrong mesh. Scoping to the tag's own subtree is what keeps the rain
    // curtain pointing at the rain sprite sheet.
    #[test]
    fn real_dat_weather_assets_are_tag_scoped() {
        let Some(bytes) = zone_dat(LA_THEINE_ZONE_DAT) else {
            return;
        };
        let tree = ffxi_dat::chunk::walk_tree(&bytes);
        let weat = find_weat_type(&tree, *b"rain").unwrap();
        let (_s, scoped) = parse_action_tree(weat);
        let (_s, whole) = crate::scheduler_runtime::parse_action_bytes(&bytes);
        assert!(scoped.sprite_sheets.contains_key(b"rain"));
        assert!(
            scoped.particle_defs.len() < whole.particle_defs.len(),
            "tag-scoped assets must be a strict subset of the DAT's"
        );
    }

    // The lens-flare chain hangs its generators below `lf0*`/`lens`, not directly inside them, so
    // the exclusion has to survive the descent. Drawing them here would put lens_flare.rs's
    // screen-space chain into the world a second time.
    #[test]
    fn real_dat_lens_flare_subtrees_are_never_claimed() {
        fn flare_generators(node: &ChunkNode<'_>, inside: bool, out: &mut Vec<[u8; 4]>) {
            for child in &node.children {
                let c = &child.chunk;
                if !child.children.is_empty() || c.kind == ChunkKind::Rmp as u8 {
                    flare_generators(child, inside || owned_elsewhere(c.name), out);
                    continue;
                }
                if inside
                    && ffxi_dat::kind::ChunkKind::from_u8(c.kind)
                        == Some(ffxi_dat::kind::ChunkKind::Generator)
                {
                    out.push(c.name);
                }
            }
        }

        if DatRoot::from_env_or_default().is_err() {
            return;
        }
        let mut checked = 0;
        for &(_zone, file_id) in ZONE_DAT_TABLE {
            let Some(bytes) = zone_dat(file_id) else {
                continue;
            };
            let tree = ffxi_dat::chunk::walk_tree(&bytes);
            for tag in [*b"rain", *b"squl", *b"snow", *b"bliz", *b"thdr", *b"bolt"] {
                let Some(weat) = find_weat_type(&tree, tag) else {
                    continue;
                };
                let mut flares = Vec::new();
                flare_generators(weat, false, &mut flares);
                if flares.is_empty() {
                    continue;
                }
                checked += 1;
                for (name, _) in collect_precipitation(weat) {
                    assert!(
                        !flares.contains(&name),
                        "DAT {file_id} weat/{}: claimed lens-flare generator {}",
                        String::from_utf8_lossy(&tag),
                        String::from_utf8_lossy(&name)
                    );
                }
            }
            if checked > 20 {
                break;
            }
        }
        assert!(
            checked > 0,
            "no precipitation tag ships a lens-flare subtree"
        );
    }

    // Every precipitation weather must resolve to a non-empty set somewhere, or deleting the
    // hand-authored placeholder would leave that weather with no visible precipitation at all.
    #[test]
    fn real_dat_every_precipitation_tag_ships_generators() {
        let Ok(root) = DatRoot::from_env_or_default() else {
            return;
        };
        let mut found: Vec<WeatherTypeId> = Vec::new();
        for &(_zone, file_id) in ZONE_DAT_TABLE {
            let Ok(loc) = root.resolve(file_id) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
                continue;
            };
            let tree = ffxi_dat::chunk::walk_tree(&bytes);
            for tag in [*b"rain", *b"squl", *b"snow", *b"bliz", *b"thdr", *b"bolt"] {
                if found.contains(&tag) {
                    continue;
                }
                // find_weat_type falls back to `suny`; only an exact hit proves the tag ships.
                let Some(weat) = find_weat_type(&tree, tag) else {
                    continue;
                };
                if weat.chunk.name == tag && !collect_precipitation(weat).is_empty() {
                    found.push(tag);
                }
            }
            if found.len() == 6 {
                return;
            }
        }
        panic!("weather tags with no precipitation generators anywhere: {found:?}");
    }
}
