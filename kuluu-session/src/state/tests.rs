use super::*;

#[test]
fn weather_fold_sets_and_zone_change_clears() {
    let mut s = SessionState::default();
    assert_eq!(s.current_weather, None);
    s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 6 });
    assert_eq!(s.current_weather, Some(6));
    s.apply_event(&AgentEvent::ZoneChanged {
        from: Some(230),
        to: 231,
        myroom: None,
        mog_zone_flag: false,
    });
    assert_eq!(s.current_weather, None);
}

// The fold half of the zone-in weather path: a WeatherUpdated applied after
// a ZoneChanged is kept, so the 0x00A weather is not re-cleared. The emit
// ordering that feeds it is pinned in session/mod.rs by
// `login_emits_zone_in_weather_after_the_zone_change`.
#[test]
fn zone_in_weather_survives_the_zone_change_clear() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::ZoneChanged {
        from: None,
        to: 103,
        myroom: None,
        mog_zone_flag: false,
    });
    s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 4 });
    assert_eq!(s.current_weather, Some(4));
}

// Stand-up cancels leavegame server-side with no 0x053 cancel packet; the
// heal→walk transition folds in as LogoutCountdownCancelled and must drop a
// live countdown. A cancel with nothing active is a no-op fold (no churn).
#[test]
fn logout_countdown_cancelled_clears_the_live_countdown() {
    let mut s = SessionState::default();
    assert!(!s.apply_event(&AgentEvent::LogoutCountdownCancelled));
    s.apply_event(&AgentEvent::LogoutCountdown {
        seconds_remaining: 25,
        shutdown: true,
    });
    assert_eq!(s.logout_countdown.map(|c| c.seconds_remaining), Some(25));
    assert!(s.apply_event(&AgentEvent::LogoutCountdownCancelled));
    assert_eq!(s.logout_countdown, None);
}

#[test]
fn widescan_list_builds_between_start_and_end_and_clears_on_zone_change() {
    let mut s = SessionState::default();
    let entry = |act: u16| WidescanEntry {
        act_index: act,
        level: 10,
        kind: 2,
        rel_x: 1,
        rel_z: 2,
        name: String::new(),
    };

    // An entry outside a build window is dropped (server frames every list).
    s.apply_event(&AgentEvent::WidescanEntryReceived { entry: entry(1) });
    assert!(s.widescan.entries.is_empty());

    s.apply_event(&AgentEvent::WidescanListStart);
    assert!(s.widescan.building);
    s.apply_event(&AgentEvent::WidescanEntryReceived { entry: entry(1) });
    s.apply_event(&AgentEvent::WidescanEntryReceived { entry: entry(2) });
    s.apply_event(&AgentEvent::WidescanListEnd);
    assert!(!s.widescan.building);
    assert_eq!(s.widescan.entries.len(), 2);

    // A fresh ListStart clears the previous list.
    s.apply_event(&AgentEvent::WidescanListStart);
    assert!(s.widescan.entries.is_empty());
    s.apply_event(&AgentEvent::WidescanEntryReceived { entry: entry(3) });
    s.apply_event(&AgentEvent::WidescanListEnd);
    assert_eq!(s.widescan.entries.len(), 1);

    let pos = WidescanPos {
        act_index: 3,
        x: 1.0,
        y: 2.0,
        z: 3.0,
    };
    s.apply_event(&AgentEvent::WidescanTrackUpdated { tracked: Some(pos) });
    assert_eq!(s.widescan.tracked, Some(pos));
    // State == Lose clears the tracked marker.
    s.apply_event(&AgentEvent::WidescanTrackUpdated { tracked: None });
    assert_eq!(s.widescan.tracked, None);

    s.apply_event(&AgentEvent::WidescanListStart);
    s.apply_event(&AgentEvent::WidescanEntryReceived { entry: entry(4) });
    s.apply_event(&AgentEvent::ZoneChanged {
        from: Some(230),
        to: 231,
        myroom: None,
        mog_zone_flag: false,
    });
    assert_eq!(
        s.widescan,
        WidescanList::default(),
        "zone change clears widescan"
    );
}

#[test]
fn zone_change_sets_and_clears_myroom() {
    let mut s = SessionState {
        char_id: Some(0xCAFE),
        ..Default::default()
    };
    let room = MyRoomInfo {
        model: 257,
        sub_map: 0,
        exit_bit: 1,
    };
    s.apply_event(&AgentEvent::ZoneChanged {
        from: None,
        to: 230,
        myroom: Some(room),
        mog_zone_flag: false,
    });
    assert_eq!(s.myroom, Some(room));
    assert!(
        s.self_in_mog_house(),
        "myroom must drive self_in_mog_house before any party attrs arrive"
    );

    s.apply_event(&AgentEvent::ZoneChanged {
        from: Some(230),
        to: 230,
        myroom: None,
        mog_zone_flag: false,
    });
    assert_eq!(s.myroom, None);
    assert!(!s.self_in_mog_house());
}

#[test]
fn job_info_and_2f_unlock_fold() {
    let mut s = SessionState::default();
    let mut job_levels = [0u8; ffxi_proto::decode::JobInfo::MAX_JOBTYPE];
    job_levels[1] = 75;
    let info = JobInfoState {
        mjob_no: 1,
        sjob_no: 3,
        unlocked: 0b1011,
        sub_job_unlocked: true,
        job_levels,
    };
    s.apply_event(&AgentEvent::JobInfoUpdated { info });
    assert_eq!(s.job_info, Some(info));

    assert_eq!(s.mh_2f_unlocked, None);
    s.apply_event(&AgentEvent::MogHouse2fUnlockUpdated { unlocked: true });
    assert_eq!(s.mh_2f_unlocked, Some(true));
}

#[test]
fn equip_updated_index_zero_clears_slot() {
    let mut s = SessionState::default();
    // Equip something in the waist slot (10).
    s.apply_event(&AgentEvent::EquipUpdated {
        slot: 10,
        container: 0,
        container_index: 7,
    });
    assert_eq!(
        s.equipment[10],
        Some(EquippedRef {
            container: 0,
            container_index: 7
        })
    );
    // Server reports an unequipped slot as inventory index 0 (= Gil); the
    // slot must clear, not point at inventory slot 0.
    s.apply_event(&AgentEvent::EquipUpdated {
        slot: 10,
        container: 0,
        container_index: 0,
    });
    assert_eq!(s.equipment[10], None, "index 0 = empty, not Gil");
}

#[test]
fn key_items_merge_across_tables_and_replace_in_place() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::KeyItemsUpdated {
        table_index: 0,
        ids: vec![1, 5],
        seen_ids: vec![1],
    });
    s.apply_event(&AgentEvent::KeyItemsUpdated {
        table_index: 1,
        ids: vec![KEY_ITEMS_PER_TABLE as u16],
        seen_ids: Vec::new(),
    });
    assert_eq!(s.key_items, vec![1, 5, KEY_ITEMS_PER_TABLE as u16]);
    assert_eq!(s.key_items_seen, vec![1]);

    s.apply_event(&AgentEvent::KeyItemsUpdated {
        table_index: 0,
        ids: vec![5],
        seen_ids: vec![5],
    });
    assert_eq!(s.key_items, vec![5, KEY_ITEMS_PER_TABLE as u16]);
    assert_eq!(s.key_items_seen, vec![5], "table 0 seen set replaced");
}

#[test]
fn key_items_seen_refresh_keeps_other_tables() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::KeyItemsUpdated {
        table_index: 1,
        ids: vec![KEY_ITEMS_PER_TABLE as u16 + 2],
        seen_ids: vec![KEY_ITEMS_PER_TABLE as u16 + 2],
    });
    let changed = s.apply_event(&AgentEvent::KeyItemsUpdated {
        table_index: 0,
        ids: vec![3],
        seen_ids: vec![3],
    });
    assert!(changed);
    assert_eq!(s.key_items_seen, vec![3, KEY_ITEMS_PER_TABLE as u16 + 2]);

    let unchanged = s.apply_event(&AgentEvent::KeyItemsUpdated {
        table_index: 0,
        ids: vec![3],
        seen_ids: vec![3],
    });
    assert!(!unchanged, "identical refresh must not report a change");
}

#[test]
fn agent_event_roundtrip() {
    let ev = AgentEvent::PositionChanged {
        pos: Position {
            pos: Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            heading: 64,
            ..Position::default()
        },
    };
    let s = serde_json::to_string(&ev).unwrap();
    let back: AgentEvent = serde_json::from_str(&s).unwrap();
    match back {
        AgentEvent::PositionChanged { pos } => {
            assert_eq!(pos.heading, 64);
            assert_eq!(pos.pos.y, 2.0);
        }
        _ => panic!("wrong variant: {back:?}"),
    }
}

#[test]
fn agent_command_roundtrip() {
    let line = r#"{"cmd":"move","x":1.0,"y":2.0,"z":3.0,"heading":42}"#;
    let cmd: AgentCommand = serde_json::from_str(line).unwrap();
    match cmd {
        AgentCommand::Move { x, y, z, heading } => {
            assert_eq!((x, y, z, heading), (1.0, 2.0, 3.0, 42));
        }
        _ => panic!("wrong variant: {cmd:?}"),
    }
}

/// The flattened op tag keeps delivery box commands one-level JSON for
/// headless agents: {"cmd":"delivery_box","op":"set",...}.
#[test]
fn delivery_box_command_roundtrip() {
    let line = r#"{"cmd":"delivery_box","op":"set","slot":0,"inventory_slot":11,"quantity":1,"recipient":"Atti"}"#;
    let cmd: AgentCommand = serde_json::from_str(line).unwrap();
    match &cmd {
        AgentCommand::DeliveryBox {
            op:
                DeliveryBoxOp::Set {
                    slot,
                    inventory_slot,
                    quantity,
                    recipient,
                },
        } => {
            assert_eq!((*slot, *inventory_slot, *quantity), (0, 11, 1));
            assert_eq!(recipient, "Atti");
        }
        _ => panic!("wrong variant: {cmd:?}"),
    }
    let back = serde_json::to_string(&cmd).unwrap();
    assert_eq!(serde_json::from_str::<AgentCommand>(&back).unwrap(), cmd);

    let line = r#"{"cmd":"delivery_box","op":"post_open"}"#;
    let cmd: AgentCommand = serde_json::from_str(line).unwrap();
    assert!(matches!(
        cmd,
        AgentCommand::DeliveryBox {
            op: DeliveryBoxOp::PostOpen
        }
    ));

    let line = r#"{"cmd":"delivery_box","op":"check","box_no":"incoming"}"#;
    let cmd: AgentCommand = serde_json::from_str(line).unwrap();
    assert!(matches!(
        cmd,
        AgentCommand::DeliveryBox {
            op: DeliveryBoxOp::Check {
                box_no: DeliveryBoxNo::Incoming
            }
        }
    ));
}

#[test]
fn action_kind_talk_decodes() {
    let line = r#"{"cmd":"action","target_id":42,"target_index":7,"kind":{"kind":"talk"}}"#;
    let cmd: AgentCommand = serde_json::from_str(line).unwrap();
    match cmd {
        AgentCommand::Action {
            target_id,
            target_index,
            kind,
        } => {
            assert_eq!((target_id, target_index), (42, 7));
            assert!(matches!(kind, ActionKind::Talk));
            assert_eq!(kind.action_id(), 0x00);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn action_kind_castmagic_fills_buf() {
    let kind = ActionKind::CastMagic {
        spell_id: 0x101,
        pos_x: 1.5,
        pos_y: 0.0,
        pos_z: -2.5,
    };
    assert_eq!(kind.action_id(), 0x03);
    let mut buf = [0u8; 16];
    kind.fill_action_buf(&mut buf);
    assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0x101);
    assert_eq!(f32::from_le_bytes(buf[4..8].try_into().unwrap()), 1.5);

    assert_eq!(f32::from_le_bytes(buf[8..12].try_into().unwrap()), -2.5);
    assert_eq!(f32::from_le_bytes(buf[12..16].try_into().unwrap()), 0.0);
}

#[test]
fn action_kind_weaponskill_fills_skill_id() {
    let kind = ActionKind::Weaponskill { skill_id: 0xCAFE };
    assert_eq!(kind.action_id(), 0x07);
    let mut buf = [0u8; 16];
    kind.fill_action_buf(&mut buf);
    assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0xCAFE);

    assert!(buf[4..].iter().all(|&b| b == 0));
}

#[test]
fn party_member_upsert_preserves_name_across_attr_only_update() {
    let mut s = SessionState::default();
    let from_list = PartyMember {
        id: 42,
        act_index: 7,
        name: Some("Vanari".into()),
        hp: 2000,
        mp: 100,
        tp: 0,
        hp_pct: 100,
        mp_pct: 100,
        zone_no: 230,
        main_job: 1,
        main_job_lv: 75,
        sub_job: 6,
        sub_job_lv: 37,
        is_party_leader: true,
        is_alliance_leader: false,
        in_mog_house: false,
        party_no: 0,
    };
    s.apply_event(&AgentEvent::PartyMemberUpdated { member: from_list });
    assert_eq!(s.party.len(), 1);
    assert_eq!(s.party[0].name.as_deref(), Some("Vanari"));
    assert!(s.party[0].is_party_leader);

    let from_attr = PartyMember {
        id: 42,
        act_index: 7,
        name: None,
        hp: 1500,
        mp: 100,
        tp: 1234,
        hp_pct: 75,
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
    };
    s.apply_event(&AgentEvent::PartyMemberUpdated { member: from_attr });
    assert_eq!(s.party.len(), 1, "upsert by id");
    assert_eq!(s.party[0].name.as_deref(), Some("Vanari"), "name preserved");
    assert!(s.party[0].is_party_leader, "leader preserved");
    assert_eq!(s.party[0].hp, 1500, "HP overwritten");
    assert_eq!(s.party[0].hp_pct, 75);
}

fn party_member(id: u32, name: &str, hp: u32) -> PartyMember {
    PartyMember {
        id,
        act_index: 1,
        name: Some(name.into()),
        hp,
        mp: 50,
        tp: 0,
        hp_pct: 80,
        mp_pct: 100,
        zone_no: 230,
        main_job: 1,
        main_job_lv: 75,
        sub_job: 0,
        sub_job_lv: 0,
        is_party_leader: id == 42,
        is_alliance_leader: false,
        in_mog_house: false,
        party_no: 0,
    }
}

#[test]
fn party_table_reset_solo_empty_table_keeps_self() {
    // LSB answers a solo player's 0x076 with GROUP_TBL(nullptr): Kind 0, zero
    // entries. Self is not in the table and its only stats source is
    // GROUP_ATTR, so an empty reset must not wipe self.
    let mut s = SessionState {
        char_id: Some(42),
        ..Default::default()
    };
    s.apply_event(&AgentEvent::PartyMemberUpdated {
        member: party_member(42, "Sylvie", 1500),
    });

    let changed = s.apply_event(&AgentEvent::PartyTableReset { members: vec![] });
    assert!(!changed, "no-op reset reports no change");
    assert_eq!(s.party.len(), 1);
    assert_eq!(s.party[0].id, 42);
    assert_eq!(s.party[0].hp, 1500, "self stats survive the empty table");
}

#[test]
fn party_table_reset_drops_unlisted_keeps_stats_seeds_skeletons() {
    let mut s = SessionState {
        char_id: Some(42),
        ..Default::default()
    };
    for m in [
        party_member(42, "Sylvie", 1500),
        party_member(7, "Vanari", 900),
        party_member(99, "LeftTheParty", 300),
    ] {
        s.apply_event(&AgentEvent::PartyMemberUpdated { member: m });
    }

    use ffxi_proto::decode::GroupTblEntry;
    let changed = s.apply_event(&AgentEvent::PartyTableReset {
        members: vec![
            GroupTblEntry {
                unique_no: 42,
                act_index: 3,
                party_no: 0,
                is_party_leader: true,
                is_alliance_leader: false,
                zone_no: 235,
            },
            GroupTblEntry {
                unique_no: 7,
                act_index: 9,
                party_no: 1,
                is_party_leader: false,
                is_alliance_leader: true,
                zone_no: 0,
            },
            GroupTblEntry {
                unique_no: 55,
                act_index: 4,
                party_no: 0,
                is_party_leader: false,
                is_alliance_leader: false,
                zone_no: 235,
            },
        ],
    });
    assert!(changed);

    let by_id = |id: u32| {
        s.party
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("missing {id}"))
    };
    assert_eq!(s.party.len(), 3, "unlisted member dropped, new id seeded");
    assert!(s.party.iter().all(|m| m.id != 99), "stale member gone");

    let self_row = by_id(42);
    assert_eq!(
        self_row.hp, 1500,
        "listed member keeps stats until the 0x0DD burst"
    );
    assert_eq!(self_row.name.as_deref(), Some("Sylvie"));
    assert_eq!(
        self_row.act_index, 3,
        "roster fields refreshed from the table"
    );
    assert_eq!(self_row.zone_no, 235);

    let mate = by_id(7);
    assert_eq!(mate.party_no, 1);
    assert!(mate.is_alliance_leader);
    assert!(!mate.is_party_leader);

    let skeleton = by_id(55);
    assert_eq!(
        skeleton.name, None,
        "new id is a skeleton row until its 0x0DD lands"
    );
    assert_eq!(skeleton.hp, 0);
}

#[test]
fn action_kind_raise_menu_accept_zero_reject_one() {
    let mut buf = [0u8; 16];
    ActionKind::RaiseMenu { accept: true }.fill_action_buf(&mut buf);
    assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 0);
    ActionKind::RaiseMenu { accept: false }.fill_action_buf(&mut buf);
    assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), 1);
}

#[test]
fn apply_event_folds_in_documented_order() {
    let mut s = SessionState::default();
    assert_eq!(s.stage, Stage::Idle);

    s.apply_event(&AgentEvent::StageChanged {
        stage: Stage::Authenticating,
    });
    assert_eq!(s.stage, Stage::Authenticating);
    assert_eq!(s.diagnostics.stage, Some(Stage::Authenticating));

    s.apply_event(&AgentEvent::Connected {
        account_id: 42,
        char_id: 7,
        character: "Tester".into(),
        zone_id: 100,
    });
    assert_eq!(s.account_id, Some(42));
    assert_eq!(s.char_id, Some(7));
    assert_eq!(s.character.as_deref(), Some("Tester"));
    assert_eq!(s.zone_id, Some(100));

    s.apply_event(&AgentEvent::EntityUpserted {
        entity: Entity {
            id: 999,
            act_index: 1,
            kind: EntityKind::Pc,
            name: Some("Other".into()),
            pos: Vec3 {
                x: 1.0,
                y: 0.0,
                z: 2.0,
            },
            heading: 64,
            hp_pct: Some(80),
            bt_target_id: 0,
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            npc_state: None,
            status: 0,
            char_flags: Default::default(),
            mount_id: None,
        },
        pos_present: true,
    });
    assert_eq!(s.entities.len(), 1);

    s.apply_event(&AgentEvent::EntityUpserted {
        entity: Entity {
            id: 999,
            act_index: 1,
            kind: EntityKind::Pc,
            name: Some("Other".into()),
            pos: Vec3 {
                x: 5.0,
                y: 0.0,
                z: 6.0,
            },
            heading: 32,
            hp_pct: Some(50),
            bt_target_id: 0,
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed: 0,
            speed_base: 0,
            look: None,
            npc_state: None,
            status: 0,
            char_flags: Default::default(),
            mount_id: None,
        },
        pos_present: true,
    });
    assert_eq!(s.entities.len(), 1, "upsert must not duplicate by id");
    assert_eq!(s.entities[0].pos.x, 5.0, "upsert must overwrite");

    s.party.push(PartyMember {
        id: 1,
        act_index: 1,
        name: None,
        hp: 0,
        mp: 0,
        tp: 0,
        hp_pct: 0,
        mp_pct: 100,
        zone_no: 100,
        main_job: 0,
        main_job_lv: 0,
        sub_job: 0,
        sub_job_lv: 0,
        is_party_leader: true,
        is_alliance_leader: false,
        in_mog_house: false,
        party_no: 0,
    });
    s.apply_event(&AgentEvent::ZoneChanged {
        from: Some(100),
        to: 230,
        myroom: None,
        mog_zone_flag: false,
    });
    assert_eq!(s.zone_id, Some(230));
    assert!(
        s.entities.is_empty(),
        "zone change must clear stale entities"
    );
    assert!(
        s.party.is_empty(),
        "zone change must clear stale party (avoids stale dead-state on home-point warp)"
    );

    s.apply_event(&AgentEvent::Disconnected {
        reason: "test".into(),
    });
    assert_eq!(s.stage, Stage::Disconnected);
}

#[test]
fn merge_kind_specialized_wins_over_other() {
    use EntityKind::*;

    assert_eq!(merge_kind(Pc, Other), Pc);
    assert_eq!(merge_kind(Npc, Other), Npc);
    assert_eq!(merge_kind(Mob, Other), Mob);
    assert_eq!(merge_kind(Pet, Other), Pet);

    assert_eq!(merge_kind(Other, Pet), Pet);
    assert_eq!(merge_kind(Other, Npc), Npc);

    assert_eq!(merge_kind(Npc, Pet), Pet);
    assert_eq!(merge_kind(Pet, Npc), Npc);

    assert_eq!(merge_kind(Other, Other), Other);
}

fn make_test_entity(id: u32, name: Option<&str>, kind: EntityKind) -> Entity {
    Entity {
        id,
        act_index: id as u16,
        kind,
        name: name.map(str::to_string),
        pos: Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        heading: 0,
        hp_pct: Some(100),
        bt_target_id: 0,
        face_target: 0,
        name_vis: None,
        claim_id: 0,
        speed: 0,
        speed_base: 0,
        look: None,
        npc_state: None,
        status: 0,
        char_flags: Default::default(),
        mount_id: None,
    }
}

#[test]
fn entity_upserted_preserves_name_across_attr_only_update() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(42, Some("Sigli-Sea"), EntityKind::Npc),
        pos_present: true,
    });
    assert_eq!(s.entities[0].name.as_deref(), Some("Sigli-Sea"));

    s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(42, None, EntityKind::Npc),
        pos_present: true,
    });
    assert_eq!(
        s.entities[0].name.as_deref(),
        Some("Sigli-Sea"),
        "name must persist across attr-only update"
    );
}

#[test]
fn entity_upserted_status_refreshes_on_pos_only_tick() {
    let mut s = SessionState::default();
    let mut ent = make_test_entity(42, Some("Antlion"), EntityKind::Mob);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent.clone(),
        pos_present: true,
    });
    assert_eq!(s.entities[0].status, 0, "spawns NORMAL");

    ent.npc_state = None;
    ent.status = 3;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert_eq!(
        s.entities[0].status, 3,
        "a pos-only tick (no npc_state) must still refresh STATUS_TYPE"
    );
}

#[test]
fn entity_upserted_preserves_position_when_pos_absent() {
    let mut s = SessionState::default();
    let mut ent = make_test_entity(42, Some("Tunnel Worm"), EntityKind::Mob);
    ent.pos = Vec3 {
        x: 123.0,
        y: 4.0,
        z: -89.0,
    };
    ent.heading = 200;
    ent.speed = 40;
    ent.speed_base = 40;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert_eq!(s.entities[0].pos.x, 123.0);

    let mut hp_only = make_test_entity(42, None, EntityKind::Mob);
    hp_only.pos = Vec3::default();
    hp_only.heading = 0;
    hp_only.speed = 0;
    hp_only.speed_base = 0;
    hp_only.hp_pct = Some(75);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: hp_only,
        pos_present: false,
    });
    assert_eq!(
        s.entities[0].pos,
        Vec3 {
            x: 123.0,
            y: 4.0,
            z: -89.0
        },
        "position must persist across a non-UPDATE_POS tick (no teleport to origin)"
    );
    assert_eq!(s.entities[0].heading, 200, "heading must persist too");
    assert_eq!(s.entities[0].speed, 40, "speed must persist too");
    assert_eq!(
        s.entities[0].hp_pct,
        Some(75),
        "the HP this tick *did* carry must still apply"
    );

    let mut moved = make_test_entity(42, None, EntityKind::Mob);
    moved.pos = Vec3 {
        x: 200.0,
        y: 4.0,
        z: -50.0,
    };
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: moved,
        pos_present: true,
    });
    assert_eq!(
        s.entities[0].pos.x, 200.0,
        "a genuine position update must overwrite, not get preserved"
    );
}

#[test]
fn entity_upserted_preserves_hp_pct_across_position_only_update() {
    let mut s = SessionState::default();
    let mut ent = make_test_entity(42, Some("Worker Bee"), EntityKind::Npc);
    ent.hp_pct = Some(50);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert_eq!(s.entities[0].hp_pct, Some(50));

    let mut pos_only = make_test_entity(42, None, EntityKind::Npc);
    pos_only.hp_pct = None;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: pos_only,
        pos_present: true,
    });
    assert_eq!(
        s.entities[0].hp_pct,
        Some(50),
        "hp_pct must persist across UPDATE_POS-only follow-up (no UPDATE_HP bit set)"
    );

    let mut died = make_test_entity(42, None, EntityKind::Npc);
    died.hp_pct = Some(0);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: died,
        pos_present: true,
    });
    assert_eq!(
        s.entities[0].hp_pct,
        Some(0),
        "Some(0) (mob died) must overwrite, not get preserved as Some(50)"
    );
}

#[test]
fn entity_upserted_name_vis_survives_pos_only_tick() {
    // #512-4: namevis is written under UPDATE_HP (entity_update.cpp:357/:408), and a
    // POS-only 0x00E carries the byte zero-filled. Merging off pos_present would
    // un-hide a hidden entity the moment it moved.
    let mut s = SessionState::default();
    let mut ent = make_test_entity(42, Some("Survival Guide"), EntityKind::Npc);
    ent.name_vis = Some(0x08); // FLAG_HIDE_NAME
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert_eq!(s.entities[0].name_vis, Some(0x08));

    let mut moved = make_test_entity(42, None, EntityKind::Npc); // POS-only: no namevis byte
    moved.pos = Vec3 {
        x: 50.0,
        y: 1.0,
        z: -20.0,
    };
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: moved,
        pos_present: true,
    });
    assert_eq!(
        s.entities[0].name_vis,
        Some(0x08),
        "a POS-only tick must not un-hide a hidden entity (zero-filled byte is not data)"
    );
}

#[test]
fn entity_upserted_name_vis_applies_on_hp_only_tick() {
    // #512-4: HideName(true) sets UPDATE_HP, not UPDATE_POS. Merging off pos_present
    // kept the stale visible value for a static NPC the server just hid.
    let mut s = SessionState::default();
    let ent = make_test_entity(42, Some("Unity Master"), EntityKind::Npc);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert_eq!(s.entities[0].name_vis, None);

    let mut hidden = make_test_entity(42, None, EntityKind::Npc);
    hidden.name_vis = Some(0x08); // HideName(true) -> updatemask |= UPDATE_HP only
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: hidden,
        pos_present: false,
    });
    assert_eq!(
        s.entities[0].name_vis,
        Some(0x08),
        "an HP-only tick must apply the new namevis even without UPDATE_POS"
    );
}

#[test]
fn entity_upserted_preserves_look_across_position_only_update() {
    use ffxi_proto::decode::LookData;
    let mut s = SessionState::default();
    let mut ent = make_test_entity(42, Some("Jonisbarius"), EntityKind::Pc);
    let look = LookData::Equipped {
        face: 3,
        race: 3,
        head: 0x1000,
        body: 0x2004,
        hands: 0x3000,
        legs: 0x4000,
        feet: 0x5000,
        main: 0x6000,
        sub: 0x7000,
        ranged: 0x8000,
    };
    ent.look = Some(look);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert!(matches!(
        s.entities[0].look,
        Some(LookData::Equipped { race: 3, .. })
    ));

    let mut pos_only = make_test_entity(42, None, EntityKind::Pc);
    pos_only.look = None;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: pos_only,
        pos_present: true,
    });
    assert!(
        matches!(s.entities[0].look, Some(LookData::Equipped { race: 3, .. })),
        "look must persist across position-only refresh (no look bits set)"
    );

    let mut changed = make_test_entity(42, None, EntityKind::Pc);
    changed.look = Some(LookData::Standard { modelid: 99 });
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: changed,
        pos_present: true,
    });
    assert!(
        matches!(s.entities[0].look, Some(LookData::Standard { modelid: 99 })),
        "Some(new_look) must overwrite, not get preserved as the prior Equipped value"
    );
}

const SELF_CHAR_ID: u32 = 7;

fn self_test_look() -> ffxi_proto::decode::LookData {
    ffxi_proto::decode::LookData::Equipped {
        face: 3,
        race: 3,
        head: 0x011,
        body: 0x022,
        hands: 0x033,
        legs: 0x044,
        feet: 0x055,
        main: 0x066,
        sub: 0x077,
        ranged: 0x088,
    }
}

fn connected_self_state() -> SessionState {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::Connected {
        account_id: 42,
        char_id: SELF_CHAR_ID,
        character: "Tester".into(),
        zone_id: 100,
    });
    s
}

#[test]
fn self_look_updated_sets_look_on_self_entity() {
    let mut s = connected_self_state();
    let mut ent = make_test_entity(SELF_CHAR_ID, Some("Tester"), EntityKind::Pc);
    ent.look = None;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });

    let dirty = s.apply_event(&AgentEvent::SelfLookUpdated {
        look: self_test_look(),
    });
    assert!(dirty, "a new self look must mark the state dirty");
    assert_eq!(s.entities[0].look, Some(self_test_look()));
}

#[test]
fn self_look_latch_applies_when_grap_list_precedes_self_entity() {
    let mut s = connected_self_state();
    s.apply_event(&AgentEvent::SelfLookUpdated {
        look: self_test_look(),
    });

    let mut ent = make_test_entity(SELF_CHAR_ID, Some("Tester"), EntityKind::Pc);
    ent.look = None;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert_eq!(s.entities[0].look, Some(self_test_look()));
}

#[test]
fn self_look_survives_pos_only_upsert() {
    let mut s = connected_self_state();
    s.apply_event(&AgentEvent::SelfLookUpdated {
        look: self_test_look(),
    });
    let mut ent = make_test_entity(SELF_CHAR_ID, Some("Tester"), EntityKind::Pc);
    ent.look = None;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });

    let mut pos_only = make_test_entity(SELF_CHAR_ID, None, EntityKind::Pc);
    pos_only.look = None;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: pos_only,
        pos_present: true,
    });
    assert_eq!(s.entities[0].look, Some(self_test_look()));
}

#[test]
fn self_look_latch_does_not_leak_to_other_entities() {
    let mut s = connected_self_state();
    s.apply_event(&AgentEvent::SelfLookUpdated {
        look: self_test_look(),
    });
    let mut other = make_test_entity(SELF_CHAR_ID + 1, Some("Someone"), EntityKind::Pc);
    other.look = None;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: other,
        pos_present: true,
    });
    assert_eq!(s.entities[0].look, None);
}

#[test]
fn entity_upserted_specialized_kind_resists_demotion_to_other() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(7, Some("Stout Servitor"), EntityKind::Npc),
        pos_present: true,
    });
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(7, Some("Stout Servitor"), EntityKind::Other),
        pos_present: true,
    });
    assert_eq!(s.entities[0].kind, EntityKind::Npc);
}

#[test]
fn entity_patched_by_id_sets_name_on_existing_entity() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(99, None, EntityKind::Other),
        pos_present: true,
    });
    s.apply_event(&AgentEvent::EntityPatched {
        id: Some(99),
        act_index: None,
        name: Some("Mihli Aliapoh".into()),
        kind: Some(EntityKind::Pet),
        hp_pct: None,
    });
    assert_eq!(s.entities[0].name.as_deref(), Some("Mihli Aliapoh"));
    assert_eq!(s.entities[0].kind, EntityKind::Pet);
}

#[test]
fn entity_patched_by_act_index_resolves_when_id_unknown() {
    let mut s = SessionState::default();
    let mut ent = make_test_entity(0xABCD, None, EntityKind::Other);
    ent.act_index = 0x07A5;
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    s.apply_event(&AgentEvent::EntityPatched {
        id: None,
        act_index: Some(0x07A5),
        name: Some("Crab Familiar".into()),
        kind: Some(EntityKind::Pet),
        hp_pct: Some(75),
    });
    assert_eq!(s.entities[0].name.as_deref(), Some("Crab Familiar"));
    assert_eq!(s.entities[0].kind, EntityKind::Pet);
    assert_eq!(s.entities[0].hp_pct, Some(75));
}

#[test]
fn name_extraction_miss_appends_to_ring_buffer_with_cap() {
    let mut s = SessionState::default();

    for i in 0..(NAME_MISSES_CAP as u32 + 5) {
        s.apply_event(&AgentEvent::NameExtractionMiss {
            miss: NameExtractionMiss {
                opcode: 0x00E,
                unique_no: i,
                act_index: i as u16,
                send_flag: 0,
                body_len: 64,
                body_hex: format!("{:02x}", i & 0xFF),
                miss_kind: NameMissKind::NameBitClear,
                at_unix_ms: 1000 + u64::from(i),
            },
        });
    }
    assert_eq!(s.name_misses.len(), NAME_MISSES_CAP);

    assert_eq!(s.name_misses.front().unwrap().unique_no, 5);

    assert_eq!(
        s.name_misses.back().unwrap().unique_no,
        NAME_MISSES_CAP as u32 + 4
    );
}

#[test]
fn name_extraction_miss_round_trips_serde() {
    let miss = NameExtractionMiss {
        opcode: 0x00D,
        unique_no: 0x0102_0304,
        act_index: 0x07A5,
        send_flag: 0x09,
        body_len: 72,
        body_hex: "deadbeef".into(),
        miss_kind: NameMissKind::NameBitSetExtractionFailed,
        at_unix_ms: 1_700_000_000_123,
    };
    let s = serde_json::to_string(&miss).unwrap();
    let back: NameExtractionMiss = serde_json::from_str(&s).unwrap();
    assert_eq!(back.unique_no, 0x0102_0304);
    assert_eq!(back.miss_kind, NameMissKind::NameBitSetExtractionFailed);

    assert!(s.contains("name_bit_set_extraction_failed"));
}

#[test]
fn entity_patched_for_unknown_entity_is_dropped() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::EntityPatched {
        id: Some(1234),
        act_index: None,
        name: Some("Ghost".into()),
        kind: Some(EntityKind::Pet),
        hp_pct: None,
    });
    assert!(s.entities.is_empty());
}

#[test]
fn heal_command_roundtrip() {
    for (line, expect) in [
        (r#"{"cmd":"heal","mode":"toggle"}"#, HealMode::Toggle),
        (r#"{"cmd":"heal","mode":"on"}"#, HealMode::On),
        (r#"{"cmd":"heal","mode":"off"}"#, HealMode::Off),
    ] {
        let cmd: AgentCommand = serde_json::from_str(line).unwrap();
        match cmd {
            AgentCommand::Heal { mode } => assert_eq!(mode, expect, "for line {line}"),
            _ => panic!("wrong variant for {line}: {cmd:?}"),
        }
    }
}

#[test]
fn use_item_command_roundtrip() {
    let line = r#"{"cmd":"use_item","container":0,"slot":3,"item_no":4112,"target_id":42,"target_index":7}"#;
    let cmd: AgentCommand = serde_json::from_str(line).unwrap();
    match cmd {
        AgentCommand::UseItem {
            container,
            slot,
            item_no,
            target_id,
            target_index,
        } => {
            assert_eq!(
                (container, slot, item_no, target_id, target_index),
                (0, 3, 4112, 42, 7)
            );
        }
        _ => panic!("wrong variant: {cmd:?}"),
    }
}

#[test]
fn bank_when_full_command_roundtrip() {
    let line = r#"{"cmd":"bank_when_full","threshold":90,"mog_house_zoneline":12345}"#;
    let cmd: AgentCommand = serde_json::from_str(line).unwrap();
    match cmd {
        AgentCommand::BankWhenFull {
            threshold,
            mog_house_zoneline,
        } => {
            assert_eq!((threshold, mog_house_zoneline), (90, 12345));
        }
        _ => panic!("wrong variant: {cmd:?}"),
    }
}

#[test]
fn reactor_goal_changed_event_roundtrip() {
    let ev = AgentEvent::ReactorGoalChanged {
        goal: ReactorGoalSnapshot::Engaged {
            target_id: 99,
            attack_issued: true,
        },
    };
    let s = serde_json::to_string(&ev).unwrap();
    let back: AgentEvent = serde_json::from_str(&s).unwrap();
    match back {
        AgentEvent::ReactorGoalChanged {
            goal:
                ReactorGoalSnapshot::Engaged {
                    target_id,
                    attack_issued,
                },
        } => {
            assert_eq!(target_id, 99);
            assert!(attack_issued);
        }
        other => panic!("wrong shape: {other:?}"),
    }
}

#[test]
fn reconnected_fold_writes_last_reconnect() {
    let mut s = SessionState::default();
    assert!(s.last_reconnect.is_none());
    s.apply_event(&AgentEvent::Reconnected { downtime_ms: 1234 });
    let info = s.last_reconnect.expect("set");
    assert_eq!(info.downtime_ms, 1234);
    assert!(info.at_unix_ms > 0, "wall-clock stamped");
}

#[test]
fn self_position_returns_self_entity_pos() {
    let mut s = SessionState::default();

    assert!(s.self_position().is_none());

    s.apply_event(&AgentEvent::Connected {
        account_id: 1,
        char_id: 99,
        character: "Self".into(),
        zone_id: 230,
    });

    assert!(s.self_position().is_none());

    s.apply_event(&AgentEvent::EntityUpserted {
        entity: Entity {
            id: 99,
            act_index: 5,
            kind: EntityKind::Pc,
            name: Some("Self".into()),
            pos: Vec3 {
                x: 10.0,
                y: 20.0,
                z: 30.0,
            },
            heading: 64,
            hp_pct: Some(100),
            bt_target_id: 0,
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed: 40,
            speed_base: 40,
            look: None,
            npc_state: None,
            status: 0,
            char_flags: Default::default(),
            mount_id: None,
        },
        pos_present: true,
    });
    let p = s.self_position().expect("self entity present");
    assert_eq!(
        p.pos,
        Vec3 {
            x: 10.0,
            y: 20.0,
            z: 30.0
        }
    );
    assert_eq!(p.heading, 64);
    assert_eq!(p.speed, 40);
    assert_eq!(p.speed_base, 40);

    s.apply_event(&AgentEvent::PositionChanged {
        pos: Position {
            pos: Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            heading: 32,
            speed: 25,
            speed_base: 25,
        },
    });
    let p = s.self_position().expect("self entity present");
    assert_eq!(
        p.pos,
        Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0
        }
    );
    assert_eq!(p.heading, 32);
}

#[test]
fn reactor_goal_changed_fold_writes_current_goal() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::ReactorGoalChanged {
        goal: ReactorGoalSnapshot::Following {
            target_id: 42,
            distance: 3.0,
        },
    });
    match s.current_goal {
        Some(ReactorGoalSnapshot::Following {
            target_id,
            distance,
        }) => {
            assert_eq!(target_id, 42);
            assert!((distance - 3.0).abs() < 1e-3);
        }
        other => panic!("expected Following, got {other:?}"),
    }
}

#[test]
fn inventory_ready_sets_all_loaded() {
    let mut s = SessionState::default();
    assert!(!s.inventory.all_loaded);
    s.apply_event(&AgentEvent::InventoryReady);
    assert!(s.inventory.all_loaded);
}

#[test]
fn inventory_fold_capacities_writes_each_container() {
    let mut s = SessionState::default();
    let mut caps = vec![0u16; 18];
    caps[0] = 80;
    caps[1] = 200;
    caps[5] = 30;
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::Capacities { capacities: caps },
    });
    assert_eq!(s.inventory.containers[&0].capacity, 80);
    assert_eq!(s.inventory.containers[&1].capacity, 200);
    assert_eq!(s.inventory.containers[&5].capacity, 30);

    // A later 0 must land too — it is LSB's "container disabled" sentinel
    // (e.g. a lapsed Mog Locker lease), not an absence of data.
    let mut caps = vec![0u16; 18];
    caps[0] = 80;
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::Capacities { capacities: caps },
    });
    assert_eq!(
        s.inventory.containers[&5].capacity, 0,
        "capacity grants must not be sticky"
    );
}

#[test]
fn inventory_fold_slot_changed_inserts_then_updates_then_removes() {
    let mut s = SessionState::default();
    let slot = ItemSlot {
        index: 3,
        item_no: 4112,
        quantity: 5,
        locked: false,
        price: 0,
        charges_remaining: None,
        next_use_vana_ts: None,
    };

    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::SlotChanged { slot: slot.clone() },
    });
    assert_eq!(s.inventory.containers[&0].slots.len(), 1);
    assert_eq!(s.inventory.containers[&0].slots[0].quantity, 5);

    let mut updated = slot.clone();
    updated.quantity = 12;
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::SlotChanged { slot: updated },
    });
    assert_eq!(s.inventory.containers[&0].slots.len(), 1, "no duplication");
    assert_eq!(s.inventory.containers[&0].slots[0].quantity, 12);

    let mut removed = slot.clone();
    removed.quantity = 0;
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::SlotChanged { slot: removed },
    });
    assert!(s.inventory.containers[&0].slots.is_empty());
}

#[test]
fn inventory_fold_quantity_changed_updates_existing_slot_only() {
    let mut s = SessionState::default();

    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::QuantityChanged {
            index: 7,
            quantity: 99,
            locked: false,
        },
    });

    assert!(
        s.inventory
            .containers
            .get(&0)
            .map(|c| c.slots.is_empty())
            .unwrap_or(true),
        "ITEM_NUM without prior ITEM_LIST drops"
    );

    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::SlotChanged {
            slot: ItemSlot {
                index: 7,
                item_no: 4112,
                quantity: 1,
                locked: false,
                price: 0,
                charges_remaining: None,
                next_use_vana_ts: None,
            },
        },
    });
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::QuantityChanged {
            index: 7,
            quantity: 25,
            locked: true,
        },
    });
    let slot = &s.inventory.containers[&0].slots[0];
    assert_eq!(slot.quantity, 25);
    assert!(slot.locked, "lock flag updated");
    assert_eq!(slot.item_no, 4112, "item_no preserved (qty-only update)");
}

#[test]
fn auction_events_fold_open_browse_history_and_busy() {
    let mut s = SessionState::default();
    assert!(!s.auction.open);

    assert!(s.apply_event(&AgentEvent::AuctionMenuOpened));
    assert!(s.auction.open);

    s.apply_event(&AgentEvent::AuctionOpStarted {
        op: AuctionBusy::Downloading,
    });
    assert_eq!(s.auction.busy, Some(AuctionBusy::Downloading));

    s.apply_event(&AgentEvent::AuctionBrowseResults {
        category: 35,
        total: 2,
        listings: vec![AhListingView {
            item_id: 4096,
            singles_for_sale: 3,
            stacks_for_sale: Some(1),
        }],
    });
    let browse = s.auction.browse.as_ref().expect("catalog stored");
    assert_eq!(browse.category, 35);
    assert_eq!(browse.listings[0].singles_for_sale, 3);
    assert_eq!(s.auction.busy, None, "results clear the spinner");

    s.apply_event(&AgentEvent::AuctionOpStarted {
        op: AuctionBusy::Downloading,
    });
    s.apply_event(&AgentEvent::AuctionSearchFailed {
        message: "connection refused".into(),
    });
    assert_eq!(s.auction.busy, None, "failure clears the spinner");

    s.apply_event(&AgentEvent::AuctionHistoryResults {
        history: AhHistoryView {
            item_id: 4096,
            stack: true,
            open_listings: 4,
            category: 35,
            sales: vec![AhSaleView {
                price: 1180,
                sell_date: 1_754_000_000,
                seller: "Atti".into(),
                buyer: "Verilight".into(),
            }],
        },
    });
    assert_eq!(s.auction.history.as_ref().unwrap().sales.len(), 1);

    s.apply_event(&AgentEvent::AuctionOpStarted {
        op: AuctionBusy::PlacingBid,
    });
    s.apply_event(&AgentEvent::AuctionBidResult {
        ok: false,
        item_no: 17440,
        price: 490,
        quantity: 1,
        result: 0xC5,
    });
    assert_eq!(s.auction.busy, None, "bid verdict clears the spinner");
}

#[test]
fn auction_sell_quote_sales_slots_and_zone_reset() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::AuctionMenuOpened);

    let quote = AhFeeQuote {
        fee: 9,
        inventory_slot: 5,
        item_no: 4570,
        stack: true,
        asking_price: 1180,
    };
    s.apply_event(&AgentEvent::AuctionSellQuote {
        quote: Some(quote),
        result: ffxi_proto::decode::AUCTION_RESULT_OPEN,
    });
    assert_eq!(s.auction.fee_quote, Some(quote));

    s.apply_event(&AgentEvent::AuctionSellResult {
        ok: true,
        result: ffxi_proto::decode::AUCTION_RESULT_OPEN,
    });
    assert_eq!(s.auction.fee_quote, None, "listed sale consumes the quote");

    let sale = AhSaleStatus {
        stat: ffxi_proto::decode::AUCTION_SALE_STAT_LISTED,
        item_no: 4570,
        quantity: 12,
        price: 1180,
        timestamp: 0,
    };
    s.apply_event(&AgentEvent::AuctionSalesSlot {
        slot: 3,
        sale: Some(sale.clone()),
    });
    assert_eq!(s.auction.sales_status[3], Some(sale));

    s.apply_event(&AgentEvent::AuctionSalesStatusReset { result: 246 });
    assert!(
        s.auction.sales_status[3].is_some(),
        "throttled Info keeps slots"
    );
    s.apply_event(&AgentEvent::AuctionSalesStatusReset {
        result: ffxi_proto::decode::AUCTION_RESULT_OPEN,
    });
    assert!(s.auction.sales_status.iter().all(Option::is_none));

    assert!(!s.apply_event(&AgentEvent::AuctionSalesSlot {
        slot: AUCTION_SLOTS as u8,
        sale: None,
    }));

    s.apply_event(&AgentEvent::ZoneChanged {
        from: Some(234),
        to: 235,
        myroom: None,
        mog_zone_flag: false,
    });
    assert_eq!(s.auction, AuctionState::default(), "AH is zone-local");
}

#[test]
fn check_result_accumulates_equipment_batches_and_general() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::CheckEquipReceived {
        target_id: 0xCAFE,
        act_index: 0x123,
        items: vec![(0, 17440), (4, 12511)],
    });
    s.apply_event(&AgentEvent::CheckEquipReceived {
        target_id: 0xCAFE,
        act_index: 0x123,
        items: vec![(15, 13465)],
    });
    s.apply_event(&AgentEvent::CheckGeneralReceived {
        target_id: 0xCAFE,
        act_index: 0x123,
        main_job: 1,
        sub_job: 13,
        main_job_lv: 75,
        sub_job_lv: 37,
        master_lv: 0,
        linkshell: "Kuluu".into(),
    });
    let r = s.check_result.as_ref().expect("accumulated");
    assert_eq!(r.target_id, 0xCAFE);
    assert_eq!(r.equipped[0], Some(17440), "batch 1 Main");
    assert_eq!(r.equipped[4], Some(12511), "batch 1 Head");
    assert_eq!(r.equipped[15], Some(13465), "batch 2 Back");
    assert_eq!(r.equipped[1], None, "unsent slot stays empty");
    assert_eq!((r.main_job, r.main_job_lv), (1, 75));
    assert_eq!((r.sub_job, r.sub_job_lv), (13, 37));
    assert_eq!(r.linkshell, "Kuluu");
}

#[test]
fn check_message_is_kept_beside_the_result_and_cleared_with_it() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::CheckMessageReceived {
        name: "Aliya".into(),
        message: "Sneak oil 2k".into(),
    });
    let m = s.check_message.as_ref().expect("message stored");
    assert_eq!(m.name, "Aliya");
    assert_eq!(m.message, "Sneak oil 2k");

    // The 0x0CA lands before the 0x0C9 batches
    // (0x0dd_equip_inspect.cpp:134-136), so a later result must not drop it.
    s.apply_event(&AgentEvent::CheckEquipReceived {
        target_id: 0xCAFE,
        act_index: 0x123,
        items: vec![(0, 17440)],
    });
    assert!(s.check_message.is_some(), "gear batch keeps the message");

    s.apply_event(&AgentEvent::CheckCleared);
    assert!(s.check_message.is_none(), "a fresh /check drops it");
}

#[test]
fn bazaar_rows_merge_by_slot_and_sold_out_rows_leave() {
    let mut s = SessionState::default();
    let row = |index: u8, quantity: u32, price: u32| AgentEvent::BazaarItemReceived {
        index,
        item_no: 4096,
        quantity,
        price,
        tax_rate: 500,
    };

    assert!(
        !s.apply_event(&row(3, 5, 100)),
        "a row with no open bazaar is dropped"
    );

    s.apply_event(&AgentEvent::BazaarOpened {
        seller_id: 0xCAFE,
        seller_index: 0x123,
        seller_name: "Aliya".into(),
    });
    s.apply_event(&row(5, 2, 900));
    s.apply_event(&row(3, 5, 100));
    let view = s.bazaar.as_ref().expect("open");
    assert_eq!(
        view.items.iter().map(|i| i.index).collect::<Vec<_>>(),
        vec![3, 5],
        "rows sort by seller slot"
    );

    // The post-purchase refresh re-sends the same slot rather than adding one.
    s.apply_event(&row(3, 4, 100));
    let view = s.bazaar.as_ref().expect("open");
    assert_eq!(view.items.len(), 2, "same slot merges");
    assert_eq!(view.items[0].quantity, 4);

    // A depleted slot comes back priced 0 (0x106_bazaar_buy.cpp:198).
    s.apply_event(&row(3, 0, 0));
    let view = s.bazaar.as_ref().expect("open");
    assert_eq!(
        view.items.iter().map(|i| i.index).collect::<Vec<_>>(),
        vec![5]
    );
}

#[test]
fn bazaar_view_drops_on_close_and_on_zone_change() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::BazaarOpened {
        seller_id: 0xCAFE,
        seller_index: 0x123,
        seller_name: "Aliya".into(),
    });
    assert!(s.apply_event(&AgentEvent::BazaarClosed));
    assert!(s.bazaar.is_none());

    s.apply_event(&AgentEvent::BazaarOpened {
        seller_id: 0xCAFE,
        seller_index: 0x123,
        seller_name: "Aliya".into(),
    });
    s.apply_event(&AgentEvent::ZoneChanged {
        from: None,
        to: 230,
        myroom: None,
        mog_zone_flag: false,
    });
    assert!(s.bazaar.is_none(), "the seller is a zone-local entity");
}

#[test]
fn check_result_resets_on_new_target_and_clears() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::CheckEquipReceived {
        target_id: 0xCAFE,
        act_index: 0x123,
        items: vec![(0, 17440)],
    });
    s.apply_event(&AgentEvent::CheckEquipReceived {
        target_id: 0xBEEF,
        act_index: 0x456,
        items: vec![(4, 12511)],
    });
    let r = s.check_result.as_ref().expect("new target");
    assert_eq!(r.target_id, 0xBEEF);
    assert_eq!(r.equipped[0], None, "old target's gear dropped");
    assert_eq!(r.equipped[4], Some(12511));

    s.apply_event(&AgentEvent::CheckCleared);
    assert!(
        s.check_result.is_none(),
        "outbound /check drops stale result"
    );

    s.apply_event(&AgentEvent::CheckEquipReceived {
        target_id: 0xBEEF,
        act_index: 0x456,
        items: vec![(4, 12511)],
    });
    s.apply_event(&AgentEvent::ZoneChanged {
        from: None,
        to: 230,
        myroom: None,
        mog_zone_flag: false,
    });
    assert!(s.check_result.is_none(), "zone change drops stale result");
}

#[test]
fn check_equip_out_of_range_slot_is_ignored() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::CheckEquipReceived {
        target_id: 0xCAFE,
        act_index: 0x123,
        items: vec![(16, 17440), (0xFF, 1)],
    });
    let r = s.check_result.as_ref().expect("result created");
    assert!(r.equipped.iter().all(|c| c.is_none()));
}

#[test]
fn apply_event_caps_chat_history() {
    let mut s = SessionState::default();
    for i in 0..(CHAT_HISTORY_CAP + 50) {
        s.apply_event(&AgentEvent::ChatLine {
            line: ChatLine {
                spans: Vec::new(),
                channel: ChatChannel::Say,
                sender: "x".into(),
                text: format!("msg {i}"),
                server_ts: 0,
            },
        });
    }
    assert_eq!(s.chat.len(), CHAT_HISTORY_CAP);

    assert_eq!(s.chat[0].text, "msg 50");
}

#[test]
fn apply_event_reports_real_mutations_only() {
    let mut s = SessionState::default();

    // Machine-input / notification-only events never mutate folded state.
    assert!(!s.apply_event(&AgentEvent::HumanReleased));
    assert!(!s.apply_event(&AgentEvent::FishingEnded));

    // Scalar fields: first fold mutates, identical resend is a no-op.
    assert!(s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 6 }));
    assert!(!s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 6 }));
    assert!(s.apply_event(&AgentEvent::WeatherUpdated { weather_number: 7 }));

    assert!(s.apply_event(&AgentEvent::StageChanged {
        stage: Stage::Authenticating,
    }));
    assert!(!s.apply_event(&AgentEvent::StageChanged {
        stage: Stage::Authenticating,
    }));
}

#[test]
fn apply_event_dedupes_identical_entity_upserts() {
    let mut s = SessionState::default();
    let entity = Entity {
        id: 999,
        act_index: 1,
        kind: EntityKind::Pc,
        name: Some("Other".into()),
        pos: Vec3 {
            x: 1.0,
            y: 0.0,
            z: 2.0,
        },
        heading: 64,
        hp_pct: Some(80),
        bt_target_id: 0,
        face_target: 0,
        name_vis: None,
        claim_id: 0,
        speed: 0,
        speed_base: 0,
        look: None,
        npc_state: None,
        status: 0,
        char_flags: Default::default(),
        mount_id: None,
    };

    // First upsert inserts.
    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: entity.clone(),
        pos_present: true,
    }));
    // Byte-identical resend folds to a no-op.
    assert!(!s.apply_event(&AgentEvent::EntityUpserted {
        entity: entity.clone(),
        pos_present: true,
    }));
    // A real change signals again.
    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: Entity {
            heading: 32,
            ..entity
        },
        pos_present: true,
    }));

    // Removing a missing entity is a no-op; removing a present one is not.
    assert!(!s.apply_event(&AgentEvent::EntityRemoved { id: 1234 }));
    assert!(s.apply_event(&AgentEvent::EntityRemoved { id: 999 }));
}

#[test]
fn apply_event_dedupes_identical_self_position() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::Connected {
        account_id: 1,
        char_id: 7,
        character: "Tester".into(),
        zone_id: 100,
    });

    let pos = Position {
        pos: Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        heading: 10,
        speed: 40,
        speed_base: 40,
    };
    // No self entity folded yet: position update touches nothing.
    assert!(!s.apply_event(&AgentEvent::PositionChanged { pos }));

    s.apply_event(&AgentEvent::EntityUpserted {
        entity: Entity {
            id: 7,
            act_index: 1,
            kind: EntityKind::Pc,
            name: Some("Tester".into()),
            pos: Vec3::default(),
            heading: 0,
            hp_pct: Some(100),
            bt_target_id: 0,
            face_target: 0,
            name_vis: None,
            claim_id: 0,
            speed: 40,
            speed_base: 40,
            look: None,
            npc_state: None,
            status: 0,
            char_flags: Default::default(),
            mount_id: None,
        },
        pos_present: true,
    });

    // First real move mutates; the identical resend does not.
    assert!(s.apply_event(&AgentEvent::PositionChanged { pos }));
    assert!(!s.apply_event(&AgentEvent::PositionChanged { pos }));
}

/// AgentCommand is an EXTENSION SURFACE: its serde tags ("cmd"/"type",
/// snake_case variant names) are the agent-socket/MCP wire contract, so it
/// evolves additive-only. This match is exhaustive on purpose — adding,
/// removing, or renaming a variant fails compilation HERE, so the wire-name
/// change lands as a reviewed edit to this sentinel instead of a silent
/// consumer break.
#[allow(dead_code)]
// Uniform `{ .. }` keeps the sentinel name-only: a variant gaining or losing
// fields must not break it, only a rename/removal should.
#[allow(clippy::unneeded_struct_pattern)]
fn _agentcommand_is_additive_only(x: &AgentCommand) {
    match x {
        AgentCommand::Move { .. } => (),
        AgentCommand::StopMove { .. } => (),
        AgentCommand::GroundCorrection { .. } => (),
        AgentCommand::TextInput { .. } => (),
        AgentCommand::RequestZoneChange { .. } => (),
        AgentCommand::MogHouseExit { .. } => (),
        AgentCommand::ChangeJob { .. } => (),
        AgentCommand::OpenMogMenu { .. } => (),
        AgentCommand::TreasureLot { .. } => (),
        AgentCommand::TreasurePass { .. } => (),
        AgentCommand::MarkKeyItemsSeen { .. } => (),
        AgentCommand::CancelBuff { .. } => (),
        AgentCommand::ReportSubArea { .. } => (),
        AgentCommand::EndEvent { .. } => (),
        AgentCommand::EndEventChoice { .. } => (),
        AgentCommand::CustomMenuRespond { .. } => (),
        AgentCommand::Disconnect { .. } => (),
        AgentCommand::ReqLogout { .. } => (),
        AgentCommand::Snapshot { .. } => (),
        AgentCommand::DebugDrive { .. } => (),
        AgentCommand::DebugHeights { .. } => (),
        AgentCommand::Screenshot { .. } => (),
        AgentCommand::Chat { .. } => (),
        AgentCommand::Tell { .. } => (),
        AgentCommand::Action { .. } => (),
        AgentCommand::Emote { .. } => (),
        AgentCommand::RequestEmoteList { .. } => (),
        AgentCommand::ReturnToHomePoint { .. } => (),
        AgentCommand::SetFps { .. } => (),
        AgentCommand::Follow { .. } => (),
        AgentCommand::Engage { .. } => (),
        AgentCommand::SetTargetLock { .. } => (),
        AgentCommand::PathTo { .. } => (),
        AgentCommand::Cancel { .. } => (),
        AgentCommand::UseItem { .. } => (),
        AgentCommand::Equip { .. } => (),
        AgentCommand::StackInventory { .. } => (),
        AgentCommand::DeliveryBox { .. } => (),
        AgentCommand::DeliveryTake { .. } => (),
        AgentCommand::MoveItem { .. } => (),
        AgentCommand::BankWhenFull { .. } => (),
        AgentCommand::ShopBuy { .. } => (),
        AgentCommand::ShopSellReq { .. } => (),
        AgentCommand::ShopSellConfirm { .. } => (),
        AgentCommand::CheckTarget { .. } => (),
        AgentCommand::OpenBazaar { .. } => (),
        AgentCommand::BuyBazaarItem { .. } => (),
        AgentCommand::CloseBazaar { .. } => (),
        AgentCommand::Heal { .. } => (),
        AgentCommand::Fish { .. } => (),
        AgentCommand::FishingInput { .. } => (),
        AgentCommand::FishingRequest { .. } => (),
        AgentCommand::WidescanRequest { .. } => (),
        AgentCommand::WidescanTrack { .. } => (),
        AgentCommand::WidescanEnd { .. } => (),
        AgentCommand::AhBrowse { .. } => (),
        AgentCommand::AhHistory { .. } => (),
        AgentCommand::AhBid { .. } => (),
        AgentCommand::AhSell { .. } => (),
        AgentCommand::AhSellConfirm { .. } => (),
        AgentCommand::AhSalesStatus { .. } => (),
        AgentCommand::AhCancelSale { .. } => (),
    }
}

#[test]
fn death_menu_offer_is_durable_and_clears_on_revive_or_zone_change() {
    use ffxi_proto::decode::DeathMenuOffer;

    let mut state = SessionState::default();
    assert!(state.apply_event(&AgentEvent::DeathMenuUpdated {
        offer: Some(DeathMenuOffer::Raise),
    }));
    assert_eq!(state.death_menu_offer, Some(DeathMenuOffer::Raise));

    assert!(state.apply_event(&AgentEvent::DeathTimerUpdated {
        seconds_until_homepoint: None,
    }));
    assert_eq!(state.death_menu_offer, None);

    state.apply_event(&AgentEvent::DeathMenuUpdated {
        offer: Some(DeathMenuOffer::Tractor),
    });
    state.apply_event(&AgentEvent::ZoneChanged {
        from: Some(100),
        to: 101,
        myroom: None,
        mog_zone_flag: false,
    });
    assert_eq!(state.death_menu_offer, None);
}

#[test]
fn ground_height_correction_is_same_column_and_height_only() {
    let mut position = Position {
        pos: Vec3 {
            x: 10.0,
            y: 20.0,
            z: 0.0,
        },
        heading: 73,
        speed: 4,
        speed_base: 5,
    };
    assert!(!apply_ground_height_correction(
        &mut position,
        11.0,
        20.0,
        -5.319
    ));
    assert_eq!(position.pos.z, 0.0);

    assert!(apply_ground_height_correction(
        &mut position,
        10.0,
        20.0,
        -5.319
    ));
    assert_eq!(
        position.pos,
        Vec3 {
            x: 10.0,
            y: 20.0,
            z: -5.319
        }
    );
    assert_eq!(position.heading, 73);
    assert_eq!(position.speed, 4);
    assert_eq!(position.speed_base, 5);
}

/// AgentEvent is an EXTENSION SURFACE: its serde tags ("cmd"/"type",
/// snake_case variant names) are the agent-socket/MCP wire contract, so it
/// evolves additive-only. This match is exhaustive on purpose — adding,
/// removing, or renaming a variant fails compilation HERE, so the wire-name
/// change lands as a reviewed edit to this sentinel instead of a silent
/// consumer break.
#[allow(dead_code)]
// Uniform `{ .. }` keeps the sentinel name-only: a variant gaining or losing
// fields must not break it, only a rename/removal should.
#[allow(clippy::unneeded_struct_pattern)]
fn _agentevent_is_additive_only(x: &AgentEvent) {
    match x {
        AgentEvent::Connected { .. } => (),
        AgentEvent::StageChanged { .. } => (),
        AgentEvent::ZoneChanged { .. } => (),
        AgentEvent::SubAreaSynced { .. } => (),
        AgentEvent::PositionChanged { .. } => (),
        AgentEvent::CharStatsUpdated { .. } => (),
        AgentEvent::EntityUpserted { .. } => (),
        AgentEvent::EntityRemoved { .. } => (),
        AgentEvent::NameExtractionMiss { .. } => (),
        AgentEvent::EntityPatched { .. } => (),
        AgentEvent::ChatLine { .. } => (),
        AgentEvent::EventStart { .. } => (),
        AgentEvent::EventDialog { .. } => (),
        AgentEvent::CutsceneStarted { .. } => (),
        AgentEvent::CutsceneCue { .. } => (),
        AgentEvent::CutsceneEnded { .. } => (),
        AgentEvent::ShopUpdated { .. } => (),
        AgentEvent::ShopSellAppraisal { .. } => (),
        AgentEvent::StatusIconsUpdated { .. } => (),
        AgentEvent::AbilityRecastsUpdated { .. } => (),
        AgentEvent::JobInfoUpdated { .. } => (),
        AgentEvent::MogHouse2fUnlockUpdated { .. } => (),
        AgentEvent::TreasurePoolUpdated { .. } => (),
        AgentEvent::TreasurePoolCleared { .. } => (),
        AgentEvent::WeatherUpdated { .. } => (),
        AgentEvent::VanaTimeSynced { .. } => (),
        AgentEvent::LogoutCountdown { .. } => (),
        AgentEvent::LogoutCountdownCancelled { .. } => (),
        AgentEvent::EventEnded { .. } => (),
        AgentEvent::ActionStarted { .. } => (),
        AgentEvent::SelfCastStarted { .. } => (),
        AgentEvent::SelfCastProgress { .. } => (),
        AgentEvent::SelfCastEnded { .. } => (),
        AgentEvent::EntityEmoted { .. } => (),
        AgentEvent::EmoteListUpdated { .. } => (),
        AgentEvent::KeyRotated { .. } => (),
        AgentEvent::Disconnected { .. } => (),
        AgentEvent::Error { .. } => (),
        AgentEvent::Diagnostics { .. } => (),
        AgentEvent::NetStats { .. } => (),
        AgentEvent::PartyMemberUpdated { .. } => (),
        AgentEvent::PartyTableReset { .. } => (),
        AgentEvent::LowHp { .. } => (),
        AgentEvent::PartyMemberLowHp { .. } => (),
        AgentEvent::EngagedBy { .. } => (),
        AgentEvent::ForcedMove { .. } => (),
        AgentEvent::SetFps { .. } => (),
        AgentEvent::TellReceived { .. } => (),
        AgentEvent::Reconnected { .. } => (),
        AgentEvent::SceneSummary { .. } => (),
        AgentEvent::InventoryUpdated { .. } => (),
        AgentEvent::InventoryReady { .. } => (),
        AgentEvent::DeliveryBoxUpdated { .. } => (),
        AgentEvent::EquipUpdated { .. } => (),
        AgentEvent::EquipCleared { .. } => (),
        AgentEvent::SelfLookUpdated { .. } => (),
        AgentEvent::SpellsKnownUpdated { .. } => (),
        AgentEvent::CommandDataUpdated { .. } => (),
        AgentEvent::KeyItemsUpdated { .. } => (),
        AgentEvent::ReactorGoalChanged { .. } => (),
        AgentEvent::HumanInControl { .. } => (),
        AgentEvent::HumanReleased { .. } => (),
        AgentEvent::MusicChanged { .. } => (),
        AgentEvent::DeathTimerUpdated { .. } => (),
        AgentEvent::DeathMenuUpdated { .. } => (),
        AgentEvent::MusicVolumeChanged { .. } => (),
        AgentEvent::LevelUp { .. } => (),
        AgentEvent::SkillLevelUp { .. } => (),
        AgentEvent::FishingCast { .. } => (),
        AgentEvent::FishHooked { .. } => (),
        AgentEvent::FishHookedSize { .. } => (),
        AgentEvent::FishingServerPhase { .. } => (),
        AgentEvent::SelfServerStatus { .. } => (),
        AgentEvent::FishingPhaseChanged { .. } => (),
        AgentEvent::FishingProgress { .. } => (),
        AgentEvent::FishingEnded { .. } => (),
        AgentEvent::CheckEquipReceived { .. } => (),
        AgentEvent::CheckGeneralReceived { .. } => (),
        AgentEvent::CheckMessageReceived { .. } => (),
        AgentEvent::CheckCleared { .. } => (),
        AgentEvent::BazaarItemReceived { .. } => (),
        AgentEvent::BazaarOpened { .. } => (),
        AgentEvent::BazaarClosed { .. } => (),
        AgentEvent::BazaarBuyResult { .. } => (),
        AgentEvent::BazaarSoldToOther { .. } => (),
        AgentEvent::WidescanListStart { .. } => (),
        AgentEvent::WidescanEntryReceived { .. } => (),
        AgentEvent::WidescanListEnd { .. } => (),
        AgentEvent::WidescanTrackUpdated { .. } => (),
        AgentEvent::AuctionMenuOpened { .. } => (),
        AgentEvent::AuctionOpStarted { .. } => (),
        AgentEvent::AuctionBrowseResults { .. } => (),
        AgentEvent::AuctionHistoryResults { .. } => (),
        AgentEvent::AuctionSearchFailed { .. } => (),
        AgentEvent::AuctionSellQuote { .. } => (),
        AgentEvent::AuctionSellResult { .. } => (),
        AgentEvent::AuctionBidResult { .. } => (),
        AgentEvent::AuctionSalesStatusReset { .. } => (),
        AgentEvent::AuctionSalesSlot { .. } => (),
        AgentEvent::AuctionCancelResult { .. } => (),
    }
}
