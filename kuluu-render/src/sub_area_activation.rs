//! Automatic sub-area interior activation: the town-shop / guild / house
//! interiors a zone swaps in place of its closed-up exterior shell as the player
//! walks through the doorway, with no server-side zone change.
//!
//! Retail keeps the main zone and one sub-area block loaded at once
//! (`MAX_ZONE_LOAD_COUNT`, see [`crate::dat_mzb::ZONE_BLOCK_SLOTS`]) and selects
//! the sub-area from a latch its trigger rects drive, not from a containment
//! test — see [`ffxi_dat::sub_area::SubAreaLatch`] for why position alone cannot
//! answer it. This module owns [`crate::dat_mzb::ZONE_SLOT_SUB_AREA`]: it is the
//! only place that issues or retires a load for that slot, so the interior and
//! the shell it replaces can never both be up.

#![cfg(not(target_arch = "wasm32"))]

use bevy::prelude::*;
use ffxi_dat::sub_area::{self, SubAreaLatch, SubAreaShell};
use ffxi_dat::zone_interaction::ZoneInteraction;

use crate::dat_mzb::{LoadMzbRequest, ZoneBlockRetire, ZONE_SLOT_SUB_AREA};
use crate::snapshot::SceneState;

/// Snapshot positions are Bevy-order (`y` is ground-plane depth, `z` is height)
/// while the RID rects keep the DAT's own order (`[1]` is height, `[2]` is
/// depth). Feeding one to the other silently misses every trigger, so the swap
/// lives here rather than at the call site — the same remap
/// `reactor::is_inside_dat_obb` open-codes.
fn dat_native(p: kuluu_snapshot::Vec3) -> [f32; 3] {
    [p.x, p.z, p.y]
}

/// The sub-area the player just entered or left, for the c2s `0x0F2`
/// SubMapChange report. `None` is `submap::NO_SUB_AREA`.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubAreaChanged {
    pub sub_area: Option<u32>,
}

/// Puts the latch into a sub-area without a trigger crossing (`/subarea`). The
/// ordinary clear rule applies from there, so the interior still drops when the
/// player walks clear of it.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetSubArea {
    pub sub_area: Option<u32>,
}

#[derive(Resource, Default)]
pub struct SubAreaActivation {
    zone_file_id: Option<u32>,
    latch: Option<SubAreaLatch>,
    active: Option<u32>,

    /// Ascending ids this install ships an interior DAT for, resolved on the
    /// task pool by [`crate::dat_mzb::build_zone_mmb_spawns`].
    loadable: Vec<u32>,

    /// Set when a latch is installed and cleared once the driver has applied the
    /// server's zone-in `SubMapNumber` to it.
    needs_seed: bool,
}

impl SubAreaActivation {
    /// Which interior is swapped in. Suppresses the shell it stands in for on
    /// both sides: visually through
    /// [`crate::dat_mzb::apply_sub_area_shell_visibility`] and
    /// [`crate::dat_mzb::select_zone_mmb_lod`], and physically through
    /// [`crate::dat_mzb::MzbCollisionGeometry::set_suppressed`].
    pub fn active(&self) -> Option<u32> {
        self.active
    }

    pub fn unloaded_interior_at(&self, native: [f32; 3]) -> bool {
        self.latch.as_ref().is_some_and(|latch| {
            latch
                .shells()
                .iter()
                .any(|shell| Some(shell.id) != self.active && shell.contains_inflated(native, 0.0))
        })
    }

    pub fn is_armed(&self) -> bool {
        self.latch.is_some()
    }

    /// Installs the zone's trigger rects and shells, read off the same DAT parse
    /// the placements come from. Called for the main block only; an interior's
    /// own DAT declares no sub-areas of its own.
    pub fn install_zone(
        &mut self,
        zone_file_id: u32,
        triggers: &[ZoneInteraction],
        shells: Vec<SubAreaShell>,
        loadable: Vec<u32>,
    ) {
        self.zone_file_id = Some(zone_file_id);
        self.latch = Some(SubAreaLatch::new(triggers, shells));
        self.active = None;
        self.loadable = loadable;
        self.needs_seed = true;
    }

    /// Whether this install ships the interior DAT for `id`. A zone that
    /// declares an interior the install does not carry keeps its shell up
    /// rather than opening a hole where the building was.
    fn is_loadable(&self, id: u32) -> bool {
        self.loadable.binary_search(&id).is_ok()
    }

    fn forget_zone(&mut self) {
        self.zone_file_id = None;
        self.latch = None;
        self.active = None;
        self.loadable.clear();
        self.needs_seed = false;
    }
}

/// Drives the latch from the player position, keeps
/// [`crate::dat_mzb::ZONE_SLOT_SUB_AREA`] holding whatever it selects, and
/// reports the change to the server.
///
/// Retires the outgoing interior in the same run that requests the incoming
/// one, so no `FixedUpdate` grounding tick lands between the two. The slot then
/// stands empty until the load does; that is survivable only because the parent
/// block floors the doorway approach and
/// [`crate::dat_mzb::MzbCollisionGeometry`] reads both slots as one surface.
pub fn drive_sub_area_activation(
    scene_state: Res<SceneState>,
    mut activation: ResMut<SubAreaActivation>,
    mut overrides: MessageReader<SetSubArea>,
    mut load_tx: MessageWriter<LoadMzbRequest>,
    mut changed_tx: MessageWriter<SubAreaChanged>,
    mut retire: ZoneBlockRetire,
) {
    let zone_file_id = crate::snapshot::effective_zone_file_id(&scene_state.snapshot);
    if zone_file_id != activation.zone_file_id && activation.is_armed() {
        activation.forget_zone();
        retire.retire(ZONE_SLOT_SUB_AREA);
        retire.collision_geometry.set_suppressed(None);
        return;
    }

    let forced = overrides.read().last().copied();
    let seed = std::mem::take(&mut activation.needs_seed).then(|| server_seed(&scene_state));
    let selected = {
        let Some(latch) = activation.latch.as_mut() else {
            return;
        };
        if let Some(seed) = seed {
            latch.set_active(seed);
        }
        if let Some(SetSubArea { sub_area }) = forced {
            latch.set_active(sub_area);
        }
        latch.update(dat_native(scene_state.snapshot.self_pos.pos))
    };
    if selected == activation.active {
        return;
    }

    // Drop it in the latch too, not just here: the trigger and shell keep
    // selecting the id every frame, so leaving the latch holding it re-runs this
    // whole block — a retire and two wire reports per frame.
    if selected.is_some_and(|id| !activation.is_loadable(id)) {
        warn!(
            sub_area = selected,
            "sub-area interior DAT is not in this install; its shell stays up"
        );
        if let Some(latch) = activation.latch.as_mut() {
            latch.set_active(None);
        }
        return;
    }

    retire.retire(ZONE_SLOT_SUB_AREA);
    // Must precede the load: suppressing after it leaves shell and interior both
    // solid for the frames the load takes.
    retire.collision_geometry.set_suppressed(selected);
    activation.active = selected;
    changed_tx.write(SubAreaChanged { sub_area: selected });

    let Some(id) = selected else {
        info!("sub-area: left the interior");
        return;
    };
    let file_id = sub_area::sub_area_file_id(id);
    info!(sub_area = id, file_id, "sub-area: entering interior");
    load_tx.write(LoadMzbRequest {
        file_id,
        chunk_idx: None,
        // The interior's placements are already expressed in the parent zone's
        // world space.
        world_pos: Vec3::ZERO,
        auto_loaded: true,
        slot: ZONE_SLOT_SUB_AREA,
        active_sub_area: Some(id),
    });
}

/// `PChar->loc.boundary` as the server hands it over in 0x00A LOGIN: which
/// interior the character was standing in when they zoned in or logged back on.
fn server_seed(scene_state: &SceneState) -> Option<u32> {
    scene_state
        .snapshot
        .sub_area
        .filter(|v| *v != NO_SUB_AREA)
        .map(u32::from)
}

/// Wire value for "not in a sub-area", matching
/// `ffxi_proto::map::submap::NO_SUB_AREA`. Kept as a local constant because
/// `kuluu-render` does not depend on `ffxi-proto`; the client-side forwarder
/// converts through the proto constant and
/// `no_sub_area_sentinel_matches_the_wire` pins the two together.
pub const NO_SUB_AREA: u16 = 0;

#[cfg(test)]
mod tests {
    use super::*;
    use ffxi_dat::datid::DatId;
    use ffxi_dat::zone_interaction::RECT_CLASS_HIT_CHECKED;

    pub(super) fn trigger(param: u32, position: [f32; 3], size: [f32; 3]) -> ZoneInteraction {
        ZoneInteraction {
            position,
            rect_class: RECT_CLASS_HIT_CHECKED,
            orientation: [0.0; 3],
            size,
            source_id: DatId(*b"m6t1"),
            dest_id: Some(DatId([0x20, 0, 0, 0])),
            param,
            terrain_flags: 0,
            map_id: 0,
            elevator_bottom_y: 0.0,
            elevator_top_y: 0.0,
        }
    }

    /// Konschtat Highlands. `zone_dat`'s table maps it to a DAT id at compile
    /// time, so the driver's zone-identity check holds with no install present.
    pub(super) const ZONE_ID: u16 = 108;
    pub(super) const SUB_AREA: u32 = 0x1C6;

    /// FFXI zone space. The doorway rect stands off from the shell it opens, the
    /// way the shipped ones do, so the latch is exercised across both edges.
    pub(super) const DOORWAY_CENTRE: [f32; 3] = [100.0, 0.0, 100.0];
    pub(super) const DOORWAY_SIZE: [f32; 3] = [4.0, 8.0, 4.0];
    pub(super) const SHELL_MIN: [f32; 3] = [95.0, -5.0, 95.0];
    pub(super) const SHELL_MAX: [f32; 3] = [125.0, 5.0, 125.0];
    pub(super) const DEEP_INSIDE: [f32; 3] = [115.0, 0.0, 110.0];
    /// Clear of the shell by more than [`sub_area::SUB_AREA_SHELL_CLEAR_MARGIN`].
    pub(super) const OUT_IN_THE_STREET: [f32; 3] = [0.0, 0.0, 0.0];

    pub(super) fn zone_file_id() -> u32 {
        ffxi_dat::zone_dat::effective_zone_dat_file_id(Some(ZONE_ID), None)
            .expect("the zone-DAT table maps this zone")
    }

    pub(super) fn armed_activation() -> SubAreaActivation {
        let mut activation = SubAreaActivation::default();
        activation.install_zone(
            zone_file_id(),
            &[trigger(SUB_AREA, DOORWAY_CENTRE, DOORWAY_SIZE)],
            vec![SubAreaShell {
                id: SUB_AREA,
                min: SHELL_MIN,
                max: SHELL_MAX,
            }],
            vec![SUB_AREA],
        );
        activation
    }

    #[test]
    fn a_zone_change_disarms_the_latch() {
        let mut activation = armed_activation();
        assert!(activation.is_armed());
        activation.forget_zone();
        assert!(!activation.is_armed());
        assert_eq!(activation.active(), None);
    }

    #[test]
    fn unloaded_shell_query_tracks_activation_replacement_and_reset() {
        let mut activation = armed_activation();
        assert!(activation.unloaded_interior_at(DEEP_INSIDE));
        assert!(!activation.unloaded_interior_at(OUT_IN_THE_STREET));
        activation.active = Some(SUB_AREA);
        assert!(!activation.unloaded_interior_at(DEEP_INSIDE));
        activation.active = None;
        assert!(activation.unloaded_interior_at(DEEP_INSIDE));
        activation.forget_zone();
        assert!(!activation.unloaded_interior_at(DEEP_INSIDE));
        activation = armed_activation();
        activation.install_zone(zone_file_id(), &[], Vec::new(), Vec::new());
        assert!(!activation.unloaded_interior_at(DEEP_INSIDE));
    }

    #[test]
    fn installing_a_zone_arms_a_seed() {
        assert!(armed_activation().needs_seed);
    }

    #[test]
    fn an_interior_this_install_does_not_ship_is_not_loadable() {
        let activation = armed_activation();
        assert!(activation.is_loadable(SUB_AREA));
        assert!(!activation.is_loadable(SUB_AREA + 1));
    }
}

/// The doorway crossing end to end: the driver, the two-slot collision read and
/// the per-slot retire, run as ECS systems over a stream of player positions.
#[cfg(test)]
mod doorway_tests {
    use super::tests::*;
    use super::*;
    use crate::dat_mmb::{LoadMmbRequest, MmbLoadQueue};
    use crate::dat_mzb::ground_tests::slab_block_at;
    use crate::dat_mzb::{
        AutoMzbOverlay, DrawDistance, LoadMzbInFlight, MzbCollisionBlock, MzbCollisionGeometry,
        PendingWaterSpawns, ZoneAreaMap, ZoneBlockSlot, ZoneChunkLightMap, ZoneGeomCache,
        MAX_GROUND_STEP_UP, ZONE_SLOT_MAIN,
    };
    use crate::ffxi_actor_render::LoadActorRequest;
    use crate::graphics_settings::GraphicsSettings;
    use crate::scene::TrackedEntities;
    use crate::snapshot::ToastEvent;
    use bevy::ecs::message::Messages;
    use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
    use bevy::tasks::{AsyncComputeTaskPool, TaskPool};

    /// Bevy-space collision columns. The town block floors the doorway approach
    /// and nothing else; the interior block floors only its own room — the
    /// measured split that makes replace-on-activate strand the player.
    const DOORWAY_COLUMN: Vec2 = Vec2::new(0.0, 0.0);
    const INTERIOR_COLUMN: Vec2 = Vec2::new(40.0, 0.0);
    const TOWN_FLOOR_Y: f32 = 1.0;
    const INTERIOR_FLOOR_Y: f32 = 3.0;
    /// The closed-up placeholder's surface, standing where the interior's own
    /// floor will be once the load lands.
    const SHELL_FLOOR_Y: f32 = 2.0;

    /// Main-block surface linked to the interior, i.e. the closed-up box the
    /// zone ships where the room will be: activating the sub-area suppresses it
    /// (`ffxi_dat::mzb::is_suppressed_placeholder`), so the column it floored
    /// keeps no floor until the interior lands.
    fn shell_placeholder_block() -> MzbCollisionBlock {
        let mut block = slab_block_at(INTERIOR_COLUMN, SHELL_FLOOR_Y);
        block.tri_sub_area = vec![SUB_AREA; block.indices.len() / 3];
        block
    }

    struct Doorway {
        app: App,
        town_geom: Entity,
        town_model: Entity,
    }

    impl Doorway {
        /// Just the driver, with the load requests it writes left on the queue.
        fn new() -> Self {
            let mut d = Self::bare();
            d.app.add_systems(Update, drive_sub_area_activation);
            d
        }

        /// The driver plus the rest of the plugin's request-to-task segment, run
        /// off the one ordering declaration production registers
        /// ([`crate::dat_mmb::zone_load_dispatch_systems`]), and with the main
        /// block reduced to the shell placeholder the interior stands in for.
        fn streaming() -> Self {
            AsyncComputeTaskPool::get_or_init(TaskPool::default);
            let mut d = Self::bare();
            d.app
                .add_message::<ToastEvent>()
                .add_message::<LoadMmbRequest>()
                .add_message::<LoadActorRequest>()
                .init_resource::<DrawDistance>()
                .init_resource::<ZoneGeomCache>()
                .init_resource::<TrackedEntities>()
                .init_resource::<GraphicsSettings>()
                .add_plugins(bevy::asset::AssetPlugin::default())
                .init_asset::<Mesh>()
                .init_asset::<StandardMaterial>()
                .add_systems(Update, crate::dat_mmb::zone_load_dispatch_systems());
            // The declaration has to *order* the segment, not merely contain it:
            // an unordered write-then-read of `LoadMzbRequest` is exactly the
            // deferred spawn this pins against, and the executor is free to run
            // it either way.
            d.app.edit_schedule(Update, |schedule| {
                schedule.set_build_settings(ScheduleBuildSettings {
                    ambiguity_detection: LogLevel::Error,
                    ..default()
                });
            });
            d.app
                .world_mut()
                .resource_mut::<MzbCollisionGeometry>()
                .set_block(ZONE_SLOT_MAIN, shell_placeholder_block());
            d
        }

        fn bare() -> Self {
            let mut app = App::new();
            app.add_message::<LoadMzbRequest>()
                .add_message::<SubAreaChanged>()
                .add_message::<SetSubArea>()
                .init_resource::<SceneState>()
                .init_resource::<MzbCollisionGeometry>()
                .init_resource::<ZoneAreaMap>()
                .init_resource::<ZoneChunkLightMap>()
                .init_resource::<PendingWaterSpawns>()
                .init_resource::<MmbLoadQueue>()
                .init_resource::<LoadMzbInFlight>();

            app.insert_resource(armed_activation());
            app.world_mut()
                .resource_mut::<SceneState>()
                .snapshot
                .zone_id = Some(ZONE_ID);
            app.world_mut()
                .resource_mut::<MzbCollisionGeometry>()
                .set_block(ZONE_SLOT_MAIN, slab_block_at(DOORWAY_COLUMN, TOWN_FLOOR_Y));

            // The MMB path tags interior zone models with AutoMzbOverlay too, so
            // both blocks carry it here: only ZoneBlockSlot may decide what a
            // retire takes down.
            let town_geom = app
                .world_mut()
                .spawn((ZoneBlockSlot(ZONE_SLOT_MAIN), AutoMzbOverlay))
                .id();
            let town_model = app
                .world_mut()
                .spawn((ZoneBlockSlot(ZONE_SLOT_MAIN), AutoMzbOverlay))
                .id();
            Self {
                app,
                town_geom,
                town_model,
            }
        }

        /// `p` is DAT-native, the frame the rects and shells are declared in.
        /// Publishing it the way the session does — swapping height and depth
        /// into snapshot order — is what makes these tests able to fail on the
        /// remap at all; writing it through unconverted cancels the swap out on
        /// both sides and passes against a driver that can never fire.
        fn walk_to(&mut self, p: [f32; 3]) {
            self.app
                .world_mut()
                .resource_mut::<SceneState>()
                .snapshot
                .self_pos
                .pos = kuluu_snapshot::Vec3 {
                x: p[0],
                y: p[2],
                z: p[1],
            };
            self.app.update();
        }

        fn loads(&mut self) -> Vec<LoadMzbRequest> {
            self.app
                .world_mut()
                .resource_mut::<Messages<LoadMzbRequest>>()
                .drain()
                .collect()
        }

        fn changes(&mut self) -> Vec<SubAreaChanged> {
            self.app
                .world_mut()
                .resource_mut::<Messages<SubAreaChanged>>()
                .drain()
                .collect()
        }

        /// What `poll_load_mzb_tasks` does when the interior lands, minus the
        /// mesh assets: the slot-1 collision block and the entities that carry
        /// it. `spawn_mzb_overlay` stamps the slot on the overlay parent *and*
        /// on its mesh children, so both come back here — a retire has to
        /// tolerate reaching a child whose parent it has already taken down.
        fn interior_lands(&mut self) -> (Entity, Entity) {
            self.app
                .world_mut()
                .resource_mut::<MzbCollisionGeometry>()
                .set_block(
                    ZONE_SLOT_SUB_AREA,
                    slab_block_at(INTERIOR_COLUMN, INTERIOR_FLOOR_Y),
                );
            let parent = self
                .app
                .world_mut()
                .spawn((ZoneBlockSlot(ZONE_SLOT_SUB_AREA), AutoMzbOverlay))
                .id();
            let child = self
                .app
                .world_mut()
                .spawn((
                    ZoneBlockSlot(ZONE_SLOT_SUB_AREA),
                    AutoMzbOverlay,
                    ChildOf(parent),
                ))
                .id();
            (parent, child)
        }

        fn any_pending(&self) -> bool {
            self.app.world().resource::<LoadMzbInFlight>().any_pending()
        }

        fn ground_at(&self, column: Vec2, feet_y: f32) -> Option<f32> {
            self.app
                .world()
                .resource::<MzbCollisionGeometry>()
                .ground_step(column, feet_y, MAX_GROUND_STEP_UP)
        }

        fn alive(&self, e: Entity) -> bool {
            self.app.world().get_entity(e).is_ok()
        }
    }

    #[test]
    fn walking_through_a_doorway_loads_the_interior_into_slot_one() {
        let mut d = Doorway::new();

        d.walk_to(OUT_IN_THE_STREET);
        assert!(d.loads().is_empty(), "the street is not an interior");
        assert_eq!(d.app.world().resource::<SubAreaActivation>().active(), None);

        d.walk_to(DOORWAY_CENTRE);
        assert_eq!(
            d.changes(),
            vec![SubAreaChanged {
                sub_area: Some(SUB_AREA)
            }]
        );
        let loads = d.loads();
        assert_eq!(loads.len(), 1, "one request, for the interior: {loads:?}");
        assert_eq!(loads[0].slot, ZONE_SLOT_SUB_AREA);
        assert_eq!(loads[0].file_id, sub_area::sub_area_file_id(SUB_AREA));
        assert_eq!(loads[0].active_sub_area, Some(SUB_AREA));
    }

    #[test]
    fn the_merged_read_floors_the_doorway_and_the_interior_at_once() {
        let mut d = Doorway::new();
        d.walk_to(DOORWAY_CENTRE);
        d.interior_lands();
        d.walk_to(DEEP_INSIDE);

        assert_eq!(
            d.ground_at(DOORWAY_COLUMN, TOWN_FLOOR_Y),
            Some(TOWN_FLOOR_Y),
            "the doorway approach is floored by the town block alone"
        );
        assert_eq!(
            d.ground_at(INTERIOR_COLUMN, INTERIOR_FLOOR_Y),
            Some(INTERIOR_FLOOR_Y),
            "the interior is floored by the sub-area block alone"
        );
    }

    #[test]
    fn walking_back_out_retires_the_interior_and_leaves_the_town_standing() {
        let mut d = Doorway::new();
        d.walk_to(DOORWAY_CENTRE);
        let (interior_parent, interior_mesh) = d.interior_lands();
        d.walk_to(DEEP_INSIDE);
        let _ = d.changes();

        d.walk_to(OUT_IN_THE_STREET);

        assert_eq!(d.changes(), vec![SubAreaChanged { sub_area: None }]);
        assert_eq!(d.app.world().resource::<SubAreaActivation>().active(), None);
        assert!(
            !d.alive(interior_parent) && !d.alive(interior_mesh),
            "the interior's entities go with its slot"
        );
        assert!(
            d.alive(d.town_geom) && d.alive(d.town_model),
            "retiring by AutoMzbOverlay would have taken the town with it"
        );
        assert_eq!(
            d.ground_at(DOORWAY_COLUMN, TOWN_FLOOR_Y),
            Some(TOWN_FLOOR_Y),
            "the town block still floors the street"
        );
        assert_eq!(
            d.ground_at(INTERIOR_COLUMN, INTERIOR_FLOOR_Y),
            None,
            "the interior's collision is gone with it"
        );
    }

    /// The retire and the load have to land in one schedule run.
    /// `ground_recovery_candidate` (kuluu/src/view_native/input.rs) reads
    /// `LoadMzbInFlight::any_pending()` to tell a streaming hole from a wedge; a
    /// frame with the shell suppressed and nothing in flight is a column with no
    /// floor that the recovery is free to act on, which is the kuluu-0nnl roof
    /// snap.
    #[test]
    fn the_interior_load_is_in_flight_the_frame_the_shell_stops_colliding() {
        let mut d = Doorway::streaming();
        assert_eq!(
            d.ground_at(INTERIOR_COLUMN, SHELL_FLOOR_Y),
            Some(SHELL_FLOOR_Y),
            "outside, the placeholder floors the column"
        );
        assert!(!d.any_pending(), "nothing is streaming out in the street");

        d.walk_to(DOORWAY_CENTRE);

        assert_eq!(
            d.app.world().resource::<SubAreaActivation>().active(),
            Some(SUB_AREA),
            "the doorway latched"
        );
        assert_eq!(
            d.ground_at(INTERIOR_COLUMN, SHELL_FLOOR_Y),
            None,
            "the placeholder is suppressed and the interior has not landed"
        );
        assert!(
            d.any_pending(),
            "the load must be in flight in the same run that took the floor away"
        );
    }

    /// A zone change must disarm the latch and take the interior down even if the
    /// player was standing in one, or the shop's floor follows them to the next
    /// zone.
    #[test]
    fn a_zone_change_retires_the_interior() {
        let mut d = Doorway::new();
        d.walk_to(DOORWAY_CENTRE);
        let (interior_parent, interior_mesh) = d.interior_lands();

        d.app
            .world_mut()
            .resource_mut::<SceneState>()
            .snapshot
            .zone_id = None;
        d.walk_to(DEEP_INSIDE);

        assert!(!d.app.world().resource::<SubAreaActivation>().is_armed());
        assert_eq!(d.app.world().resource::<SubAreaActivation>().active(), None);
        assert!(!d.alive(interior_parent) && !d.alive(interior_mesh));
        assert_eq!(d.ground_at(INTERIOR_COLUMN, INTERIOR_FLOOR_Y), None);
    }
}
