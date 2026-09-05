use super::*;
use crate::state::{Entity, EntityKind, FishParams, FishingInput, PartyMember};

fn step_test_cfg() -> ReactorConfig {
    ReactorConfig {
        max_step_per_tick: 1.0,
        ..ReactorConfig::default()
    }
}

fn upsert(id: u32, pos: Vec3, hp_pct: u8, kind: EntityKind, act_index: u16) -> AgentEvent {
    upsert_with_bt(id, pos, hp_pct, kind, act_index, 0)
}

fn upsert_with_bt(
    id: u32,
    pos: Vec3,
    hp_pct: u8,
    kind: EntityKind,
    act_index: u16,
    bt_target_id: u32,
) -> AgentEvent {
    upsert_with_speed(
        id,
        pos,
        hp_pct,
        kind,
        act_index,
        bt_target_id,
        crate::state::BASE_PACKET_SPEED,
        crate::state::BASE_PACKET_SPEED,
    )
}

fn upsert_with_speed(
    id: u32,
    pos: Vec3,
    hp_pct: u8,
    kind: EntityKind,
    act_index: u16,
    bt_target_id: u32,
    speed: u8,
    speed_base: u8,
) -> AgentEvent {
    AgentEvent::EntityUpserted {
        entity: Entity {
            id,
            act_index,
            kind,
            name: None,
            pos,
            heading: 0,
            hp_pct: Some(hp_pct),
            bt_target_id,
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed,
            speed_base,
            look: None,
            npc_state: None,
            status: 0,
            char_flags: Default::default(),
            mount_id: None,
        },
        pos_present: true,
    }
}

fn connected(char_id: u32) -> AgentEvent {
    AgentEvent::Connected {
        account_id: 0,
        char_id,
        character: "Tester".into(),
        zone_id: 0,
    }
}

fn party_update(id: u32, pct: u8) -> AgentEvent {
    AgentEvent::PartyMemberUpdated {
        member: PartyMember {
            id,
            act_index: 1,
            name: Some("M".into()),
            hp: 100,
            mp: 100,
            tp: 0,
            hp_pct: pct,
            mp_pct: 100,
            zone_no: 230,
            main_job: 1,
            main_job_lv: 75,
            sub_job: 6,
            sub_job_lv: 37,
            is_party_leader: false,
            is_alliance_leader: false,
            in_mog_house: false,
            party_no: 0,
        },
    }
}

fn hooked_reactor(cfg: ReactorConfig) -> Reactor {
    let mut reactor = Reactor::new(cfg);
    reactor.handle_command(AgentCommand::Fish);
    reactor.observe_event(&AgentEvent::FishingCast { hook_delay: 0 });
    reactor.tick();
    reactor.observe_event(&AgentEvent::FishHooked {
        params: FishParams {
            stamina: 100,
            arrow_delay: 5,
            regen: 128,
            move_frequency: 3,
            arrow_damage: 5,
            arrow_regen: 2,
            time: 30,
            angler_sense: 0,
            intuition: 0,
        },
    });
    reactor
}

fn sees_fishing_arrow(reactor: &mut Reactor) -> bool {
    (0..30).any(|_| {
        reactor
            .tick()
            .derived_events
            .iter()
            .any(|event| matches!(event, AgentEvent::FishingProgress { arrow: Some(_), .. }))
    })
}

#[test]
fn reactor_profiles_make_player_input_policy_explicit() {
    assert_eq!(ReactorConfig::default().profile, ReactorProfile::Player);
    assert_eq!(ReactorConfig::player().profile, ReactorProfile::Player);
    assert_eq!(ReactorConfig::agent().profile, ReactorProfile::Agent);

    let mut player = hooked_reactor(ReactorConfig::player());
    assert!(
        !sees_fishing_arrow(&mut player),
        "player fishing must wait for the player's hook input"
    );
    player.handle_command(AgentCommand::FishingInput {
        input: FishingInput::Hook,
    });
    assert!(
        sees_fishing_arrow(&mut player),
        "player fishing starts the arrow sequence after manual hook input"
    );

    let mut agent = hooked_reactor(ReactorConfig::agent());
    assert!(
        sees_fishing_arrow(&mut agent),
        "agent fishing hooks and plays the arrow sequence automatically"
    );
}

#[test]
fn idle_tick_produces_nothing() {
    let mut r = Reactor::new(ReactorConfig::default());
    let out = r.tick();
    assert!(out.commands.is_empty());
    assert!(out.derived_events.is_empty());
}

#[test]
fn follow_steps_toward_target_then_holds() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
    r.observe_event(&upsert(
        2,
        Vec3 {
            x: 20.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        2,
    ));
    r.handle_command(AgentCommand::Follow {
        target_id: 2,
        distance: 5.0,
    });

    let cmds = r.tick().commands;
    assert_eq!(cmds.len(), 1);
    match &cmds[0] {
        AgentCommand::Move { x, .. } => {
            assert!(
                (x - 1.0).abs() < 1e-3,
                "step toward target capped at max_step: got {x}"
            );
        }
        other => panic!("expected Move, got {other:?}"),
    }

    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 17.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    assert!(r.tick().commands.is_empty(), "within distance: hold");
}

#[test]
fn follow_against_unknown_target_emits_nothing() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
    r.handle_command(AgentCommand::Follow {
        target_id: 999,
        distance: 5.0,
    });
    assert!(r.tick().commands.is_empty(), "no entity → no movement");
}

#[test]
fn follow_holds_at_pc_model_radius() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
    r.observe_event(&upsert(
        2,
        Vec3 {
            x: 0.8,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        2,
    ));
    r.handle_command(AgentCommand::Follow {
        target_id: 2,
        distance: 0.0,
    });
    assert_eq!(
        r.tick().commands.len(),
        1,
        "outside PC contact radius (0.8 > ~0.70): step"
    );

    r.observe_event(&upsert(
        2,
        Vec3 {
            x: 0.6,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        2,
    ));
    assert!(
        r.tick().commands.is_empty(),
        "inside PC contact radius (0.6 < ~0.70): hold"
    );
}

#[test]
fn follow_hold_scales_with_target_kind() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
    r.observe_event(&upsert(
        2,
        Vec3 {
            x: 0.8,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Mob,
        2,
    ));
    r.handle_command(AgentCommand::Follow {
        target_id: 2,
        distance: 0.0,
    });
    assert!(
        r.tick().commands.is_empty(),
        "inside Mob contact radius (0.8 < ~0.90): hold"
    );

    r.observe_event(&upsert(
        2,
        Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Mob,
        2,
    ));
    assert_eq!(
        r.tick().commands.len(),
        1,
        "outside Mob contact radius (1.0 > ~0.90): step"
    );
}

#[test]
fn follow_distance_floor_still_honored() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
    r.observe_event(&upsert(
        2,
        Vec3 {
            x: 4.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        2,
    ));
    r.handle_command(AgentCommand::Follow {
        target_id: 2,
        distance: 5.0,
    });
    assert!(
        r.tick().commands.is_empty(),
        "within explicit floor distance (4.0 < 5.0): hold"
    );
}

#[test]
fn agent_engage_emits_attack_once_then_only_face() {
    let mut r = Reactor::new(ReactorConfig::agent());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
    r.observe_event(&upsert(
        99,
        Vec3 {
            x: 5.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Mob,
        7,
    ));
    r.handle_command(AgentCommand::Engage { target_id: 99 });

    let t1 = r.tick().commands;
    let attacks_t1 = t1
        .iter()
        .filter(|c| {
            matches!(
                c,
                AgentCommand::Action {
                    kind: ActionKind::Attack,
                    ..
                }
            )
        })
        .count();
    assert_eq!(attacks_t1, 1, "tick 1 emits exactly one Attack");

    let t2 = r.tick().commands;
    let attacks_t2 = t2
        .iter()
        .filter(|c| {
            matches!(
                c,
                AgentCommand::Action {
                    kind: ActionKind::Attack,
                    ..
                }
            )
        })
        .count();
    assert_eq!(attacks_t2, 0, "tick 2 does not re-issue Attack");

    assert!(t2.iter().any(|c| matches!(c, AgentCommand::Move { .. })));
}

#[test]
fn player_engage_starts_unlocked_and_follows_manual_lock_state() {
    let mut r = Reactor::new(ReactorConfig::player());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(1, Vec3::default(), 100, EntityKind::Pc, 1));
    r.observe_event(&upsert(
        99,
        Vec3 {
            x: 5.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Mob,
        7,
    ));
    r.handle_command(AgentCommand::Engage { target_id: 99 });
    let unlocked = r.tick().commands;
    assert!(
        !unlocked
            .iter()
            .any(|c| matches!(c, AgentCommand::Move { .. })),
        "player engage must not force the heading before manual lock-on"
    );

    r.handle_command(AgentCommand::SetTargetLock { locked: true });
    let relocked = r.tick().commands;
    assert!(
        relocked
            .iter()
            .any(|c| matches!(c, AgentCommand::Move { .. })),
        "re-locking resumes facing the target"
    );
}

#[test]
fn set_target_lock_is_not_forwarded_to_server() {
    let mut r = Reactor::new(ReactorConfig::default());
    let routing = r.handle_command(AgentCommand::SetTargetLock { locked: false });
    assert!(
        routing.forward.is_none(),
        "lock-on is client/reactor state, never a server packet"
    );
}

#[test]
fn cancel_clears_goal() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.handle_command(AgentCommand::Engage { target_id: 99 });
    assert!(matches!(r.current_goal(), Goal::Engaged { .. }));
    r.handle_command(AgentCommand::Cancel);
    assert!(matches!(r.current_goal(), Goal::Idle));
    assert!(r.tick().commands.is_empty());
}

fn emits_idle_goal(events: &[AgentEvent]) -> bool {
    events.iter().any(|e| {
        matches!(
            e,
            AgentEvent::ReactorGoalChanged {
                goal: ReactorGoalSnapshot::Idle
            }
        )
    })
}

#[test]
fn death_timer_disengages_and_emits_goal_change() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.handle_command(AgentCommand::Engage { target_id: 99 });
    assert!(matches!(r.current_goal(), Goal::Engaged { .. }));

    let derived = r.observe_event(&AgentEvent::DeathTimerUpdated {
        seconds_until_homepoint: Some(60),
    });
    assert!(
        matches!(r.current_goal(), Goal::Idle),
        "death must force disengage"
    );
    assert!(
        emits_idle_goal(&derived),
        "the reset must be emitted so the folded current_goal updates"
    );
}

#[test]
fn zone_change_while_engaged_emits_idle_goal_change() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.handle_command(AgentCommand::Engage { target_id: 99 });
    assert!(matches!(r.current_goal(), Goal::Engaged { .. }));

    let derived = r.observe_event(&AgentEvent::ZoneChanged {
        from: Some(116),
        to: 240,
        myroom: None,
        mog_zone_flag: false,
    });
    assert!(matches!(r.current_goal(), Goal::Idle));
    assert!(
        emits_idle_goal(&derived),
        "home-point warp must propagate disengage to current_goal"
    );
}

#[test]
fn death_after_disengage_is_idempotent_no_event() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    assert!(matches!(r.current_goal(), Goal::Idle));

    let derived = r.observe_event(&AgentEvent::DeathTimerUpdated {
        seconds_until_homepoint: Some(60),
    });
    assert!(
        !emits_idle_goal(&derived),
        "already Idle: no spurious goal-change event while dead"
    );
}

#[test]
fn revive_event_does_not_disengage() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.handle_command(AgentCommand::Engage { target_id: 99 });
    let derived = r.observe_event(&AgentEvent::DeathTimerUpdated {
        seconds_until_homepoint: None,
    });
    assert!(
        matches!(r.current_goal(), Goal::Engaged { .. }),
        "an alive CHAR_STATUS (None) must not disengage"
    );
    assert!(!emits_idle_goal(&derived));
}

#[test]
fn explicit_move_clears_goal_and_passes_through() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.handle_command(AgentCommand::Follow {
        target_id: 2,
        distance: 5.0,
    });
    assert!(matches!(r.current_goal(), Goal::Following { .. }));
    let m = AgentCommand::Move {
        x: 1.0,
        y: 2.0,
        z: 3.0,
        heading: 64,
    };
    let routing = r.handle_command(m);
    assert!(
        matches!(routing.forward, Some(AgentCommand::Move { .. })),
        "Move passes through"
    );

    assert!(matches!(
        routing.derived_events.as_slice(),
        [AgentEvent::ReactorGoalChanged {
            goal: ReactorGoalSnapshot::Idle
        }]
    ));
    assert!(matches!(r.current_goal(), Goal::Idle));
}

#[test]
fn explicit_move_while_idle_emits_no_goal_event() {
    let mut r = Reactor::new(ReactorConfig::default());
    let m = AgentCommand::Move {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        heading: 0,
    };
    let routing = r.handle_command(m);
    assert!(matches!(routing.forward, Some(AgentCommand::Move { .. })));
    assert!(
        routing.derived_events.is_empty(),
        "no transition → no goal event (avoids Idle→Idle log spam)"
    );
}

#[test]
fn passthrough_chat_unchanged() {
    let mut r = Reactor::new(ReactorConfig::default());
    let chat = AgentCommand::Chat {
        kind: 0,
        text: "hello".into(),
    };
    let routing = r.handle_command(chat);
    assert!(matches!(routing.forward, Some(AgentCommand::Chat { .. })));
    assert!(routing.derived_events.is_empty());
}

#[test]
fn snapshot_emits_scene_summary_and_forwards() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    let routing = r.handle_command(AgentCommand::Snapshot);
    assert!(
        matches!(routing.forward, Some(AgentCommand::Snapshot)),
        "Snapshot still forwards to session for Diagnostics"
    );
    assert_eq!(routing.derived_events.len(), 1);
    assert!(matches!(
        &routing.derived_events[0],
        AgentEvent::SceneSummary { .. }
    ));
}

#[test]
fn goal_commands_are_absorbed_no_forward() {
    let mut r = Reactor::new(ReactorConfig::default());
    for cmd in [
        AgentCommand::Follow {
            target_id: 1,
            distance: 5.0,
        },
        AgentCommand::Engage { target_id: 1 },
        AgentCommand::PathTo {
            x: 1.0,
            y: 0.0,
            z: 0.0,
            force: false,
        },
        AgentCommand::Cancel,
    ] {
        let routing = r.handle_command(cmd);
        assert!(routing.forward.is_none());
    }
}

#[test]
fn follow_emits_reactor_goal_changed() {
    let mut r = Reactor::new(ReactorConfig::default());
    let routing = r.handle_command(AgentCommand::Follow {
        target_id: 42,
        distance: 3.0,
    });
    assert!(routing.forward.is_none());
    match routing.derived_events.as_slice() {
        [AgentEvent::ReactorGoalChanged {
            goal:
                ReactorGoalSnapshot::Following {
                    target_id,
                    distance,
                },
        }] => {
            assert_eq!(*target_id, 42);
            assert!((*distance - 3.0).abs() < 1e-3);
        }
        other => panic!("expected single ReactorGoalChanged(Following), got {other:?}"),
    }
}

#[test]
fn engage_emits_reactor_goal_changed() {
    let mut r = Reactor::new(ReactorConfig::default());
    let routing = r.handle_command(AgentCommand::Engage { target_id: 99 });
    assert!(routing.forward.is_none());
    match routing.derived_events.as_slice() {
        [AgentEvent::ReactorGoalChanged {
            goal:
                ReactorGoalSnapshot::Engaged {
                    target_id,
                    attack_issued,
                },
        }] => {
            assert_eq!(*target_id, 99);

            assert!(!*attack_issued, "attack_issued is false until first tick");
        }
        other => panic!("expected ReactorGoalChanged(Engaged), got {other:?}"),
    }
}

#[test]
fn path_to_emits_reactor_goal_changed() {
    let mut r = Reactor::new(ReactorConfig::default());

    let routing = r.handle_command(AgentCommand::PathTo {
        x: 1.0,
        y: 2.0,
        z: 3.0,
        force: true,
    });
    assert!(routing.forward.is_none());

    match routing.derived_events.as_slice() {
        [AgentEvent::ReactorGoalChanged {
            goal:
                ReactorGoalSnapshot::Pathing {
                    x,
                    y,
                    z,
                    waypoints_remaining,
                },
        }, AgentEvent::ChatLine { line }] => {
            assert!((*x - 1.0).abs() < 1e-3);
            assert!((*y - 2.0).abs() < 1e-3);
            assert!((*z - 3.0).abs() < 1e-3);

            assert_eq!(*waypoints_remaining, 1);
            assert_eq!(line.channel, ChatChannel::Debug);
            assert!(line.text.contains("pathto"));
        }
        other => panic!("expected [ReactorGoalChanged(Pathing), ChatLine], got {other:?}"),
    }
}

#[test]
fn pathto_without_route_refuses_and_reports() {
    let mut r = Reactor::new(ReactorConfig::default());
    let routing = r.handle_command(AgentCommand::PathTo {
        x: 5.0,
        y: 0.0,
        z: 0.0,
        force: false,
    });
    assert!(routing.forward.is_none());
    assert!(
        matches!(r.current_goal(), Goal::Idle),
        "refused pathto must leave the goal Idle, got {:?}",
        r.current_goal()
    );
    match routing.derived_events.as_slice() {
        [AgentEvent::ChatLine { line }] => {
            assert_eq!(line.channel, ChatChannel::Debug);
            assert!(
                line.text.contains("no walkable route"),
                "got {:?}",
                line.text
            );
        }
        other => panic!("expected a single refusal ChatLine, got {other:?}"),
    }
}

#[test]
fn pathto_force_beelines_without_route() {
    let mut r = Reactor::new(ReactorConfig::default());
    let routing = r.handle_command(AgentCommand::PathTo {
        x: 5.0,
        y: 6.0,
        z: 7.0,
        force: true,
    });
    assert!(routing.forward.is_none());
    match r.current_goal() {
        Goal::Pathing {
            waypoints,
            idx,
            clamp,
        } => {
            assert_eq!(*idx, 0);
            assert!(!*clamp, "forced pathing must not wall-slide");
            assert_eq!(waypoints.len(), 1);
            let wp = waypoints[0];
            assert!((wp.x - 5.0).abs() < 1e-3 && (wp.y - 6.0).abs() < 1e-3);
        }
        other => panic!("expected forced Pathing goal, got {other:?}"),
    }

    assert!(routing.derived_events.iter().any(|e| matches!(
        e,
        AgentEvent::ChatLine { line } if line.text.contains("[force]")
    )));
}

#[test]
fn pathing_uses_navmesh_when_available() {
    let mut walkable = vec![true; 100];
    for row in 0..7u32 {
        walkable[(row * 10 + 5) as usize] = false;
    }
    let nav = kuluu_nav::GridNav::from_walkable(10, 10, walkable, kuluu_nav::glam::Vec2::ZERO, 1.0);

    let mut r = Reactor::new(ReactorConfig::default());

    r.state.zone_id = Some(123);
    r.set_nav_for_test(123, nav);

    let routing = r.handle_command(AgentCommand::PathTo {
        x: 9.0,
        y: 0.0,
        z: 0.0,
        force: false,
    });
    assert!(routing.forward.is_none());

    let goal = r.current_goal().clone();
    let Goal::Pathing { waypoints, idx, .. } = &goal else {
        panic!("expected Pathing goal, got {goal:?}");
    };
    assert_eq!(*idx, 0);
    assert!(
        waypoints.iter().any(|w| w.z >= 7.0),
        "navmesh path should route around the wall, got {waypoints:?}"
    );
    assert!(
        waypoints.last().map(|w| w.x as i32 == 9).unwrap_or(false),
        "last waypoint should be the destination"
    );
}

#[test]
fn cancel_clears_goal_emits_idle_event() {
    let mut r = Reactor::new(ReactorConfig::default());

    let _ = r.handle_command(AgentCommand::Engage { target_id: 1 });

    let routing = r.handle_command(AgentCommand::Cancel);
    assert!(routing.forward.is_none());
    assert!(matches!(
        routing.derived_events.as_slice(),
        [AgentEvent::ReactorGoalChanged {
            goal: ReactorGoalSnapshot::Idle,
        }]
    ));
}

#[test]
fn low_hp_emits_once_per_downward_crossing() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));

    let derived = r.observe_event(&upsert(1, Vec3::default(), 80, EntityKind::Pc, 1));
    assert!(derived.is_empty());

    let derived = r.observe_event(&upsert(1, Vec3::default(), 20, EntityKind::Pc, 1));
    assert!(matches!(
        derived.as_slice(),
        [AgentEvent::LowHp { pct: 20 }]
    ));

    let derived = r.observe_event(&upsert(1, Vec3::default(), 15, EntityKind::Pc, 1));
    assert!(derived.is_empty(), "latched: no repeat");

    let derived = r.observe_event(&upsert(1, Vec3::default(), 80, EntityKind::Pc, 1));
    assert!(derived.is_empty());

    let derived = r.observe_event(&upsert(1, Vec3::default(), 10, EntityKind::Pc, 1));
    assert!(matches!(
        derived.as_slice(),
        [AgentEvent::LowHp { pct: 10 }]
    ));
}

#[test]
fn party_member_low_hp_latches_per_member() {
    let mut r = Reactor::new(ReactorConfig::default());

    assert!(r.observe_event(&party_update(10, 80)).is_empty());
    assert!(r.observe_event(&party_update(11, 90)).is_empty());

    let derived = r.observe_event(&party_update(10, 20));
    assert!(matches!(
        derived.as_slice(),
        [AgentEvent::PartyMemberLowHp { id: 10, pct: 20 }]
    ));

    assert!(r.observe_event(&party_update(11, 30)).is_empty());

    let derived = r.observe_event(&party_update(11, 10));
    assert!(matches!(
        derived.as_slice(),
        [AgentEvent::PartyMemberLowHp { id: 11, pct: 10 }]
    ));
}

#[test]
fn pathing_walks_to_target_and_returns_to_idle() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));

    r.handle_command(AgentCommand::PathTo {
        x: 0.5,
        y: 0.0,
        z: 0.0,
        force: true,
    });
    let out = r.tick();
    assert_eq!(out.commands.len(), 1);
    match &out.commands[0] {
        AgentCommand::Move { x, z, .. } => {
            assert!((x - 0.5).abs() < 1e-3);
            assert!(z.abs() < 1e-3);
        }
        other => panic!("expected Move, got {other:?}"),
    }
    assert!(matches!(r.current_goal(), Goal::Idle));
}

#[test]
fn pathing_self_clear_emits_idle_event() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));

    r.handle_command(AgentCommand::PathTo {
        x: 0.5,
        y: 0.0,
        z: 0.0,
        force: true,
    });
    let out = r.tick();
    assert!(matches!(r.current_goal(), Goal::Idle));
    assert!(matches!(
        out.derived_events.as_slice(),
        [AgentEvent::ReactorGoalChanged {
            goal: ReactorGoalSnapshot::Idle,
        }]
    ));
}

#[test]
fn pathing_takes_multiple_ticks_for_distant_target() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    r.handle_command(AgentCommand::PathTo {
        x: 12.0,
        y: 0.0,
        z: 0.0,
        force: true,
    });

    let out = r.tick();
    match &out.commands[0] {
        AgentCommand::Move { x, .. } => assert!((x - 1.0).abs() < 1e-3),
        other => panic!("got {other:?}"),
    }
    assert!(matches!(r.current_goal(), Goal::Pathing { .. }));
    assert!(
        out.derived_events.is_empty(),
        "mid-path tick should not emit goal-changed"
    );
}

#[test]
fn pathing_consumes_step_across_multiple_waypoints() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    r.goal = Goal::Pathing {
        waypoints: vec![
            Vec3 {
                x: 0.2,
                y: 0.0,
                z: 0.0,
            },
            Vec3 {
                x: 0.4,
                y: 0.0,
                z: 0.0,
            },
            Vec3 {
                x: 0.6,
                y: 0.0,
                z: 0.0,
            },
            Vec3 {
                x: 0.8,
                y: 0.0,
                z: 0.0,
            },
        ],
        idx: 0,
        clamp: false,
    };

    let out = r.tick();
    assert_eq!(out.commands.len(), 1);
    match &out.commands[0] {
        AgentCommand::Move { x, y, .. } => {
            assert!(
                (x - 0.8).abs() < 1e-3,
                "tick should consume all four 0.2-yalm waypoints in one budget of 1.0, got x={x}"
            );
            assert!(y.abs() < 1e-3);
        }
        other => panic!("expected Move, got {other:?}"),
    }

    assert!(matches!(r.current_goal(), Goal::Idle));
    assert!(matches!(
        out.derived_events.as_slice(),
        [AgentEvent::ReactorGoalChanged {
            goal: ReactorGoalSnapshot::Idle,
        }]
    ));
}

#[test]
fn pathing_partial_consume_carries_remainder_into_next_segment() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    r.goal = Goal::Pathing {
        waypoints: (1..=6)
            .map(|i| Vec3 {
                x: 0.3 * i as f32,
                y: 0.0,
                z: 0.0,
            })
            .collect(),
        idx: 0,
        clamp: false,
    };

    let out = r.tick();
    match &out.commands[0] {
        AgentCommand::Move { x, .. } => {
            assert!(
                (x - 1.0).abs() < 1e-3,
                "expected x=1.0 (0.3+0.3+0.3+0.1), got {x}"
            );
        }
        other => panic!("expected Move, got {other:?}"),
    }

    let Goal::Pathing { idx, .. } = r.current_goal() else {
        panic!("expected still Pathing");
    };
    assert_eq!(*idx, 3);

    assert!(matches!(
        out.derived_events.as_slice(),
        [AgentEvent::ReactorGoalChanged { .. }]
    ));
}

#[test]
fn heading_toward_pins_cardinal_quarters() {
    let origin = Vec3::default();

    assert_eq!(
        heading_toward(
            origin,
            Vec3 {
                x: 10.0,
                y: 0.0,
                z: 0.0
            }
        ),
        0
    );

    assert_eq!(
        heading_toward(
            origin,
            Vec3 {
                x: 0.0,
                y: -10.0,
                z: 0.0
            }
        ),
        64
    );

    assert_eq!(
        heading_toward(
            origin,
            Vec3 {
                x: -10.0,
                y: 0.0,
                z: 0.0
            }
        ),
        128
    );

    assert_eq!(
        heading_toward(
            origin,
            Vec3 {
                x: 0.0,
                y: 10.0,
                z: 0.0
            }
        ),
        192
    );
}

#[test]
fn step_point_caps_at_target() {
    let from = Vec3::default();
    let to = Vec3 {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    };

    let p = step_point(from, to, 100.0);
    assert!((p.x - 1.0).abs() < 1e-3);
}

#[test]
fn step_point_interpolates_height() {
    let from = Vec3::default();
    let to = Vec3 {
        x: 10.0,
        y: 0.0,
        z: -10.0,
    };

    let p = step_point(from, to, 5.0);
    assert!(
        (p.z + 5.0).abs() < 1e-3,
        "a partial step must carry half the height change, got {}",
        p.z
    );
}

#[test]
fn step_point_reaches_target_height_on_the_final_step() {
    let from = Vec3::default();
    let to = Vec3 {
        x: 1.0,
        y: 0.0,
        z: -9.279,
    };

    let p = step_point(from, to, 100.0);
    assert!(
        (p.z - to.z).abs() < 1e-6,
        "the capped final step lands on the destination height, got {}",
        p.z
    );
}

#[test]
fn engaged_by_emits_on_mob_targeting_self() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));

    let derived = r.observe_event(&upsert_with_bt(
        99,
        Vec3::default(),
        100,
        EntityKind::Other,
        7,
        0,
    ));
    assert!(derived.is_empty(), "no aggro on initial sighting");

    let derived = r.observe_event(&upsert_with_bt(
        99,
        Vec3::default(),
        100,
        EntityKind::Other,
        7,
        1,
    ));
    assert!(matches!(
        derived.as_slice(),
        [AgentEvent::EngagedBy { entity_id: 99 }]
    ));
}

#[test]
fn engaged_by_does_not_repeat_while_target_held() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));

    r.observe_event(&upsert_with_bt(
        99,
        Vec3::default(),
        100,
        EntityKind::Other,
        7,
        0,
    ));

    let d1 = r.observe_event(&upsert_with_bt(
        99,
        Vec3::default(),
        100,
        EntityKind::Other,
        7,
        1,
    ));
    assert_eq!(d1.len(), 1);

    let d2 = r.observe_event(&upsert_with_bt(
        99,
        Vec3::default(),
        100,
        EntityKind::Other,
        7,
        1,
    ));
    assert!(d2.is_empty(), "no repeat while target unchanged");

    r.observe_event(&upsert_with_bt(
        99,
        Vec3::default(),
        100,
        EntityKind::Other,
        7,
        0,
    ));
    let d3 = r.observe_event(&upsert_with_bt(
        99,
        Vec3::default(),
        100,
        EntityKind::Other,
        7,
        1,
    ));
    assert_eq!(d3.len(), 1, "re-engage after release fires again");
}

#[test]
fn zoneline_trigger_fires_once_on_entry_and_latches() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    r.state.zone_id = Some(230);

    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: -5.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    let out1 = r.tick();
    assert!(
        !out1
            .commands
            .iter()
            .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
        "must not fire while outside any trigger"
    );

    r.observe_event(&upsert(
        1,
        Vec3 {
            x: -113.372,
            y: -57.418,
            z: -4.075,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    let out2 = r.tick();
    let req = out2
        .commands
        .iter()
        .find_map(|c| match c {
            AgentCommand::RequestZoneChange { line_id } => Some(*line_id),
            _ => None,
        })
        .expect("expected RequestZoneChange on entry");
    assert_eq!(req, 845493882, "should match the west-exit line_id");

    r.observe_event(&upsert(
        1,
        Vec3 {
            x: -113.372,
            y: -57.418,
            z: -4.075,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    let out3 = r.tick();
    assert!(
        !out3
            .commands
            .iter()
            .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
        "must not re-fire while still inside same trigger"
    );

    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: -5.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    let out4 = r.tick();
    assert!(
        !out4
            .commands
            .iter()
            .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
        "must not fire on leave"
    );

    r.observe_event(&upsert(
        1,
        Vec3 {
            x: -113.372,
            y: -57.418,
            z: -4.075,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    let out5 = r.tick();
    assert!(
        out5.commands
            .iter()
            .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
        "re-entry must fire fresh RequestZoneChange"
    );
}

#[test]
fn zoneline_trigger_seeds_on_zone_change_no_immediate_refire() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));

    r.zoneline_trigger_latched = Some(812855930);

    r.observe_event(&AgentEvent::ZoneChanged {
        from: Some(100),
        to: 230,
        myroom: None,
        mog_zone_flag: false,
    });
    r.state.zone_id = Some(230);
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: -113.372,
            y: -57.418,
            z: -4.075,
        },
        100,
        EntityKind::Pc,
        1,
    ));

    let out1 = r.tick();
    assert!(
        !out1
            .commands
            .iter()
            .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
        "must not fire on first tick after ZoneChanged (spawn-inside grace)"
    );
    assert_eq!(
        r.zoneline_trigger_latched,
        Some(845493882),
        "seed should adopt the spawn-inside trigger as the baseline latch"
    );

    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: -5.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    let out2 = r.tick();
    assert!(
        !out2
            .commands
            .iter()
            .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
        "walking off the seeded trigger must not fire"
    );

    r.observe_event(&upsert(
        1,
        Vec3 {
            x: -113.372,
            y: -57.418,
            z: -4.075,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    let out3 = r.tick();
    assert!(
        out3.commands
            .iter()
            .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
        "deliberate re-entry after seeding must fire"
    );
}

#[test]
fn mog_house_prefix_sets_agree_across_crates() {
    // ffxi-dat and kuluu-nav each classify MH lines from their own copy of the
    // LSB prefix pair (0x05e_maprect.cpp:74-75); check_zoneline_trigger
    // correlates the two by rect_id == line_id, so the sets must stay equal.
    assert_eq!(
        [
            ffxi_dat::zone_interaction::MOG_HOUSE_PREFIX_CLASSIC.as_bytes(),
            ffxi_dat::zone_interaction::MOG_HOUSE_PREFIX_WOTG.as_bytes(),
        ],
        [
            kuluu_nav::zonelines::MOG_HOUSE_TAG_PREFIXES[0].as_slice(),
            kuluu_nav::zonelines::MOG_HOUSE_TAG_PREFIXES[1].as_slice(),
        ]
    );
}

#[test]
fn zoneline_trigger_inert_inside_mog_house() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    r.observe_event(&AgentEvent::ZoneChanged {
        from: Some(230),
        to: 230,
        myroom: Some(crate::state::MyRoomInfo {
            model: 257,
            sub_map: 0,
            exit_bit: 1,
        }),
        mog_zone_flag: false,
    });
    r.state.zone_id = Some(230);

    // Town west-exit trigger coords; must not fire from inside the MH.
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: -113.372,
            y: -57.418,
            z: -4.075,
        },
        100,
        EntityKind::Pc,
        1,
    ));
    let first = r.tick();
    let second = r.tick();
    assert!(
        !first
            .commands
            .iter()
            .chain(second.commands.iter())
            .any(|c| matches!(c, AgentCommand::RequestZoneChange { .. })),
        "town zonelines must be inert while myroom is Some"
    );
}

#[test]
fn nav_is_inert_inside_mog_house() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    r.set_nav_for_test(
        230,
        GridNav::from_walkable(1, 1, vec![true], glam::Vec2::ZERO, 1.0),
    );
    r.observe_event(&AgentEvent::ZoneChanged {
        from: Some(230),
        to: 230,
        myroom: Some(crate::state::MyRoomInfo {
            model: 257,
            sub_map: 0,
            exit_bit: 1,
        }),
        mog_zone_flag: false,
    });
    r.state.zone_id = Some(230);
    assert!(
        r.ensure_nav_loaded().is_none(),
        "town navmesh must not drive pathing inside the MH"
    );
}

/// The retail zone-230 residence-door trigger
/// (src=zmr0, center (164.933,-5.547,164.792), yaw 3.93, size 12×8×2 —
/// verified against the install in ffxi-dat::zone_interaction tests).
fn zmr0_rect() -> ffxi_dat::zone_interaction::ZoneInteraction {
    ffxi_dat::zone_interaction::ZoneInteraction {
        position: [164.933, -5.547, 164.792],
        rect_class: ffxi_dat::zone_interaction::RECT_CLASS_HIT_CHECKED,
        orientation: [0.0, 3.93, 0.0],
        size: [12.0, 8.0, 2.0],
        source_id: ffxi_dat::datid::DatId(*b"zmr0"),
        dest_id: Some(ffxi_dat::datid::DatId(*b"zmr1")),
        param: 253,
        terrain_flags: 0,
        map_id: 1,
        elevator_bottom_y: -5.547,
        elevator_top_y: -5.547,
    }
}

#[test]
fn dat_obb_wide_axis_in_short_axis_out() {
    let rect = zmr0_rect();
    // Box center (state axes: x, ground z, vertical y).
    let center = Vec3 {
        x: 164.933,
        y: 164.792,
        z: -5.547,
    };
    assert!(is_inside_dat_obb(center, &rect));

    // 4y along the door's wide (12y) axis — bearing 135° in ground space.
    let wide = Vec3 {
        x: center.x - 2.83,
        y: center.y + 2.83,
        ..center
    };
    assert!(is_inside_dat_obb(wide, &rect), "wide axis half-extent is 6");

    // 3y along the walk-through (2y-deep) axis — bearing 225°.
    let deep = Vec3 {
        x: center.x - 2.12,
        y: center.y - 2.12,
        ..center
    };
    assert!(
        !is_inside_dat_obb(deep, &rect),
        "walk axis half-extent is 1"
    );

    // The lua exit reposition point (159.5, 160) sits ~7.2y out — outside,
    // so leaving the MH cannot re-trigger the door.
    let exit_spawn = Vec3 {
        x: 159.5,
        y: 160.0,
        z: -2.0,
    };
    assert!(!is_inside_dat_obb(exit_spawn, &rect));

    // Vertical extent is centered: ±4y around the box center (-5.547).
    let above = Vec3 { z: -2.0, ..center };
    assert!(is_inside_dat_obb(above, &rect));
    let far_above = Vec3 { z: 3.0, ..center };
    assert!(!is_inside_dat_obb(far_above, &rect));
}

#[test]
fn engaged_by_skips_friendly_entities_and_self() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));

    let d = r.observe_event(&upsert_with_bt(
        50,
        Vec3::default(),
        100,
        EntityKind::Pc,
        2,
        1,
    ));
    assert!(d.is_empty(), "PCs aren't aggro");

    let d = r.observe_event(&upsert_with_bt(
        60,
        Vec3::default(),
        100,
        EntityKind::Npc,
        3,
        1,
    ));
    assert!(d.is_empty(), "NPCs aren't aggro");

    let d = r.observe_event(&upsert_with_bt(
        1,
        Vec3::default(),
        100,
        EntityKind::Pc,
        1,
        1,
    ));
    assert!(d.is_empty(), "self isn't aggroing self");
}

fn inv_capacities(caps: [u16; 18]) -> AgentEvent {
    AgentEvent::InventoryUpdated {
        container: 0,
        update: crate::state::InventoryUpdate::Capacities {
            capacities: caps.to_vec(),
        },
    }
}

fn inv_slot(container: u8, index: u8, item_no: u16) -> AgentEvent {
    AgentEvent::InventoryUpdated {
        container,
        update: crate::state::InventoryUpdate::SlotChanged {
            slot: crate::state::ItemSlot {
                index,
                item_no,
                quantity: 1,
                locked: false,
                price: 0,
                charges_remaining: None,
                next_use_vana_ts: None,
            },
        },
    }
}

#[test]
fn bank_when_full_holds_until_all_loaded() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));

    let mut caps = [0u16; 18];
    caps[0] = 80;
    r.observe_event(&inv_capacities(caps));
    for i in 0..30u8 {
        r.observe_event(&inv_slot(0, i, 4112));
    }
    r.handle_command(AgentCommand::BankWhenFull {
        threshold: 30,
        mog_house_zoneline: 12345,
    });
    let out = r.tick();
    assert!(
        out.commands.is_empty(),
        "must wait for InventoryReady before triggering"
    );
    assert!(matches!(r.current_goal(), Goal::Banking { .. }));
}

#[test]
fn bank_when_full_emits_zoneline_when_threshold_crossed() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    let mut caps = [0u16; 18];
    caps[0] = 80;
    r.observe_event(&inv_capacities(caps));
    for i in 0..30u8 {
        r.observe_event(&inv_slot(0, i, 4112));
    }
    r.observe_event(&AgentEvent::InventoryReady);
    r.handle_command(AgentCommand::BankWhenFull {
        threshold: 30,
        mog_house_zoneline: 12345,
    });
    let out = r.tick();
    assert!(matches!(
        out.commands.as_slice(),
        [AgentCommand::RequestZoneChange { line_id: 12345 }]
    ));
    assert!(
        matches!(r.current_goal(), Goal::Idle),
        "one-shot — goal clears after firing"
    );
    assert!(matches!(
        out.derived_events.as_slice(),
        [AgentEvent::ReactorGoalChanged {
            goal: ReactorGoalSnapshot::Idle
        }]
    ));
}

#[test]
fn bank_when_full_holds_when_under_threshold() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    let mut caps = [0u16; 18];
    caps[0] = 80;
    r.observe_event(&inv_capacities(caps));

    for i in 0..5u8 {
        r.observe_event(&inv_slot(0, i, 4112));
    }
    r.observe_event(&AgentEvent::InventoryReady);
    r.handle_command(AgentCommand::BankWhenFull {
        threshold: 30,
        mog_house_zoneline: 12345,
    });
    let out = r.tick();
    assert!(out.commands.is_empty());
    assert!(matches!(r.current_goal(), Goal::Banking { .. }));
}

#[test]
fn bank_when_full_triggers_on_any_field_bag() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    let mut caps = [0u16; 18];
    caps[5] = 30;
    r.observe_event(&inv_capacities(caps));
    for i in 0..30u8 {
        r.observe_event(&inv_slot(5, i, 4112));
    }
    r.observe_event(&AgentEvent::InventoryReady);
    r.handle_command(AgentCommand::BankWhenFull {
        threshold: 30,
        mog_house_zoneline: 7777,
    });
    let out = r.tick();
    assert!(matches!(
        out.commands.as_slice(),
        [AgentCommand::RequestZoneChange { line_id: 7777 }]
    ));
}

#[test]
fn bank_when_full_ignores_safe_and_storage() {
    let mut r = Reactor::new(ReactorConfig::default());
    r.observe_event(&connected(1));
    let mut caps = [0u16; 18];
    caps[1] = 80;
    caps[2] = 80;
    r.observe_event(&inv_capacities(caps));
    for i in 0..40u8 {
        r.observe_event(&inv_slot(1, i, 4112));
        r.observe_event(&inv_slot(2, i, 4112));
    }
    r.observe_event(&AgentEvent::InventoryReady);
    r.handle_command(AgentCommand::BankWhenFull {
        threshold: 30,
        mog_house_zoneline: 12345,
    });
    let out = r.tick();
    assert!(
        out.commands.is_empty(),
        "safe/storage are bank dest, not field bag"
    );
}

#[test]
fn forced_move_event_installs_override_and_lerps() {
    use crate::state::Position;
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));

    r.handle_command(AgentCommand::Follow {
        target_id: 1,
        distance: 5.0,
    });

    let target = Vec3 {
        x: 0.5,
        y: 0.0,
        z: 0.0,
    };
    r.observe_event(&AgentEvent::ForcedMove {
        mode: 0x00,
        target: Position {
            pos: target,
            heading: 64,
            speed: 0,
            speed_base: 0,
        },
        duration_ms: 5_000,
    });
    assert!(
        r.current_override().is_some(),
        "ForcedMove event installs an override"
    );

    let out = r.tick();
    assert_eq!(out.commands.len(), 1, "exactly one Move emitted per tick");
    match &out.commands[0] {
        AgentCommand::Move { x, y, z, heading } => {
            assert!((x - 0.5).abs() < 1e-3, "lerp reached target.x");
            assert!(y.abs() < 1e-3);
            assert!(z.abs() < 1e-3);
            assert_eq!(*heading, 64, "heading from override carries through");
        }
        other => panic!("expected Move, got {other:?}"),
    }
}

#[test]
fn forced_move_suppresses_explicit_move_command() {
    let mut r = Reactor::new(step_test_cfg());
    r.set_override_for_test(
        Vec3 {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        },
        0,
        Duration::from_secs(5),
    );
    let routing = r.handle_command(AgentCommand::Move {
        x: 99.0,
        y: 99.0,
        z: 99.0,
        heading: 192,
    });
    assert!(
        routing.forward.is_none(),
        "explicit Move dropped while override active"
    );
    assert!(routing.derived_events.is_empty());
}

#[test]
fn forced_move_expires_and_resumes_normal_flow() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
    ));

    r.set_override_for_test(
        Vec3 {
            x: 100.0,
            y: 0.0,
            z: 0.0,
        },
        0,
        Duration::from_millis(1),
    );
    std::thread::sleep(Duration::from_millis(5));
    let _ = r.tick();
    assert!(
        r.current_override().is_none(),
        "override clears once expiry passes"
    );

    let out = r.tick();
    assert!(out.commands.is_empty());
    assert!(out.derived_events.is_empty());
}

#[test]
fn hp_threshold_at_exact_value_is_above() {
    let cfg = ReactorConfig {
        low_hp_threshold: 25,
        ..ReactorConfig::default()
    };
    let mut r = Reactor::new(cfg);
    r.observe_event(&connected(1));
    let d = r.observe_event(&upsert(1, Vec3::default(), 25, EntityKind::Pc, 1));
    assert!(d.is_empty(), "exactly threshold should not fire");
    let d = r.observe_event(&upsert(1, Vec3::default(), 24, EntityKind::Pc, 1));
    assert!(matches!(d.as_slice(), [AgentEvent::LowHp { pct: 24 }]));
}

#[test]
fn pathing_suppresses_move_when_server_speed_is_zero() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));

    r.observe_event(&upsert_with_speed(
        1,
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        1,
        0,
        0,
        40,
    ));
    r.handle_command(AgentCommand::PathTo {
        x: 10.0,
        y: 0.0,
        z: 0.0,
        force: true,
    });
    let out = r.tick();
    assert!(
        out.commands.is_empty(),
        "speed=0 must suppress Move emission, got {:?}",
        out.commands
    );

    assert!(matches!(r.goal, Goal::Pathing { .. }));
}

fn step_for(speed: u8, mounted: bool) -> f32 {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));
    r.observe_event(&upsert_with_speed(
        1,
        Vec3::default(),
        100,
        EntityKind::Pc,
        1,
        0,
        speed,
        40,
    ));
    if mounted {
        r.observe_event(&AgentEvent::SelfServerStatus {
            status: ffxi_proto::decode::animation::CHOCOBO,
            mount_id: 0,
        });
    }
    r.handle_command(AgentCommand::PathTo {
        x: 100.0,
        y: 0.0,
        z: 0.0,
        force: true,
    });
    match r.tick().commands.as_slice() {
        [AgentCommand::Move { x, .. }] => *x,
        other => panic!("expected single scaled Move, got {other:?}"),
    }
}

#[test]
fn pathing_scales_step_by_the_retail_speed_decode() {
    // `max_step_per_tick` is the budget at the unmounted BASE_PACKET_SPEED,
    // so the step is just the ratio of decoded yalms per second.
    assert!((step_for(50, false) - 1.0).abs() < 1e-4);
    assert!((step_for(25, false) - 0.5).abs() < 1e-4);
    assert!((step_for(20, false) - 0.4).abs() < 1e-4);
}

#[test]
fn pathing_is_faster_mounted_even_though_the_server_sends_less_speed() {
    let mounted = step_for(40, true);
    assert!((mounted - 1.6).abs() < 1e-4, "expected 1.6, got {mounted}");
    assert!(mounted > step_for(50, false));
}

#[test]
fn pathing_step_clamps_at_the_retail_speed_ceiling() {
    // MAX_MOVE_SPEED_YPS / (BASE_PACKET_SPEED * SPEED_TO_YPS) = 30 / 5.
    let capped = step_for(u8::MAX, true);
    assert!((capped - 6.0).abs() < 1e-4, "expected 6.0, got {capped}");
    // Unmounted a u8 tops out below the ceiling, so it is not clamped.
    assert!((step_for(u8::MAX, false) - 5.1).abs() < 1e-4);
}

#[test]
fn follow_suppresses_step_when_server_speed_is_zero() {
    let mut r = Reactor::new(step_test_cfg());
    r.observe_event(&connected(1));

    r.observe_event(&upsert_with_speed(
        1,
        Vec3::default(),
        100,
        EntityKind::Pc,
        1,
        0,
        0,
        40,
    ));

    r.observe_event(&upsert(
        2,
        Vec3 {
            x: 20.0,
            y: 0.0,
            z: 0.0,
        },
        100,
        EntityKind::Pc,
        2,
    ));
    r.handle_command(AgentCommand::Follow {
        target_id: 2,
        distance: 3.0,
    });
    let out = r.tick();

    let cur = Vec3::default();
    for cmd in &out.commands {
        if let AgentCommand::Move { x, y, z, .. } = cmd {
            assert!(
                (*x - cur.x).abs() < 1e-3 && (*y - cur.y).abs() < 1e-3 && (*z - cur.z).abs() < 1e-3,
                "speed=0 follow must not step (only face); got Move to ({x},{y},{z})"
            );
        }
    }
}
