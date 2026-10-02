use super::*;

/// 0x8B MAP_MARKER carries x/y in milli-units; retail scales by `0.001`
/// (research/XiEvents/OpCodes/0x008B.md).
const MILLI_PER_UNIT: f32 = 1000.0;

/// Drives the Map screen from event-script map opcodes, which arrive as
/// edge-triggered ViewerEvents on the snapshot's event ring (0xC8 MAP_TUTORIAL
/// opens, 0x8B MAP_MARKER upserts a marker, 0x8A CLOSE_MAP closes; the opcode
/// semantics: research/XiEvents/OpCodes/0x00C8.md, 0x008B.md, 0x008A.md).
pub fn event_map_sync_system(
    state: Res<SceneState>,
    events: Res<kuluu_render::EventLog>,
    mut cursor: Local<u64>,
    mut mode: ResMut<InputMode>,
    mut map_state: ResMut<kuluu_render::hud::map_screen::MapScreenState>,
    mut markers: ResMut<kuluu_render::hud::map_screen::MapMarkers>,
) {
    let total = events.pushed_total;
    let len = events.recent.len() as u64;
    let first_global = total - len;
    for i in (*cursor).max(first_global)..total {
        match &events.recent[(i - first_global) as usize] {
            kuluu_snapshot::ViewerEvent::MapOpen { map_id, .. } => {
                open_event_map(*map_id, state.snapshot.zone_id, &mut mode, &mut map_state)
            }
            kuluu_snapshot::ViewerEvent::MapMarkerPlaced {
                map_id,
                x_milli,
                y_milli,
                label,
            } => upsert_event_marker(*map_id, *x_milli, *y_milli, label.clone(), &mut markers),
            kuluu_snapshot::ViewerEvent::MapClosed => close_event_map(&mut mode, &mut map_state),
            _ => {}
        }
    }
    *cursor = total;
}

/// Open (or keep open) the Map screen on the event's map. `viewed` stays
/// None when the authored zone is the live one, so the surface shows the live
/// map rather than a Change-Map override.
fn open_event_map(
    map_id: u16,
    live_zone: Option<u16>,
    mode: &mut InputMode,
    map_state: &mut kuluu_render::hud::map_screen::MapScreenState,
) {
    let already_open = matches!(
        mode,
        InputMode::Menu(stack) if stack.current().is_some_and(|l| l.kind == MenuKind::Map)
    );
    if !already_open {
        let mut stack = MenuStack::root();
        stack.push(MenuKind::Map);
        *mode = InputMode::Menu(stack);
    }
    map_state.viewed = (live_zone != Some(map_id)).then_some((map_id, 0));
}

/// Place or move the event's marker. Re-running the event replaces a
/// same-label marker instead of stacking duplicates on top of it.
fn upsert_event_marker(
    map_id: u16,
    x_milli: i32,
    y_milli: i32,
    label: String,
    markers: &mut kuluu_render::hud::map_screen::MapMarkers,
) {
    let world = kuluu_snapshot::Vec3 {
        x: x_milli as f32 / MILLI_PER_UNIT,
        y: 0.0,
        z: y_milli as f32 / MILLI_PER_UNIT,
    };
    let zone = markers.by_zone.entry(map_id).or_default();
    if let Some(slot) = zone.iter_mut().find(|m| m.label == label) {
        slot.world = world;
    } else {
        zone.push(kuluu_render::hud::map_screen::MapMarker { world, label });
    }
}

/// Close the event-opened Map. An event-driven close lands back in the world
/// even when a player-opened /map would pop to the Root command menu.
fn close_event_map(
    mode: &mut InputMode,
    map_state: &mut kuluu_render::hud::map_screen::MapScreenState,
) {
    let is_top = matches!(
        mode,
        InputMode::Menu(stack) if stack.current().is_some_and(|l| l.kind == MenuKind::Map)
    );
    if !is_top {
        return;
    }
    *mode = InputMode::World;
    map_state.reset();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::world::World;
    use kuluu_render::hud::map_screen::{MapMarkers, MapScreenState, MapSubMode};
    use kuluu_snapshot::ViewerEvent;

    fn world_with_zone(zone: Option<u16>) -> World {
        let mut world = World::new();
        let mut scene_state = SceneState::default();
        scene_state.snapshot.zone_id = zone;
        world.insert_resource(scene_state);
        world.insert_resource(kuluu_render::EventLog::default());
        world.insert_resource(InputMode::World);
        world.insert_resource(MapScreenState::default());
        world.insert_resource(MapMarkers::default());
        world.clear_trackers();
        world
    }

    fn run(world: &mut World) {
        let mut state = bevy::ecs::system::SystemState::<(
            Res<SceneState>,
            Res<kuluu_render::EventLog>,
            Local<u64>,
            ResMut<InputMode>,
            ResMut<MapScreenState>,
            ResMut<MapMarkers>,
        )>::new(world);
        let (scene, events, cursor, mode, map_state, markers) =
            state.get_mut(world).expect("event-map params");
        event_map_sync_system(scene, events, cursor, mode, map_state, markers);
    }

    fn mode_is_map_open(world: &World) -> bool {
        matches!(
            world.resource::<InputMode>(),
            InputMode::Menu(stack)
                if stack.current().is_some_and(|l| l.kind == MenuKind::Map)
        )
    }

    fn mode_is_world(world: &World) -> bool {
        matches!(world.resource::<InputMode>(), InputMode::World)
    }

    fn push(world: &mut World, ev: ViewerEvent) {
        world.resource_mut::<kuluu_render::EventLog>().push(ev);
    }

    #[test]
    fn map_open_opens_the_surface_on_the_live_zone() {
        let mut world = world_with_zone(Some(230));
        push(
            &mut world,
            ViewerEvent::MapOpen {
                map_id: 230,
                tutorial: true,
            },
        );
        run(&mut world);

        assert!(mode_is_map_open(&world));
        let map_state = world.resource::<MapScreenState>();
        assert_eq!(map_state.viewed, None, "live zone shows the live map");
    }

    #[test]
    fn map_open_for_a_foreign_zone_sets_the_viewed_override() {
        let mut world = world_with_zone(Some(230));
        push(
            &mut world,
            ViewerEvent::MapOpen {
                map_id: 150,
                tutorial: false,
            },
        );
        run(&mut world);

        assert_eq!(world.resource::<MapScreenState>().viewed, Some((150, 0)));
    }

    #[test]
    fn marker_upsert_places_then_replaces_the_same_label() {
        let mut world = world_with_zone(Some(230));
        push(
            &mut world,
            ViewerEvent::MapMarkerPlaced {
                map_id: 230,
                x_milli: -10264,
                y_milli: -363,
                label: "Ailevia".into(),
            },
        );
        run(&mut world);

        let markers = world.resource::<MapMarkers>();
        assert_eq!(markers.for_zone(230).len(), 1);
        assert_eq!(markers.for_zone(230)[0].label, "Ailevia");
        assert_eq!(markers.for_zone(230)[0].world.x, -10.264);
        assert_eq!(markers.for_zone(230)[0].world.z, -0.363);

        push(
            &mut world,
            ViewerEvent::MapMarkerPlaced {
                map_id: 230,
                x_milli: -10000,
                y_milli: 0,
                label: "Ailevia".into(),
            },
        );
        run(&mut world);

        let markers = world.resource::<MapMarkers>();
        assert_eq!(markers.for_zone(230).len(), 1);
        assert_eq!(markers.for_zone(230)[0].world.x, -10.0);
    }

    #[test]
    fn map_closed_returns_to_the_world_and_resets_the_surface() {
        let mut world = world_with_zone(Some(230));
        push(
            &mut world,
            ViewerEvent::MapOpen {
                map_id: 150,
                tutorial: false,
            },
        );
        run(&mut world);
        world.resource_mut::<MapScreenState>().mode = MapSubMode::Markers;

        push(&mut world, ViewerEvent::MapClosed);
        run(&mut world);

        assert!(mode_is_world(&world));
        let map_state = world.resource::<MapScreenState>();
        assert_eq!(map_state.mode, MapSubMode::Command);
        assert_eq!(map_state.viewed, None);
    }

    #[test]
    fn a_stale_map_close_is_ignored_while_the_surface_is_closed() {
        let mut world = world_with_zone(Some(230));
        push(&mut world, ViewerEvent::MapClosed);
        run(&mut world);
        assert!(mode_is_world(&world));
    }

    /// A back-to-back open + marker + close burst (an auto-advancing client produces
    /// exactly this): the sync still ends in World mode, and the placed marker survives
    /// for the player's later /map ("used by NPCs that help new players and mark your
    /// maps", research/XiEvents/OpCodes/0x008B.md). Event 503 itself does not pace this
    /// way — its MESWAIT (0x23) parks between MAP_MARKER and CLOSE_MAP, so retail holds
    /// the map open over the coupon line until dismissal; pinned by ffxi-event's
    /// close_map_holds_until_the_player_answers_the_line.
    #[test]
    fn event503_same_tick_open_marker_close_leaves_the_marker_not_the_map() {
        let mut world = world_with_zone(Some(230));
        push(
            &mut world,
            ViewerEvent::MapOpen {
                map_id: 230,
                tutorial: true,
            },
        );
        push(
            &mut world,
            ViewerEvent::MapMarkerPlaced {
                map_id: 230,
                x_milli: -10264,
                y_milli: -363,
                label: "Ailevia".into(),
            },
        );
        push(&mut world, ViewerEvent::MapClosed);
        run(&mut world);

        assert!(mode_is_world(&world), "the burst ends back in World mode");
        let markers = world.resource::<MapMarkers>();
        assert_eq!(
            markers.for_zone(230).len(),
            1,
            "the marker persists for the player's later /map"
        );
        assert_eq!(markers.for_zone(230)[0].label, "Ailevia");
        assert_eq!(markers.for_zone(230)[0].world.x, -10.264);
        assert_eq!(markers.for_zone(230)[0].world.z, -0.363);
    }
}
