use super::*;

#[test]
fn voyage_disconnect_clears_snapshot_and_legacy_state_deserializes() {
    let voyage = kuluu_snapshot::Voyage {
        start: 123_456,
        duration: 897,
        reverse: true,
        route: 2,
    };
    let mut state = SessionState::default();
    state.apply_event(&AgentEvent::VoyageSynced { voyage });
    assert_eq!(
        crate::wire_translate::state_to_snapshot(&state).voyage,
        Some(voyage)
    );
    assert!(state.apply_event(&AgentEvent::Disconnected {
        reason: "test".into()
    }));
    assert_eq!(
        crate::wire_translate::state_to_snapshot(&state).voyage,
        None
    );
    let mut old = serde_json::to_value(SessionState::default()).unwrap();
    old.as_object_mut().unwrap().remove("voyage");
    assert_eq!(
        serde_json::from_value::<SessionState>(old).unwrap().voyage,
        None
    );
}

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

/// Stand-up cancels leavegame server-side with no 0x053 cancel packet
/// (vendor/server/src/map/packets/s2c/0x053_systemmes.cpp); the heal→walk
/// transition folds in as LogoutCountdownCancelled and must drop a live
/// countdown. A cancel with nothing active is a no-op fold (no churn).
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
        door_open: false,
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

    let mut self_member = party_member(0xCAFE, "Vanari", 1500);
    self_member.in_mog_house = true;
    s.apply_event(&AgentEvent::PartyMemberUpdated {
        member: self_member,
    });
    assert!(
        !s.self_in_mog_house(),
        "the party MoghouseFlg byte must not override the 0x00A myroom cluster"
    );
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
        can_thumbs_up_mentor: true,
        mentor_rank: 2,
        mastery_rank: 1,
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
            monstrosity: None,
            job_master_display: None,
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
            monstrosity: None,
            job_master_display: None,
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

/// `Flags4.JobMasterFlag` is written on every non-despawn 0x0D outside all
/// SendFlg blocks (char_update.cpp), so a pos-only upsert (`char_flags: None`)
/// must still refresh the preserved flags — unlike the General words.
#[test]
fn job_master_flag_refreshes_on_pos_only_updates() {
    let mut s = SessionState::default();

    let mut e = make_test_entity(9, Some("Star"), EntityKind::Pc);
    e.char_flags = Some(ffxi_proto::decode::CharFlags::default());
    e.job_master_display = Some(false);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: e,
        pos_present: true,
    });

    let mut e = make_test_entity(9, None, EntityKind::Pc);
    e.char_flags = None;
    e.job_master_display = Some(true);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: e,
        pos_present: true,
    });

    assert!(
        s.entities[0]
            .char_flags
            .expect("the General update materialized the flags")
            .job_master_display,
        "a pos-only tick must carry the fresh star"
    );

    let mut e = make_test_entity(9, None, EntityKind::Pc);
    e.char_flags = None;
    e.job_master_display = Some(false);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: e,
        pos_present: true,
    });

    assert!(
        !s.entities[0].char_flags.unwrap().job_master_display,
        "a pos-only 'off' must clear the star"
    );
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
        monstrosity: None,
        job_master_display: None,
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
    // #512-4: namevis is written under UPDATE_HP (entity_update.cpp CEntityUpdatePacket::updateWith), and a
    // POS-only 0x00E carries the byte zero-filled. Merging off pos_present would
    // un-hide a hidden entity the moment it moved.
    let mut s = SessionState::default();
    let mut ent = make_test_entity(42, Some("Survival Guide"), EntityKind::Npc);
    ent.name_vis = Some(8);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert_eq!(s.entities[0].name_vis, Some(8));

    let mut moved = make_test_entity(42, None, EntityKind::Npc);
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

/// #512-4: HideName(true) sets UPDATE_HP, not UPDATE_POS. Merging off
/// pos_present kept the stale visible value for a static NPC the server
/// just hid.
#[test]
fn entity_upserted_name_vis_applies_on_hp_only_tick() {
    let mut s = SessionState::default();
    let ent = make_test_entity(42, Some("Unity Master"), EntityKind::Npc);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: ent,
        pos_present: true,
    });
    assert_eq!(s.entities[0].name_vis, None);

    let mut hidden = make_test_entity(42, None, EntityKind::Npc);
    hidden.name_vis = Some(8);
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: hidden,
        pos_present: false,
    });
    assert_eq!(
        s.entities[0].name_vis,
        Some(8),
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
        allegiance: None,
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
        allegiance: None,
    });
    assert_eq!(s.entities[0].name.as_deref(), Some("Crab Familiar"));
    assert_eq!(s.entities[0].kind, EntityKind::Pet);
    assert_eq!(s.entities[0].hp_pct, Some(75));
}

/// Self's entity carries no flags until its first 0x037
/// (vendor/server/src/map/packets/char_status.cpp) — the patch must
/// materialize rather than skip.
#[test]
fn entity_patched_allegiance_materializes_flags_and_preserves_the_rest() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(1, None, EntityKind::Pc),
        pos_present: true,
    });
    assert!(s.entities[0].char_flags.is_none());

    let changed = s.apply_event(&AgentEvent::EntityPatched {
        id: Some(1),
        act_index: None,
        name: None,
        kind: None,
        hp_pct: None,
        allegiance: Some(9),
    });
    assert!(changed);
    let flags = s.entities[0].char_flags.expect("materialized by the patch");
    assert_eq!(flags.allegiance, 9);

    assert!(
        !s.apply_event(&AgentEvent::EntityPatched {
            id: Some(1),
            act_index: None,
            name: None,
            kind: None,
            hp_pct: None,
            allegiance: Some(9),
        }),
        "a repeat of the same value is a no-op fold"
    );

    s.apply_event(&AgentEvent::EntityPatched {
        id: Some(1),
        act_index: None,
        name: None,
        kind: None,
        hp_pct: None,
        allegiance: Some(3),
    });
    let flags = s.entities[0].char_flags.expect("still materialized");
    assert_eq!(
        flags.allegiance, 3,
        "a later value updates in place without zeroing the other flags"
    );
}

#[test]
fn name_extraction_miss_appends_to_ring_buffer_with_cap() {
    const BODY_BYTE_MASK: u32 = 0xFF;
    let mut s = SessionState::default();

    for i in 0..(NAME_MISSES_CAP as u32 + 5) {
        s.apply_event(&AgentEvent::NameExtractionMiss {
            miss: NameExtractionMiss {
                opcode: 0x00E,
                unique_no: i,
                act_index: i as u16,
                send_flag: 0,
                body_len: 64,
                body_hex: format!("{:02x}", i & BODY_BYTE_MASK),
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
        allegiance: None,
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
            monstrosity: None,
            job_master_display: None,
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
        unselectable: false,
        price: 0,
        charges_remaining: None,
        next_use_vana_ts: None,
        use_delay_end_vana_ts: None,
        ready: None,

        extdata: None,
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
fn inventory_fold_item_list_after_item_attr_keeps_charge_state() {
    let mut s = SessionState::default();
    let charged = ItemSlot {
        index: 3,
        item_no: 28652,
        quantity: 1,
        locked: false,
        unselectable: false,
        price: 0,
        charges_remaining: Some(1),
        next_use_vana_ts: Some(1_000),
        use_delay_end_vana_ts: Some(5_030),
        ready: Some(false),

        extdata: None,
    };
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::SlotChanged {
            slot: charged.clone(),
        },
    });

    let bare = ItemSlot {
        locked: true,
        charges_remaining: None,
        next_use_vana_ts: None,
        use_delay_end_vana_ts: None,
        ready: None,
        ..charged.clone()
    };
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::SlotChanged { slot: bare },
    });
    let slots = &s.inventory.containers[&0].slots;
    assert_eq!(slots.len(), 1);
    assert!(slots[0].locked, "the ITEM_LIST lock flag still applies");
    assert_eq!(slots[0].charges_remaining, Some(1));
    assert_eq!(slots[0].use_delay_end_vana_ts, Some(5_030));
    assert_eq!(slots[0].ready, Some(false));

    let replaced = ItemSlot {
        item_no: 4112,
        charges_remaining: None,
        next_use_vana_ts: None,
        use_delay_end_vana_ts: None,
        ready: None,
        ..charged
    };
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: 0,
        update: InventoryUpdate::SlotChanged { slot: replaced },
    });
    let slots = &s.inventory.containers[&0].slots;
    assert_eq!(slots[0].item_no, 4112);
    assert_eq!(
        slots[0].charges_remaining, None,
        "a different item in the slot takes no timers from its predecessor"
    );
}

/// The packet order an enchanted-equipment use produces
/// (vendor/server/src/map/ai/states/item_state.cpp CItemState::init then CItemState::Cleanup):
/// a bare ITEM_LIST marks the slot NoSelect as the use starts, and only seconds later — once the
/// item state exits — does the ITEM_ATTR carrying the new recast arrive. The NoSelect slot has no
/// charge info of its own, so folding it must keep the charges *and* take the new lock.
#[test]
fn inventory_fold_no_select_covers_a_use_until_its_recast_lands() {
    let mut s = SessionState::default();
    let equipped = ItemSlot {
        index: 3,
        item_no: 28652,
        quantity: 1,
        locked: true,
        unselectable: false,
        price: 0,
        charges_remaining: Some(1),
        next_use_vana_ts: Some(0),
        use_delay_end_vana_ts: Some(0),
        ready: Some(true),

        extdata: None,
    };
    let fold = |s: &mut SessionState, slot: ItemSlot| {
        s.apply_event(&AgentEvent::InventoryUpdated {
            container: 0,
            update: InventoryUpdate::SlotChanged { slot },
        });
    };
    fold(&mut s, equipped.clone());

    let bare = |unselectable| ItemSlot {
        unselectable,
        charges_remaining: None,
        next_use_vana_ts: None,
        use_delay_end_vana_ts: None,
        ready: None,
        ..equipped.clone()
    };
    fold(&mut s, bare(true));
    let slot = &s.inventory.containers[&0].slots[0];
    assert!(slot.unselectable, "the use owns the slot");
    assert_eq!(
        slot.charges_remaining,
        Some(1),
        "a lock-only update restates no charge info and must not erase it"
    );

    fold(&mut s, bare(false));
    fold(
        &mut s,
        ItemSlot {
            next_use_vana_ts: Some(86_400),
            ready: Some(false),
            ..equipped
        },
    );
    let slot = &s.inventory.containers[&0].slots[0];
    assert!(
        !slot.unselectable,
        "the use resolves: the slot is selectable again, and the recast is what keeps it greyed"
    );
    assert_eq!(slot.next_use_vana_ts, Some(86_400));
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
                unselectable: false,
                price: 0,
                charges_remaining: None,
                next_use_vana_ts: None,
                use_delay_end_vana_ts: None,
                ready: None,

                extdata: None,
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
    // (0x0dd_equip_inspect.cpp GP_CLI_COMMAND_EQUIP_INSPECT::process), so a later result must not drop it.
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

    // A depleted slot comes back priced 0 (0x106_bazaar_buy.cpp GP_CLI_COMMAND_BAZAAR_BUY::process).
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

/// A 0x058 target push with no engage goal of our own (an /assist push, a
/// stray commit) never arms the weapon: the byte stays NONE.
#[test]
fn target_push_without_an_engage_goal_never_draws() {
    use ffxi_proto::decode::animation::NONE;
    let mut s = SessionState::default();
    assert!(!s.apply_event(&AgentEvent::TargetChanged {
        target_id: Some(99)
    }));
    assert_eq!(s.self_server_status, NONE);
}

/// With our engage pending, the server's 0x058 is the accept: the byte flips
/// to ATTACK on it, before any 0x037 arrives. The standing Engaged goal
/// (a re-aim while fighting) passes the same gate.
#[test]
fn target_push_with_the_engage_goal_draws_on_the_accept() {
    use ffxi_proto::decode::animation::{ATTACK, NONE};
    for goal in [
        ReactorGoalSnapshot::Engaging {
            target_id: 99,
            attack_issued: true,
        },
        ReactorGoalSnapshot::Engaged {
            target_id: 99,
            attack_issued: true,
        },
    ] {
        let mut s = SessionState::default();
        s.apply_event(&AgentEvent::ReactorGoalChanged { goal });
        assert_eq!(
            s.self_server_status, NONE,
            "sending the engage draws nothing"
        );
        assert!(s.apply_event(&AgentEvent::TargetChanged {
            target_id: Some(99)
        }));
        assert_eq!(s.self_server_status, ATTACK);
    }
}

/// The refusal event is inert in the fold: the reactor owns the goal change,
/// and the byte never moved.
#[test]
fn engage_refused_changes_nothing_in_the_fold() {
    use ffxi_proto::decode::animation::NONE;
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::ReactorGoalChanged {
        goal: ReactorGoalSnapshot::Engaging {
            target_id: 99,
            attack_issued: true,
        },
    });
    assert!(!s.apply_event(&AgentEvent::EngageRefused {
        message_num: crate::session::MSG_BASIC_WAIT_LONGER
    }));
    assert_eq!(s.self_server_status, NONE);
}

#[test]
fn engaging_goal_event_roundtrip() {
    let ev = AgentEvent::ReactorGoalChanged {
        goal: ReactorGoalSnapshot::Engaging {
            target_id: 99,
            attack_issued: false,
        },
    };
    let s = serde_json::to_string(&ev).unwrap();
    let back: AgentEvent = serde_json::from_str(&s).unwrap();
    assert!(matches!(
        back,
        AgentEvent::ReactorGoalChanged {
            goal: ReactorGoalSnapshot::Engaging {
                target_id: 99,
                attack_issued: false,
            },
        }
    ));
}

/// 0x037 remains the authority in both directions: it can flip the byte to
/// ATTACK on its own and back to NONE, and a 0x058 clear also sheathes.
#[test]
fn char_status_and_target_clear_both_move_the_byte() {
    use ffxi_proto::decode::animation::{ATTACK, NONE};
    let mut s = SessionState::default();
    assert!(s.apply_event(&AgentEvent::SelfServerStatus {
        status: ATTACK,
        mount_id: 0
    }));
    assert_eq!(s.self_server_status, ATTACK);
    assert!(s.apply_event(&AgentEvent::TargetChanged { target_id: None }));
    assert_eq!(s.self_server_status, NONE);
    assert!(s.apply_event(&AgentEvent::SelfServerStatus {
        status: ATTACK,
        mount_id: 0
    }));
    assert!(s.apply_event(&AgentEvent::SelfServerStatus {
        status: NONE,
        mount_id: 0
    }));
    assert_eq!(s.self_server_status, NONE);
}

/// A cutscene's 0x7E mount cue arms the local mount on the client; a stale on-foot
/// 0x037 must not clobber it until the server re-asserts a mounted byte of its own.
#[test]
fn cutscene_mount_latch_releases_on_terminal_lifecycle() {
    use ffxi_proto::decode::animation::{CHOCOBO, NONE};
    for terminal in [
        AgentEvent::CutsceneEnded,
        AgentEvent::Disconnected {
            reason: "fixture".into(),
        },
    ] {
        let mut state = SessionState::default();
        state.apply_event(&AgentEvent::CsMountArmed {
            status: CHOCOBO,
            mount_id: 0,
        });
        state.apply_event(&terminal);
        assert!(!state.cs_mount_armed);
        state.apply_event(&AgentEvent::SelfServerStatus {
            status: NONE,
            mount_id: 0,
        });
        assert_eq!(state.self_server_status, NONE);
    }
}

#[test]
fn cs_mount_cue_holds_against_stale_on_foot_char_status() {
    use ffxi_proto::decode::animation::{CHOCOBO, NONE};
    let mut s = SessionState::default();
    // The CS cue arms the mount (status 5 = chocobo).
    assert!(s.apply_event(&AgentEvent::CsMountArmed {
        status: CHOCOBO,
        mount_id: 0
    }));
    assert_eq!(s.self_server_status, CHOCOBO);
    assert!(s.cs_mount_armed, "the cue arms the latch");

    // A stale on-foot 0x037 (the server never saw the cue) is dropped.
    assert!(
        !s.apply_event(&AgentEvent::SelfServerStatus {
            status: NONE,
            mount_id: 0
        }),
        "the stale on-foot byte must not clobber the cue's write"
    );
    assert_eq!(s.self_server_status, CHOCOBO, "the mount byte survives");
    assert!(s.cs_mount_armed, "the latch stays armed");

    // The server's own mounted byte (MOUNTED effect at event finish) releases the latch.
    // The byte is already CHOCOBO, so `apply_event` reports no change — but the latch
    // is released regardless.
    s.apply_event(&AgentEvent::SelfServerStatus {
        status: CHOCOBO,
        mount_id: 0,
    });
    assert_eq!(s.self_server_status, CHOCOBO);
    assert!(!s.cs_mount_armed, "the mounted byte releases the latch");

    // Now a genuine on-foot 0x037 (dismount) applies.
    assert!(s.apply_event(&AgentEvent::SelfServerStatus {
        status: NONE,
        mount_id: 0
    }));
    assert_eq!(s.self_server_status, NONE);
}

/// The dismissal edge clears the displayed frame so its advance hint cannot linger over
/// camera moves and holds; a second dismissal with nothing up is a no-op, and the next
/// message opcode reopens it.
#[test]
fn apply_event_dialog_dismissed_clears_the_frame() {
    let mut s = SessionState::default();
    assert!(
        !s.apply_event(&AgentEvent::DialogDismissed),
        "nothing up yet"
    );

    let dialog = DialogState {
        event_id: 503,
        prompt: Some("The coupon line".into()),
        ..Default::default()
    };
    assert!(s.apply_event(&AgentEvent::EventDialog {
        dialog: dialog.clone()
    }));
    assert!(s.dialog.is_some());

    assert!(s.apply_event(&AgentEvent::DialogDismissed));
    assert!(s.dialog.is_none());
    assert!(
        !s.apply_event(&AgentEvent::DialogDismissed),
        "already clear"
    );

    assert!(s.apply_event(&AgentEvent::EventDialog { dialog }));
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
        monstrosity: None,
        job_master_display: None,
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
            monstrosity: None,
            job_master_display: None,
        },
        pos_present: true,
    });

    // First real move mutates; the identical resend does not.
    assert!(s.apply_event(&AgentEvent::PositionChanged { pos }));
    assert!(!s.apply_event(&AgentEvent::PositionChanged { pos }));
}

#[test]
fn party_solicit_lands_and_clears_on_the_state_field() {
    // s2c 0x0DC arrives → pending invite on the state (the snapshot carries it
    // to the Party row's helper law — FFXiMain.dll retail-2026-09 RVA 0x2016E0,
    // vault main_menu.md section 24); s2c 0x0DE clears it.
    let mut state = SessionState::default();
    assert!(state.party_invite.is_none());
    assert!(state.apply_event(&AgentEvent::PartySolicitReceived {
        char_id: 0x00A1_0002,
        name: "Pruneo".into(),
        alliance: false,
    }));
    let invite = state.party_invite.clone().unwrap();
    assert_eq!(
        (invite.char_id, invite.name.as_str(), invite.alliance),
        (0x00A1_0002, "Pruneo", false)
    );

    // Re-stamping the same invite is not a change.
    assert!(!state.apply_event(&AgentEvent::PartySolicitReceived {
        char_id: 0x00A1_0002,
        name: "Pruneo".into(),
        alliance: false,
    }));

    assert!(state.apply_event(&AgentEvent::PartySolicitWithdrawn));
    assert!(state.party_invite.is_none());
    // With nothing pending, withdrawing changes nothing.
    assert!(!state.apply_event(&AgentEvent::PartySolicitWithdrawn));
}

/// The trade fold's phase law (vendor/server/src/map/items/transactions/
/// player_trade.cpp; vault main_menu.md §25): an invite opens the pending
/// answer window, RES{Start} makes it live, slot views re-stamp and ANY change
/// un-confirms both sides, Make marks us confirmed locally, and a terminal RES
/// clears the transaction — duplicate pushes of either kind change nothing.
#[test]
fn trade_phases_fold_into_the_state_field() {
    use ffxi_proto::decode::{TradeResKind as K, TRADE_GIL_ITEM_NO};
    use kuluu_snapshot::TradeOffer;

    let mut state = SessionState::default();
    assert!(state.trade.is_none());
    // With no transaction open even our own echo pushes change nothing (LSB's
    // one-transaction rule).
    assert!(!state.apply_event(&AgentEvent::TradeMySlotSet {
        trade_index: 0,
        offer: None,
    }));

    // s2c 0x021 → the answer window (retail opens its gift pair likewise).
    assert!(state.apply_event(&AgentEvent::TradeIncoming { char_id: 42 }));
    let opened = state.trade.clone().unwrap();
    assert_eq!(
        (opened.partner_id, opened.incoming_pending, opened.active),
        (42, true, false)
    );
    // Re-stamping the same invite is not a change.
    assert!(!state.apply_event(&AgentEvent::TradeIncoming { char_id: 42 }));

    // RES{Start} — the answer landed and LSB opened the transaction for both
    // sides, so our window goes live.
    assert!(state.apply_event(&AgentEvent::TradeRes { kind: K::Start }));
    let live = state.trade.clone().unwrap();
    assert!(!live.incoming_pending && live.active);

    // Gil staged in slot 0 (s2c 0x025 echo) and Make confirmed locally… an
    // identical re-push neither changes the slot nor clears that stamp.
    let gil = TradeOffer {
        item_no: TRADE_GIL_ITEM_NO,
        qty: 10,
    };
    assert!(state.apply_event(&AgentEvent::TradeMySlotSet {
        trade_index: 0,
        offer: Some(gil),
    }));
    assert!(state.apply_event(&AgentEvent::TradeMadeLocally));
    assert!(!state.apply_event(&AgentEvent::TradeMySlotSet {
        trade_index: 0,
        offer: Some(gil),
    }));
    assert!(state.trade.as_deref().is_some_and(|w| w.my_confirmed));

    // A partner-side view (s2c 0x023) mutates the deal and un-confirms us.
    let theirs = TradeOffer {
        item_no: 4112,
        qty: 1,
    };
    assert!(state.apply_event(&AgentEvent::TradePartnerSlotSet {
        trade_index: 7,
        offer: Some(theirs),
    }));
    assert!(state
        .trade
        .as_deref()
        .is_some_and(|w| !w.my_confirmed && w.partner_slots.len() == 8));

    // A terminal RES clears the transaction; further terminals change nothing.
    assert!(state.apply_event(&AgentEvent::TradeRes { kind: K::End }));
    assert!(state.trade.is_none());
    assert!(!state.apply_event(&AgentEvent::TradeRes { kind: K::Cancell }));
}

/// The client-local macro store law (bead kuluu-o9qt7.19.2, vault
/// main_menu.md §41): MacrosLoaded replaces the folded list wholesale — a
/// later commit that dropped slots must not linger beside the old ones.
#[test]
fn macros_loaded_replaces_wholesale() {
    use kuluu_snapshot::MacroSlot;
    let mut state = SessionState::default();
    let first = vec![
        MacroSlot {
            block: 0,
            index: 1,
            text: "/ra".to_string(),
        },
        MacroSlot {
            block: 1,
            index: 2,
            text: "/heal".to_string(),
        },
    ];
    assert!(state.apply_event(&AgentEvent::MacrosLoaded {
        slots: first.clone()
    }));
    assert_eq!(state.macros, first);
    let second = vec![MacroSlot {
        block: 0,
        index: 1,
        text: "/reraise".to_string(),
    }];
    assert!(state.apply_event(&AgentEvent::MacrosLoaded {
        slots: second.clone()
    }));
    assert_eq!(state.macros, second);
}

/// The emote display order folds wholesale too (bead kuluu-o9qt7.34): retail's
/// fill law re-reads the whole blob at open (FFXiMain.dll retail-2026-09
/// `.text 0x225960` resets state words), so a later load must replace — never
/// merge — what kuluu folded before.
#[test]
fn emote_order_loaded_replaces_wholesale() {
    let mut state = SessionState::default();
    assert_eq!(
        state.emote_order,
        Vec::<u8>::new(),
        "never-arranged default"
    );
    assert!(state.apply_event(&AgentEvent::EmoteOrderLoaded {
        ids: vec![6, 4, 12]
    }));
    assert_eq!(state.emote_order, vec![6, 4, 12]);
    assert!(state.apply_event(&AgentEvent::EmoteOrderLoaded { ids: vec![9] }));
    assert_eq!(state.emote_order, vec![9]);
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
        AgentCommand::Synthesize { .. } => (),
        AgentCommand::PlayerSearch { .. } => (),
        AgentCommand::Move { .. } => (),
        AgentCommand::StopMove { .. } => (),
        AgentCommand::GroundCorrection { .. } => (),
        AgentCommand::TextInput { .. } => (),
        AgentCommand::RequestZoneChange { .. } => (),
        AgentCommand::MogHouseExit { .. } => (),
        AgentCommand::ChangeJob { .. } => (),
        AgentCommand::OpenMogMenu { .. } => (),
        // myroom's door row + roomlist remodel, c2s 0x0CB MYROOM_IS (beads
        // kuluu-o9qt7.13.5/.6).
        AgentCommand::MogHouseDoorSelect { .. } => (),
        AgentCommand::MogHouseRemodel { .. } => (),
        // The Mog House furniture/plant request family, c2s 0x0FA..0x0FF (bead kuluu-o9qt7.38).
        AgentCommand::MyroomLayout { .. } => (),
        AgentCommand::MyroomLayoutDone { .. } => (),
        AgentCommand::MyroomBankin { .. } => (),
        AgentCommand::MyroomPlantAdd { .. } => (),
        AgentCommand::MyroomPlantCheck { .. } => (),
        AgentCommand::MyroomPlantCrop { .. } => (),
        AgentCommand::MyroomPlantStop { .. } => (),
        // The arrangement mode request the session actor gates and applies (bead kuluu-o9qt7.39).
        AgentCommand::MyroomMode { .. } => (),
        AgentCommand::CommitMacros { .. } => (),
        AgentCommand::CommitEmoteOrder { .. } => (),
        // Header-only currency report requests (bead kuluu-o9qt7.35).
        AgentCommand::RequestCurrencies { .. } => (),
        AgentCommand::RequestUnityRanking { .. } => (),
        AgentCommand::RequestUnityQuest { .. } => (),
        AgentCommand::SetUnityChat { .. } => (),
        AgentCommand::RequestAlterEgoPoints { .. } => (),
        AgentCommand::UpgradeAlterEgo { .. } => (),
        AgentCommand::SetMeritMode { .. } => (),
        AgentCommand::EditMerit { .. } => (),
        AgentCommand::SpendJobPoint { .. } => (),
        AgentCommand::RequestJobPoints => (),
        AgentCommand::TreasureLot { .. } => (),
        AgentCommand::TreasurePass { .. } => (),
        AgentCommand::MarkKeyItemsSeen { .. } => (),
        AgentCommand::CancelBuff { .. } => (),
        AgentCommand::ReportSubArea { .. } => (),
        AgentCommand::CutsceneMotionDone { .. } => (),
        AgentCommand::EndEvent { .. } => (),
        AgentCommand::EndEventBack { .. } => (),
        AgentCommand::EndEventChoice { .. } => (),
        AgentCommand::CustomMenuRespond { .. } => (),
        AgentCommand::PartyInviteRespond { .. } => (),
        // The player-trade command family (bead kuluu-o9qt7.9, vault
        // main_menu.md §25): names are the agent-socket wire contract.
        AgentCommand::TradeRequest { .. } => (),
        AgentCommand::TradeRespond { .. } => (),
        AgentCommand::TradeStage { .. } => (),
        AgentCommand::TradeSetGil { .. } => (),
        AgentCommand::TradeClearSlot { .. } => (),
        AgentCommand::TradeConfirm { .. } => (),
        AgentCommand::TradeCancel { .. } => (),
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
        // The set arms: c2s 0x051/0x053/0x052 (`.agents/skills/retail-observe/references/2026-10-25-equip-set-apply.md`).
        AgentCommand::EquipSetWear { .. } => (),
        AgentCommand::StyleLockSet { .. } => (),
        AgentCommand::EquipSetCheck { .. } => (),
        AgentCommand::StackInventory { .. } => (),
        AgentCommand::DeliveryBox { .. } => (),
        AgentCommand::DeliveryTake { .. } => (),
        AgentCommand::MoveItem { .. } => (),
        AgentCommand::DropItem { .. } => (),
        AgentCommand::BankWhenFull { .. } => (),
        AgentCommand::ShopBuy { .. } => (),
        AgentCommand::ShopSellReq { .. } => (),
        AgentCommand::ShopSellConfirm { .. } => (),
        AgentCommand::ShopSellCancel { .. } => (),
        AgentCommand::CloseShop { .. } => (),
        AgentCommand::CheckTarget { .. } => (),
        AgentCommand::OpenBazaar { .. } => (),
        AgentCommand::SetConfigFlag { .. } => (),
        AgentCommand::AnnounceConfigBfPair { .. } => (),
        AgentCommand::BuyBazaarItem { .. } => (),
        AgentCommand::CloseBazaar { .. } => (),
        // Our own stall's price menu and comment field, c2s 0x109/0x10A/0x10B plus
        // 0x0DE INSPECT_MESSAGE (owner captures 2026-10-24).
        AgentCommand::BazaarPricesEnter => (),
        AgentCommand::BazaarPriceSet { .. } => (),
        AgentCommand::BazaarPricesDone => (),
        AgentCommand::BazaarComment { .. } => (),
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

/// Retail keeps only the server's `PlayTime` base plus a stamp of when it
/// arrived and adds elapsed seconds at `/playtime` (FFXiMain.dll retail-2026-09
/// RVA 0x7CAB0, store pair `.text 0xFB3EE`/`.text 0xFB3FA`), so a session zoned in
/// some time ago has to report more than the raw base.
#[test]
fn play_time_sync_stores_the_base_pair_and_adds_elapsed_since_arrival() {
    const BASE_SECS: u32 = 3 * 86_400 + 4 * 3600 + 5 * 60;
    const ELAPSED_WHOLE_SECS: u64 = 90;

    let mut state = SessionState::default();
    assert_eq!(state.play_time_seconds(), 0);

    assert!(state.apply_event(&AgentEvent::PlayTimeSynced { seconds: BASE_SECS }));
    let base = state.play_time_base.expect("LOGIN stores the pair");
    assert_eq!(base.base_secs, BASE_SECS);
    assert_eq!(base.seconds_at(base.arrival_ms), u64::from(BASE_SECS));
    assert_eq!(
        base.seconds_at(base.arrival_ms + MS_PER_SECOND * ELAPSED_WHOLE_SECS + (MS_PER_SECOND - 1)),
        u64::from(BASE_SECS) + ELAPSED_WHOLE_SECS,
        "the elapsed term divides down to whole seconds like retail's"
    );

    assert!(
        !state.apply_event(&AgentEvent::PlayTimeSynced { seconds: BASE_SECS }),
        "an identical base is not a snapshot change"
    );

    let reported = state.play_time_seconds();
    assert!(
        reported >= u64::from(BASE_SECS),
        "the wire number never drops below the server's base, got {reported}"
    );
}

/// The 0x062 block is what un-greys the Status window's Combat/Magic/Craft rows,
/// so it must survive both folds: onto the state and into the snapshot the renderer
/// reads (FFXiMain.dll retail-2026-09 `.text 0x148B50` opens those three disabled;
/// only content in this table enables them).
#[test]
fn char_skills_update_folds_onto_state_and_snapshot() {
    let mut skills = [0u16; kuluu_snapshot::CharSkills::SKILL_COUNT];
    const HAND_TO_HAND: u16 = 301;
    skills[1] = HAND_TO_HAND;
    let block = kuluu_snapshot::CharSkills {
        command_recast: [7; kuluu_snapshot::CharSkills::COMMAND_RECAST_COUNT],
        skills,
    };

    let mut state = SessionState::default();
    assert!(state.apply_event(&AgentEvent::CharSkillsUpdated { skills: block }));
    assert_eq!(state.char_skills, Some(block));
    assert!(
        !state.apply_event(&AgentEvent::CharSkillsUpdated { skills: block }),
        "an identical block is not a snapshot change"
    );

    let snap = crate::wire_translate::state_to_snapshot(&state);
    let projected = snap.char_skills.expect("snapshot carries the block");
    assert_eq!(projected.skills[1], HAND_TO_HAND);
    assert_eq!(projected.command_recast, block.command_recast);
}

/// s2c 0x063 MISCDATA Merits folds replace one another and ride the snapshot (bead
/// kuluu-o9qt7.43) — retail's dispatcher re-stores 0x485826/0x48582A on every arrival
/// (.text FFXiMain.dll retail-2026-09 `0x99290`).
#[test]
fn merits_record_folds_replace_and_project() {
    let mut state = SessionState::default();
    assert!(state.merits.is_none());
    let m = kuluu_snapshot::MisdataMerits {
        limit_points: 7,
        merit_points: 4,
        blu_bonus: 1,
        can_use_merit_mode: true,
        xp_capped_or_merit_mode: false,
        merit_mode_enabled: true,
        max_merit_points: 56,
    };
    assert!(state.apply_event(&AgentEvent::MeritsUpdated { merits: m }));
    assert_eq!(state.merits, Some(m));
    assert!(
        !state.apply_event(&AgentEvent::MeritsUpdated { merits: m }),
        "an identical record is not a snapshot change"
    );
    let snap = crate::wire_translate::state_to_snapshot(&state);
    assert_eq!(snap.merits, Some(m));
}

/// s2c 0x063 MISCDATA Homepoints folds replace one another and ride the snapshot (bead
/// kuluu-o9qt7.45) — retail rep-movsds over the whole mask store on every arrival
/// (.text FFXiMain.dll retail-2026-09 `0x99330`).
#[test]
fn homepoints_record_folds_replace_and_project() {
    let mut state = SessionState::default();
    assert!(state.homepoints.is_none());
    let masks = kuluu_snapshot::HomepointMasks {
        home_point: [1, 2, 4, 8],
        survival_guide: [0x10, 0x20, 0x40, 0x80],
        waypoint: [0xFFFF_FFFF, 0, 0, 0],
        telepoint: 0xDEAD_BEEF,
        atmos: 0xCAFE_BABE,
        eschan_portal: 0xF00D_D00D,
    };
    assert!(state.apply_event(&AgentEvent::HomepointsUpdated { masks }));
    assert_eq!(state.homepoints, Some(masks));
    assert!(
        !state.apply_event(&AgentEvent::HomepointsUpdated { masks }),
        "an identical record is not a snapshot change"
    );
    let next = kuluu_snapshot::HomepointMasks {
        telepoint: 1,
        ..masks
    };
    assert!(
        state.apply_event(&AgentEvent::HomepointsUpdated { masks: next }),
        "a fresh record replaces the last, not a merge"
    );
    assert_eq!(state.homepoints, Some(next));
    let snap = crate::wire_translate::state_to_snapshot(&state);
    assert_eq!(snap.homepoints, Some(next));
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

#[test]
fn linkshell_slot_events_fold_the_pair_and_ignore_unknown_slots() {
    // s2c 0x0E0 law (vault main_menu.md §27): one packet describes one slot, LSB pushes both at
    // login, and a zeroed pair means "nothing equipped in that slot".
    let mut state = SessionState::default();
    assert!(!state.linkshells.one.is_occupied());

    assert!(state.apply_event(&AgentEvent::LinkShellSlotChanged {
        linkshell_slot: 1,
        category: 0,
        item_index: 12,
    }));
    assert_eq!(state.linkshells.one.item_index, 12);

    // The second login push reports slot 2; the first slot must survive it.
    assert!(state.apply_event(&AgentEvent::LinkShellSlotChanged {
        linkshell_slot: 2,
        category: 1,
        item_index: 3,
    }));
    assert_eq!(state.linkshells.one.item_index, 12);
    assert_eq!(
        state.linkshells.two,
        kuluu_snapshot::LinkShellSlot {
            category: 1,
            item_index: 3
        }
    );

    // Repeating the same report changes nothing.
    assert!(!state.apply_event(&AgentEvent::LinkShellSlotChanged {
        linkshell_slot: 2,
        category: 1,
        item_index: 3,
    }));

    // Unequip arrives as zeros; slot ids outside LSB's LinkshellId 1/2 are ignored.
    assert!(state.apply_event(&AgentEvent::LinkShellSlotChanged {
        linkshell_slot: 2,
        category: 0,
        item_index: 0,
    }));
    assert!(!state.linkshells.two.is_occupied());
    assert!(!state.apply_event(&AgentEvent::LinkShellSlotChanged {
        linkshell_slot: 3,
        category: 0,
        item_index: 9,
    }));
}

#[test]
fn myroom_mode_changes_fold_from_the_actor_mirror() {
    // The arrangement mode has no wire (retail keeps it in the myroom manager's own word), so the
    // session actor's accepted transitions are the only thing that may move this field (bead
    // kuluu-o9qt7.39; FFXiMain.dll retail-2026-09 `.text 0x1D34FA`).
    let mut state = SessionState::default();
    assert_eq!(state.myroom_mode, crate::myroom_layout::MODE_NORMAL);

    assert!(state.apply_event(&AgentEvent::MyroomModeChanged {
        previous: crate::myroom_layout::MODE_NORMAL,
        next: crate::myroom_layout::MODE_LAYOUT,
    }));
    assert_eq!(state.myroom_mode, crate::myroom_layout::MODE_LAYOUT);

    // A replayed change reports no mutation.
    assert!(!state.apply_event(&AgentEvent::MyroomModeChanged {
        previous: crate::myroom_layout::MODE_NORMAL,
        next: crate::myroom_layout::MODE_LAYOUT,
    }));
}

#[test]
fn synth_answers_replace_the_last_outcome_whole() {
    // s2c 0x06F is one-shot feedback (vault main_menu.md §28): every answer
    // replaces whatever we held, and a duplicate answer changes nothing.
    let mut state = SessionState::default();
    assert!(state.synth_result.is_none());

    let success = kuluu_snapshot::SynthOutcome {
        result: ffxi_proto::synth::result::SUCCESS,
        grade: 0,
        count: 33,
        item_no: 17319,
        broken_items: vec![705],
    };
    assert!(state.apply_event(&AgentEvent::SynthCompleted {
        outcome: success.clone()
    }));
    assert_eq!(state.synth_result.as_ref(), Some(&success));

    // A later failure replaces the success whole — no merge, no queue.
    let failed = kuluu_snapshot::SynthOutcome {
        result: ffxi_proto::synth::result::FAILED,
        grade: 0,
        count: 0,
        item_no: 0,
        broken_items: vec![],
    };
    assert!(state.apply_event(&AgentEvent::SynthCompleted {
        outcome: failed.clone()
    }));
    assert_eq!(state.synth_result.as_ref(), Some(&failed));

    assert!(!state.apply_event(&AgentEvent::SynthCompleted { outcome: failed }));
}

#[test]
fn mission_battery_folds_whole_per_port() {
    // s2c 0x056 (vault main_menu.md §30): login pushes the battery; every frame
    // replaces its own port whole and changes nothing else.
    let mut state = SessionState::default();
    assert!(state.missions.info.is_none());

    let info = kuluu_snapshot::MissionInfo {
        nation: 1,
        nation_mission: 5,
        rotz: 0,
        cop: 0,
        cop2: 0,
        addon_acp: 0,
        addon_amk: 0,
        addon_asa: 0,
        tales_beginning: 0x3F,
        soa: 0,
        rov: 0,
    };
    assert!(state.apply_event(&AgentEvent::MissionInfoUpdated { info }));
    assert_eq!(state.missions.info, Some(info));
    assert!(!state.apply_event(&AgentEvent::MissionInfoUpdated { info }));

    let entry = kuluu_snapshot::MissionLogEntry {
        port: 0xD0,
        data: [1, 2, 3, 4, 5, 6, 7, 8],
    };
    assert!(state.apply_event(&AgentEvent::MissionLogUpdated { entry }));
    let updated = kuluu_snapshot::MissionLogEntry {
        port: 0xD0,
        data: [9; 8],
    };
    assert!(state.apply_event(&AgentEvent::MissionLogUpdated { entry: updated }));
    assert_eq!(state.missions.logs.len(), 1);
    assert_eq!(state.missions.logs[0], updated);

    assert!(state.apply_event(&AgentEvent::MissionTvrUpdated { tvr: 7 }));
    assert_eq!(state.missions.tvr, 7);
}

/// The s2c 0x0B4 CONFIG fold (vault main_menu.md §42): each frame restates the
/// SAVE_CONF prefix whole; only an actual byte change reports as changed.
#[test]
fn config_confdata_folds_whole() {
    let mut state = SessionState::default();
    assert!(state.config_confdata.is_none());
    assert!(state.apply_event(&AgentEvent::ConfigUpdated {
        confdata: [0x01, 0, 0, 0]
    }));
    assert_eq!(state.config_confdata, Some([0x01, 0, 0, 0]));
    assert!(!state.apply_event(&AgentEvent::ConfigUpdated {
        confdata: [0x01, 0, 0, 0]
    }));
    assert!(state.apply_event(&AgentEvent::ConfigUpdated {
        confdata: [0x03, 0x40, 0, 0x20]
    }));
}

/// The s2c 0x044 folds (vault main_menu.md §39): one PUP frame lands the whole
/// automaton record and repeats are no-ops; same law for MON.
#[test]
fn extended_job_frames_fold_whole() {
    let mut state = crate::state::SessionState::default();
    let automaton = kuluu_snapshot::AutomatonInfo {
        is_sub_job: false,
        head: 2, // AutomatonHead::Valoredge (vendor/server/src/map/enums/automaton.h)
        frame: 0x21,
        attachments: [0; 12],
        unlocked_heads: 0x3F,
        unlocked_frames: 0x0F,
    };
    assert!(state.apply_event(&AgentEvent::AutomatonUpdated {
        automaton: automaton.clone()
    }));
    assert_eq!(state.automaton, Some(Box::new(automaton.clone())));
    assert!(!state.apply_event(&AgentEvent::AutomatonUpdated { automaton }));

    let monstrosity = kuluu_snapshot::MonstrosityInfo {
        species: 71,
        equipped_instincts: [0; 12],
    };
    assert!(state.apply_event(&AgentEvent::MonstrosityUpdated { monstrosity }));
    assert_eq!(state.monstrosity, Some(monstrosity));
    assert!(!state.apply_event(&AgentEvent::MonstrosityUpdated { monstrosity }));
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
        AgentEvent::MissionInfoUpdated { .. } => (),
        AgentEvent::MissionTvrUpdated { .. } => (),
        AgentEvent::MissionLogUpdated { .. } => (),
        AgentEvent::SynthCompleted { .. } => (),
        AgentEvent::PlayerSearchCompleted { .. } => (),
        AgentEvent::LinkShellSlotChanged { .. } => (),
        AgentEvent::AutomatonUpdated { .. } => (),
        AgentEvent::ConfigUpdated { .. } => (),
        AgentEvent::MonstrosityUpdated { .. } => (),
        AgentEvent::Connected { .. } => (),
        AgentEvent::MacrosLoaded { .. } => (),
        AgentEvent::EmoteOrderLoaded { .. } => (),
        // Raw currency report folds (bead kuluu-o9qt7.35).
        AgentEvent::CurrenciesLoaded { .. } => (),
        AgentEvent::StageChanged { .. } => (),
        AgentEvent::ZoneChanged { .. } => (),
        AgentEvent::SubAreaSynced { .. } => (),
        AgentEvent::VoyageSynced { .. } => (),
        AgentEvent::PositionChanged { .. } => (),
        AgentEvent::CharStatsUpdated { .. } => (),
        AgentEvent::EntityUpserted { .. } => (),
        AgentEvent::EntityRemoved { .. } => (),
        AgentEvent::OwnPetSynced { .. } => (),
        AgentEvent::NameExtractionMiss { .. } => (),
        AgentEvent::EntityPatched { .. } => (),
        AgentEvent::ChatLine { .. } => (),
        AgentEvent::EventStart { .. } => (),
        AgentEvent::EventDialog { .. } => (),
        AgentEvent::DialogDismissed { .. } => (),
        AgentEvent::CutsceneStarted { .. } => (),
        AgentEvent::CutsceneCue { .. } => (),
        AgentEvent::CutsceneEnded { .. } => (),
        AgentEvent::ShopUpdated { .. } => (),
        AgentEvent::ShopSellAppraisal { .. } => (),
        AgentEvent::ShopClosed { .. } => (),
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
        AgentEvent::PartySolicitReceived { .. } => (),
        AgentEvent::PartySolicitWithdrawn => (),
        // The player-trade fold family (bead kuluu-o9qt7.9, vault
        // main_menu.md §25).
        AgentEvent::TradeIncoming { .. } => (),
        AgentEvent::TradeOutgoingOpened { .. } => (),
        AgentEvent::TradeRes { .. } => (),
        AgentEvent::TradeMySlotSet { .. } => (),
        AgentEvent::TradePartnerSlotSet { .. } => (),
        AgentEvent::TradeMadeLocally => (),
        AgentEvent::LowHp { .. } => (),
        AgentEvent::PartyMemberLowHp { .. } => (),
        AgentEvent::EngagedBy { .. } => (),
        AgentEvent::TargetChanged { .. } => (),
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
        AgentEvent::PlayTimeSynced { .. } => (),
        AgentEvent::CharSkillsUpdated { .. } => (),
        AgentEvent::MeritsUpdated { .. } => (),
        AgentEvent::AlterEgoPointsUpdated { .. } => (),
        AgentEvent::UnityRankingChunk { .. } => (),
        AgentEvent::UnityRankingRequested { .. } => (),
        AgentEvent::UnityQuestUpdated { .. } => (),
        AgentEvent::UnityQuestRequested { .. } => (),
        AgentEvent::LoginStateSynced { .. } => (),
        AgentEvent::MeritsListed { .. } => (),
        AgentEvent::JobPointsListed { .. } => (),
        AgentEvent::JobPointsRecordUpdated { .. } => (),
        // Raw homepoint mask fold (bead kuluu-o9qt7.45).
        AgentEvent::HomepointsUpdated { .. } => (),
        AgentEvent::DeathMenuUpdated { .. } => (),
        AgentEvent::MusicVolumeChanged { .. } => (),
        AgentEvent::LevelUp { .. } => (),
        AgentEvent::SkillLevelUp { .. } => (),
        AgentEvent::EngageRefused { .. } => (),
        AgentEvent::Knockbacks { .. } => (),
        AgentEvent::FishingCast { .. } => (),
        AgentEvent::FishHooked { .. } => (),
        AgentEvent::FishHookedSize { .. } => (),
        AgentEvent::FishingServerPhase { .. } => (),
        AgentEvent::SelfServerStatus { .. } => (),
        AgentEvent::CsMountArmed { .. } => (),
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
        AgentEvent::MapOpen { .. } => (),
        AgentEvent::MapMarkerPlaced { .. } => (),
        AgentEvent::MapClosed => (),
        AgentEvent::EventSoundChannelsSet { .. } => (),
        AgentEvent::EventSoundChannelsChange { .. } => (),
        AgentEvent::EventMapPropsState { .. } => (),
        AgentEvent::MapReloadRequested { .. } => (),
        AgentEvent::ActorRenderFlagChanged { .. } => (),
        // The actor-mirrored arrangement mode (bead kuluu-o9qt7.39).
        AgentEvent::MyroomModeChanged { .. } => (),
    }
}

/// The wire-id index stays in lockstep with the entities Vec across every
/// mutation path.
#[test]
fn entity_index_stays_in_lockstep_with_the_vec() {
    let mut s = SessionState::default();
    for id in [10u32, 20, 30] {
        assert!(s.apply_event(&AgentEvent::EntityUpserted {
            entity: make_test_entity(id, Some("m"), EntityKind::Mob),
            pos_present: true,
        }));
    }
    for (i, e) in s.entities.iter().enumerate() {
        assert_eq!(s.entity_index.get(&e.id), Some(&i));
    }

    assert!(s.apply_event(&AgentEvent::EntityRemoved { id: 20 }));
    for (i, e) in s.entities.iter().enumerate() {
        assert_eq!(s.entity_index.get(&e.id), Some(&i));
    }
    assert!(!s.entity_index.contains_key(&20));

    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(40, None, EntityKind::Npc),
        pos_present: true,
    }));
    for (i, e) in s.entities.iter().enumerate() {
        assert_eq!(s.entity_index.get(&e.id), Some(&i));
    }

    s.apply_event(&AgentEvent::ZoneChanged {
        from: None,
        to: 103,
        myroom: None,
        mog_zone_flag: false,
    });
    assert!(s.entities.is_empty());
    assert!(s.entity_index.is_empty());
}

/// The pending sets carry exactly the ids that changed since the last drain.
#[test]
fn pending_entity_sets_carry_exactly_the_changed_ids() {
    let mut s = SessionState::default();

    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(1, Some("a"), EntityKind::Mob),
        pos_present: true,
    }));
    let (up, rem) = s.take_pending_entities();
    assert_eq!(up, std::collections::HashSet::from([1u32]));
    assert!(rem.is_empty());

    let same = make_test_entity(1, Some("a"), EntityKind::Mob);
    assert!(!s.apply_event(&AgentEvent::EntityUpserted {
        entity: same,
        pos_present: true,
    }));
    let (up, rem) = s.take_pending_entities();
    assert!(up.is_empty(), "no-op upsert must not stamp");
    assert!(rem.is_empty());

    let mut moved = make_test_entity(1, Some("a"), EntityKind::Mob);
    moved.pos.z += 5.0;
    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: moved,
        pos_present: true,
    }));
    assert!(!s.apply_event(&AgentEvent::EntityRemoved { id: 999 }));
    let (up, rem) = s.take_pending_entities();
    assert_eq!(up, std::collections::HashSet::from([1u32]));
    assert!(rem.is_empty());

    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(2, None, EntityKind::Npc),
        pos_present: true,
    }));
    assert!(s.apply_event(&AgentEvent::EntityRemoved { id: 2 }));
    let (up, rem) = s.take_pending_entities();
    assert!(up.is_empty(), "voided upsert must not survive the drain");
    assert_eq!(rem, std::collections::HashSet::from([2u32]));

    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(3, None, EntityKind::Mob),
        pos_present: true,
    }));
    s.apply_event(&AgentEvent::ZoneChanged {
        from: Some(103),
        to: 104,
        myroom: None,
        mog_zone_flag: false,
    });
    let (up, rem) = s.take_pending_entities();
    assert!(up.is_empty());
    assert_eq!(
        rem,
        std::collections::HashSet::from([1u32, 3]),
        "entity 1 is still live at the wipe, so both ids are marked removed"
    );

    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(4, None, EntityKind::Mob),
        pos_present: true,
    }));
    let (up, rem) = s.take_pending_entities();
    assert_eq!(up, std::collections::HashSet::from([4u32]));
    assert!(rem.is_empty());
}

#[test]
fn self_position_events_stamp_the_self_id() {
    let mut s = SessionState::default();
    s.apply_event(&AgentEvent::Connected {
        account_id: 1,
        char_id: 7,
        character: "Cow".into(),
        zone_id: 103,
    });
    assert!(s.apply_event(&AgentEvent::EntityUpserted {
        entity: make_test_entity(7, Some("Cow"), EntityKind::Pc),
        pos_present: true,
    }));
    s.take_pending_entities();

    let p1 = Position {
        pos: Vec3 {
            x: 1.0,
            y: 2.0,
            z: 9.0,
        },
        heading: 4,
        speed: 5,
        speed_base: 5,
    };
    assert!(s.apply_event(&AgentEvent::PositionChanged { pos: p1 }));
    let (up, rem) = s.take_pending_entities();
    assert_eq!(up, std::collections::HashSet::from([7u32]));
    assert!(rem.is_empty());

    assert!(!s.apply_event(&AgentEvent::PositionChanged { pos: p1 }));
    let (up, _) = s.take_pending_entities();
    assert!(up.is_empty(), "identical self position must not stamp");
}

#[test]
fn respawn_cancels_pending_removal_before_batch_drain() {
    let mut state = SessionState::default();
    for event in [
        AgentEvent::EntityUpserted {
            entity: make_test_entity(9, Some("old"), EntityKind::Mob),
            pos_present: true,
        },
        AgentEvent::EntityRemoved { id: 9 },
        AgentEvent::EntityUpserted {
            entity: make_test_entity(9, Some("new"), EntityKind::Mob),
            pos_present: true,
        },
    ] {
        assert!(state.apply_event(&event));
    }
    let (upserts, removals) = state.take_pending_entities();
    assert_eq!(upserts, std::collections::HashSet::from([9]));
    assert!(removals.is_empty());
    assert_eq!(
        state.entities[state.entity_index[&9]].name.as_deref(),
        Some("new")
    );
}

#[test]
fn legacy_clock_hold_event_uses_hour_only_defaults() {
    let event: AgentEvent = serde_json::from_value(serde_json::json!({
        "type": "cutscene_cue",
        "cue": {"ClockHold": {"stop": true, "hour": null}}
    }))
    .unwrap();
    assert!(matches!(
        event,
        AgentEvent::CutsceneCue {
            cue: CutsceneCue::ClockHold {
                stop: true,
                hour: None,
                minute: 0,
                day_from_epoch: None
            }
        }
    ));
}

/// The raw Attr is part of the record, and it survives a fold that restates no Attr — but only for
/// the item it was stamped with.
#[test]
fn item_attr_stamps_the_raw_attr_and_a_bare_fold_keeps_it_for_the_same_item() {
    // FFXiMain.dll retail-2026-09 `.text 0xEC630` copies the whole 24-byte Attr into the inventory
    // record, and the Mog House furniture law reads its layer/pose bytes out of that copy
    // (`.text 0x1CCFD1`). A later s2c 0x01F-shaped fold carries no Attr at all; losing it here would
    // empty the room.
    let attr = [
        0x00, 0x41, 0x00, 0x00, 0x00, 0x00, 0xf8, 0x02, 0x06, 0x03, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff, 0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80,
    ];
    let mut s = SessionState::default();

    let placed = ItemSlot {
        index: 7,
        item_no: 100,
        quantity: 1,
        locked: false,
        unselectable: false,
        price: 0,
        charges_remaining: None,
        next_use_vana_ts: None,
        use_delay_end_vana_ts: None,
        ready: None,
        extdata: Some(attr),
    };
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: crate::myroom_furniture::CONTAINER_MOGSAFE,
        update: InventoryUpdate::SlotChanged { slot: placed },
    });
    let shelf = crate::myroom_furniture::CONTAINER_MOGSAFE;
    assert_eq!(
        s.inventory.containers[&shelf].slots[0].extdata,
        Some(attr),
        "the Attr arrives verbatim on the record"
    );

    // Same item, no Attr restated: kept.
    let mut bare = ItemSlot {
        index: 7,
        item_no: 100,
        quantity: 1,
        locked: true,
        unselectable: false,
        price: 0,
        charges_remaining: None,
        next_use_vana_ts: None,
        use_delay_end_vana_ts: None,
        ready: None,
        extdata: None,
    };
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: shelf,
        update: InventoryUpdate::SlotChanged { slot: bare.clone() },
    });
    let slots = &s.inventory.containers[&shelf].slots;
    assert_eq!(slots.len(), 1);
    assert!(slots[0].locked, "the fold's own fields still apply");
    assert_eq!(slots[0].extdata, Some(attr));

    // Another item in the same seat: its Attr is unknown until s2c 0x020 says otherwise — a stale
    // pose from the previous occupant would place furniture the server never put there.
    bare.item_no = 4112;
    bare.locked = false;
    s.apply_event(&AgentEvent::InventoryUpdated {
        container: shelf,
        update: InventoryUpdate::SlotChanged { slot: bare },
    });
    let slots = &s.inventory.containers[&shelf].slots;
    assert_eq!(slots[0].item_no, 4112);
    assert_eq!(slots[0].extdata, None);
}

/// The law applied to the state's two Mog House shelves: what a player placed, and only what they
/// placed on the floor they are standing on.
#[test]
fn mog_house_shelf_records_rebuild_the_furniture_table() {
    use crate::myroom_furniture::{
        furniture_index, rebuild_furniture_objects, StaticItemKinds, CONTAINER_MOGSAFE2,
    };
    let installed = |layer: u8, col: i8, level: i8, row: i8| {
        let mut e = [0u8; 24];
        e[1] = layer | 0x40;
        e[6] = col as u8;
        e[7] = level as u8;
        e[8] = row as u8;
        e
    };
    let mut s = SessionState::default();
    for (container, slot, item_no, extdata) in [
        (
            crate::myroom_furniture::CONTAINER_MOGSAFE,
            3u8,
            100u16,
            installed(0, 5, 2, -9),
        ),
        // Same shelf, other floor: exists but is not drawn from here.
        (
            crate::myroom_furniture::CONTAINER_MOGSAFE,
            4u8,
            100u16,
            installed(1, 6, 0, 0),
        ),
        // Stored on the shelf rather than placed in the room.
        (
            crate::myroom_furniture::CONTAINER_MOGSAFE,
            5u8,
            200u16,
            [0u8; 24],
        ),
        // A flowerpot on the second shelf.
        (CONTAINER_MOGSAFE2, 2u8, 300u16, installed(0, -7, 1, 4)),
    ] {
        s.apply_event(&AgentEvent::InventoryUpdated {
            container,
            update: InventoryUpdate::SlotChanged {
                slot: ItemSlot {
                    index: slot,
                    item_no,
                    quantity: 1,
                    locked: false,
                    unselectable: false,
                    price: 0,
                    charges_remaining: None,
                    next_use_vana_ts: None,
                    use_delay_end_vana_ts: None,
                    ready: None,
                    extdata: Some(extdata),
                },
            },
        });
    }

    let kinds = StaticItemKinds(vec![(100, 10), (200, 10), (300, 12)]);
    let shelf_records = |s: &SessionState| {
        s.inventory
            .containers
            .iter()
            .filter(|(id, _)| crate::myroom_furniture::is_mog_house_shelf(**id))
            .flat_map(|(id, c)| {
                c.slots
                    .iter()
                    .map(move |slot| (*id, slot.index, slot.item_no, slot.extdata))
            })
            .collect::<Vec<_>>()
    };

    let downstairs = rebuild_furniture_objects(shelf_records(&s), 0, &kinds);
    assert_eq!(
        downstairs
            .iter()
            .map(|o| (o.container, o.slot, o.item_no, o.col))
            .collect::<Vec<_>>(),
        vec![
            (crate::myroom_furniture::CONTAINER_MOGSAFE, 3, 100, 5),
            (CONTAINER_MOGSAFE2, 2, 300, -7),
        ],
    );
    assert_eq!(downstairs[1].index, furniture_index(CONTAINER_MOGSAFE2, 2));

    // The other two pose bytes ride along untouched: byte 7 is LSB's stacking level and byte 8 its
    // row (vendor/server/src/map/items/item_furnishing.cpp getLevel/getRow).
    assert_eq!(
        (downstairs[0].level, downstairs[0].row),
        (2, -9),
        "shelf records keep Attr bytes 7 and 8 verbatim"
    );

    // Standing upstairs flips which records survive; the login packet's sub-map byte is what moved.
    s.myroom = Some(MyRoomInfo {
        model: 0x1FF,
        sub_map: 2,
        exit_bit: 0,
        door_open: false,
    });
    let upstairs_floor = crate::myroom_furniture::current_furniture_floor(
        s.myroom.map(|m| m.sub_map).unwrap_or_default(),
    );
    assert_eq!(upstairs_floor, 1);
    let upstairs = rebuild_furniture_objects(shelf_records(&s), upstairs_floor, &kinds);
    assert_eq!(
        upstairs
            .iter()
            .map(|o| (o.container, o.slot))
            .collect::<Vec<_>>(),
        vec![(crate::myroom_furniture::CONTAINER_MOGSAFE, 4)],
    );
}

/// The Unity ranking fold (.agents/skills/retail-observe/references/2026-10-15-unity-and-fp-windows.md):
/// chunks 1 and 2 fill members and points of their set (any nonzero result set is set 1), the last
/// chunk marks the set complete, and a sent c2s 0x116 clears that mark; s2c 0x110 arms c2s 0x117
/// and the sent 0x117 disarms it. Every arrival rides the snapshot.
#[test]
fn unity_ranking_chunks_complete_sets_and_requests_clear_the_marks() {
    use ffxi_proto::decode::MiscdataUnity;
    use kuluu_snapshot::UNITY_CONCORDS;
    let members = [3u32; UNITY_CONCORDS];
    let points = [900u32; UNITY_CONCORDS];
    let mut state = SessionState::default();
    assert!(state.apply_event(&AgentEvent::UnityRankingChunk {
        result_set: 2,
        data_type: MiscdataUnity::DATA_TYPE_MEMBERS,
        concord_values: members,
    }));
    assert!(state.apply_event(&AgentEvent::UnityRankingChunk {
        result_set: 1,
        data_type: MiscdataUnity::DATA_TYPE_POINTS,
        concord_values: points,
    }));
    assert_eq!(state.unity.rankings[1].members, members, "nonzero is set 1");
    assert_eq!(state.unity.rankings[1].points, points);
    assert!(!state.unity.rankings[1].complete);
    assert_eq!(state.unity.rankings[0], Default::default());

    assert!(state.apply_event(&AgentEvent::UnityRankingChunk {
        result_set: 1,
        data_type: MiscdataUnity::LAST_CHUNK,
        concord_values: [0; UNITY_CONCORDS],
    }));
    assert!(state.unity.rankings[1].complete);
    assert_eq!(
        state.unity.rankings[1].points, points,
        "the last chunk carries no values to overwrite"
    );
    assert!(
        crate::wire_translate::state_to_snapshot(&state)
            .unity
            .rankings[1]
            .complete,
        "rides the snapshot"
    );

    assert!(!state.apply_event(&AgentEvent::UnityRankingRequested { result_set: 0 }));
    assert!(state.apply_event(&AgentEvent::UnityRankingRequested { result_set: 1 }));
    assert!(!state.unity.rankings[1].complete);

    assert!(state.apply_event(&AgentEvent::UnityQuestUpdated {
        sparks: 0,
        deeds: 0,
        plaudits: 0,
    }));
    assert!(state.unity.quest_armed);
    assert!(state.apply_event(&AgentEvent::UnityQuestRequested));
    assert!(!state.unity.quest_armed);
}

/// s2c 0x08E replaces the alter ego point block, the first s2c 0x119 readies the Status data for
/// good, and zone-in's login state lands as sent; none of it is cleared by a zone change.
#[test]
fn alter_ego_points_status_ready_and_login_state_fold() {
    let mut state = SessionState::default();
    let block = kuluu_snapshot::AlterEgoPoints {
        points: 640,
        upgrades: [1; kuluu_snapshot::ALTER_EGO_UPGRADE_SLOTS],
        costs: [2; kuluu_snapshot::ALTER_EGO_UPGRADE_SLOTS],
    };
    assert!(state.apply_event(&AgentEvent::AlterEgoPointsUpdated { points: block }));
    assert!(!state.apply_event(&AgentEvent::AlterEgoPointsUpdated { points: block }));
    assert_eq!(state.alter_ego_points, Some(block));

    assert!(!state.status_data_ready);
    assert!(state.apply_event(&AgentEvent::AbilityRecastsUpdated {
        recasts: Vec::new()
    }));
    assert!(
        state.status_data_ready,
        "an empty 0x119 still readies Status"
    );
    assert!(!state.apply_event(&AgentEvent::AbilityRecastsUpdated {
        recasts: Vec::new()
    }));

    assert!(state.apply_event(&AgentEvent::LoginStateSynced {
        login_state: 5,
        is_monstrosity: true,
    }));
    state.apply_event(&AgentEvent::ZoneChanged {
        from: Some(100),
        to: 101,
        myroom: None,
        mog_zone_flag: false,
    });
    let snap = crate::wire_translate::state_to_snapshot(&state);
    assert_eq!(snap.alter_ego_points, Some(block));
    assert!(snap.status_data_ready);
    assert_eq!(snap.login_state, 5);
    assert!(snap.login_is_monstrosity);
}

/// s2c 0x08C entries are stored by merit index with bit 0 of the wire id ignored: an entry with
/// bit 0 set deletes its merit, any other inserts or replaces it
/// (.agents/skills/retail-observe/references/2026-10-15-merit-and-job-point-windows.md).
#[test]
fn merit_entries_fold_by_index_and_odd_ids_delete() {
    use crate::state::{merit_index_of_wire_id, MeritWire, MERIT_WIRE_DELETE_BIT};
    use kuluu_snapshot::MeritEntry;
    const HP_WIRE_ID: u16 = 0x40;
    const MP_WIRE_ID: u16 = 0x42;
    let hp = merit_index_of_wire_id(HP_WIRE_ID);
    let mp = merit_index_of_wire_id(MP_WIRE_ID);
    let wire = |index: u16, next: u8, count: u8| MeritWire { index, next, count };

    let mut state = SessionState::default();
    assert!(state.apply_event(&AgentEvent::MeritsListed {
        entries: vec![wire(HP_WIRE_ID, 3, 1), wire(MP_WIRE_ID, 1, 0)],
    }));
    assert_eq!(
        state.merit_entries.get(&hp),
        Some(&MeritEntry { next: 3, count: 1 })
    );
    assert_eq!(state.merit_entries.len(), 2);
    assert!(
        !state.apply_event(&AgentEvent::MeritsListed {
            entries: vec![wire(HP_WIRE_ID, 3, 1)],
        }),
        "the same entry again changes nothing"
    );

    assert!(state.apply_event(&AgentEvent::MeritsListed {
        entries: vec![wire(HP_WIRE_ID, 4, 2)],
    }));
    assert_eq!(state.merit_entries[&hp], MeritEntry { next: 4, count: 2 });

    assert!(state.apply_event(&AgentEvent::MeritsListed {
        entries: vec![wire(MP_WIRE_ID | MERIT_WIRE_DELETE_BIT, 9, 9)],
    }));
    assert!(!state.merit_entries.contains_key(&mp), "an odd id deletes");
    assert!(!state.apply_event(&AgentEvent::MeritsListed {
        entries: vec![wire(MP_WIRE_ID | MERIT_WIRE_DELETE_BIT, 0, 0)],
    }));
    let snap = crate::wire_translate::state_to_snapshot(&state);
    assert_eq!(snap.merit_entries, state.merit_entries);
}

/// s2c 0x08D stores level and next by type id, skips an empty first word or a job past 23, and
/// marks the job point data received even when it stores nothing; the 0x063 JobPoints record
/// replaces the last one.
#[test]
fn job_point_entries_and_record_fold() {
    use crate::state::JobPointWire;
    use kuluu_snapshot::{JobPointType, JobPointsJob, JobPointsRecord, JOB_POINT_JOBS};
    const RDM: u16 = 5;
    let wire = |job_no: u16, index: u8, next: u16, level: u8| JobPointWire {
        index,
        job_no,
        next,
        level,
    };
    let rdm_type = RDM * ffxi_proto::decode::JobPointWireEntry::TYPES_PER_JOB + 2;

    let mut state = SessionState::default();
    assert!(state.apply_event(&AgentEvent::JobPointsListed {
        entries: vec![wire(0, 0, 7, 7)],
    }));
    assert!(
        state.job_points_received,
        "an all-empty packet still counts"
    );
    assert!(state.job_point_types.is_empty());

    let past_last_job = u16::try_from(JOB_POINT_JOBS).unwrap();
    assert!(state.apply_event(&AgentEvent::JobPointsListed {
        entries: vec![wire(RDM, 2, 300, 4), wire(past_last_job, 1, 1, 1)],
    }));
    assert_eq!(
        state.job_point_types.get(&rdm_type),
        Some(&JobPointType {
            level: 4,
            next: 300
        })
    );
    assert_eq!(state.job_point_types.len(), 1, "job 24 is skipped");
    assert!(!state.apply_event(&AgentEvent::JobPointsListed {
        entries: vec![wire(RDM, 2, 300, 4)],
    }));

    let mut record = JobPointsRecord {
        access: 1,
        ..Default::default()
    };
    record.jobs[usize::from(RDM)] = JobPointsJob {
        capacity_points: 100,
        current_jp: 12,
        total_jp_spent: 40,
    };
    assert!(state.apply_event(&AgentEvent::JobPointsRecordUpdated { record }));
    assert!(!state.apply_event(&AgentEvent::JobPointsRecordUpdated { record }));
    let snap = crate::wire_translate::state_to_snapshot(&state);
    assert_eq!(snap.job_points, Some(record));
    assert!(snap.job_points_received);
    assert_eq!(snap.job_point_types, state.job_point_types);
}

/// The 0x061 main job level crosses into the snapshot for the merit and job point gates.
#[test]
fn char_stats_carry_the_main_job_level_to_the_snapshot() {
    let mut state = SessionState::default();
    state.apply_event(&AgentEvent::CharStatsUpdated {
        stats: CharStatsRaw {
            mjob_lv: 96,
            ..Default::default()
        },
    });
    let stats = crate::wire_translate::state_to_snapshot(&state).stats;
    assert_eq!(stats.map(|s| s.mjob_lv), Some(96));
}

/// The 0x061 Unity fields and main job cross into the snapshot's stat block unchanged.
#[test]
fn char_stats_carry_unity_fields_and_main_job_to_the_snapshot() {
    let mut state = SessionState::default();
    state.apply_event(&AgentEvent::CharStatsUpdated {
        stats: CharStatsRaw {
            mjob_no: 23,
            unity_faction: 4,
            unity_points: 12_345,
            unity_points1: 6,
            unity_points2: 9,
            ..Default::default()
        },
    });
    let stats = crate::wire_translate::state_to_snapshot(&state)
        .stats
        .expect("stats");
    assert_eq!(
        (
            stats.mjob_no,
            stats.unity_faction,
            stats.unity_points,
            stats.unity_points1,
            stats.unity_points2
        ),
        (23, 4, 12_345, 6, 9)
    );
}
