#![cfg(not(target_arch = "wasm32"))]

use ffxi_dat::ChunkKind;
use std::collections::HashMap;

use bevy::prelude::*;
use ffxi_dat::chunk::ChunkNode;
use ffxi_dat::particle_gen::{AttachType, SoundGeneratorDef};
use ffxi_dat::sep::Sep;
use ffxi_dat::weather::WeatherTypeId;
use kuluu_snapshot::Vec3 as WireVec3;

use crate::audio::{
    sfx_attenuation_calc3d, AudioMuteState, BgmSlots, PcmAudio, SfxCache, ATTACHED_VERTICAL_WEIGHT,
    UNATTACHED_VERTICAL_WEIGHT,
};
use crate::camera::OperatorCamera;
use crate::components::InGameEntity;
use crate::particle_sim::ActorAutoRunEffects;
use crate::scene::mzb_to_bevy;
use crate::scheduler_runtime::ActionAssets;
use crate::scheduler_runtime::RETAIL_FPS;
use crate::snapshot::{effective_zone_file_id, SceneState};
use crate::weather_particles::WEAT_DIR;
use crate::zone_clouds::find_weat_type;

/// A DAT-placed sound emitter.
///
/// research/XIClient/src/XIClient/source/World/Generator/Effects/CYySoundElem.cpp CYySoundElem::OnPlayUpdate
/// `OnPlayUpdate` re-runs Calc3D every frame; a looping cue is stopped rather than
/// destroyed when the listener leaves `far`, so it can start again on the way back.
#[derive(Component, Debug)]
pub struct ZonePlacedSfx {
    se_id: u32,
    loops: bool,
    /// CYyGenerator.cpp CYyGenerator::Idle — a "never" generator holds one live cue and re-emits only
    /// once it is gone. Every shipped one-shot emitter with a sub-10-frame period is one of
    /// these (58 corpus-wide, most authoring a 1-frame period), so without it they would
    /// re-fire 30 times a second.
    singleton: bool,
    near: f32,
    far: f32,
    origin: Vec3,
    // An actor-attached emitter (CYyGenerator attach type SourceActor) mixes from its own
    // GlobalTransform, a child of the actor root; a zone placement mixes from `origin`.
    attached: bool,
    frames_per_emission: f32,
    emission_variance: f32,
    countdown_frames: f32,
    rng: u64,
    audio: Option<Entity>,
}

/// research/XIClient/src/XIClient/source/World/Weather/WeatherTransition.cpp WeatherTransition::WeatherTransition activates the generators
/// under the live `weat/<tag>` container and the destructor (:122-145) deactivates them, so
/// those emitters exist only while that weather does. The zone's own generators live as
/// long as the zone.
#[derive(Resource, Default)]
pub struct ZoneSfx {
    zone_key: Option<Option<u32>>,
    weather_key: Option<(Option<u32>, WeatherTypeId)>,
    zone_entities: Vec<Entity>,
    weather_entities: Vec<Entity>,
}

impl ZoneSfx {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

// WeatherTransition.cpp ActivateWeatherGenerators gates activation on the generator's auto-run bit. The rest is
// ours: an emitter with no base position has no world placement to mix from (those are the
// scheduler-driven cues, e.g. `s_ju/weat/clod/tobi/naki`), and an actor-attached emitter
// rides a target this module does not own.
fn is_zone_placed(def: &SoundGeneratorDef) -> bool {
    def.auto_run && def.is_placed() && def.attach_type == AttachType::None
}

// A generator names its Sep by a DatId that is only unique within its own chunk directory:
// 308 Sep names in West Ronfaure alone bind to more than one se id. Measured over every
// shipped zone DAT, 5,832 of 5,895 generators resolve in their own directory and the
// remaining 63 only through a whole-file name lookup.
fn flat_sep_index(node: &ChunkNode<'_>, out: &mut HashMap<[u8; 4], Sep>) {
    for child in &node.children {
        if ffxi_dat::kind::ChunkKind::from_u8(child.chunk.kind)
            == Some(ffxi_dat::kind::ChunkKind::Sep)
        {
            if let Ok(sep) = Sep::parse(child.chunk.name, child.chunk.data) {
                out.insert(child.chunk.name, sep);
            }
        }
        flat_sep_index(child, out);
    }
}

fn collect_placed_sounds(
    node: &ChunkNode<'_>,
    flat: &HashMap<[u8; 4], Sep>,
    skip_weat: bool,
    out: &mut Vec<(SoundGeneratorDef, Sep)>,
) {
    let siblings: HashMap<[u8; 4], Sep> = node
        .children
        .iter()
        .filter(|c| {
            ffxi_dat::kind::ChunkKind::from_u8(c.chunk.kind) == Some(ffxi_dat::kind::ChunkKind::Sep)
        })
        .filter_map(|c| {
            Sep::parse(c.chunk.name, c.chunk.data)
                .ok()
                .map(|s| (c.chunk.name, s))
        })
        .collect();

    for child in &node.children {
        let c = &child.chunk;
        if !child.children.is_empty() || c.kind == ChunkKind::Rmp as u8 {
            if !(skip_weat && c.name == WEAT_DIR) {
                collect_placed_sounds(child, flat, skip_weat, out);
            }
            continue;
        }
        if ffxi_dat::kind::ChunkKind::from_u8(c.kind) != Some(ffxi_dat::kind::ChunkKind::Generator)
        {
            continue;
        }
        let Ok(Some(def)) = SoundGeneratorDef::parse(c.data) else {
            continue;
        };
        if !is_zone_placed(&def) {
            continue;
        }
        let Some(sep) = siblings.get(&def.sep_id).or_else(|| flat.get(&def.sep_id)) else {
            continue;
        };
        out.push((def, *sep));
    }
}

/// Seeded per emitter so a row of identical bird calls does not fire in
/// lockstep. The countdown is drawn from the same window as the re-emission
/// period rather than started at zero, or every in-range emitter in the zone
/// fires together on the first tick after zone-in (West Ronfaure ships 30+
/// bird calls) and only de-syncs afterwards.
fn emitter(
    def: &SoundGeneratorDef,
    sep: &Sep,
    origin: Vec3,
    attached: bool,
    index: usize,
) -> ZonePlacedSfx {
    let mut rng = SFX_RNG_SEED ^ (index as u64).wrapping_mul(SFX_RNG_STRIDE);
    let countdown_frames = next_unit(&mut rng) * (def.frames_per_emission + def.emission_variance);
    ZonePlacedSfx {
        se_id: sep.se_id,
        loops: sep.loops(),
        singleton: def.is_singleton(),
        near: def.near,
        far: def.far,
        origin,
        attached,
        frames_per_emission: def.frames_per_emission,
        emission_variance: def.emission_variance,
        countdown_frames,
        rng,
        audio: None,
    }
}

fn spawn_emitters(
    defs: &[(SoundGeneratorDef, Sep)],
    commands: &mut Commands,
    out: &mut Vec<Entity>,
) {
    for (index, (def, sep)) in defs.iter().enumerate() {
        let bp = def.base_position;
        let origin = mzb_to_bevy(WireVec3 {
            x: bp[0],
            y: bp[1],
            z: bp[2],
        });
        out.push(
            commands
                .spawn((InGameEntity, emitter(def, sep, origin, false, index)))
                .id(),
        );
    }
}

// research/xim Actor.kt startAutoRunParticles starts every auto-run generator of a model at
// model-ready, the Sep-linked ones included: the Home Point's `snd0` (Sep 9013, a loop) is
// what hums at the crystal. Actor-attached, so it rides the actor root rather than a zone
// placement; a placed or unattached one belongs to the zone pass instead.
pub fn actor_auto_run_sounds(assets: &ActionAssets) -> Vec<(SoundGeneratorDef, Sep)> {
    let mut out: Vec<(SoundGeneratorDef, Sep)> = assets
        .sound_defs
        .iter()
        .filter(|(_, def)| def.auto_run && def.attach_type == AttachType::SourceActor)
        .filter_map(|(name, def)| {
            assets
                .seps
                .get(&def.sep_id)
                .map(|sep| (*name, (*def, *sep)))
        })
        .map(|(_, pair)| pair)
        .collect();
    out.sort_by_key(|(def, sep)| (sep.se_id, def.sep_id));
    out
}

/// The base position is in the actor's DAT-local frame; the root's transform
/// carries the FFXI->Bevy basis, as it does for the actor's particle meshes.
fn spawn_actor_auto_run_sounds(
    q_added: Query<(Entity, &ActorAutoRunEffects), Added<ActorAutoRunEffects>>,
    mut commands: Commands,
) {
    for (actor_root, fx) in &q_added {
        for (index, (def, sep)) in actor_auto_run_sounds(&fx.assets).iter().enumerate() {
            let local = Vec3::from_array(def.base_position);
            commands.spawn((
                InGameEntity,
                ChildOf(actor_root),
                Transform::from_translation(local),
                emitter(def, sep, local, true, index),
            ));
        }
    }
}

const SFX_RNG_SEED: u64 = crate::scheduler_runtime::SPLITMIX64_GOLDEN_RATIO;
const SFX_RNG_STRIDE: u64 = 0x0000_0100_0000_01B3;

// The draw is the high word, so it spans the whole unit interval rather than the lower half.
fn next_unit(state: &mut u64) -> f32 {
    *state = crate::scheduler_runtime::lcg_next(*state);
    (*state >> u32::BITS) as u32 as f32 / u32::MAX as f32
}

fn sync_zone_sfx(
    scene_state: Res<SceneState>,
    zone_weather: Res<crate::weather::ZoneWeather>,
    mut store: ResMut<ZoneSfx>,
    mut commands: Commands,
    dat_root: Res<crate::dat_root::SharedDatRoot>,
) {
    let Some(root) = dat_root.get() else {
        return;
    };
    let file_id = effective_zone_file_id(&scene_state.snapshot);
    let weather = zone_weather
        .active_weather_type()
        .unwrap_or(ffxi_dat::weather::WEATHER_TYPE_FALLBACK);

    let zone_stale = store.zone_key != Some(file_id);
    let weather_stale = store.weather_key != Some((file_id, weather));
    if !zone_stale && !weather_stale {
        return;
    }

    // OnExit(InGame) does not fire on a zone warp, so the previous set is despawned here.
    if zone_stale {
        for e in store.zone_entities.drain(..) {
            commands.entity(e).try_despawn();
        }
        store.zone_key = Some(file_id);
    }
    if weather_stale {
        for e in store.weather_entities.drain(..) {
            commands.entity(e).try_despawn();
        }
        store.weather_key = Some((file_id, weather));
    }

    let Some(bytes) = file_id.and_then(|id| {
        root.resolve(id)
            .ok()
            .and_then(|loc| std::fs::read(loc.path_under(root)).ok())
    }) else {
        return;
    };

    let tree = ffxi_dat::chunk::walk_tree(&bytes);
    let mut flat = HashMap::new();
    flat_sep_index(&tree, &mut flat);

    if zone_stale {
        let mut defs = Vec::new();
        collect_placed_sounds(&tree, &flat, true, &mut defs);
        spawn_emitters(&defs, &mut commands, &mut store.zone_entities);
        info!(
            "zone_sfx: DAT {file_id:?} → {} placed emitter(s)",
            store.zone_entities.len()
        );
    }
    if weather_stale {
        if let Some(weat) = find_weat_type(&tree, weather) {
            let mut defs = Vec::new();
            collect_placed_sounds(weat, &flat, false, &mut defs);
            spawn_emitters(&defs, &mut commands, &mut store.weather_entities);
            if !store.weather_entities.is_empty() {
                info!(
                    "zone_sfx: DAT {file_id:?} weat/{} → {} placed emitter(s)",
                    String::from_utf8_lossy(&weather),
                    store.weather_entities.len()
                );
            }
        }
    }
}

fn update_zone_sfx(
    time: Res<Time>,
    slots: Res<BgmSlots>,
    mute: Res<AudioMuteState>,
    listener: Query<&GlobalTransform, With<OperatorCamera>>,
    mut cache: ResMut<SfxCache>,
    mut pcm_assets: ResMut<Assets<PcmAudio>>,
    mut emitters: Query<(Entity, &mut ZonePlacedSfx, Option<&GlobalTransform>)>,
    mut sinks: Query<&mut bevy::audio::AudioSink>,
    playing: Query<(), With<AudioPlayer<PcmAudio>>>,
    mut commands: Commands,
) {
    if emitters.is_empty() {
        return;
    }
    let Some(install) = slots.install_root.clone() else {
        return;
    };
    // Calc3D measures from `CameraManager::CachedEyePosition` (CYySepRes.cpp CYySepRes::Calc3D), not from
    // the player — unlike the entity-swing cues, whose cutoff is LSB's player-measured
    // streaming radius (see `sfx_attenuation`).
    let Some(eye) = listener.iter().next().map(|t| t.translation()) else {
        return;
    };
    let frames = time.delta_secs() * RETAIL_FPS;

    // The playing cue is a CHILD of its emitter: despawn is recursive, so a zone warp or an
    // OnExit(InGame) that reaps the emitter cannot leave a waterfall looping in the void.
    for (emitter, mut em, xf) in emitters.iter_mut() {
        let (origin, vertical_weight) = if em.attached {
            (
                xf.map(|t| t.translation()).unwrap_or(em.origin),
                ATTACHED_VERTICAL_WEIGHT,
            )
        } else {
            (em.origin, UNATTACHED_VERTICAL_WEIGHT)
        };
        let gain = if mute.sfx {
            0.0
        } else {
            sfx_attenuation_calc3d(eye, origin, em.near, em.far, vertical_weight)
        };

        if em.loops {
            match (em.audio, gain > 0.0) {
                (Some(a), true) => {
                    if let Ok(mut sink) = sinks.get_mut(a) {
                        sink.set_volume(bevy::audio::Volume::Linear(gain));
                    }
                }
                (Some(a), false) => {
                    commands.entity(a).try_despawn();
                    em.audio = None;
                }
                (None, true) => {
                    let se_id = em.se_id;
                    if let Some(handle) = cache.handle(&install, &mut pcm_assets, se_id, true) {
                        em.audio = Some(
                            commands
                                .spawn((
                                    ChildOf(emitter),
                                    AudioPlayer(handle),
                                    PlaybackSettings::ONCE
                                        .with_volume(bevy::audio::Volume::Linear(gain)),
                                ))
                                .id(),
                        );
                    }
                }
                (None, false) => {}
            }
            continue;
        }

        if em.singleton {
            if em.audio.is_some_and(|a| playing.contains(a)) {
                continue;
            }
            em.audio = None;
        } else {
            em.countdown_frames -= frames;
            if em.countdown_frames > 0.0 {
                continue;
            }
            let jitter = next_unit(&mut em.rng) * em.emission_variance;
            em.countdown_frames = em.frames_per_emission + jitter;
        }
        if gain <= 0.0 {
            continue;
        }
        let se_id = em.se_id;
        if let Some(handle) = cache.handle(&install, &mut pcm_assets, se_id, false) {
            let cue = commands
                .spawn((
                    ChildOf(emitter),
                    AudioPlayer(handle),
                    PlaybackSettings::DESPAWN.with_volume(bevy::audio::Volume::Linear(gain)),
                ))
                .id();
            if em.singleton {
                em.audio = Some(cue);
            }
        }
    }
}

pub struct ZoneSfxPlugin;

impl Plugin for ZoneSfxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ZoneSfx>().add_systems(
            Update,
            (sync_zone_sfx, spawn_actor_auto_run_sounds, update_zone_sfx)
                .chain()
                // The weather key comes from the set sample_zone_weather selects; unordered,
                // zone-in reads the whole DAT once against the `suny` fallback and again the
                // next frame. The in-game gate keeps the launcher backdrop's mirrored zone id
                // from spawning a zone's emitters behind the character-select screen.
                .after(crate::weather::WeatherSampleSet)
                .run_if(crate::camera::in_game),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEST_RONFAURE_ZONE_DAT: u32 = 200;
    // Ru'Lude Gardens: its weat/thdr ships the only placed, auto-run thunderclap emitters
    // (`set1`/`set2`, far 200 / near 150) — permanent scenery if the weather gate is lost.
    const RULUDE_GARDENS_ZONE_DAT: u32 = 101;

    fn zone_dat(file_id: u32) -> Option<Vec<u8>> {
        let root = ffxi_dat::archive::open_test_install()?;
        let loc = root.resolve(file_id).ok()?;
        std::fs::read(loc.path_under(&root)).ok()
    }

    fn placed(bytes: &[u8], skip_weat: bool) -> Vec<(SoundGeneratorDef, Sep)> {
        let tree = ffxi_dat::chunk::walk_tree(bytes);
        let mut flat = HashMap::new();
        flat_sep_index(&tree, &mut flat);
        let mut out = Vec::new();
        collect_placed_sounds(&tree, &flat, skip_weat, &mut out);
        out
    }

    const HOME_POINT_MODEL_DAT: u32 = 1351;
    const HOME_POINT_AMBIENT_SE: u32 = 9013;

    fn sep_body(se_id: u32, flags: u32) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[8..12].copy_from_slice(&se_id.to_le_bytes());
        body[12..16].copy_from_slice(&flags.to_le_bytes());
        body
    }

    fn actor_assets(
        defs: &[([u8; 4], SoundGeneratorDef)],
        seps: &[([u8; 4], u32)],
    ) -> ActionAssets {
        let mut assets = ActionAssets::default();
        for (name, def) in defs {
            assets.sound_defs.insert(*name, *def);
        }
        for (name, se_id) in seps {
            assets
                .seps
                .insert(*name, Sep::parse(*name, &sep_body(*se_id, 0)).unwrap());
        }
        assets
    }

    /// Only an auto-run, source-attached Sep generator rides the actor; a zone
    /// placement or a routine-driven cue is someone else's.
    #[test]
    fn actor_auto_run_sounds_take_the_attached_auto_run_generators() {
        let attached = SoundGeneratorDef {
            sep_id: *b"9013",
            auto_run: true,
            attach_type: AttachType::SourceActor,
            far: 20.0,
            ..Default::default()
        };
        let routine_driven = SoundGeneratorDef {
            auto_run: false,
            ..attached
        };
        let placed = SoundGeneratorDef {
            attach_type: AttachType::None,
            base_position: [1.0, 2.0, 3.0],
            ..attached
        };
        let unresolved = SoundGeneratorDef {
            sep_id: *b"none",
            ..attached
        };
        let assets = actor_assets(
            &[
                (*b"snd0", attached),
                (*b"snd1", routine_driven),
                (*b"snd2", placed),
                (*b"snd3", unresolved),
            ],
            &[(*b"9013", HOME_POINT_AMBIENT_SE)],
        );
        let out = actor_auto_run_sounds(&assets);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, attached);
        assert_eq!(out[0].1.se_id, HOME_POINT_AMBIENT_SE);
    }

    /// ROM/3/25.DAT: `snd0` links Sep `9013` (loop flag set), auto-run, attached to the actor.
    #[test]
    fn home_point_model_carries_its_ambient_loop() {
        let Some(bytes) = zone_dat(HOME_POINT_MODEL_DAT) else {
            return;
        };
        let (_, assets, _) = crate::scheduler_runtime::parse_action_bytes(&bytes);
        let out = actor_auto_run_sounds(&assets);
        assert_eq!(out.len(), 1, "{:?}", out);
        let (def, sep) = out[0];
        assert_eq!(sep.se_id, HOME_POINT_AMBIENT_SE);
        assert!(sep.loops());
        assert!(def.is_singleton());
        assert_eq!(def.far, 20.0);
    }

    // The waterfall spray is a looping cue and the bird calls are one-shots; the Sep loop
    // bit is what separates them without opening 2,947 sound files at zone load.
    #[test]
    fn real_dat_west_ronfaure_placed_emitters_split_by_the_sep_loop_bit() {
        const WATERFALL_SE: u32 = 2024;
        const BIRD_SE: [u32; 2] = [2081, 2084];

        let Some(bytes) = zone_dat(WEST_RONFAURE_ZONE_DAT) else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        let found = placed(&bytes, true);
        let waterfalls: Vec<_> = found
            .iter()
            .filter(|(_, s)| s.se_id == WATERFALL_SE)
            .collect();
        assert_eq!(waterfalls.len(), 8, "f_ro/mode/ligh/taki/sef1..sef8");
        for (def, sep) in &waterfalls {
            assert!(sep.loops(), "the waterfall spray is a looping cue");
            assert_eq!((def.far, def.near), (30.0, 3.0));
        }

        let birds = found
            .iter()
            .filter(|(_, s)| BIRD_SE.contains(&s.se_id))
            .count();
        assert!(birds >= 30, "f_ro/effe/aose/ma*,mb* — got {birds}");
        for (_, sep) in found.iter().filter(|(_, s)| BIRD_SE.contains(&s.se_id)) {
            assert!(!sep.loops(), "the bird calls are one-shots");
        }
    }

    // Weather-scoped emitters must never enter the always-on set, or San d'Oria's rooftops
    // get permanent thunderclaps.
    #[test]
    fn real_dat_weather_emitters_are_excluded_from_the_zone_set() {
        const THUNDERCLAP_SE: u32 = 2020;

        let Some(bytes) = zone_dat(RULUDE_GARDENS_ZONE_DAT) else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        assert!(
            !placed(&bytes, true)
                .iter()
                .any(|(_, s)| s.se_id == THUNDERCLAP_SE),
            "weat/thdr emitters leaked into the zone-static set"
        );

        let tree = ffxi_dat::chunk::walk_tree(&bytes);
        let mut flat = HashMap::new();
        flat_sep_index(&tree, &mut flat);
        let weat = find_weat_type(&tree, *b"thdr").expect("s_ju ships weat/thdr");
        let mut thunder = Vec::new();
        collect_placed_sounds(weat, &flat, false, &mut thunder);
        assert_eq!(thunder.len(), 2, "weat/thdr/set1 and set2");
        for (def, sep) in &thunder {
            assert_eq!(sep.se_id, THUNDERCLAP_SE);
            assert!(!sep.loops());
            assert_eq!((def.far, def.near), (200.0, 150.0));
        }
    }

    // `s_ju/weat/clod/tobi/naki` and its siblings are scheduler-driven: no base position and
    // no auto-run bit. Spawning them as world emitters would put a bird call at the origin.
    #[test]
    fn real_dat_unplaced_and_manual_emitters_are_rejected() {
        let Some(bytes) = zone_dat(RULUDE_GARDENS_ZONE_DAT) else {
            eprintln!("skipping: no FFXI install");
            return;
        };
        for (def, _) in placed(&bytes, true) {
            assert!(def.is_placed() && def.auto_run, "{def:?}");
        }
    }

    #[test]
    fn sep_resolution_prefers_the_generators_own_directory() {
        let mut flat = HashMap::new();
        flat.insert(
            *b"0111",
            Sep {
                name: *b"0111",
                se_id: 100_011,
                flags: 0,
            },
        );
        let local = Sep {
            name: *b"0111",
            se_id: 100_001,
            flags: 0,
        };
        let mut siblings = HashMap::new();
        siblings.insert(*b"0111", local);
        assert_eq!(
            siblings
                .get(b"0111")
                .or_else(|| flat.get(b"0111"))
                .unwrap()
                .se_id,
            100_001
        );
        assert_eq!(flat.get(b"0111").unwrap().se_id, 100_011);
    }

    // The re-emission period is `frames_per_emission + uirand(emission_variance)`
    // (CYyGenerator.cpp CYyGenerator::Idle), so the jitter draw must stay in the unit interval or a bird
    // call lands outside the authored window.
    #[test]
    fn emission_jitter_draw_stays_in_the_unit_interval() {
        let mut state = SFX_RNG_SEED;
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for _ in 0..1024 {
            let u = next_unit(&mut state);
            assert!((0.0..=1.0).contains(&u), "{u}");
            min = min.min(u);
            max = max.max(u);
        }
        assert!(min < 0.05 && max > 0.95, "draw range [{min}, {max}]");
    }

    // The whole reason the singleton rule has to exist: 58 shipped one-shot emitters author a
    // 1-frame emission period, and every last one of them is a "never" generator that holds a
    // single live cue. Running them on the timed cadence instead would re-fire each 30 times a
    // second. Conversely no timed one-shot authors a period under 10 frames.
    #[test]
    fn real_dat_every_sub_ten_frame_one_shot_is_a_singleton() {
        const MIN_TIMED_PERIOD_FRAMES: f32 = 10.0;
        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut seen = std::collections::HashSet::new();
        let mut singletons = 0usize;
        let mut timed = 0usize;
        for &(_zone, file_id) in ffxi_dat::zone_dat::ZONE_DAT_TABLE {
            if !seen.insert(file_id) {
                continue;
            }
            let Ok(loc) = root.resolve(file_id) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(loc.path_under(&root)) else {
                continue;
            };
            for (def, sep) in placed(&bytes, true) {
                if sep.loops() {
                    continue;
                }
                if def.is_singleton() {
                    singletons += 1;
                    continue;
                }
                timed += 1;
                assert!(
                    def.frames_per_emission + def.emission_variance >= MIN_TIMED_PERIOD_FRAMES,
                    "DAT {file_id} se{:06} re-fires every {} frames",
                    sep.se_id,
                    def.frames_per_emission
                );
            }
        }
        assert!(singletons > 0 && timed > 0, "{singletons} / {timed}");
    }

    // Zone-scoped lifecycle: a zone warp keeps AppPhase::InGame, so the previous zone's
    // emitters — and the looping cues hanging off them — have to be despawned here or a
    // waterfall keeps running in the new zone.
    #[test]
    fn real_dat_zone_change_despawns_the_previous_zones_emitters() {
        const WEST_RONFAURE: u16 = 100;
        const LA_THEINE: u16 = 102;

        let Some(root) = ffxi_dat::archive::open_test_install() else {
            return;
        };
        let mut app = App::new();
        app.init_resource::<SceneState>()
            .init_resource::<crate::weather::ZoneWeather>()
            .init_resource::<ZoneSfx>()
            .insert_resource(crate::dat_root::SharedDatRoot(Some(std::sync::Arc::new(
                root,
            ))))
            .add_systems(Update, sync_zone_sfx);

        let mut enter = |zone: u16| {
            app.world_mut()
                .resource_mut::<SceneState>()
                .snapshot
                .zone_id = Some(zone);
            app.update();
            app.world_mut()
                .query::<&ZonePlacedSfx>()
                .iter(app.world())
                .count()
        };

        let first = enter(WEST_RONFAURE);
        assert!(first > 0, "West Ronfaure ships placed sound generators");

        let second = enter(LA_THEINE);
        assert!(second > 0, "La Theine ships placed sound generators");
        assert_eq!(
            app.world().resource::<ZoneSfx>().zone_entities.len(),
            second,
            "the store must hold exactly the new zone's emitters"
        );

        let mut store = app.world_mut().resource_mut::<ZoneSfx>();
        store.clear();
        assert!(store.zone_entities.is_empty() && store.weather_key.is_none());
    }
}
