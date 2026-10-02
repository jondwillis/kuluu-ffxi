use super::*;
use ffxi_proto::decode::PosHead;

/// Every battle-line expectation below is the no-install wording, so these
/// shadow the real entry points with the basic-message table absent: what a
/// session composes with one is pinned against a real install in
/// `tests/install_conformance.rs`.
fn decode_battle_message(
    data: &[u8],
    name_cache: &std::collections::HashMap<u32, String>,
    kind_cache: &std::collections::HashMap<u32, crate::state::EntityKind>,
    is_029: bool,
) -> Option<ChatLine> {
    super::decode_battle_message(data, name_cache, kind_cache, is_029, None)
        .into_iter()
        .next()
}

fn decode_battle2_action(
    data: &[u8],
    name_cache: &std::collections::HashMap<u32, String>,
    kind_cache: &std::collections::HashMap<u32, crate::state::EntityKind>,
) -> Vec<ChatLine> {
    super::decode_battle2_action(data, name_cache, kind_cache, None)
}

fn build_battle2_line(
    message_num: u16,
    cas_name: &str,
    tar_name: &str,
    cas_is_pc: bool,
    tar_is_pc: bool,
    amount: u32,
    action_id: u32,
    category: u8,
) -> Option<ChatLine> {
    let mut numbers = [0i64; sysmes::PARAM_SLOTS];
    numbers[MES_PARAM_ACTION_ID] = action_id as i64;
    numbers[MES_PARAM_MAIN_VALUE] = amount as i64;
    super::build_battle2_line(
        None,
        message_num,
        cas_name,
        tar_name,
        cas_is_pc,
        tar_is_pc,
        amount,
        action_id,
        category,
        numbers,
    )
    .into_iter()
    .next()
}

/// Drives the real [`handle_sub_packet`] arm for `opcode` and returns the
/// events it emitted, in emission order.
fn sub_packet_events(opcode: u16, body: &[u8]) -> Vec<AgentEvent> {
    sub_packet_events_with_names(
        opcode,
        body,
        &ffxi_dat::autotranslate_names::InstalledNames::default(),
    )
}

#[test]
fn level_up_battle_packets_preserve_the_leveling_caster_through_viewer_dispatch() {
    // vendor/server/src/map/packets/s2c/0x029_battle_message.h PacketData;
    // vendor/server/src/map/packets/s2c/0x02d_battle_message2.h PacketData;
    // vendor/server/src/map/utils/charutils.cpp DistributeExperiencePoints;
    // vendor/server/src/map/enums/msg_basic.h MsgBasic::LevelUp.
    const TEST_PINNED_LEVEL_UP_MESSAGE: u16 = 9;
    const TEST_PINNED_BATTLE_BODY_LEN: usize = 24;
    const TEST_PINNED_MESSAGE_OFFSET: usize = 20;
    const LEVELING_CASTER: u32 = 0x1234_5678;
    const DEFEATED_TARGET: u32 = 0x8765_4321;
    let mut body = [0u8; TEST_PINNED_BATTLE_BODY_LEN];
    body[..4].copy_from_slice(&LEVELING_CASTER.to_le_bytes());
    body[4..8].copy_from_slice(&DEFEATED_TARGET.to_le_bytes());
    body[TEST_PINNED_MESSAGE_OFFSET..TEST_PINNED_MESSAGE_OFFSET + 2]
        .copy_from_slice(&TEST_PINNED_LEVEL_UP_MESSAGE.to_le_bytes());
    for opcode in [
        ffxi_proto::map::s2c::BATTLE_MESSAGE,
        ffxi_proto::map::s2c::BATTLE_MESSAGE2,
    ] {
        let events = sub_packet_events(opcode, &body);
        let level_ups: Vec<_> = events
            .into_iter()
            .filter(|event| matches!(event, AgentEvent::LevelUp { .. }))
            .filter_map(crate::wire_translate::event_to_viewer_event)
            .collect();
        assert_eq!(level_ups.len(), 1);
        assert!(matches!(
            level_ups[0],
            kuluu_snapshot::ViewerEvent::LevelUp {
                player_id: LEVELING_CASTER
            }
        ));
        assert!(
            !sub_packet_events(opcode, &body[..TEST_PINNED_MESSAGE_OFFSET])
                .iter()
                .any(|event| matches!(event, AgentEvent::LevelUp { .. }))
        );
        let mut unrelated = body;
        unrelated[TEST_PINNED_MESSAGE_OFFSET..TEST_PINNED_MESSAGE_OFFSET + 2].fill(0);
        assert!(!sub_packet_events(opcode, &unrelated)
            .iter()
            .any(|event| matches!(event, AgentEvent::LevelUp { .. })));
    }
}

fn sub_packet_events_with_names(
    opcode: u16,
    body: &[u8],
    names: &ffxi_dat::autotranslate_names::InstalledNames,
) -> Vec<AgentEvent> {
    let (tx, mut rx) = broadcast::channel(64);
    handle_sub_packet(
        &framing::SubPacket {
            opcode,
            sequence: 0,
            data: body,
        },
        &tx,
        &mut Vec::new(),
        &mut crate::event_dialog::CutsceneScope::default(),
        0,
        "Tester",
        &mut None,
        &mut std::collections::HashMap::new(),
        &mut std::collections::HashMap::new(),
        &mut std::collections::HashMap::new(),
        &mut std::collections::HashMap::new(),
        &mut std::collections::HashMap::new(),
        &mut 0,
        &mut Position::default(),
        &mut false,
        &mut NpcNameResolver::new(None),
        &mut EmoteTextResolver::new(None),
        names,
        &mut treasure::SysMesResolver::new(None),
        &mut MesBasicResolver::new(None),
        &mut treasure::TreasurePool::default(),
        &mut false,
        &mut SelfMogState::default(),
        &mut ShopSession::default(),
        None,
    );
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

#[test]
// vendor/server/src/map/packets/entity_update.cpp CEntityUpdatePacket::updateWith
// vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN
pub(super) fn ferry_packet_state_contract() {
    use crate::state::SessionState;
    use crate::wire_translate::state_to_snapshot;
    use kuluu_snapshot::{EntityLook, Voyage};

    const SHIP: u32 = 17_793_088;
    const SHIP_INDEX: u16 = 64;
    const BODY_LEN: usize = 68;
    const SEND_FLAGS: usize = 6;
    const UPDATE_COMBAT: u8 = 7;
    const STATUS: usize = 28;
    const ANIMATION: usize = 27;
    const LOOK: usize = 44;
    const SELECTOR: usize = 48;
    const ANIMATION_START: usize = 52;
    const SHIP_MODEL: u16 = 4;
    const SELECTOR_VALUE: u32 = 14;
    const TIMESTAMP: u32 = 0x1200_3400;
    const ARRIVE: u8 = 18;
    const DEPART: u8 = 19;
    const DISAPPEAR: u8 = 2;
    const LOGIN_LEN: usize = 122;
    const LOGIN_ZONE: usize = 44;
    const VOYAGE_START: usize = 116;
    const VOYAGE_DURATION: usize = 120;
    const VOYAGE_FLAGS: usize = 35;
    const ROUTE_FLAGS: usize = 38;
    const REVERSE: u8 = 4;
    const ROUTE: u8 = 2;
    const ROUTE_SHIFT: u8 = 3;
    const JOURNEY_ZONE: u32 = 228;
    const DURATION: u16 = 897;

    let mut body = [0u8; BODY_LEN];
    body[..4].copy_from_slice(&SHIP.to_le_bytes());
    body[4..6].copy_from_slice(&SHIP_INDEX.to_le_bytes());
    body[SEND_FLAGS] = UPDATE_COMBAT;
    body[LOOK..LOOK + 2].copy_from_slice(&SHIP_MODEL.to_le_bytes());
    body[SELECTOR..SELECTOR + 4].copy_from_slice(&SELECTOR_VALUE.to_le_bytes());
    let mut state = SessionState::default();
    for (animation, status, timestamp) in
        [(ARRIVE, 0, TIMESTAMP), (DEPART, DISAPPEAR, TIMESTAMP + 1)]
    {
        body[ANIMATION] = animation;
        body[STATUS] = status;
        body[ANIMATION_START..ANIMATION_START + 4].copy_from_slice(&timestamp.to_le_bytes());
        let events = sub_packet_events(ffxi_proto::map::s2c::CHAR_NPC, &body);
        assert!(!events
            .iter()
            .any(|event| matches!(event, AgentEvent::EntityRemoved { .. })));
        for event in events {
            state.apply_event(&event);
        }
        let snapshot = state_to_snapshot(&state);
        let ship = snapshot
            .entities
            .iter()
            .find(|entity| entity.id == SHIP)
            .unwrap();
        assert_eq!(
            ship.look,
            Some(EntityLook::Transport {
                size: SHIP_MODEL,
                model_id: Some(SELECTOR_VALUE),
                animation_start: Some(timestamp),
                travel_secs: None,
            })
        );
        assert_eq!(ship.animation, animation);
        assert_eq!(ship.is_invisible(), status == DISAPPEAR);
    }

    let mut login = [0u8; LOGIN_LEN];
    login[LOGIN_ZONE..LOGIN_ZONE + 4].copy_from_slice(&JOURNEY_ZONE.to_le_bytes());
    login[VOYAGE_START..VOYAGE_START + 4].copy_from_slice(&TIMESTAMP.to_le_bytes());
    login[VOYAGE_DURATION..VOYAGE_DURATION + 2].copy_from_slice(&DURATION.to_le_bytes());
    login[VOYAGE_FLAGS] = REVERSE;
    login[ROUTE_FLAGS] = ROUTE << ROUTE_SHIFT;
    let events = sub_packet_events(ffxi_proto::map::s2c::LOGIN, &login);
    let zone_at = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ZoneChanged { .. }))
        .unwrap();
    let voyage_at = events
        .iter()
        .position(|event| matches!(event, AgentEvent::VoyageSynced { .. }))
        .unwrap();
    assert!(voyage_at > zone_at);
    for event in events {
        state.apply_event(&event);
    }
    assert_eq!(
        state_to_snapshot(&state).voyage,
        Some(Voyage {
            start: TIMESTAMP,
            duration: DURATION,
            reverse: true,
            route: ROUTE,
        })
    );
    assert!(state.entities.iter().all(|entity| entity.id != SHIP));
    login[VOYAGE_START..].fill(0);
    for event in sub_packet_events(ffxi_proto::map::s2c::LOGIN, &login) {
        state.apply_event(&event);
    }
    assert_eq!(state_to_snapshot(&state).voyage, None);
}

/// s2c 0x058 ASSIST is the server's retarget push (`/assist`, engage,
/// auto-target-after-kill); the dispatch must turn `AssistNo` into the client's
/// new target (vendor/server/src/map/packets/s2c/0x058_assist.h).
#[test]
fn assist_packet_retargets_to_assist_no() {
    use ffxi_proto::decode::Assist;

    let mut body = vec![0u8; Assist::SIZE];
    body[Assist::UNIQUE_NO_OFFSET..Assist::UNIQUE_NO_OFFSET + 4]
        .copy_from_slice(&0x0100_0F42u32.to_le_bytes());
    body[Assist::ASSIST_NO_OFFSET..Assist::ASSIST_NO_OFFSET + 4]
        .copy_from_slice(&0x0100_07D1u32.to_le_bytes());
    body[Assist::ACT_INDEX_OFFSET..Assist::ACT_INDEX_OFFSET + 2]
        .copy_from_slice(&0x0442u16.to_le_bytes());

    let events = sub_packet_events(ffxi_proto::map::s2c::ASSIST, &body);
    assert!(
        matches!(
            events.as_slice(),
            [AgentEvent::TargetChanged {
                target_id: Some(0x0100_07D1),
            }]
        ),
        "{events:?}"
    );
}

/// LSB builds the packet over a zeroed buffer and leaves `AssistNo` at 0 when
/// the target went away (`CCharEntity::OnChangeTarget` with a null target,
/// 0x058_assist.cpp GP_SERV_COMMAND_ASSIST::GP_SERV_COMMAND_ASSIST), so 0 must not be reported as entity 0.
#[test]
fn assist_packet_reports_a_zero_assist_no_as_no_target() {
    let events = sub_packet_events(
        ffxi_proto::map::s2c::ASSIST,
        &[0u8; ffxi_proto::decode::Assist::SIZE],
    );
    assert!(
        matches!(
            events.as_slice(),
            [AgentEvent::TargetChanged { target_id: None }]
        ),
        "{events:?}"
    );
}

#[test]
fn assist_packet_rejects_a_truncated_body() {
    let events = sub_packet_events(
        ffxi_proto::map::s2c::ASSIST,
        &[0u8; ffxi_proto::decode::Assist::SIZE - 1],
    );
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn death_menu_packet_emits_the_server_offer() {
    use ffxi_proto::decode::DeathMenuOffer;

    const DEATH_MENU_BODY_SIZE: usize = 8;
    const DEATH_MENU_TRACTOR_TYPE: u16 = 2;
    let mut body = [0u8; DEATH_MENU_BODY_SIZE];
    body[0..4].copy_from_slice(&0x0102_0304u32.to_le_bytes());
    body[4..6].copy_from_slice(&0x0506u16.to_le_bytes());
    body[6..8].copy_from_slice(&DEATH_MENU_TRACTOR_TYPE.to_le_bytes());

    let events = sub_packet_events(ffxi_proto::map::s2c::DEATH_MENU, &body);
    assert!(matches!(
        events.as_slice(),
        [AgentEvent::DeathMenuUpdated {
            offer: Some(DeathMenuOffer::Tractor),
        }]
    ));
}

#[test]
fn death_menu_packet_rejects_a_truncated_body() {
    let events = sub_packet_events(ffxi_proto::map::s2c::DEATH_MENU, &[0; 7]);
    assert!(events.is_empty());
    assert!(
        !first_decode_err(ffxi_proto::map::s2c::DEATH_MENU),
        "the decode failure must have passed through the warning dedup gate"
    );
}

/// `ZoneChanged` clears `SessionState::current_weather`, so the LOGIN arm
/// must emit the 0x00A zone-in weather *after* it. Reversed, a zoning
/// character renders the default sky until the next 0x057 — which LSB only
/// sends on a weather change (vendor/server/src/map/zone.cpp CZone::SetWeather).
#[test]
fn login_emits_zone_in_weather_after_the_zone_change() {
    const WEATHER_NUMBER: u16 = 4;
    use ffxi_proto::decode::ServerLogin;

    let mut body = vec![0u8; ServerLogin::WEATHER_OFFSET_TIME_OFFSET + 4];
    body[ServerLogin::WEATHER_NUMBER_OFFSET..ServerLogin::WEATHER_NUMBER_OFFSET + 2]
        .copy_from_slice(&WEATHER_NUMBER.to_le_bytes());

    let events = sub_packet_events(ffxi_proto::map::s2c::LOGIN, &body);
    let zone_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ZoneChanged { .. }))
        .expect("LOGIN emits ZoneChanged");
    let weather_at = events
            .iter()
            .position(
                |e| matches!(e, AgentEvent::WeatherUpdated { weather_number } if *weather_number == WEATHER_NUMBER),
            )
            .expect("LOGIN emits the zone-in weather");
    assert!(
        weather_at > zone_at,
        "WeatherUpdated must follow ZoneChanged, got {events:?}"
    );
}

/// `ZoneChanged` clears `SessionState::death_homepoint_secs`, and 0x037
/// CHAR_STATUS only arrives on a status *change*, so a character who zoned in
/// still KO'd depends on the LOGIN arm re-publishing the timer afterwards.
#[test]
fn login_emits_the_homepoint_timer_after_the_zone_change_only_while_ko() {
    use ffxi_proto::decode::{dead_counter_seconds_until_homepoint, PosHead, ServerLogin};

    /// An arbitrary counter, converted by the decoder's own formula rather than a
    /// second copy of it here.
    const DEAD_COUNTER: u32 = 129_600;
    const FULL_HP_PCT: u8 = 100;
    let remaining = dead_counter_seconds_until_homepoint(DEAD_COUNTER);
    assert!(remaining > 0, "the fixture must describe a live countdown");

    let mut body = vec![0u8; ServerLogin::DEAD_COUNTER_OFFSET + 4];
    body[ServerLogin::DEAD_COUNTER_OFFSET..ServerLogin::DEAD_COUNTER_OFFSET + 4]
        .copy_from_slice(&DEAD_COUNTER.to_le_bytes());

    body[PosHead::HPP_OFFSET] = FULL_HP_PCT;
    assert!(
        !sub_packet_events(ffxi_proto::map::s2c::LOGIN, &body)
            .iter()
            .any(|e| matches!(e, AgentEvent::DeathTimerUpdated { .. })),
        "a living character carries the same counter and must publish no timer"
    );

    body[PosHead::HPP_OFFSET] = 0;
    let events = sub_packet_events(ffxi_proto::map::s2c::LOGIN, &body);
    let zone_at = events
        .iter()
        .position(|e| matches!(e, AgentEvent::ZoneChanged { .. }))
        .expect("LOGIN emits ZoneChanged");
    let timer_at = events
        .iter()
        .position(|e| {
            matches!(
                e,
                AgentEvent::DeathTimerUpdated {
                    seconds_until_homepoint: Some(secs),
                } if *secs == remaining
            )
        })
        .expect("LOGIN emits the zone-in homepoint timer");
    assert!(
        timer_at > zone_at,
        "DeathTimerUpdated must follow ZoneChanged, got {events:?}"
    );
}

/// A 0x051 body whose GrapIDTbl does not decode must go through
/// [`warn_decode_err`] like every sibling arm — dropped silently, a wrong
/// body-offset assumption looks exactly like "self kept the launcher seed"
/// with nothing in the log. Observed through the dedup gate, which
/// `warn_decode_err` consumes for the opcode it logs.
#[test]
fn grap_list_decode_failure_is_logged() {
    use ffxi_proto::decode::LookData;
    use ffxi_proto::map::s2c;

    let body = vec![0u8; LookData::GRAP_LIST_TBL_OFFSET + LookData::GRAP_ID_TBL_LEN];
    assert!(
        LookData::decode_grap_list(&body).is_none(),
        "a zeroed GrapIDTbl is the undecodable case"
    );
    assert!(
        sub_packet_events(s2c::GRAP_LIST, &body).is_empty(),
        "no look is published from an undecodable body"
    );
    assert!(
        !first_decode_err(s2c::GRAP_LIST),
        "the failure must have been reported, consuming the dedup gate"
    );
}

#[test]
fn decode_err_dedup_is_per_opcode() {
    // Opcodes chosen well outside the retail range so parallel tests that
    // exercise real decode paths cannot race on the same entries.
    const DEDUP_OPCODE_A_PINNED: u16 = 0xFFFE;
    const DEDUP_OPCODE_B_PINNED: u16 = 0xFFFD;
    assert!(
        first_decode_err(DEDUP_OPCODE_A_PINNED),
        "first failure must pass the gate"
    );
    assert!(
        !first_decode_err(DEDUP_OPCODE_A_PINNED),
        "repeat failure for the same opcode must be deduped"
    );
    assert!(
        first_decode_err(DEDUP_OPCODE_B_PINNED),
        "dedup must be per-opcode, not global"
    );
}

#[test]
fn origin_seed_with_valid_fallback_is_repaired() {
    let dest = v(-16.039, -132.804, -4.217);
    assert_eq!(
        apply_zoneline_spawn_fallback(v(0.0, 0.0, 0.0), Some(dest)),
        dest,
        "origin seed must be replaced by the baked destination"
    );
}

#[test]
fn sane_seed_is_never_overridden() {
    let server = v(573.0, -326.6, -1.1);
    let dest = v(-16.039, -132.804, -4.217);
    assert_eq!(
        apply_zoneline_spawn_fallback(server, Some(dest)),
        server,
        "a non-origin server seed must win over the fallback"
    );
}

#[test]
fn origin_seed_without_fallback_stays_origin() {
    assert_eq!(
        apply_zoneline_spawn_fallback(v(0.0, 0.0, 0.0), None),
        v(0.0, 0.0, 0.0)
    );
}

#[test]
fn origin_fallback_does_not_replace_origin_seed() {
    assert_eq!(
        apply_zoneline_spawn_fallback(v(0.0, 0.0, 0.0), Some(v(0.2, -0.1, 0.0))),
        v(0.0, 0.0, 0.0)
    );
}

#[test]
fn myroom_login_keeps_forced_origin_seed() {
    // vendor/server/scripts/globals/moghouse.lua moghouseZoneLines setPos(0, 0, 0, 192):
    // the MH origin spawn is authoritative, not a bad seed to repair.
    let town_side = v(162.591, -4.103, 162.423);
    assert_eq!(
        spawn_seed_pos(v(0.0, 0.0, 0.0), Some(town_side), true),
        v(0.0, 0.0, 0.0),
        "MYROOM login must not be desynced to the town-side to_pos"
    );
    assert_eq!(
        spawn_seed_pos(v(0.0, 0.0, 0.0), Some(town_side), false),
        town_side,
        "outside MYROOM the origin repair still applies"
    );
}

/// A far (>snap) self-position carrier snaps us to the server in steady state, but
/// during the post-zone-in settle window (`refuse_snap`) it is an out-of-order /
/// duplicate position from around the transition and must keep our local seed instead
/// of yanking us into another zone's coordinate space ("same spot, different zone").
#[test]
fn far_carrier_snaps_in_steady_state_but_not_during_settle() {
    let local = v(-15.0, -132.8, -4.2);
    let stale = v(579.5, -305.1, -1.9);

    assert!(
        matches!(
            reconcile_self_pos(local, stale, false),
            SelfPosReconcile::Snap
        ),
        "steady state: a far carrier snaps to the server"
    );
    assert!(
        matches!(
            reconcile_self_pos(local, stale, true),
            SelfPosReconcile::KeepLocal
        ),
        "settle window: a far (out-of-order) carrier keeps our local seed"
    );

    let near = v(-14.0, -132.8, -4.2);
    assert!(
        matches!(
            reconcile_self_pos(local, near, true),
            SelfPosReconcile::KeepLocal
        ),
        "close carrier keeps local regardless of settle window"
    );
}

/// Pins the XIM doorOffset branches (AssetViewer.kt): 2F interiors
/// shift the door 3.15 yalms along native z, the [S]-city/Adoulin bases shift
/// along x.
#[test]
fn mh_door_pos_applies_xim_per_model_offsets() {
    assert_eq!(mh_door_pos(257), v(0.0, -8.0, -1.0), "classic 1F");
    assert_eq!(mh_door_pos(745), v(-0.5, -8.0, -1.0), "San d'Oria [S]");
    assert_eq!(mh_door_pos(219), v(-1.0, -8.0, -1.0), "Windurst [S]");
    assert_eq!(mh_door_pos(292), v(-1.0, -8.0, -1.0), "Adoulin");
    assert_eq!(mh_door_pos(199), v(-1.15, -8.0, -1.0), "Bastok [S]");
    for model in 615..=618 {
        let pos = mh_door_pos(model);
        assert_eq!((pos.x, pos.z), (0.0, -1.0), "2F model {model}");
        assert!(
            (pos.y - (-8.0 - 3.15)).abs() < 1e-5,
            "2F model {model} ground offset, got {}",
            pos.y
        );
    }
}

#[test]
fn myroom_job_packet_layout_matches_lsb_struct() {
    let buf = build_subpacket_myroom_job(0xBEEF, Some(5), Some(13));
    assert_eq!(buf.len(), 8, "4 hdr + MainJobIndex + SupportJobIndex + pad");
    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        id_and_size & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::MYROOM_JOB,
        "opcode MYROOM_JOB"
    );
    assert_eq!(id_and_size >> 9, 2, "size_words");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
    assert_eq!(buf[4], 5, "MainJobIndex");
    assert_eq!(buf[5], 13, "SupportJobIndex");
    assert_eq!(&buf[6..8], &[0, 0], "padding00");

    let keep = build_subpacket_myroom_job(0, None, None);
    assert_eq!(&keep[4..6], &[0, 0], "None → 0 = keep current job");
}

/// Pins the c2s 0x05D GP_CLI_COMMAND_MOTION layout
/// (vendor/server/src/map/packets/c2s/0x05d_motion.h): Mode at byte 11,
/// BEFORE Param — the s2c 0x05A layout puts Mode after Param, so a
/// transposition between the two must fail here.
#[test]
fn motion_packet_layout_matches_lsb_struct() {
    use ffxi_proto::map::emote;
    let buf = build_subpacket_motion(
        0xBEEF,
        0x0100_0F43,
        0x0443,
        emote::BELL,
        emote::mode::MOTION,
        emote::BELL_NOTE_MIN,
    );
    assert_eq!(
        buf.len(),
        16,
        "hdr + UniqueNo + ActIndex + Number/Mode/Param/pad"
    );
    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        id_and_size & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::MOTION,
        "opcode"
    );
    assert_eq!(id_and_size >> 9, 4, "size_words");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
    assert_eq!(
        u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        0x0100_0F43,
        "UniqueNo"
    );
    assert_eq!(u16::from_le_bytes([buf[8], buf[9]]), 0x0443, "ActIndex");
    assert_eq!(buf[10], emote::BELL, "Number");
    assert_eq!(buf[11], emote::mode::MOTION, "Mode precedes Param (c2s)");
    assert_eq!(
        u16::from_le_bytes([buf[12], buf[13]]),
        emote::BELL_NOTE_MIN,
        "Param"
    );
    assert_eq!(&buf[14..16], &[0, 0], "padding00");
}

/// c2s 0x119 GP_CLI_COMMAND_EMOTE_LIST is header-only (4 bytes, 1 word).
#[test]
fn emote_list_req_is_header_only() {
    let buf = build_subpacket_emote_list_req(0x1234);
    assert_eq!(buf.len(), 4);
    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        id_and_size & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::EMOTE_LIST
    );
    assert_eq!(id_and_size >> 9, 1, "size_words");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0x1234, "sync");
}

// Citation: vendor/server/src/map/packets/c2s/0x0f4_tracking_list.h — the
// wide-scan list request carries uint32 SendFlg and LSB only frames a reply
// when SendFlg == 1. A drive-by edit that zeroed/dropped it would silently
// stop wide-scan from ever populating.
#[test]
fn tracking_list_req_carries_sendflg_one() {
    let buf = build_subpacket_tracking_list(0x1234);
    assert_eq!(buf.len(), 8);
    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        id_and_size & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::TRACKING_LIST
    );
    assert_eq!(id_and_size >> 9, 2, "size_words");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0x1234, "sync");
    assert_eq!(
        u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]),
        ffxi_proto::map::tracking::SEND_FLG_REQUEST,
        "SendFlg must be 1",
    );
    assert_eq!(ffxi_proto::map::tracking::SEND_FLG_REQUEST, 1);
}

/// Mirrors the LSB 0x05D validator gate order (0x05d_motion.cpp).
#[test]
fn emote_send_block_mirrors_lsb_validator() {
    use ffxi_proto::map::emote;
    assert_eq!(emote_send_block_reason(8, emote::mode::ALL, 0, false), None);
    assert!(
        emote_send_block_reason(8, emote::mode::ALL, 0, true).is_some(),
        "blockedBy InEvent"
    );
    assert!(
        emote_send_block_reason(8, 3, 0, false).is_some(),
        "oneOf<EmoteMode>"
    );
    assert!(
        emote_send_block_reason(39, emote::mode::ALL, 0, false).is_some(),
        "39 is a gap in the Emote enum"
    );
    assert!(
        emote_send_block_reason(emote::BELL, emote::mode::ALL, 5, false).is_some(),
        "bell note below 0x06"
    );
    assert_eq!(
        emote_send_block_reason(emote::BELL, emote::mode::ALL, emote::BELL_NOTE_MAX, false),
        None
    );
}

#[test]
fn door_menu_picks_encode_zmrq_maprect_pairs() {
    use crate::local_menu::{Advance, LocalMenuSession, HOME_ROW, MOG_GARDEN_ROW};
    use crate::state::MyRoomInfo;

    let room = MyRoomInfo {
        model: 257,
        sub_map: 0,
        exit_bit: 1,
    };
    for (row, want_bit, want_mode) in [(HOME_ROW, 1u8, 0u8), (MOG_GARDEN_ROW, 0, 127)] {
        let mut menu = LocalMenuSession::new();
        let frame = menu.open_mh_exit(&room, None);
        let idx = frame.choices.iter().position(|c| c == row).expect(row);
        let Advance::Exit(kind) = menu.advance(Some(idx as u32)) else {
            panic!("{row} must be a terminal exit");
        };
        let (bit, mode) = kind.wire_pair();
        let buf = build_subpacket_maprect_mh_exit(7, bit, mode, 1.0, 2.0, 3.0, 0x42);
        assert_eq!(&buf[4..8], b"zmrq", "RectID fourcc");
        assert_eq!(buf[22], want_bit, "MyRoomExitBit for {row}");
        assert_eq!(buf[23], want_mode, "MyRoomExitMode for {row}");
    }
    assert_eq!(u32::from_le_bytes(*b"zmrq"), ZMRQ_LE);
}

#[test]
fn baked_zoneline_resolves_bastok_mines_destination() {
    let to_pos = kuluu_nav::to_pos_for_line(813314682)
        .expect("line 813314682 (S. Gustaberg → Bastok Mines) must exist");
    let dest = v(to_pos[0], to_pos[1], to_pos[2]);
    assert!(
        apply_zoneline_spawn_fallback(v(0.0, 0.0, 0.0), Some(dest)) == dest,
        "baked to_pos {to_pos:?} should be treated as a valid destination"
    );
}

const STATIC_TARGID: u16 = 0x123;
const DYNAMIC_TARGID: u16 = 0x712;

#[test]
fn standard_model_with_monster_flag_is_a_mob() {
    for look in [0u16, 5, 6] {
        assert_eq!(
            classify_char_npc(Some(look), STATIC_TARGID, false, true),
            EntityKind::Mob
        );
    }
}

#[test]
fn standard_model_without_monster_flag_is_an_npc() {
    for look in [0u16, 5, 6] {
        assert_eq!(
            classify_char_npc(Some(look), STATIC_TARGID, false, false),
            EntityKind::Npc
        );
    }
}

#[test]
fn dynamic_targid_mob_is_a_mob_without_monster_flag() {
    assert_eq!(
        classify_char_npc(Some(0), DYNAMIC_TARGID, false, false),
        EntityKind::Mob
    );
    assert_eq!(
        classify_char_npc(Some(0), DYNAMIC_TARGID, false, true),
        EntityKind::Mob
    );
}

#[test]
fn pc_owned_standard_model_is_a_pet() {
    assert_eq!(
        classify_char_npc(Some(0), DYNAMIC_TARGID, true, true),
        EntityKind::Pet
    );
}

#[test]
fn equipped_models_are_npcs_and_furniture_is_other() {
    assert_eq!(
        classify_char_npc(Some(1), STATIC_TARGID, false, true),
        EntityKind::Npc
    );
    assert_eq!(
        classify_char_npc(Some(7), STATIC_TARGID, false, false),
        EntityKind::Npc
    );
    for door_size in [2u16, 3, 4] {
        assert_eq!(
            classify_char_npc(Some(door_size), STATIC_TARGID, false, true),
            EntityKind::Other
        );
    }
    assert_eq!(
        classify_char_npc(None, STATIC_TARGID, false, true),
        EntityKind::Other
    );
}

/// A 0x0E CHAR_NPC body crafted exactly as LSB writes it —
/// vendor/server/src/map/packets/entity_update.cpp `updateWith`, sub.data base = LSB 0x04:
/// `[26]` hpp (LSB 0x1E), `[27]` animation (LSB 0x1F), `[28]` status (LSB 0x20, written on
/// EVERY update regardless of mask), `[29..33)` m_flags u32 (LSB 0x21, UPDATE_HP only),
/// `[39]` namevis (LSB 0x2B, UPDATE_HP only).
fn worm_body(send_flag: u8, status: u8, m_flags: u32, namevis: u8) -> Vec<u8> {
    // 48 bytes: PosHead::SIZE_WITH_BT_TARGET (44) plus the look-size word at
    // sub.data[0x2C..0x2E] (LSB 0x30, entity_update.cpp `ref<uint16>(0x30)`),
    // which classify_char_npc needs to see Some(0) = standard mob mesh.
    let mut b = vec![0u8; 48];
    b[0..4].copy_from_slice(&1000u32.to_le_bytes());
    b[4..6].copy_from_slice(&DYNAMIC_TARGID.to_le_bytes());
    b[6] = send_flag;
    b[8..12].copy_from_slice(&1.5f32.to_le_bytes());
    b[12..16].copy_from_slice(&(-2.0f32).to_le_bytes());
    b[16..20].copy_from_slice(&0.0f32.to_le_bytes());
    b[24] = 5;
    b[PosHead::HPP_OFFSET] = 100;
    b[28] = status;
    b[29..33].copy_from_slice(&m_flags.to_le_bytes());
    b[39] = namevis;
    b[44] = 0;
    b[45] = 0;
    b
}

fn feed_worm(s: &mut crate::state::SessionState, body: &[u8]) {
    for ev in sub_packet_events(ffxi_proto::map::s2c::CHAR_NPC, body) {
        if matches!(ev, AgentEvent::EntityUpserted { .. }) {
            s.apply_event(&ev);
        }
    }
}

fn worm_entity(s: &crate::state::SessionState) -> &crate::state::Entity {
    s.entities
        .iter()
        .find(|e| e.id == 1000)
        .expect("worm present")
}

/// The worm's full dive/surface cycle as LSB drives it (mob_controller.cpp CMobController::DoRoamTick):
/// spawn above ground -> dive (name hidden + untargetable; status stays NORMAL for the first
/// 3 s) -> move underground (status INVISIBLE piggybacks on POS ticks) -> surface (one UPDATE_HP
/// packet carries status=UPDATE(1) with the flag cleared; name stays hidden ~2 more seconds).
/// The surfaced worm must be targetable again — this is the "stale targeting info" regression.
#[test]
fn worm_dive_surface_lifecycle_stays_targetable_after_emerging() {
    /// The untargetable bit of the m_flags word.
    const FLAG_UNTARGETABLE: u32 = 2048;

    let mut s = crate::state::SessionState::default();

    feed_worm(&mut s, &worm_body(0x0F, 0, 0, 0));
    assert_eq!(worm_entity(&s).status, 0);
    assert!(
        crate::wire_translate::entity_to_wire(worm_entity(&s)).is_targetable(),
        "spawned worm must be targetable"
    );

    feed_worm(&mut s, &worm_body(0x04, 0, FLAG_UNTARGETABLE, 0x08));
    assert!(
        !crate::wire_translate::entity_to_wire(worm_entity(&s)).is_targetable(),
        "diving worm must be untargetable"
    );

    feed_worm(&mut s, &worm_body(0x01, 3, 0, 0));
    assert_eq!(
        worm_entity(&s).status,
        3,
        "POS-only tick must refresh status"
    );
    assert!(
        !crate::wire_translate::entity_to_wire(worm_entity(&s)).is_targetable(),
        "underground worm must stay untargetable"
    );

    feed_worm(&mut s, &worm_body(0x04, 1, 0, 0x08));
    assert_eq!(worm_entity(&s).status, 1);
    assert!(
        crate::wire_translate::entity_to_wire(worm_entity(&s)).is_targetable(),
        "surfaced worm must be targetable again (stale-info regression)"
    );

    feed_worm(&mut s, &worm_body(0x01, 1, 0, 0));
    assert!(
        crate::wire_translate::entity_to_wire(worm_entity(&s)).is_targetable(),
        "surfaced worm must stay targetable while roaming"
    );
}

/// End-to-end HP chain for the delta bridge: when a mob takes damage, LSB sets UPDATE_HP on
/// its next 0x00E (battle_entity.cpp `addHP` -> updatemask |= UPDATE_HP; entity_update.cpp
/// writes HPP at 0x1E under that bit and broadcasts to every char who spawned the entity).
/// The fold must mark the entity pending so the O(changed) delta carries the new hpp — this
/// is what drives the live nameplate HP bar. Regression: "HP bars never move".
#[test]
fn hp_update_packet_marks_entity_pending_with_new_hpp() {
    let mut s = crate::state::SessionState::default();

    feed_worm(&mut s, &worm_body(0x0F, 0, 0, 0));
    assert_eq!(worm_entity(&s).hp_pct, Some(100), "spawn carries hpp");
    let spawn_pos = worm_entity(&s).pos;
    let (upserts, _) = s.take_pending_entities();
    assert!(
        upserts.contains(&1000),
        "spawn must be pending for the delta bridge"
    );

    let mut body = worm_body(0x04, 0, 0, 0);
    body[PosHead::HPP_OFFSET] = 42;
    feed_worm(&mut s, &body);
    assert_eq!(
        worm_entity(&s).hp_pct,
        Some(42),
        "UPDATE_HP tick must apply the new hpp"
    );
    assert_eq!(
        worm_entity(&s).pos,
        spawn_pos,
        "HP-only tick must not move the entity"
    );
    let (upserts, _) = s.take_pending_entities();
    assert!(
        upserts.contains(&1000),
        "the HP change must reach the delta bridge as a pending upsert"
    );

    feed_worm(&mut s, &worm_body(0x01, 0, 0, 0));
    assert_eq!(
        worm_entity(&s).hp_pct,
        Some(42),
        "POS-only tick must not clobber hpp"
    );
}

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

#[test]
fn walkaway_release_only_user_driven_past_threshold() {
    assert!(!should_release_on_walkaway(
        false,
        Some(EVENT_WALKAWAY_YALMS + 1.0)
    ));
    assert!(!should_release_on_walkaway(true, None));
    assert!(!should_release_on_walkaway(
        true,
        Some(EVENT_WALKAWAY_YALMS - 0.1)
    ));
    assert!(should_release_on_walkaway(
        true,
        Some(EVENT_WALKAWAY_YALMS + 0.1)
    ));
}

const PINNED_EVENT: (u32, u16, u16) = (0x0100_02FF, 7, 0x0123);
const ID_FLIP: u16 = 0xFF;
const FLUSH_ZONE: u16 = 230;
const FLUSH_SEQ: u16 = 0x0044;

fn flush_inputs(user_driven: bool, watchdog_fires: bool, walked_away: bool) -> EventEndFlushInputs {
    EventEndFlushInputs {
        user_driven,
        watchdog_fires,
        walked_away,
        tag_in_flight: false,
    }
}

/// The auto-release that would otherwise fire every tick in !user_driven
/// must not drain the event whose pending tag is mid-transaction.
#[test]
fn a_tag_in_flight_holds_the_flush_even_in_agent_mode() {
    let flushes = |tag_in_flight: bool| {
        let mut pending = vec![PINNED_EVENT];
        flush_pending_event_end(
            EventEndFlushInputs {
                user_driven: false,
                watchdog_fires: true,
                walked_away: true,
                tag_in_flight,
            },
            &mut pending,
            Some(PINNED_EVENT),
            FLUSH_ZONE,
            FLUSH_SEQ,
        )
        .is_some()
    };

    assert!(
        flushes(false),
        "no tag in flight: the release policy is unchanged"
    );
    assert!(
        !flushes(true),
        "a tag in flight holds the event open for OnEventUpdate"
    );
}

#[test]
fn grace_watchdog_spares_a_dialog_the_player_is_reading() {
    const USER: bool = true;
    const WATCHDOG: bool = true;
    const WALKED: bool = true;
    let open = Some(PINNED_EVENT);

    let flushes = |inputs: EventEndFlushInputs, active: Option<(u32, u16, u16)>| {
        let mut pending = vec![PINNED_EVENT];
        flush_pending_event_end(inputs, &mut pending, active, FLUSH_ZONE, FLUSH_SEQ).is_some()
    };

    assert!(!flushes(flush_inputs(USER, WATCHDOG, !WALKED), open));
    assert!(!flushes(flush_inputs(USER, !WATCHDOG, !WALKED), open));
    assert!(!flushes(flush_inputs(USER, !WATCHDOG, !WALKED), None));

    assert!(flushes(flush_inputs(USER, WATCHDOG, !WALKED), None));
    assert!(flushes(flush_inputs(USER, WATCHDOG, WALKED), open));
    assert!(flushes(flush_inputs(USER, !WATCHDOG, WALKED), open));

    for active in [None, open] {
        for watchdog in [false, true] {
            assert_eq!(
                flushes(flush_inputs(!USER, watchdog, !WALKED), active),
                active.is_none()
            );
        }
    }

    let mut empty = Vec::new();
    assert!(flush_pending_event_end(
        flush_inputs(!USER, WATCHDOG, !WALKED),
        &mut empty,
        None,
        FLUSH_ZONE,
        FLUSH_SEQ
    )
    .is_none());
}

#[test]
fn agent_mode_keeps_the_server_event_until_the_vm_finishes() {
    let mut pending = vec![PINNED_EVENT];
    assert!(flush_pending_event_end(
        flush_inputs(false, false, false),
        &mut pending,
        Some(PINNED_EVENT),
        FLUSH_ZONE,
        FLUSH_SEQ,
    )
    .is_none());
    assert_eq!(pending, vec![PINNED_EVENT]);
    assert!(take_pending_event_end(
        &mut pending,
        PINNED_EVENT.0,
        PINNED_EVENT.2
    ));
}

#[test]
fn walking_away_clears_the_vm_dialog_it_released() {
    let mut pending = vec![PINNED_EVENT];
    let flush = flush_pending_event_end(
        flush_inputs(true, false, true),
        &mut pending,
        Some(PINNED_EVENT),
        FLUSH_ZONE,
        FLUSH_SEQ,
    )
    .expect("walking away releases the pinned event");
    assert!(flush.clear_dialog);

    let other = (
        PINNED_EVENT.0 ^ u32::from(ID_FLIP),
        PINNED_EVENT.1,
        PINNED_EVENT.2,
    );
    let mut pending = vec![PINNED_EVENT];
    let flush = flush_pending_event_end(
        flush_inputs(true, false, true),
        &mut pending,
        Some(other),
        FLUSH_ZONE,
        FLUSH_SEQ,
    )
    .expect("walking away releases the pinned event");
    assert!(
        !flush.clear_dialog,
        "a dialog the flush did not release must keep running"
    );
}

#[test]
fn a_pinned_event_is_owed_exactly_one_event_end() {
    let (unique_no, act_index, event_id) = PINNED_EVENT;
    let mut pending = vec![PINNED_EVENT];
    assert!(take_pending_event_end(&mut pending, unique_no, event_id));
    assert!(!take_pending_event_end(&mut pending, unique_no, event_id));

    let mut pending = vec![PINNED_EVENT];
    assert!(!take_pending_event_end(
        &mut pending,
        unique_no,
        event_id ^ ID_FLIP
    ));
    assert_eq!(pending, vec![(unique_no, act_index, event_id)]);
}

#[test]
fn should_emit_pos_rate_limits_to_10hz() {
    assert!(!should_emit_pos(
        std::time::Duration::from_millis(50),
        0.1,
        false,
    ));

    assert!(should_emit_pos(
        std::time::Duration::from_millis(100),
        0.0,
        false,
    ));
    assert!(should_emit_pos(
        std::time::Duration::from_millis(120),
        0.0,
        false,
    ));
}

#[test]
fn should_emit_pos_bypasses_rate_limit_on_big_jump() {
    assert!(should_emit_pos(
        std::time::Duration::from_millis(10),
        0.6,
        false,
    ));

    assert!(!should_emit_pos(
        std::time::Duration::from_millis(10),
        0.5,
        false,
    ));
}

#[test]
fn should_emit_pos_bypasses_rate_limit_on_heading_change() {
    assert!(should_emit_pos(
        std::time::Duration::from_millis(10),
        0.0,
        true,
    ));
}

/// Pre-GAMEOK drain (break_on_idle=false): keep reading until the self
/// position seed (CHAR_PC) lands. The s2c 0x00A LOGIN
/// (vendor/server/src/map/packets/s2c/0x00a_login.cpp) is tracked but is not
/// a break condition — it rides a second datagram behind the zone-in burst
/// and is processed opportunistically by the keepalive loop.
#[test]
fn flood_drain_waits_for_self_pos_seed() {
    assert!(
        !should_break_flood(false, false) && should_break_flood(false, true),
        "unseeded pre-GAMEOK drain must wait; a seeded one may break"
    );
    assert!(
        should_break_flood(true, false) && should_break_flood(true, true),
        "quiescence drain breaks on idle unconditionally"
    );
}

#[test]
fn cadence_drops_30hz_integrator_to_10hz_emission() {
    let mut last_emit: Option<std::time::Duration> = None;
    let mut now = std::time::Duration::ZERO;
    let mut emits = 0;
    for _ in 0..30 {
        now += std::time::Duration::from_millis(33);
        let elapsed = match last_emit {
            None => std::time::Duration::from_secs(10),
            Some(t) => now - t,
        };
        if should_emit_pos(elapsed, 0.165, false) {
            emits += 1;
            last_emit = Some(now);
        }
    }

    assert!(
        (7..=11).contains(&emits),
        "expected ~10 emissions/s (10 Hz cadence vs 30 Hz integrator), got {emits}",
    );
}

#[test]
fn reconcile_self_pos_keep_local_under_2_yalms() {
    let local = v(0.0, 0.0, 0.0);
    let server = v(1.0, 1.0, 0.5);
    assert_eq!(
        reconcile_self_pos(local, server, false),
        SelfPosReconcile::KeepLocal,
    );
}

#[test]
fn reconcile_self_pos_rubberband_between_2_and_10() {
    let local = v(0.0, 0.0, 0.0);
    let server = v(3.0, 4.0, 0.0);
    match reconcile_self_pos(local, server, false) {
        SelfPosReconcile::Rubberband { target } => {
            assert_eq!(target, server);
        }
        other => panic!("expected Rubberband, got {other:?}"),
    }
}

#[test]
fn reconcile_self_pos_snap_above_10_yalms() {
    let local = v(0.0, 0.0, 0.0);
    let server = v(12.0, 5.0, 0.0);
    assert_eq!(
        reconcile_self_pos(local, server, false),
        SelfPosReconcile::Snap,
    );
}

#[test]
fn reconcile_self_pos_boundaries() {
    let local = v(0.0, 0.0, 0.0);
    let just_inside = v(2.0, 0.0, 0.0);
    assert_eq!(
        reconcile_self_pos(local, just_inside, false),
        SelfPosReconcile::KeepLocal,
    );

    let edge = v(10.0, 0.0, 0.0);
    assert!(matches!(
        reconcile_self_pos(local, edge, false),
        SelfPosReconcile::Rubberband { .. },
    ));
}

#[test]
fn lerp_toward_advances_at_capped_step() {
    let (next, reached) = lerp_toward(v(0.0, 0.0, 0.0), v(10.0, 0.0, 0.0), 5.0);
    assert!(!reached);
    assert!((next.x - 5.0).abs() < 1e-3);
}

#[test]
fn lerp_toward_clamps_to_target_on_overshoot() {
    let (next, reached) = lerp_toward(v(0.0, 0.0, 0.0), v(2.0, 0.0, 0.0), 5.0);
    assert!(reached);
    assert_eq!(next, v(2.0, 0.0, 0.0));
}

#[test]
fn event_end_writes_csid_to_event_para_field() {
    let buf = build_subpacket_event_end(
        0x1234,
        0xDEADBEEF,
        0x4242,
        230,
        535,
        0,
        ffxi_proto::map::c2s::event_end_mode::END,
    );
    assert_eq!(buf.len(), 20, "header(4) + body(16)");

    assert_eq!(&buf[4..8], &0xDEADBEEFu32.to_le_bytes(), "UniqueNo");
    assert_eq!(&buf[8..12], &0u32.to_le_bytes(), "EndPara (choice=0)");
    assert_eq!(&buf[12..14], &0x4242u16.to_le_bytes(), "ActIndex");
    assert_eq!(&buf[14..16], &0u16.to_le_bytes(), "Mode (End=0)");

    assert_eq!(
        &buf[18..20],
        &535u16.to_le_bytes(),
        "EventPara MUST carry the CSID — LSB validator reads from here",
    );

    assert_eq!(
        &buf[16..18],
        &230u16.to_le_bytes(),
        "EventNum carries the zone id (retail echoes LOGIN EventNum, \
             0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN); LSB's 0x05B handler never reads it",
    );
}

/// The UpdatePending mode byte of a pending-tag round-trip: same 0x05B shape
/// as End, Mode=1 instead of 0 (GP_CLI_COMMAND_EVENTEND_MODE,
/// vendor/server/src/map/packets/c2s/0x05b_eventend.h).
#[test]
fn event_end_writes_update_pending_mode_byte() {
    let buf = build_subpacket_event_end(
        0x1234,
        0xDEADBEEF,
        0x4242,
        230,
        535,
        7,
        ffxi_proto::map::c2s::event_end_mode::UPDATE_PENDING,
    );
    assert_eq!(
        &buf[8..12],
        &7u32.to_le_bytes(),
        "EndPara (Work_Zone[1] at send time)"
    );
    assert_eq!(
        &buf[14..16],
        &ffxi_proto::map::c2s::event_end_mode::UPDATE_PENDING.to_le_bytes(),
        "Mode = UpdatePending"
    );
}

/// Pins the c2s 0x05C GP_CLI_COMMAND_EVENTENDXZY layout against
/// vendor/server/src/map/packets/c2s/0x05c_eventendxzy.h: x/y/z f32, UniqueNo
/// u32, EndPara u32, EventNum u16, EventPara u16, ActIndex u16, Mode u8,
/// dir i8. Note EventNum/EventPara precede ActIndex here, unlike 0x05B.
#[test]
fn event_end_xzy_writes_lsb_layout() {
    let buf = build_subpacket_event_end_xzy(
        0x1234, 0xDEADBEEF, 1.5, -2.25, 3.75, 63, 9, 0x4242, 230, 535,
    );
    assert_eq!(buf.len(), 32, "header(4) + body(28)");

    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        id_and_size & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::EVENT_END_XZY
    );
    assert_eq!(id_and_size >> 9, 8, "size_words = 32/4");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0x1234, "sync");

    assert_eq!(&buf[4..8], &1.5f32.to_le_bytes(), "x");
    assert_eq!(&buf[8..12], &(-2.25f32).to_le_bytes(), "y");
    assert_eq!(&buf[12..16], &3.75f32.to_le_bytes(), "z");
    assert_eq!(&buf[16..20], &0xDEADBEEFu32.to_le_bytes(), "UniqueNo");
    assert_eq!(&buf[20..24], &9u32.to_le_bytes(), "EndPara (Work_Zone[1])");
    assert_eq!(&buf[24..26], &230u16.to_le_bytes(), "EventNum (zone)");
    assert_eq!(&buf[26..28], &535u16.to_le_bytes(), "EventPara (event id)");
    assert_eq!(&buf[28..30], &0x4242u16.to_le_bytes(), "ActIndex");
    assert_eq!(
        buf[30], 1,
        "Mode = UpdatePending (the only mode the validator accepts)"
    );
    assert_eq!(buf[31], 63, "dir");
}

/// Pins the c2s 0x064 GP_CLI_COMMAND_SCENARIOITEM layout against
/// vendor/server/src/map/packets/c2s/0x064_scenarioitem.h: UniqueNo u32 @4,
/// LookItemFlag u32[16] @8, ActIndex u16 @72, TableIndex u16 @74.
#[test]
fn scenario_item_packet_layout_matches_lsb_struct() {
    let mut look = [0u32; decode::ScenarioItem::WORDS];
    look[0] = 0b101;
    look[15] = 0x8000_0001;
    let buf = build_subpacket_scenario_item(0x1234, 0xDEAD_BEEF, 0x4242, 6, &look);
    assert_eq!(buf.len(), 76, "header(4) + body(72)");

    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        id_and_size & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::SCENARIO_ITEM
    );
    assert_eq!(id_and_size >> 9, 19, "size_words = 76/4");
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0x1234, "sync");

    assert_eq!(&buf[4..8], &0xDEAD_BEEFu32.to_le_bytes(), "UniqueNo");
    assert_eq!(&buf[8..12], &0b101u32.to_le_bytes(), "LookItemFlag[0]");
    assert_eq!(
        &buf[68..72],
        &0x8000_0001u32.to_le_bytes(),
        "LookItemFlag[15]"
    );
    assert_eq!(&buf[72..74], &0x4242u16.to_le_bytes(), "ActIndex");
    assert_eq!(&buf[74..76], &6u16.to_le_bytes(), "TableIndex");
}

/// vendor/server/src/map/packets/c2s/0x064_scenarioitem.cpp GP_CLI_COMMAND_SCENARIOITEM::process keyItemId —
/// keyItemId = TableIndex*512 + i*32 + bit; the fold must be its inverse.
#[test]
fn mark_seen_bits_round_trip_through_ids_from_flags() {
    let mut flags = [0u32; decode::ScenarioItem::WORDS];
    assert!(fold_seen_ids_into_look_flags(
        1,
        &[513, 512 + 95, 3],
        &mut flags
    ));
    assert_eq!(flags[0], 1 << 1, "id 513 = table 1 bit 1");
    assert_eq!(flags[2], 1 << 31, "id 607 = word 2 bit 31");
    assert_eq!(
        decode::ScenarioItem::ids_from_flags(1, &flags),
        vec![513, 607],
        "round-trips through the tested decode oracle; id 3 (table 0) ignored"
    );
    assert!(
        !fold_seen_ids_into_look_flags(1, &[513], &mut flags),
        "already-set bits report no change"
    );
}

/// vendor/server/src/map/packets/c2s/0x064_scenarioitem.cpp PacketValidator —
/// UniqueNo must equal char id and ActIndex must equal targid, ActIndex 0
/// is always rejected, and the send is blocked while InEvent; every
/// blocked case must skip without mutating local seen-state.
#[test]
fn mark_seen_requires_self_targid() {
    assert!(mark_seen_send_block_reason(false, None, true).is_err());
    assert!(mark_seen_send_block_reason(true, Some(0x123), true).is_err());
    assert!(
        mark_seen_send_block_reason(false, Some(0x123), false).is_err(),
        "table without a received 0x055 must not synthesize an empty update"
    );
    assert_eq!(
        mark_seen_send_block_reason(false, Some(0x123), true),
        Ok(0x123)
    );
}

/// A 0x02A-shaped [`ZoneMessage`], the way `emit_zone_message_chat` builds
/// one: the speaker is dropped when the hide-name flag is set.
fn tnw(mes_num: u16, num: [i32; 4], name: &str) -> ZoneMessage {
    let hidden = mes_num & decode::MESNUM_HIDE_NAME_FLAG != 0;
    ZoneMessage {
        message_index: mes_num & !decode::MESNUM_HIDE_NAME_FLAG,
        speaker: (!hidden && !name.is_empty()).then(|| name.to_string()),
        actor: (!name.is_empty()).then(|| name.to_string()),
        nums: num.to_vec(),
    }
}

#[test]
fn talknumwork_resolves_key_item_marker_from_zone_text() {
    // Key item 1 = Zeruhn Report (vendor/server/scripts/enum/key_item.lua).
    let line = zone_message_chat_line(
        &tnw(6438 | decode::MESNUM_HIDE_NAME_FLAG, [1, 0, 0, 0], ""),
        Some("Obtained key item: {KeyItem:0}.".to_string()),
        "Zeid",
    );
    assert_eq!(line.text, "Obtained key item: Zeruhn Report.");
    assert_eq!(line.channel, ChatChannel::System);
    assert_eq!(line.sender, "");
}

/// Zone-230 KEYITEM_OBTAINED is located by content: LSB text ids are identity
/// DAT entry indexes, but the physical index moves with the client build
/// (horizonxi-2023 6437, retail-2026-09 6442 — pinned per KNOWN_CLIENTS row in
/// ffxi-dat dmsg::tests::real_zone230_keyitem_obtained_decodes_marker).
const ZONE230_KEYITEM_OBTAINED_PREFIX: &str = "Obtained key item:";
const ZONE230: u16 = 230;

fn test_dat_root() -> Option<ffxi_dat::DatRoot> {
    ffxi_dat::archive::open_test_install()
}

/// Full 0x02A chat composition against the retail DAT: the zone string's
/// inline key-item tag must decode to `{KeyItem:0}` and resolve through
/// `num[0]` (pre-fix this line rendered "Obtained key item: {Auto:128}3\u{FFFD}.").
/// Key item 1 = Zeruhn Report (vendor/server/scripts/enum/key_item.lua).
/// Self-skips without game files.
#[test]
fn talknumwork_composes_real_keyitem_line_from_zone_dat() {
    let Some(root) = test_dat_root() else {
        eprintln!("skipping: no FFXI install");
        return;
    };
    let file_id = ffxi_dat::zone_dat::string_dat_file_id(ZONE230);
    let loc = root.resolve(file_id).expect("string DAT resolves");
    let bytes = std::fs::read(loc.path_under(&root)).expect("string DAT readable");
    let dat = ffxi_dat::dmsg::StringDat::parse(&bytes).expect("zone 230 dialog table parses");
    let index = dat
        .first_entry_starting_with(ZONE230_KEYITEM_OBTAINED_PREFIX)
        .unwrap_or_else(|| {
            panic!(
                "no {ZONE230_KEYITEM_OBTAINED_PREFIX:?} entry in a {}-entry table",
                dat.len()
            )
        });
    let index = u16::try_from(index).expect("dialog index fits a MesNum");
    let mut ds =
        crate::event_dialog::DialogSession::new(Some(std::sync::Arc::new(root)), "Tester".into());
    let zone_text = ds
        .zone_text(ZONE230, usize::from(index))
        .expect("zone 230 string DAT must load");
    assert!(zone_text.starts_with(ZONE230_KEYITEM_OBTAINED_PREFIX));
    let line = zone_message_chat_line(
        &tnw(index | decode::MESNUM_HIDE_NAME_FLAG, [1, 0, 0, 0], ""),
        Some(zone_text),
        "Tester",
    );
    assert_eq!(line.text, "Obtained key item: Zeruhn Report.");
    assert_eq!(line.channel, ChatChannel::System);
}

#[test]
fn talknumwork_shows_speaker_name_when_not_hidden() {
    let line = zone_message_chat_line(
        &tnw(100, [7, 0, 0, 0], "Trion"),
        Some("{SpeakerName} counts {Num:0}.".to_string()),
        "Zeid",
    );
    assert_eq!(line.sender, "Trion");
    assert_eq!(line.channel, ChatChannel::Say);
    assert_eq!(line.text, "Trion counts 7.");
}

#[test]
fn talknumwork_degrades_to_placeholder_without_zone_strings() {
    let line = zone_message_chat_line(
        &tnw(6438 | decode::MESNUM_HIDE_NAME_FLAG, [512, 0, 0, 0], ""),
        None,
        "Zeid",
    );
    assert!(
        line.text.contains("6438") && line.text.contains("512"),
        "placeholder must expose the masked index and params: {}",
        line.text
    );
}

fn armed_cast(total_ms: u32) -> Option<CastInFlight> {
    Some(CastInFlight {
        lock_until: std::time::Instant::now() + std::time::Duration::from_millis(5_000),
        bar: Some(CastBar {
            name: "Poison".into(),
            total_ms,
            started_at: None,
        }),
    })
}

fn battle2_self(action_kind: u8, cmd_arg: u32) -> Battle2Header {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(0, 6);
    w.write(0, 4);
    w.write(u64::from(action_kind), 4);
    w.write(u64::from(cmd_arg), 32);
    w.write(0, 32);
    w.write(0, 32);
    decode_battle2_header(&w.into_bytes()).expect("header decodes")
}

/// The bar is armed at send but unstarted; it only starts when the server's
/// own MagicStart arrives (vendor/server/src/map/ai/states/magic_state.cpp CMagicState::CMagicState),
/// so it cannot lead the cast pose and the "starts casting" line by a round trip.
/// The FourCCs are the literal LSB constants (vendor/server/src/map/enums/four_cc.h FourCC BlueMagicCast).
#[test]
fn self_cast_bar_starts_on_magic_start_not_on_send() {
    const CABK: u32 = 0x6B626163;
    const SPBK: u32 = 0x6B627073;
    const POISON_CAST_MS: u32 = 1000;

    let (tx, mut rx) = broadcast::channel(8);
    let mut cast = armed_cast(POISON_CAST_MS);
    assert!(
        rx.try_recv().is_err(),
        "arming the gate must not announce a cast"
    );

    apply_self_battle2_to_cast(
        &battle2_self(ffxi_vocab::magic::CATEGORY_MAGIC_START, CABK),
        &mut cast,
        &tx,
    );
    match rx.try_recv().expect("MagicStart starts the bar") {
        AgentEvent::SelfCastStarted { name, total_ms } => {
            assert_eq!(name, "Poison");
            assert_eq!(total_ms, POISON_CAST_MS);
        }
        other => panic!("expected SelfCastStarted, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "exactly one start event");

    apply_self_battle2_to_cast(
        &battle2_self(ffxi_vocab::magic::CATEGORY_MAGIC_START, CABK),
        &mut cast,
        &tx,
    );
    assert!(
        rx.try_recv().is_err(),
        "a repeat MagicStart re-starts nothing"
    );

    apply_self_battle2_to_cast(
        &battle2_self(ffxi_vocab::magic::CATEGORY_MAGIC_START, SPBK),
        &mut cast,
        &tx,
    );
    assert!(matches!(
        rx.try_recv().expect("interrupt ends the bar"),
        AgentEvent::SelfCastEnded { interrupted: true }
    ));
    assert!(cast.is_none(), "an interrupt clears the in-flight action");
}

/// MagicFinish ends the bar instead of leaving it to the optimistic lock timer.
#[test]
fn self_cast_bar_ends_on_magic_finish() {
    const CAWH: u32 = 0x68776163;

    let (tx, mut rx) = broadcast::channel(8);
    let mut cast = armed_cast(1000);
    apply_self_battle2_to_cast(
        &battle2_self(ffxi_vocab::magic::CATEGORY_MAGIC_START, CAWH),
        &mut cast,
        &tx,
    );
    let _started = rx.try_recv().expect("start");
    apply_self_battle2_to_cast(
        &battle2_self(ffxi_vocab::magic::CATEGORY_MAGIC_FINISH, 0),
        &mut cast,
        &tx,
    );
    assert!(matches!(
        rx.try_recv().expect("finish ends the bar"),
        AgentEvent::SelfCastEnded { interrupted: false }
    ));
    assert!(cast.is_none());
}

/// A zone message buffered during the zone-in flood must replay as a chat
/// line once the keepalive loop's DialogSession exists — degrading to the
/// placeholder without a DAT, never dropping silently.
#[test]
fn buffered_flood_talknumwork_replays_as_chat_line() {
    let mut ds = crate::event_dialog::DialogSession::new(None, "Tester".into());
    let (tx, mut rx) = broadcast::channel(4);
    let mut body = vec![0u8; decode::TalkNumWork::SIZE];
    body[22..24].copy_from_slice(&42u16.to_le_bytes());
    emit_zone_message_chat(
        ffxi_proto::map::s2c::TALKNUMWORK,
        &body,
        &mut ds,
        230,
        "Tester",
        &tx,
    );
    let AgentEvent::ChatLine { line } = rx.try_recv().expect("replay must emit an event") else {
        panic!("expected ChatLine");
    };
    assert!(
        line.text.contains("zone message 42"),
        "no-DAT replay degrades to the placeholder: {}",
        line.text
    );
}

/// The three newly-handled family members must each reach chat. Every LSB
/// fishing line arrives on one of them
/// (vendor/server/src/map/utils/fishingutils.cpp), so a silently-unhandled
/// opcode here is a whole feature going quiet.
#[test]
fn every_zone_message_opcode_emits_a_chat_line() {
    use ffxi_proto::map::s2c;
    const MES_NUM: u16 = 7265;

    for (opcode, size, mes_num_off) in [
        (s2c::TALKNUM, decode::TalkNum::SIZE, 6),
        (s2c::TALKNUMWORK2, decode::TalkNumWork2::SIZE, 6),
        (s2c::TALKNUMNAME, decode::TalkNumName::SIZE, 6),
        (s2c::TALKNUMWORK, decode::TalkNumWork::SIZE, 22),
    ] {
        assert!(
            ZONE_MESSAGE_OPCODES.contains(&opcode),
            "{opcode:#05X} is not routed to the zone-message handler"
        );
        let mut ds = crate::event_dialog::DialogSession::new(None, "Tester".into());
        let (tx, mut rx) = broadcast::channel(4);
        let mut body = vec![0u8; size];
        body[mes_num_off..mes_num_off + 2].copy_from_slice(&MES_NUM.to_le_bytes());
        emit_zone_message_chat(opcode, &body, &mut ds, 230, "Tester", &tx);

        let AgentEvent::ChatLine { line } = rx
            .try_recv()
            .unwrap_or_else(|e| panic!("{opcode:#05X} emitted nothing: {e}"))
        else {
            panic!("expected ChatLine");
        };
        assert!(
            line.text.contains(&MES_NUM.to_string()),
            "{opcode:#05X} placeholder must expose the index: {}",
            line.text
        );
    }
}

/// LSB's fishing catch constructor sets the hide-name flag and puts the
/// fish id in Num1[0], so the line renders unattributed with the item name
/// substituted. vendor/server/src/map/packets/s2c/0x027_talknumwork2.cpp
#[test]
fn talknumwork2_substitutes_the_caught_item() {
    let msg = ZoneMessage {
        message_index: 7267,
        speaker: None,
        actor: Some("Kuluu".to_string()),
        nums: vec![4304, 1, 0, 0],
    };
    let line = zone_message_chat_line(
        &msg,
        Some("{PlayerName} caught {Item:0}!".to_string()),
        "Kuluu",
    );
    assert_eq!(line.channel, ChatChannel::System);
    assert!(
        line.text.starts_with("Kuluu caught ") && !line.text.contains("{Item:0}"),
        "item marker must resolve: {}",
        line.text
    );

    // Retail colours the item name apart from the rest of the line, so the
    // substitution must survive as its own span rather than being flattened
    // into the surrounding text.
    let item: Vec<&crate::state::ChatSpan> = line
        .spans
        .iter()
        .filter(|s| s.kind == crate::state::ChatSpanKind::Item)
        .collect();
    assert_eq!(item.len(), 1, "spans: {:?}", line.spans);
    assert!(!item[0].text.contains("caught"), "{:?}", item[0]);
    assert!(
        line.spans.iter().any(|s| s.text.contains("Kuluu caught")),
        "surrounding text stays plain: {:?}",
        line.spans
    );
}

#[test]
#[ignore = "requires the installed retail dialog DATs"]
fn ferry_fishing_chat_replays_reported_catches_with_installed_dat() {
    const FERRY_ZONE: u16 = 220;
    const REPORTED_CATCH: u16 = 7288;
    // The capture's server sat at an era no LSB pin matches: zone 220's
    // fishing base is 7241 on horizonxi-2023, 7254 on retail-2026-09 and
    // 7250 in the pinned LSB, so the base has to be learned from the wire.
    const SERVER_BASE: u16 = 7249;
    const NUM1_START: usize = 12;
    const STRING1_START: usize = 28;
    const MESNUM_START: usize = 6;
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    let root = std::sync::Arc::new(root);
    let mut dialog = crate::event_dialog::DialogSession::new(Some(root), "Observer".into());
    let (tx, mut rx) = broadcast::channel(16);
    for (item, name) in [(4451i32, "Silver Shark"), (5128, "Cone Calamary")] {
        let mut body = vec![0; decode::TalkNumWork2::SIZE];
        body[MESNUM_START..MESNUM_START + 2].copy_from_slice(
            &(REPORTED_CATCH | decode::TalkNumWork::MESNUM_HIDE_NAME_FLAG).to_le_bytes(),
        );
        body[NUM1_START..NUM1_START + 4].copy_from_slice(&item.to_le_bytes());
        body[NUM1_START + 4..NUM1_START + 8].copy_from_slice(&1i32.to_le_bytes());
        body[STRING1_START..STRING1_START + 6].copy_from_slice(b"Angler");
        emit_zone_message_chat(
            ffxi_proto::map::s2c::TALKNUMWORK2,
            &body,
            &mut dialog,
            FERRY_ZONE,
            "Observer",
            &tx,
        );
        let AgentEvent::ChatLine { line } = rx.try_recv().unwrap() else {
            panic!("catch must emit chat")
        };
        assert!(line.text.starts_with("Angler caught "), "{}", line.text);
        assert!(line.text.contains(name), "{}", line.text);
        assert!(!line.text.contains('{'), "{}", line.text);
        assert!(line
            .spans
            .iter()
            .any(|s| s.kind == crate::state::ChatSpanKind::Item));
        println!("{}", line.text);
    }
    let crate::event_dialog::FishingChat::Line { text, .. } = dialog.fishing_chat(
        FERRY_ZONE,
        SERVER_BASE + u16::from(ffxi_proto::fishing_messages::kind::KEEN_ANGLERS_SENSE),
        ffxi_proto::map::s2c::TALKNUMWORK,
    ) else {
        panic!("keen angler message must use the learned server base")
    };
    let line = zone_message_chat_line(
        &ZoneMessage {
            message_index: SERVER_BASE,
            speaker: None,
            actor: None,
            nums: vec![4451, 3, 3, 3],
        },
        Some(text),
        "Observer",
    );
    assert!(line.text.contains("Silver Shark"), "{}", line.text);
    assert!(!line.text.contains('{'), "{}", line.text);
    println!("{}", line.text);
}

/// The Esc cancel EndPara crosses the wire exactly as LSB's
/// utils.EVENT_CANCELLED_OPTION (vendor/server/scripts/utils/utils.lua).
#[test]
fn event_end_cancel_writes_lsb_cancel_option() {
    let buf = build_subpacket_event_end(
        0x1234,
        0xDEADBEEF,
        0x4242,
        230,
        535,
        ffxi_event::EVENT_CANCELLED_END_PARA,
        ffxi_proto::map::c2s::event_end_mode::END,
    );
    assert_eq!(&buf[8..12], &0x4000_0000u32.to_le_bytes(), "EndPara");
}

#[test]
fn eventucoff_mode_strips_packed_event_id() {
    use ffxi_proto::map::eventucoff_mode;
    let packed = eventucoff_mode::CANCEL_EVENT | (535u32 << 8);
    assert_eq!(
        eventucoff_mode_of(&packed.to_le_bytes()),
        Some(eventucoff_mode::CANCEL_EVENT)
    );
    assert_eq!(eventucoff_mode_of(&[2, 0]), None, "truncated body");
}

#[test]
fn eventucoff_cancel_event_clears_pending_and_emits_event_ended() {
    let (tx, mut rx) = broadcast::channel(8);
    let mut pending = vec![(0xDEADBEEFu32, 7u16, 535u16)];
    let packed = ffxi_proto::map::eventucoff_mode::CANCEL_EVENT | (535u32 << 8);
    // Locked by 0x46 case 1 and never unlocked, the retail-common shape.
    let mut cutscene = crate::event_dialog::CutsceneScope::default();
    cutscene.start(535, &tx);
    cutscene.push(
        crate::event_dialog::ResolvedCue::Scene(crate::state::CutsceneCue::CameraLock {
            lock: true,
        }),
        &tx,
    );
    while rx.try_recv().is_ok() {}

    handle_eventucoff(&packed.to_le_bytes(), &mut pending, &mut cutscene, &tx);
    assert!(
        pending.is_empty(),
        "server force-close drops tracked events"
    );
    assert!(
        !cutscene.camera_locked(),
        "the server's cancel must give the camera back"
    );
    assert!(matches!(
        rx.try_recv(),
        Ok(AgentEvent::CutsceneCue {
            cue: crate::state::CutsceneCue::CameraLock { lock: false }
        })
    ));
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::CutsceneEnded)));
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::EventEnded)));
    assert!(rx.try_recv().is_err(), "exactly one release emitted");
}

#[test]
fn eventucoff_fishing_emits_fishing_ended_and_keeps_pending() {
    let (tx, mut rx) = broadcast::channel(8);
    let mut pending = vec![(1u32, 2u16, 3u16)];
    handle_eventucoff(
        &ffxi_proto::map::eventucoff_mode::FISHING.to_le_bytes(),
        &mut pending,
        &mut crate::event_dialog::CutsceneScope::default(),
        &tx,
    );
    assert_eq!(pending.len(), 1);
    assert!(matches!(rx.try_recv(), Ok(AgentEvent::FishingEnded)));
}

/// EventRecvPending follows every processed 0x05B
/// (vendor/server/src/map/packets/c2s/0x05b_eventend.cpp GP_CLI_COMMAND_EVENTEND::process) and can land
/// after a chained event's 0x032 trigger — it must not clear anything.
#[test]
fn eventucoff_recv_pending_ack_is_inert() {
    const EVENT_RECV_PENDING: u32 = 1;
    let (tx, mut rx) = broadcast::channel(8);
    let mut pending = vec![(1u32, 2u16, 3u16)];
    handle_eventucoff(
        &EVENT_RECV_PENDING.to_le_bytes(),
        &mut pending,
        &mut crate::event_dialog::CutsceneScope::default(),
        &tx,
    );
    assert_eq!(pending.len(), 1);
    assert!(rx.try_recv().is_err());
}

#[test]
fn tell_packet_layout_matches_phoenix_struct() {
    let buf = build_subpacket_tell(0xABCD, "Vanari", "hi");
    assert_eq!(buf.len(), 24, "total = 4 hdr + 20 body, padded to mul-of-4");

    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        id_and_size & framing::SUBPACKET_OPCODE_MASK,
        codec::CHAT_NAME,
        "opcode"
    );
    assert_eq!(id_and_size >> 9, 6, "size_words");
    let sync = u16::from_le_bytes([buf[2], buf[3]]);
    assert_eq!(sync, 0xABCD, "sync passed through");

    // The server's PacketValidator requires unknown00 == 3 (0x0b6_chat_name.cpp);
    // a 0 here is what silently dropped every tell / customMenu reply.
    assert_eq!(buf[4], 3, "unknown00 == 3 (server-required)");
    assert_eq!(buf[5], 0, "unknown01");
    assert_eq!(&buf[6..12], b"Vanari", "recipient name");
    assert!(buf[12..21].iter().all(|&b| b == 0), "sName NUL-padded");
    assert_eq!(&buf[21..23], b"hi", "message body");
    assert_eq!(buf[23], 0, "trailing NUL");
}

#[test]
fn tell_packet_truncates_oversize_inputs() {
    let long_name = "a".repeat(50);
    let buf = build_subpacket_tell(0, &long_name, "x");

    // sName is char[15] read via asStringFromUntrustedSource(sName,
    // sizeof(sName)) (0x0b6_chat_name.cpp GP_CLI_COMMAND_CHAT_NAME::process recipientName), so a full unterminated
    // 15-byte field is legal on the wire.
    assert_eq!(&buf[6..21], &[b'a'; 15][..], "first 15 chars of name");
    assert_eq!(&buf[21..22], b"x", "message follows the full sName field");
}

#[test]
fn tell_packet_carries_full_fifteen_char_name() {
    let name = "Abcdefghijklmno";
    assert_eq!(name.len(), crate::session::codec::CHAT_NAME_SNAME_LEN);
    let buf = build_subpacket_tell(0, name, "hi");

    assert_eq!(&buf[6..21], name.as_bytes(), "all 15 name bytes carried");
    assert_eq!(&buf[21..23], b"hi", "message body");
    assert_eq!(buf[23], 0, "trailing NUL");
}

#[test]
fn item_use_packet_layout_matches_phoenix_struct() {
    let buf = build_subpacket_item_use(0xBEEF, 0x12345678, 0x0042, 0x00, 7);
    assert_eq!(buf.len(), 20);

    let id_and_size = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        id_and_size & framing::SUBPACKET_OPCODE_MASK,
        codec::ITEM_USE,
        "opcode"
    );
    assert_eq!(id_and_size >> 9, 5, "size_words");
    let sync = u16::from_le_bytes([buf[2], buf[3]]);
    assert_eq!(sync, 0xBEEF, "sync passed through");

    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        0x12345678,
        "UniqueNo (recipient)"
    );
    assert_eq!(
        u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        0,
        "ItemNum must be 0 — server-validated (mustEqual 0)"
    );
    assert_eq!(
        u16::from_le_bytes(buf[12..14].try_into().unwrap()),
        0x0042,
        "ActIndex (recipient)"
    );
    assert_eq!(buf[14], 7, "PropertyItemIndex (slot)");
    assert_eq!(buf[15], 0, "padding00");
    assert_eq!(
        u32::from_le_bytes(buf[16..20].try_into().unwrap()),
        0,
        "Category = LOC_INVENTORY"
    );
}

#[test]
fn event_0x032_decodes_full_layout() {
    let mut data = vec![0u8; 16];
    data[0..4].copy_from_slice(&0x1234_5678u32.to_le_bytes());
    data[4..6].copy_from_slice(&7u16.to_le_bytes());
    data[6..8].copy_from_slice(&42u16.to_le_bytes());
    data[8..10].copy_from_slice(&3u16.to_le_bytes());
    data[10..12].copy_from_slice(&1u16.to_le_bytes());
    data[12..14].copy_from_slice(&5u16.to_le_bytes());
    data[14..16].copy_from_slice(&9u16.to_le_bytes());

    let d = decode_event_0x032(&data).expect("decoded");
    assert_eq!(d.npc_id, 0x1234_5678);
    assert_eq!(d.act_index, 7);
    assert_eq!(d.event_num, 42);
    assert_eq!(d.event_para, 3);
    assert_eq!(d.mode, 1);
    assert_eq!(d.event_num2, 5);
    assert_eq!(d.event_para2, 9);
    assert!(d.strings.is_empty());
    assert!(d.nums.is_empty());
    assert_eq!(d.event_id, ((0x1234_5678u64 << 16) | 42u64) as u32);
}

#[test]
fn event_0x033_extracts_strings_and_data() {
    let mut data = vec![0u8; 108];
    data[0..4].copy_from_slice(&100u32.to_le_bytes());
    data[4..6].copy_from_slice(&1u16.to_le_bytes());
    data[6..8].copy_from_slice(&50u16.to_le_bytes());

    data[12..16].copy_from_slice(b"Selh");

    data[28..34].copy_from_slice(b"Bastok");

    data[76..80].copy_from_slice(&100i32.to_le_bytes());
    data[80..84].copy_from_slice(&200i32.to_le_bytes());

    let d = decode_event_0x033(&data).expect("decoded");
    assert_eq!(d.strings, vec!["Selh".to_string(), "Bastok".to_string()]);
    assert_eq!(d.nums.len(), 8);
    assert_eq!(d.nums[0], 100);
    assert_eq!(d.nums[1], 200);
    assert_eq!(d.nums[2], 0);
}

#[test]
fn event_0x034_extracts_nums_and_param_block() {
    let mut data = vec![0u8; 48];
    data[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());

    data[4..8].copy_from_slice(&(-5i32).to_le_bytes());
    data[8..12].copy_from_slice(&1234i32.to_le_bytes());

    data[36..38].copy_from_slice(&3u16.to_le_bytes());
    data[38..40].copy_from_slice(&77u16.to_le_bytes());
    data[40..42].copy_from_slice(&2u16.to_le_bytes());
    data[42..44].copy_from_slice(&1u16.to_le_bytes());

    let d = decode_event_0x034(&data).expect("decoded");
    assert_eq!(d.npc_id, 0xDEAD_BEEF);
    assert_eq!(d.act_index, 3);
    assert_eq!(d.event_num, 77);
    assert_eq!(d.event_para, 2);
    assert_eq!(d.mode, 1);
    assert_eq!(d.nums.len(), 8);
    assert_eq!(d.nums[0], -5);
    assert_eq!(d.nums[1], 1234);
}

/// A HorizonXI server's renumbered id for the Pashhow Marshlands outpost
/// vendor; LSB's vendor/server/sql/npc_list.sql has Tahmasp at 0x0106D290.
const PASHHOW_CONQUEST_BANNER: u32 = 0x0106_D291;
const PASHHOW_CONQUEST_BANNER_ACT_INDEX: u16 = 657;
const PASHHOW_ZONE: u16 = 109;
/// Tahmasp's vendorEvent in
/// vendor/server/scripts/zones/Pashhow_Marshlands/npcs/Tahmasp.lua, dispatched
/// by vendor/server/scripts/globals/conquest.lua xi.conquest.vendorOnTrigger.
const OUTPOST_VENDOR_EVENT: u16 = 32756;
/// The vendor dialog reads its strings from Southern San d'Oria's table.
const OUTPOST_VENDOR_TEXT_ZONE: u16 = 230;

// The conquest outpost vendor: LSB's conquest.lua xi.conquest.vendorOnTrigger
// calls startEvent(OUTPOST_VENDOR_EVENT, nation, fee, 0, fee / 10, getCP(), 0, 0, 0),
// packed into num[0..7] by 0x034_eventnum.cpp
// GP_SERV_COMMAND_EVENTNUM::GP_SERV_COMMAND_EVENTNUM. Dropping those on the
// floor leaves every {Num:N} marker in the vendor dialog unresolved.
#[test]
fn event_trigger_0x034_carries_params_and_the_redirected_text_table() {
    const NATION: i32 = 1;
    const FEE: i32 = 300;
    const CP: i32 = 4200;

    let mut data = vec![0u8; 48];
    data[0..4].copy_from_slice(&PASHHOW_CONQUEST_BANNER.to_le_bytes());
    data[4..8].copy_from_slice(&NATION.to_le_bytes());
    data[8..12].copy_from_slice(&FEE.to_le_bytes());
    data[20..24].copy_from_slice(&CP.to_le_bytes());
    data[36..38].copy_from_slice(&PASHHOW_CONQUEST_BANNER_ACT_INDEX.to_le_bytes());
    data[38..40].copy_from_slice(&PASHHOW_ZONE.to_le_bytes());
    data[40..42].copy_from_slice(&OUTPOST_VENDOR_EVENT.to_le_bytes());
    data[44..46].copy_from_slice(&OUTPOST_VENDOR_TEXT_ZONE.to_le_bytes());

    let sub = framing::SubPacket {
        opcode: ffxi_proto::map::s2c::EVENTNUM,
        sequence: 0,
        data: &data,
    };
    let t = event_trigger(&sub).expect("trigger");
    assert_eq!(t.unique_no, PASHHOW_CONQUEST_BANNER);
    assert_eq!(t.act_index, PASHHOW_CONQUEST_BANNER_ACT_INDEX);
    assert_eq!(
        t.event_id, OUTPOST_VENDOR_EVENT,
        "EventPara is the script id, not EventNum"
    );
    assert_eq!(t.event_zone, PASHHOW_ZONE);
    assert_eq!(
        t.text_zone, OUTPOST_VENDOR_TEXT_ZONE,
        "EventNum2 redirects the string table"
    );
    assert_eq!(t.params[0], NATION);
    assert_eq!(t.params[1], FEE);
    assert_eq!(t.params[4], CP);
}

// 0x033 has no EventNum2 field at all, so the strings come from the zone the
// script lives in rather than from a zero we would otherwise resolve as
// zone 0.
#[test]
fn event_trigger_without_a_text_table_falls_back_to_the_event_zone() {
    let mut data = vec![0u8; 108];
    data[6..8].copy_from_slice(&109u16.to_le_bytes());
    let sub = framing::SubPacket {
        opcode: ffxi_proto::map::s2c::EVENTSTR,
        sequence: 0,
        data: &data,
    };
    let t = event_trigger(&sub).expect("trigger");
    assert_eq!(t.event_zone, 109);
    assert_eq!(t.text_zone, 109);
}

#[test]
fn battle_message_0x029_substitutes_user_target_amount() {
    use std::collections::HashMap;

    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0x1111_1111u32.to_le_bytes());
    data[4..8].copy_from_slice(&0x2222_2222u32.to_le_bytes());
    data[8..12].copy_from_slice(&12u32.to_le_bytes());
    data[12..16].copy_from_slice(&0u32.to_le_bytes());
    data[16..18].copy_from_slice(&3u16.to_le_bytes());
    data[18..20].copy_from_slice(&4u16.to_le_bytes());
    data[20..22].copy_from_slice(&1u16.to_le_bytes());

    let mut cache = HashMap::new();
    cache.insert(0x1111_1111u32, "Sylvie".to_string());
    cache.insert(0x2222_2222u32, "Mandy".to_string());

    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert_eq!(line.channel, ChatChannel::Battle);
    assert_eq!(line.sender, "Sylvie");
    assert!(line.text.contains("Sylvie"));
    assert!(line.text.contains("Mandy"));
    assert!(line.text.contains("12"));
}

#[test]
fn battle_message_0x02d_uses_reordered_data_offsets() {
    use std::collections::HashMap;

    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&1u32.to_le_bytes());
    data[4..8].copy_from_slice(&2u32.to_le_bytes());
    data[8..10].copy_from_slice(&7u16.to_le_bytes());
    data[10..12].copy_from_slice(&8u16.to_le_bytes());
    data[12..16].copy_from_slice(&999u32.to_le_bytes());
    data[16..20].copy_from_slice(&0u32.to_le_bytes());
    data[20..22].copy_from_slice(&1u16.to_le_bytes());

    let cache = HashMap::new();
    let line = decode_battle_message(&data, &cache, &HashMap::new(), false).expect("decoded");
    assert!(
        line.text.contains("999"),
        "expected amount=999 from offsets [12..16], got: {}",
        line.text
    );
}

#[test]
fn battle_message_falls_back_to_hex_id_for_unknown_actor() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    data[4..8].copy_from_slice(&0u32.to_le_bytes());
    data[8..12].copy_from_slice(&5u32.to_le_bytes());
    data[20..22].copy_from_slice(&1u16.to_le_bytes());
    let line =
        decode_battle_message(&data, &HashMap::new(), &HashMap::new(), true).expect("decoded");
    assert_eq!(line.sender, "#DEADBEEF");
    assert!(line.text.contains("<no one>") || line.text.contains("#DEADBEEF"));
}

#[test]
fn battle_message_97_routes_player_to_tar_and_target_to_cas() {
    use std::collections::HashMap;

    let killer_id = 0xAAAA_AAAAu32;
    let victim_id = 0xBBBB_BBBBu32;

    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&killer_id.to_le_bytes());
    data[4..8].copy_from_slice(&victim_id.to_le_bytes());
    data[20..22].copy_from_slice(&97u16.to_le_bytes());

    let mut cache = HashMap::new();
    cache.insert(killer_id, "Orcish_Fodder".to_string());
    cache.insert(victim_id, "Vanari".to_string());

    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");

    assert_eq!(line.sender, "Vanari");

    let v_pos = line.text.find("Vanari").expect("victim in text");
    let o_pos = line.text.find("Orcish_Fodder").expect("killer in text");
    assert!(
        v_pos < o_pos,
        "victim must precede killer in the rendered template, got: {}",
        line.text
    );
}

#[test]
fn battle_message_6_defeats_strips_baked_article_for_pc_subject() {
    use crate::state::EntityKind;
    use std::collections::HashMap;

    let pc_id = 0x0100_0001u32;
    let mob_id = 0x0100_0700u32;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&pc_id.to_le_bytes());
    data[4..8].copy_from_slice(&mob_id.to_le_bytes());
    data[20..22].copy_from_slice(&6u16.to_le_bytes());

    let mut names = HashMap::new();
    names.insert(pc_id, "Atti".to_string());
    names.insert(mob_id, "Tunnel Worm".to_string());
    let mut kinds = HashMap::new();
    kinds.insert(pc_id, EntityKind::Pc);
    kinds.insert(mob_id, EntityKind::Mob);

    let line = decode_battle_message(&data, &names, &kinds, true).expect("decoded");
    assert_eq!(
        line.text, "Atti defeats Tunnel Worm.",
        "PC subject must not carry the baked article, got: {}",
        line.text
    );
    assert!(
        !line.text.starts_with("The "),
        "leading article leaked: {}",
        line.text
    );
}

#[test]
fn battle_message_6_defeats_keeps_article_for_mob_subject() {
    use crate::state::EntityKind;
    use std::collections::HashMap;

    let mob_a = 0x0100_0700u32;
    let mob_b = 0x0100_0701u32;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&mob_a.to_le_bytes());
    data[4..8].copy_from_slice(&mob_b.to_le_bytes());
    data[20..22].copy_from_slice(&6u16.to_le_bytes());

    let mut names = HashMap::new();
    names.insert(mob_a, "Goblin Smithy".to_string());
    names.insert(mob_b, "Forest Hare".to_string());
    let mut kinds = HashMap::new();
    kinds.insert(mob_a, EntityKind::Mob);
    kinds.insert(mob_b, EntityKind::Mob);

    let line = decode_battle_message(&data, &names, &kinds, true).expect("decoded");
    assert_eq!(
        line.text, "The Goblin Smithy defeats Forest Hare.",
        "got: {}",
        line.text
    );
}

#[test]
fn is_fresh_bundle_dedups_retransmits_and_survives_wrap() {
    assert!(is_fresh_bundle(None, 0));
    assert!(is_fresh_bundle(None, 5000));

    assert!(!is_fresh_bundle(Some(42), 42));

    assert!(is_fresh_bundle(Some(42), 43));

    assert!(!is_fresh_bundle(Some(43), 42));

    assert!(is_fresh_bundle(Some(0xFFFF), 0x0001));
    assert!(!is_fresh_bundle(Some(0x0001), 0xFFFF));
}

/// Model of LSB's c2s dispatch window
/// (vendor/server/src/map/map_networking.cpp MapNetworking::parse): subpacket
/// dispatched iff `client_packet_id < sync <= header`, then
/// `client_packet_id = header`. Feeds it bundles built the way the
/// session builds them (one sync per subpacket, header from
/// [`datagram_header_id`]) and asserts nothing is ever skipped —
/// multi-subpacket bundles are the case most exposed to a header
/// counter that drifts from the subpacket syncs, which silently
/// deafens the server to the session.
#[test]
fn datagram_header_keeps_every_subpacket_inside_the_server_window() {
    let mut client_packet_id: u16 = crate::map_client::BOOTSTRAP_SUB_SYNC;
    let mut sub_seq: u16 = crate::map_client::BOOTSTRAP_SUB_SYNC.wrapping_add(1);

    for bundle_len in [1usize, 1, 2, 1, 3, 1, 1, 5, 2, 1] {
        let mut syncs = Vec::new();
        for _ in 0..bundle_len {
            syncs.push(sub_seq);
            sub_seq = sub_seq.wrapping_add(1);
        }
        let header = datagram_header_id(sub_seq);
        assert_eq!(header, *syncs.last().unwrap());
        for sync in syncs {
            assert!(
                client_packet_id < sync && sync <= header,
                "sync {sync} outside server window ({client_packet_id}, {header}]"
            );
        }
        client_packet_id = header;
    }
}

#[test]
fn battle_message_8_exp_gain_substitutes_hash_marker() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[12..16].copy_from_slice(&420u32.to_le_bytes());
    data[16..20].copy_from_slice(&0u32.to_le_bytes());
    data[20..22].copy_from_slice(&8u16.to_le_bytes());
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "hello".to_string());

    let line = decode_battle_message(&data, &cache, &HashMap::new(), false).expect("decoded");
    assert!(
        line.text.contains("420") && !line.text.contains('#'),
        "expected '#' to be replaced with 420, got: {}",
        line.text
    );
    assert!(line.text.contains("hello"));
}

#[test]
fn battle_message_38_skill_gain_substitutes_skill_and_x() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[8..12].copy_from_slice(&48u32.to_le_bytes());
    data[12..16].copy_from_slice(&3u32.to_le_bytes());
    data[20..22].copy_from_slice(&38u16.to_le_bytes());
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "hello".to_string());
    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");

    assert!(
        line.text.contains("Fishing") && line.text.contains("rises 0.3 points"),
        "expected '<skill>'→Fishing and 'X'→0.3 (decimal), got: {}",
        line.text
    );
}

#[test]
fn battle_message_53_skill_level_up_renders_x_as_integer() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[8..12].copy_from_slice(&1u32.to_le_bytes());
    data[12..16].copy_from_slice(&12u32.to_le_bytes());
    data[20..22].copy_from_slice(&53u16.to_le_bytes());
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "hello".to_string());
    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert!(
        line.text.contains("level 12") && !line.text.contains("1.2"),
        "expected integer level, got: {}",
        line.text
    );
}

#[test]
fn battle_message_253_exp_chain_substitutes_two_hashes_in_order() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[12..16].copy_from_slice(&320u32.to_le_bytes());
    data[16..20].copy_from_slice(&5u32.to_le_bytes());
    data[20..22].copy_from_slice(&253u16.to_le_bytes());
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "hello".to_string());
    let line = decode_battle_message(&data, &cache, &HashMap::new(), false).expect("decoded");
    assert!(
        line.text.contains("chain 5!") && line.text.contains("gains 320"),
        "expected 'chain 5!' and 'gains 320', got: {}",
        line.text
    );
    assert!(
        !line.text.contains('#'),
        "stray '#' remained: {}",
        line.text
    );
}

#[test]
fn substitute_battle_x_marker_respects_token_boundary() {
    let s = substitute_battle_placeholders(
        "reaches level X. BoXing.",
        "cas",
        "tar",
        false,
        false,
        0,
        7,
        53,
        None,
    );
    assert!(s.contains("reaches level 7"), "got: {s}");
    assert!(s.contains("BoXing"), "within-word X must survive, got: {s}");
}

#[test]
fn battle_message_2_magic_damage_resolves_spell_name() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[4..8].copy_from_slice(&0xBEEFu32.to_le_bytes());
    data[8..12].copy_from_slice(&144u32.to_le_bytes());
    data[12..16].copy_from_slice(&0u32.to_le_bytes());
    data[20..22].copy_from_slice(&2u16.to_le_bytes());
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "Daisy".to_string());
    cache.insert(0xBEEFu32, "Mandragora".to_string());
    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert!(
        line.text.contains("Daisy")
            && line.text.contains("Mandragora")
            && line.text.contains("Fire")
            && !line.text.contains("<spell>")
            && !line.text.contains("spell #"),
        "expected resolved spell name in: {}",
        line.text
    );
}

#[test]
fn battle_message_565_obtains_gil_override_appends_unit() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[4..8].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[8..12].copy_from_slice(&4u32.to_le_bytes());
    data[12..16].copy_from_slice(&0u32.to_le_bytes());
    data[20..22].copy_from_slice(&565u16.to_le_bytes());
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "Mithy".to_string());
    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert_eq!(line.text, "Mithy obtains 4 gil.", "got: {}", line.text);
}

#[test]
fn battle2_self_ja_uses_ability_line_resolves_from_override() {
    // msg 116 (Boost/Warcry "uses" line) is absent from LSB's msg_basic.h; the
    // FALLBACK_TEMPLATES entry must fill it so a self JA-finish (category 6) still logs.
    let line = build_battle2_line(116, "Nicotine", "Nicotine", true, true, 0, 39, 6)
        .expect("msg 116 must resolve via override");
    assert!(
        line.text.contains("uses") && line.text.contains("Boost"),
        "got: {}",
        line.text
    );
}

/// The whole battle path against a real install: an id the scrape cannot serve
/// ("The <player> uses .." in msg_basic.h) must come out of the client's own
/// basic-message table instead, with the ability named and no elision left.
/// Row-keyed wording is pinned in ffxi-dat tests/mesbasic.rs; this pins the
/// wiring. Self-skips without game files.
#[test]
fn a_job_ability_line_is_composed_from_the_installed_table() {
    let Some(root) = test_dat_root() else {
        eprintln!("skipping: no FFXI install");
        return;
    };
    let Some(table) = MesBasicDat::open(&root) else {
        eprintln!("skipping: install has no basic-message table");
        return;
    };
    let table = MesBasicTables::from_dat(table);
    // Boost is ability 39 (vendor/server/sql/abilities.sql), whose message1 is
    // the elided UsesJobAbility line.
    const BOOST: u32 = 39;
    let mut numbers = [0i64; sysmes::PARAM_SLOTS];
    numbers[MES_PARAM_ACTION_ID] = BOOST as i64;
    let lines = super::build_battle2_line(
        Some(&table),
        100,
        "Daisy",
        "Daisy",
        true,
        true,
        0,
        BOOST,
        JOB_ABILITY_FINISH_CATEGORY,
        numbers,
    );
    assert_eq!(lines.len(), 1, "got: {lines:?}");
    assert_eq!(lines[0].text, "Daisy uses Boost.");
    assert_eq!(lines[0].channel, ChatChannel::Battle);
    assert_eq!(lines[0].sender, "Daisy");
    assert_eq!(
        lines[0]
            .spans
            .iter()
            .map(|s| s.text.as_str())
            .collect::<String>(),
        lines[0].text,
        "span text must reconstruct the line"
    );
}

/// A two-clause entry is two retail log lines, not one wrapped one.
/// Self-skips without game files.
#[test]
fn a_two_clause_entry_becomes_two_chat_lines() {
    let Some(root) = test_dat_root() else {
        eprintln!("skipping: no FFXI install");
        return;
    };
    let Some(table) = MesBasicDat::open(&root) else {
        eprintln!("skipping: install has no basic-message table");
        return;
    };
    let table = MesBasicTables::from_dat(table);
    const BOOST: u32 = 39;
    const DAMAGE: u32 = 42;
    let mut numbers = [0i64; sysmes::PARAM_SLOTS];
    numbers[MES_PARAM_ACTION_ID] = BOOST as i64;
    numbers[MES_PARAM_MAIN_VALUE] = DAMAGE as i64;
    let lines = super::build_battle2_line(
        Some(&table),
        317,
        "Daisy",
        "Rock Lizard",
        true,
        false,
        DAMAGE,
        BOOST,
        JOB_ABILITY_FINISH_CATEGORY,
        numbers,
    );
    assert_eq!(lines.len(), 2, "got: {lines:?}");
    assert_eq!(lines[0].text, "Daisy uses Boost.");
    assert_eq!(lines[1].text, "The Rock Lizard takes 42 points of damage.");
}

/// Msg 116 has no msg_basic.h enumerator at all, so without the installed table
/// the line only exists because FALLBACK_TEMPLATES carries it. With one, both
/// clauses come from the client's own wording. Self-skips without game files.
#[test]
fn an_id_the_scrape_does_not_know_still_composes_from_the_install() {
    let Some(root) = test_dat_root() else {
        eprintln!("skipping: no FFXI install");
        return;
    };
    let Some(table) = MesBasicDat::open(&root) else {
        eprintln!("skipping: install has no basic-message table");
        return;
    };
    let table = MesBasicTables::from_dat(table);
    const BOOST: u32 = 39;
    assert!(
        ffxi_vocab::msg_basic::lookup(116).is_none(),
        "the scrape gained an entry for 116; this test no longer proves what it says"
    );
    let mut numbers = [0i64; sysmes::PARAM_SLOTS];
    numbers[MES_PARAM_ACTION_ID] = BOOST as i64;
    let lines = super::build_battle2_line(
        Some(&table),
        116,
        "Daisy",
        "Daisy",
        true,
        true,
        0,
        BOOST,
        JOB_ABILITY_FINISH_CATEGORY,
        numbers,
    );
    assert_eq!(lines.len(), 2, "got: {lines:?}");
    assert_eq!(lines[0].text, "Daisy uses Boost.");
    assert_eq!(lines[1].text, "Daisy's attacks are enhanced.");
}

/// vendor/server/src/map/enums/action/category.h ActionCategory ItemFinish.
const ITEM_FINISH_CATEGORY: u8 = 5;

/// LSB sends an item's finish line as MsgBasic::ItemUse with the item id in the
/// target's param, the packet's value slot (vendor/server/src/map/ai/states/item_state.cpp
/// CItemState::Update); the install's entry reads that slot and names the item
/// through the item DAT's chat-log spelling. Self-skips without game files.
#[test]
fn an_item_use_line_names_the_item_from_the_item_dat() {
    let Some(root) = test_dat_root() else {
        eprintln!("skipping: no FFXI install");
        return;
    };
    let Some(tables) = MesBasicTables::open(&root) else {
        eprintln!("skipping: install has no basic-message table");
        return;
    };
    // vendor/server/sql/item_basic.sql hatchling_shield.
    const HATCHLING_SHIELD: u32 = 28652;
    // vendor/server/src/map/enums/msg_basic.h MsgBasic ItemUse.
    const ITEM_USE: u16 = 28;
    let mut numbers = [0i64; sysmes::PARAM_SLOTS];
    numbers[MES_PARAM_ACTION_ID] = HATCHLING_SHIELD as i64;
    numbers[MES_PARAM_MAIN_VALUE] = HATCHLING_SHIELD as i64;
    let lines = super::build_battle2_line(
        Some(&tables),
        ITEM_USE,
        "Daisy",
        "Daisy",
        true,
        true,
        HATCHLING_SHIELD,
        HATCHLING_SHIELD,
        ITEM_FINISH_CATEGORY,
        numbers,
    );
    assert_eq!(lines.len(), 1, "got: {lines:?}");
    assert_eq!(lines[0].text, "Daisy uses a hatchling shield.");
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|s| s.kind == ChatSpanKind::Item && s.text == "hatchling shield"),
        "the item name is its own span: {:?}",
        lines[0].spans
    );
}

/// vendor/server/src/map/enums/action/category.h ActionCategory AbilityFinish.
const JOB_ABILITY_FINISH_CATEGORY: u8 = 6;

#[test]
fn fallback_templates_only_shadow_msg_basic_deliberately() {
    for &(id, template) in FALLBACK_TEMPLATES {
        match ffxi_vocab::msg_basic::lookup(id) {
            None => assert!(
                !DELIBERATE_SHADOWS.contains(&id),
                "id {id} is listed as a deliberate shadow but the scrape has no entry to shadow"
            ),
            Some(scraped) => {
                assert_ne!(
                    template, scraped,
                    "id {id} silently shadows an identical scraped msg_basic value"
                );
                assert!(
                    DELIBERATE_SHADOWS.contains(&id),
                    "id {id} overrides scraped msg_basic {scraped:?} without a DELIBERATE_SHADOWS listing"
                );
            }
        }
    }
}

#[test]
fn battle_rejection_ids_route_to_the_main_log() {
    for &id in BATTLE_REJECTION_IDS {
        assert_eq!(
            battle_line_channel(id),
            ChatChannel::System,
            "id {id} must reach the main log"
        );
    }
}

#[test]
fn battle_result_and_state_ids_stay_on_the_battle_tab() {
    for id in [1u16, 2, 3, 6, 8, 9, 15, 38, 53, 97, 203, 253] {
        assert_eq!(
            battle_line_channel(id),
            ChatChannel::Battle,
            "id {id} must stay on the Battle tab"
        );
    }
}

/// A rejection id with no msg_basic template would be dropped by
/// template_for_id, hiding the refusal again; fail the build on scrape drift.
#[test]
fn battle_rejection_ids_all_resolve_in_the_scrape() {
    for &id in BATTLE_REJECTION_IDS {
        assert!(
            ffxi_vocab::msg_basic::lookup(id).is_some(),
            "id {id} has no msg_basic template"
        );
    }
}

#[test]
fn battle_message_5_unable_to_see_routes_to_the_main_log() {
    use std::collections::HashMap;

    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0x1111_1111u32.to_le_bytes());
    data[4..8].copy_from_slice(&0x2222_2222u32.to_le_bytes());
    data[20..22].copy_from_slice(&5u16.to_le_bytes());

    let mut cache = HashMap::new();
    cache.insert(0x1111_1111u32, "Sylvie".to_string());
    cache.insert(0x2222_2222u32, "Mandy".to_string());

    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert_eq!(line.channel, ChatChannel::System);
    assert!(line.text.contains("Mandy"), "got: {}", line.text);
}

#[test]
fn battle_message_4_out_of_range_routes_to_the_main_log() {
    use std::collections::HashMap;

    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0x1111_1111u32.to_le_bytes());
    data[4..8].copy_from_slice(&0x2222_2222u32.to_le_bytes());
    data[20..22].copy_from_slice(&4u16.to_le_bytes());

    let mut cache = HashMap::new();
    cache.insert(0x1111_1111u32, "Sylvie".to_string());
    cache.insert(0x2222_2222u32, "Mandy".to_string());

    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert_eq!(line.channel, ChatChannel::System);
    assert!(line.text.contains("Mandy"), "got: {}", line.text);
}

#[test]
fn substitute_status_placeholder_resolves_effect_name() {
    let s = substitute_battle_placeholders(
        "gains the effect of <status>.",
        "cas",
        "tar",
        false,
        false,
        40,
        0,
        186,
        None,
    );
    assert!(
        s.contains("Protect") && !s.contains("<status>") && !s.contains("status #"),
        "expected resolved status name in: {s}"
    );
}

#[test]
fn battle_message_43_readies_weaponskill_substitutes_entity() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&0xCAFEu32.to_le_bytes());
    data[4..8].copy_from_slice(&0u32.to_le_bytes());
    data[8..12].copy_from_slice(&1u32.to_le_bytes());
    data[12..16].copy_from_slice(&0u32.to_le_bytes());
    data[20..22].copy_from_slice(&43u16.to_le_bytes());
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "Daisy".to_string());
    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert!(
        line.text.contains("Daisy readies Combo") && !line.text.contains("<entity>"),
        "expected '<entity>' → Daisy and skill 1 → the weapon skill Combo, got: {}",
        line.text
    );
}

#[test]
fn battle2_mob_tp_move_resolves_name_not_damage() {
    // MobSkillFinish (11) carries the skill id in cmd_arg and the damage in param, so a
    // Goobbue's Uppercut (mob skill 584) for 160 damage must not log "skill #160".
    let line = build_battle2_line(185, "Goobbue Farmer", "Oldman", false, true, 160, 584, 11)
        .expect("msg 185 must resolve");
    assert!(
        line.text.contains("uses Uppercut") && line.text.contains("160 points"),
        "got: {}",
        line.text
    );
}

#[test]
fn battle2_mob_readies_resolves_skill_from_param() {
    // SkillStart (7) puts the skill id in param instead (mobskill_state.cpp CState).
    let line = build_battle2_line(43, "Goobbue Farmer", "Oldman", false, true, 584, 0, 7)
        .expect("msg 43 must resolve");
    assert!(line.text.contains("readies Uppercut"), "got: {}", line.text);
}

#[test]
fn battle2_mob_magic_start_says_starts_casting() {
    // MagicStart (8) carries msg 327 with the spell id in param
    // (vendor/server/src/map/ai/states/magic_state.cpp CMagicState::init).
    let line = build_battle2_line(327, "Goblin Tinkerer", "Oldman", false, true, 4, 0, 8)
        .expect("msg 327 must resolve");
    assert!(
        line.text.contains("starts casting Cure"),
        "got: {}",
        line.text
    );
}

#[test]
fn battle2_plain_job_ability_names_the_ability() {
    // Sneak Attack (abilityId 44) has abilities.sql message1 = 0, so LSB falls back to
    // msg 100 — whose LSB comment is "The <player> uses .." and needs the override.
    let line = build_battle2_line(100, "Oldman", "Oldman", true, true, 0, 44, 6)
        .expect("msg 100 must resolve via override");
    assert_eq!(line.text, "Oldman uses Sneak Attack.", "got: {}", line.text);
}

#[test]
fn battle2_ability_start_readies_names_the_ability() {
    // msg 326 rides AbilityStart (10), where param is an ability id, not a weapon skill.
    let line = build_battle2_line(326, "Oldman", "Goobbue Farmer", true, false, 52, 0, 10)
        .expect("msg 326 must resolve");
    assert!(line.text.contains("readies Charm"), "got: {}", line.text);
}

struct BattleBitWriter {
    data: Vec<u8>,
    pos: usize,
}

impl BattleBitWriter {
    fn new(start_bit: usize) -> Self {
        Self {
            data: vec![0u8; 1024],
            pos: start_bit,
        }
    }
    fn write(&mut self, value: u64, bits: u32) {
        let byte_offset = self.pos / 8;
        let bit_in_byte = self.pos % 8;
        let total_bits = bits as usize + bit_in_byte;
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let shifted = (value & mask) << bit_in_byte;
        let cover = total_bits.div_ceil(8);
        const BYTE_MASK: u64 = 0xFF;
        for i in 0..cover {
            self.data[byte_offset + i] |= ((shifted >> (i * 8)) & BYTE_MASK) as u8;
        }
        self.pos += bits as usize;
    }
    fn into_bytes(self) -> Vec<u8> {
        let used = self.pos.div_ceil(8);
        self.data[..used].to_vec()
    }
}

// vendor/server/src/map/packets/s2c/0x028_battle2.cpp GP_SERV_COMMAND_BATTLE2::pack — an off-by-one-field read here
// silently hands the recast timer ("Action info") back as an entity id.
#[test]
fn battle2_header_reports_primary_target() {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(0, 4);
    w.write(8, 4);
    w.write(220, 32);
    w.write(0xFFFF, 32);
    w.write(0xBEEFu64, 32);
    w.write(0, 4);
    // The reader widens a 32-bit read to a 64-bit fetch, so the tail needs slack; a real
    // packet always carries the target's result blocks here.
    w.write(0, 32);
    w.write(0, 32);

    let h = decode_battle2_header(&w.into_bytes()).unwrap();
    assert_eq!(h.actor_id, 0xCAFE);
    assert_eq!(h.action_kind, 8);
    assert_eq!(h.action_id, 220);
    assert_eq!(h.primary_target_id, Some(0xBEEF));

    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(0, 6);
    w.write(0, 4);
    w.write(8, 4);
    w.write(220, 32);
    w.write(0xFFFF, 32);
    w.write(0, 32);
    let h = decode_battle2_header(&w.into_bytes()).unwrap();
    assert_eq!(h.primary_target_id, None);
}

// vendor/server/src/map/packets/s2c/0x028_battle2.cpp GP_SERV_COMMAND_BATTLE2::pack — resolution(3), kind(2),
// animation(12) open every result block. A basic attack never sets `action.actionid`
// (vendor/server/src/map/entities/battle_entity.cpp CBattleEntity::OnAttack), so these bits are the ONLY
// per-swing data: an off-by-one here picks the wrong swing routine and the wrong hit
// reaction, i.e. the wrong sound or none.
const BATTLE2_PARRIED_LEFT_ATTACK: ffxi_proto::melee::MeleeResult =
    ffxi_proto::melee::MeleeResult {
        resolution: ffxi_proto::melee::ActionResolution::Parry,
        animation: ffxi_proto::melee::AttackAnimation::LeftAttack,
        info: ffxi_proto::melee::ActionInfo::NONE,
        hit_distortion: ffxi_proto::melee::HitDistortion::None,
        knockback: ffxi_proto::melee::KnockbackLevel::None,
    };

fn battle2_single_result_body() -> Vec<u8> {
    let (resolution, animation, _info, _hit_distortion, _knockback) =
        BATTLE2_PARRIED_LEFT_ATTACK.to_wire();
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(1, 4);
    w.write(ffxi_proto::melee::CATEGORY_BASIC_ATTACK as u64, 4);
    w.write(0, 32);
    w.write(0, 32);
    w.write(0xBEEFu64, 32);
    w.write(1, 4);
    w.write(u64::from(resolution), 3);
    w.write(0, 2);
    w.write(u64::from(animation), 12);
    w.write(0, 32);
    w.write(0, 32);
    w.into_bytes()
}

#[test]
fn battle2_basic_attack_reports_resolution_and_swing_animation() {
    let h = decode_battle2_header(&battle2_single_result_body()).unwrap();
    assert_eq!(h.action_kind, ffxi_proto::melee::CATEGORY_BASIC_ATTACK);
    assert_eq!(h.action_id, 0, "a basic attack carries no cmd_arg");
    assert_eq!(h.primary_target_id, Some(0xBEEF));
    assert_eq!(h.first_result, Some(BATTLE2_PARRIED_LEFT_ATTACK));
}

// The outcome bits after animation(12): info(5), hitDistortion(2), knockback(3) in LSB write
// order (vendor/server/src/map/packets/s2c/0x028_battle2.cpp). A hand-packed critical left-attack
// with level-2 knockback must come back split, not lumped into one 5-bit "scale".
#[test]
fn battle2_result_outcome_bits_roundtrip() {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(1, 4);
    w.write(ffxi_proto::melee::CATEGORY_BASIC_ATTACK as u64, 4);
    w.write(0, 32);
    w.write(0, 32);
    w.write(0xBEEFu64, 32);
    w.write(1, 4);
    // Bit layout per vendor/server/src/map/packets/s2c/0x028_battle2.cpp:
    w.write(0, 3); // resolution: Hit
    w.write(1, 2); // kind
    w.write(1, 12); // animation: LeftAttack
    w.write(2, 5); // info: CriticalHit
    w.write(3, 2); // hitDistortion: Heavy
    w.write(2, 3); // knockback: level 2
    w.write(0, 17); // param
    w.write(67, 10); // messageID: AttackCrit
    w.write(0, 31); // modifier
    w.write(0, 1); // no proc block
    w.write(0, 1); // no reaction block

    let h = decode_battle2_header(&w.into_bytes()).unwrap();
    let outcome = h
        .first_outcome
        .expect("a result block decodes its outcome bits");
    assert_eq!(outcome.to_wire(), (2, 3, 2), "CriticalHit, Heavy, level 2");
    assert!(outcome.is_critical());
    let r = h.first_result.expect("a basic-attack result decodes");
    assert_eq!(r.resolution, ffxi_proto::melee::ActionResolution::Hit);
    assert_eq!(r.animation, ffxi_proto::melee::AttackAnimation::LeftAttack);
}

// A non-basic-attack category whose result block happens to carry low resolution/animation
// bits (e.g. a spell's animation id 1) must not decode as a melee swing — the gate is the
// category, not the bit ranges.
#[test]
fn battle2_non_basic_category_reports_no_melee_result() {
    let (resolution, animation, _info, _hit_distortion, _knockback) =
        BATTLE2_PARRIED_LEFT_ATTACK.to_wire();
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(1, 4);
    w.write(4, 4);
    w.write(220, 32);
    w.write(0, 32);
    w.write(0xBEEFu64, 32);
    w.write(1, 4);
    w.write(u64::from(resolution), 3);
    w.write(0, 2);
    w.write(u64::from(animation), 12);
    w.write(0, 32);
    w.write(0, 32);

    let h = decode_battle2_header(&w.into_bytes()).unwrap();
    assert_eq!(h.action_kind, 4);
    assert_eq!(h.primary_target_id, Some(0xBEEF));
    assert_eq!(h.first_result, None);
    assert_eq!(
        h.first_outcome,
        Some(ffxi_proto::melee::ResultOutcome {
            info: ffxi_proto::melee::ActionInfo::NONE.bits(),
            hit_distortion: ffxi_proto::melee::HitDistortion::None.to_wire(),
            knockback: ffxi_proto::melee::KnockbackLevel::None.to_wire(),
        }),
        "the outcome is read for every category even though the swing pair is gated off",
    );
}

// The same 12 bits, uninterpreted, key the caster's effect DAT for every non-attack
// category. Sneak Attack rides AbilityFinish (6) as cmd_arg 44 / animation 17, and it is
// 17 the renderer needs: 0x113C+44 is an unrelated ability's routine.
#[test]
fn battle2_header_reports_the_raw_animation_index() {
    const SNEAK_ATTACK_ABILITY_ID: u64 = 44;
    const SNEAK_ATTACK_ANIMATION: u64 = 17;

    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(1, 4);
    w.write(6, 4);
    w.write(SNEAK_ATTACK_ABILITY_ID, 32);
    w.write(0, 32);
    w.write(0xBEEFu64, 32);
    w.write(1, 4);
    w.write(0, 3);
    w.write(0, 2);
    w.write(SNEAK_ATTACK_ANIMATION, 12);
    w.write(0, 32);
    w.write(0, 32);

    let h = decode_battle2_header(&w.into_bytes()).unwrap();
    assert_eq!(h.action_id, SNEAK_ATTACK_ABILITY_ID as u32);
    assert_eq!(h.animation, Some(SNEAK_ATTACK_ANIMATION as u16));
    assert_eq!(h.first_result, None, "category 6 is not a melee swing");
}

#[test]
fn battle2_animation_is_absent_without_a_result() {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(0, 4);
    w.write(6, 4);
    w.write(44, 32);
    w.write(0, 32);
    w.write(0xBEEFu64, 32);
    w.write(0, 4);
    w.write(0, 32);
    w.write(0, 32);

    let h = decode_battle2_header(&w.into_bytes()).unwrap();
    assert_eq!(h.animation, None);
}

// A target that carries no result blocks must not read the packet tail as a resolution:
// resolution 0 is `Hit` and animation 0 is `RightAttack`, so a fabricated pair arms the
// victim's flinch + impact SE for a swing that may have missed.
#[test]
fn battle2_target_without_results_reports_no_resolution() {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(0, 4);
    w.write(ffxi_proto::melee::CATEGORY_BASIC_ATTACK as u64, 4);
    w.write(0, 32);
    w.write(0, 32);
    w.write(0xBEEFu64, 32);
    w.write(0, 4);
    w.write(0xFFFF_FFFFu64, 32);
    w.write(0xFFFF_FFFFu64, 32);

    let h = decode_battle2_header(&w.into_bytes()).unwrap();
    assert_eq!(h.first_result, None);
}

#[test]
fn battle2_body_without_target_block_reports_no_resolution() {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(0, 6);
    w.write(0, 4);
    w.write(ffxi_proto::melee::CATEGORY_BASIC_ATTACK as u64, 4);
    w.write(0, 32);
    w.write(0, 32);
    w.write(0, 32);

    let h = decode_battle2_header(&w.into_bytes()).unwrap();
    assert_eq!(h.primary_target_id, None);
    assert_eq!(h.first_result, None);
}

#[test]
fn battle2_truncated_result_block_reports_no_resolution() {
    let full = battle2_single_result_body();
    let mut saw_target_without_result = false;
    for len in 0..full.len() {
        let Some(h) = decode_battle2_header(&full[..len]) else {
            continue;
        };
        assert!(
            h.first_result.is_none() || h.first_result == Some(BATTLE2_PARRIED_LEFT_ATTACK),
            "prefix of {len} bytes fabricated {:?}",
            h.first_result
        );
        saw_target_without_result |= h.primary_target_id.is_some() && h.first_result.is_none();
    }
    assert!(
        saw_target_without_result,
        "no prefix ran out of bits inside the result block"
    );
}

#[test]
fn battle2_single_hit_emits_damage_line() {
    use std::collections::HashMap;
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(0, 4);
    w.write(0, 4);
    w.write(0, 32);
    w.write(0, 32);

    w.write(0xBEEFu64, 32);
    w.write(1, 4);

    w.write(0, 3);
    w.write(0, 2);
    w.write(0, 12);
    w.write(0, 5);
    w.write(0, 5);
    w.write(42, 17);
    w.write(1, 10);
    w.write(0, 31);
    w.write(0, 1);
    w.write(0, 1);

    let data = w.into_bytes();
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "Daisy".to_string());
    cache.insert(0xBEEFu32, "Mandragora".to_string());

    let lines = decode_battle2_action(&data, &cache, &HashMap::new());
    assert_eq!(lines.len(), 1, "expected one line, got: {:?}", lines);
    let l = &lines[0];
    assert_eq!(l.channel, ChatChannel::Battle);
    assert!(
        l.text.contains("Daisy") && l.text.contains("Mandragora") && l.text.contains("42"),
        "expected damage line, got: {}",
        l.text
    );
}

#[test]
fn battle2_magic_damage_substitutes_spell_from_cmd_arg() {
    use std::collections::HashMap;
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFE, 32);
    w.write(1, 6);
    w.write(0, 4);
    w.write(4, 4);
    w.write(144, 32);
    w.write(0, 32);
    w.write(0xBEEF, 32);
    w.write(1, 4);
    w.write(0, 3);
    w.write(0, 2);
    w.write(0, 12);
    w.write(0, 5);
    w.write(0, 5);
    w.write(87, 17);
    w.write(2, 10);
    w.write(0, 31);
    w.write(0, 1);
    w.write(0, 1);

    let data = w.into_bytes();
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "Daisy".to_string());
    cache.insert(0xBEEFu32, "Mandragora".to_string());

    let lines = decode_battle2_action(&data, &cache, &HashMap::new());
    assert_eq!(lines.len(), 1);
    let l = &lines[0];
    assert!(
        l.text.contains("Daisy") && l.text.contains("Fire") && l.text.contains("87"),
        "expected casts/Fire/87 in: {}",
        l.text
    );
}

#[test]
fn battle2_starts_casting_resolves_spell_from_param() {
    use std::collections::HashMap;
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFE, 32);
    w.write(1, 6);
    w.write(0, 4);
    w.write(8, 4);
    w.write(0x68776163, 32);
    w.write(0, 32);
    w.write(0xBEEF, 32);
    w.write(1, 4);
    w.write(0, 3);
    w.write(0, 2);
    w.write(0, 12);
    w.write(0, 5);
    w.write(0, 5);
    w.write(144, 17);
    w.write(327, 10);
    w.write(0, 31);
    w.write(0, 1);
    w.write(0, 1);

    let data = w.into_bytes();
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "Daisy".to_string());
    cache.insert(0xBEEFu32, "Mandragora".to_string());

    let lines = decode_battle2_action(&data, &cache, &HashMap::new());
    assert_eq!(lines.len(), 1);
    let l = &lines[0];
    assert!(
        l.text.contains("Daisy") && l.text.contains("Fire") && !l.text.contains("spell #"),
        "expected resolved 'Fire' (not a raw spell # fallback) in: {}",
        l.text
    );
}

#[test]
fn battle2_drops_results_with_zero_message_id() {
    use std::collections::HashMap;
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFE, 32);
    w.write(1, 6);
    w.write(0, 4);
    w.write(0, 4);
    w.write(0, 32);
    w.write(0, 32);
    w.write(0xBEEF, 32);
    w.write(1, 4);
    w.write(0, 3);
    w.write(0, 2);
    w.write(0, 12);
    w.write(0, 5);
    w.write(0, 5);
    w.write(0, 17);
    w.write(0, 10);
    w.write(0, 31);
    w.write(0, 1);
    w.write(0, 1);
    let data = w.into_bytes();
    let lines = decode_battle2_action(&data, &HashMap::new(), &HashMap::new());
    assert!(lines.is_empty(), "expected drop, got: {:?}", lines);
}

#[test]
fn battle2_bitwriter_matches_lsb_pack_byte_layout() {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    let bytes = w.into_bytes();
    assert_eq!(bytes[0], 0x00, "workSize slot reserved at byte 0");
    assert_eq!(
        &bytes[1..5],
        &[0xFE, 0xCA, 0x00, 0x00],
        "actor_id LE-packed at byte 1..5 — if this fails, BitWriter \
             no longer matches LSB packBitsBE; do NOT flip BitReader to \
             compensate"
    );
}

#[test]
fn battle2_decoder_pins_worksize_prefix_convention() {
    use std::collections::HashMap;
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFE, 32);
    w.write(1, 6);
    w.write(0, 4);
    w.write(0, 4);
    w.write(0, 32);
    w.write(0, 32);
    w.write(0xBEEF, 32);
    w.write(1, 4);
    w.write(0, 3);
    w.write(0, 2);
    w.write(0, 12);
    w.write(0, 5);
    w.write(0, 5);
    w.write(42, 17);
    w.write(1, 10);
    w.write(0, 31);
    w.write(0, 1);
    w.write(0, 1);

    let mut data = w.into_bytes();

    let bitstream_bits = data.len() * 8 - 8;
    data[0] = bitstream_bits.div_ceil(8) as u8;

    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "Daisy".to_string());
    cache.insert(0xBEEFu32, "Mandragora".to_string());

    let lines = decode_battle2_action(&data, &cache, &HashMap::new());
    assert_eq!(
        lines.len(),
        1,
        "wire-shape regression: expected 1 line from a body with 1-byte workSize prefix, got: {:?}",
        lines
    );
    let l = &lines[0];
    assert!(
            l.text.contains("Daisy") && l.text.contains("Mandragora") && l.text.contains("42"),
            "decoded line lost actor/target/damage — check that start-bit 8 is preserved at session.rs:decode_battle2_action; got: {}",
            l.text
        );
}

#[test]
fn battle_message_unknown_id_returns_none() {
    use std::collections::HashMap;
    let mut data = vec![0u8; 24];
    data[20..22].copy_from_slice(&0xFFFFu16.to_le_bytes());
    assert!(decode_battle_message(&data, &HashMap::new(), &HashMap::new(), true).is_none());
}

fn check_message(message_num: u16, data1: u32, data2: u32, cas: u32, tar: u32) -> Vec<u8> {
    let mut data = vec![0u8; 24];
    data[0..4].copy_from_slice(&cas.to_le_bytes());
    data[4..8].copy_from_slice(&tar.to_le_bytes());
    data[8..12].copy_from_slice(&data1.to_le_bytes());
    data[12..16].copy_from_slice(&data2.to_le_bytes());
    data[20..22].copy_from_slice(&message_num.to_le_bytes());
    data
}

#[test]
fn check_mob_even_even_renders_difficulty_and_level() {
    use std::collections::HashMap;
    let data = check_message(174, 53, 64 + 4, 1, 2);
    let mut cache = HashMap::new();
    cache.insert(1u32, "Daisy".to_string());
    cache.insert(2u32, "Goblin".to_string());

    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert_eq!(line.sender, "Daisy");
    assert!(
        line.text.contains("Goblin")
            && line.text.contains("Lv. 53")
            && line.text.contains("Even Match"),
        "missing core check fields: {}",
        line.text
    );
    assert!(
        !line.text.to_ascii_lowercase().contains("defense")
            && !line.text.to_ascii_lowercase().contains("evasion"),
        "even/even should suppress def/eva phrase: {}",
        line.text
    );
}

#[test]
fn check_mob_decomposes_def_and_eva_offsets() {
    use std::collections::HashMap;
    let cache: HashMap<u32, String> = [(2u32, "Mob".to_string())].into_iter().collect();
    let cases: &[(u16, Option<&str>, Option<&str>)] = &[
        (170, Some("high defense"), Some("high evasion")),
        (171, None, Some("high evasion")),
        (172, Some("low defense"), Some("high evasion")),
        (173, Some("high defense"), None),
        (174, None, None),
        (175, Some("low defense"), None),
        (176, Some("high defense"), Some("low evasion")),
        (177, None, Some("low evasion")),
        (178, Some("low defense"), Some("low evasion")),
    ];
    for &(msg, def_phrase, eva_phrase) in cases {
        let data = check_message(msg, 25, 64 + 3, 1, 2);
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        for (label, phrase) in [("def", def_phrase), ("eva", eva_phrase)] {
            if let Some(p) = phrase {
                assert!(
                    line.text.contains(p),
                    "msg {msg} missing {label} phrase {p:?}: {}",
                    line.text
                );
            } else {
                let unwanted = match label {
                    "def" => "defense",
                    _ => "evasion",
                };
                assert!(
                    !line.text.to_ascii_lowercase().contains(unwanted),
                    "msg {msg} should not mention {unwanted}: {}",
                    line.text
                );
            }
        }
    }
}

#[test]
fn check_mob_renders_all_difficulty_tiers() {
    use std::collections::HashMap;
    let cache: HashMap<u32, String> = [(2u32, "Mob".to_string())].into_iter().collect();
    let tiers = [
        (0u32, "Too Weak"),
        (1, "Incredibly Easy Prey"),
        (2, "Easy Prey"),
        (3, "Decent Challenge"),
        (4, "Even Match"),
        (5, "Tough"),
        (6, "Very Tough"),
        (7, "Incredibly Tough"),
    ];
    for (tier, expected) in tiers {
        let data = check_message(174, 50, 64 + tier, 1, 2);
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert!(
            line.text.contains(expected),
            "tier {tier} expected {expected:?}: {}",
            line.text
        );
    }
}

#[test]
fn checkparam_renders_acc_att_pairs() {
    use std::collections::HashMap;
    let cache: HashMap<u32, String> = [(1u32, "Daisy".to_string())].into_iter().collect();
    for (msg, label) in [
        (712u16, "Main weapon"),
        (713, "Auxiliary weapon"),
        (714, "Ranged weapon"),
        (715, "Evasion"),
    ] {
        let data = check_message(msg, 321, 654, 1, 1);
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert!(
            line.text.contains("321") && line.text.contains("654"),
            "msg {msg}: missing numeric pair in {}",
            line.text
        );
        assert!(
            line.text.contains(label),
            "msg {msg}: missing label {label:?} in {}",
            line.text
        );
    }
}

#[test]
fn checkparam_aux_and_ranged_handle_unequipped_slot() {
    use std::collections::HashMap;
    let cache: HashMap<u32, String> = [(1u32, "Daisy".to_string())].into_iter().collect();
    for msg in [713u16, 714] {
        let data = check_message(msg, 0, 0, 1, 1);
        let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
        assert!(
            line.text.to_ascii_lowercase().contains("none equipped"),
            "msg {msg} with (0,0) should read \"none equipped\", got: {}",
            line.text
        );
    }
}

#[test]
fn check_impossible_to_gauge_uses_mob_placeholder() {
    use std::collections::HashMap;
    let data = check_message(249, 0, 0, 1, 2);
    let mut cache = HashMap::new();
    cache.insert(1u32, "Daisy".to_string());
    cache.insert(2u32, "King Behemoth".to_string());

    let line = decode_battle_message(&data, &cache, &HashMap::new(), true).expect("decoded");
    assert!(
        line.text.contains("King Behemoth")
            && line.text.to_ascii_lowercase().contains("impossible"),
        "{}",
        line.text
    );
}

// 0x009 body: UniqueNo u32 @0, ActIndex u16 @4, MesNo u16 @6, Attr u8 @8
// (vendor/server/src/map/packets/s2c/0x009_message.h GP_SERV_COMMAND_MESSAGE UniqueNo).
fn std_message_body(unique_no: u32, mes_no: u16) -> Vec<u8> {
    let mut data = vec![0u8; 12];
    data[0..4].copy_from_slice(&unique_no.to_le_bytes());
    data[6..8].copy_from_slice(&mes_no.to_le_bytes());
    data
}

#[test]
fn examine_message_renders_checker_name() {
    use std::collections::HashMap;
    let mut cache = HashMap::new();
    cache.insert(0xCAFEu32, "Daisy".to_string());
    let line = decode_std_message_examine(&std_message_body(0xCAFE, MSG_STD_EXAMINE), &cache)
        .expect("Examine decodes");
    assert_eq!(line.text, "Daisy examines you.");
    assert_eq!(line.sender, "Daisy");
}

#[test]
fn non_examine_std_message_is_not_synthesized() {
    let cache = std::collections::HashMap::new();
    assert!(decode_std_message_examine(&std_message_body(1, 88), &cache).is_none());
    assert!(decode_std_message_examine(&[0u8; 4], &cache).is_none());
}

#[test]
fn miscdata_status_icons_drops_placeholder_slots() {
    let mut data = vec![0u8; 4 + 64 + 128];
    data[0..2].copy_from_slice(&0x0009u16.to_le_bytes());

    data[4..6].copy_from_slice(&33u16.to_le_bytes());

    data[6..8].copy_from_slice(&0x00FFu16.to_le_bytes());

    data[8..10].copy_from_slice(&12u16.to_le_bytes());

    let (icons, expiries) = decode_miscdata_status_icons(&data).expect("decoded");
    assert_eq!(icons, vec![33, 12]);
    assert_eq!(expiries.len(), icons.len());
}

#[test]
fn status_icon_expiry_recovers_remaining_seconds() {
    let now_unix = 1_700_000_000u64;
    let vana_now = (now_unix - super::VANA_EPOCH_UNIX) as u32;
    let remaining = 300u32;
    let timestamp = vana_now.wrapping_add(remaining).wrapping_mul(60);
    let expiry = super::status_icon_expiry_unix(timestamp, now_unix);
    assert_eq!(expiry as u64, now_unix + remaining as u64);
    assert_eq!(super::status_icon_expiry_unix(0x7FFF_FFFF, now_unix), 0);
    assert_eq!(super::status_icon_expiry_unix(0, now_unix), 0);
}

#[test]
fn abil_recast_decodes_running_timers() {
    const TIMER_SECS: u16 = 120;
    let mut data = vec![0u8; 8 * 31 + 8];
    data[0..2].copy_from_slice(&TIMER_SECS.to_le_bytes());
    data[3] = 5; // TimerId (Provoke recast group)
    data[8..10].copy_from_slice(&0u16.to_le_bytes()); // second slot ready -> skipped
    data[11] = 7;
    let before = kuluu_snapshot::recast_now_unix();
    let recasts = super::decode_abil_recast(&data);
    let after = kuluu_snapshot::recast_now_unix();
    assert_eq!(recasts.len(), 1);
    assert_eq!(recasts[0].0, 5);
    // Expiry must be stamped with recast_now_unix() — the clock every consumer
    // (gate + display) reads.
    assert!(recasts[0].1 >= before + TIMER_SECS as u32);
    assert!(recasts[0].1 <= after + TIMER_SECS as u32);
}

#[test]
fn miscdata_status_icons_rejects_wrong_type() {
    let mut data = vec![0u8; 4 + 64 + 128];
    data[0..2].copy_from_slice(&0x0005u16.to_le_bytes());

    data[4..6].copy_from_slice(&33u16.to_le_bytes());
    assert!(decode_miscdata_status_icons(&data).is_none());
}

#[test]
fn miscdata_status_icons_truncated_returns_none() {
    let data = vec![0u8; 10];
    assert!(decode_miscdata_status_icons(&data).is_none());
}

#[test]
fn shop_list_decodes_rows_and_skips_zero_padding() {
    let mut data = vec![0u8; 4 + 12 * 3];
    data[0..2].copy_from_slice(&5u16.to_le_bytes());

    data[4..8].copy_from_slice(&100u32.to_le_bytes());
    data[8..10].copy_from_slice(&4096u16.to_le_bytes());
    data[10] = 0;

    data[16..20].copy_from_slice(&99999u32.to_le_bytes());
    data[20..22].copy_from_slice(&256u16.to_le_bytes());
    data[22] = 1;

    let page = decode_shop_list(&data).expect("decoded");
    assert_eq!(page.offset_index, 5);
    assert!(!page.last, "Flags bit 0 clear means more pages follow");
    assert_eq!(page.rows.len(), 2);
    assert_eq!(page.rows[0].price, 100);
    assert_eq!(page.rows[0].item_no, 4096);
    assert_eq!(
        page.rows[0].shop_index,
        5,
        "the row's own ShopIndex byte is ignored: the index is ShopItemOffsetIndex plus the row's position in the page"
    );
    assert_eq!(page.rows[1].item_no, 256);
    assert_eq!(page.rows[1].price, 99999);
    assert_eq!(page.rows[1].shop_index, 6);
}

/// Drives the real [`handle_sub_packet`] shop arms over one `ShopSession`, so
/// a multi-packet stock and the window's lifetime are exercised the way the
/// wire delivers them rather than through the decoders alone.
fn shop_session_events(packets: &[(u16, Vec<u8>)]) -> (ShopSession, Vec<AgentEvent>) {
    shop_session_events_from(
        ShopSession {
            last_talk_target: 0x0100_0007,
            ..Default::default()
        },
        packets,
    )
}

fn shop_session_events_from(
    mut shop: ShopSession,
    packets: &[(u16, Vec<u8>)],
) -> (ShopSession, Vec<AgentEvent>) {
    let (tx, mut rx) = broadcast::channel(64);
    for (opcode, body) in packets {
        handle_sub_packet(
            &framing::SubPacket {
                opcode: *opcode,
                sequence: 0,
                data: body,
            },
            &tx,
            &mut Vec::new(),
            &mut crate::event_dialog::CutsceneScope::default(),
            0,
            "Tester",
            &mut None,
            &mut std::collections::HashMap::new(),
            &mut std::collections::HashMap::new(),
            &mut std::collections::HashMap::new(),
            &mut std::collections::HashMap::new(),
            &mut std::collections::HashMap::new(),
            &mut 0,
            &mut Position::default(),
            &mut false,
            &mut NpcNameResolver::new(None),
            &mut EmoteTextResolver::new(None),
            &ffxi_dat::autotranslate_names::InstalledNames::default(),
            &mut treasure::SysMesResolver::new(None),
            &mut MesBasicResolver::new(None),
            &mut treasure::TreasurePool::default(),
            &mut false,
            &mut SelfMogState::default(),
            &mut shop,
            None,
        );
    }
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    (shop, out)
}

fn shop_open_body(expected: u16) -> Vec<u8> {
    let mut body = vec![0u8; 4];
    body[0..2].copy_from_slice(&expected.to_le_bytes());
    body
}

fn shop_list_body(offset: u16, last: bool, rows: &[(u16, u32)]) -> Vec<u8> {
    let mut body = vec![0u8; 4 + 12 * rows.len()];
    body[0..2].copy_from_slice(&offset.to_le_bytes());
    body[2] = if last { 0x89 } else { 0x00 };
    for (i, (item_no, price)) in rows.iter().enumerate() {
        let off = 4 + i * 12;
        body[off..off + 4].copy_from_slice(&price.to_le_bytes());
        body[off + 4..off + 6].copy_from_slice(&item_no.to_le_bytes());
    }
    body
}

/// vendor/server/src/map/lua/lua_base_entity.cpp sendMenu case 2 pushes
// The third shop row's item number; the value coincides with the event-VM
// work-gil const, which is unrelated.
const SHOP_ITEM_THIRD_NO_PINNED: u16 = 4098;

/// SHOP_OPEN then SHOP_LIST; vendor/server/src/map/packets/s2c/0x03c_shop_list.cpp
/// splits stock past 19 rows across pages, the last flagged 0x89.
#[test]
fn a_two_page_shop_opens_once_and_lists_every_row() {
    let (shop, events) = shop_session_events(&[
        (ffxi_proto::map::s2c::SHOP_OPEN, shop_open_body(3)),
        (
            ffxi_proto::map::s2c::SHOP_LIST,
            shop_list_body(0, false, &[(4096, 100), (4097, 200)]),
        ),
        (
            ffxi_proto::map::s2c::SHOP_LIST,
            shop_list_body(2, true, &[(SHOP_ITEM_THIRD_NO_PINNED, 300)]),
        ),
    ]);

    let open = shop.open.expect("window open");
    assert!(open.opened);
    assert!(open.complete);
    assert_eq!(open.expected_items, 3);
    assert_eq!(open.vendor_id, 0x0100_0007, "the NPC we talked to");
    assert_eq!(
        open.items
            .iter()
            .map(|i| (i.shop_index, i.item_no, i.price))
            .collect::<Vec<_>>(),
        vec![
            (0, 4096, 100),
            (1, 4097, 200),
            (2, SHOP_ITEM_THIRD_NO_PINNED, 300),
        ]
    );

    let updates = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ShopUpdated { .. }))
        .count();
    assert_eq!(updates, 3, "open plus one per page");
}

/// The shop table lives in GC_ZONE (research/XIClient GC_ZONE::gcShop), so
/// crossing a zoneline takes the vendor and their stock with it. The LOGIN
/// is decoded far enough to reach the shop teardown; a zero body still
/// names a zone.
#[test]
fn a_zone_change_closes_the_shop() {
    let mut login = vec![0u8; 0x100];
    login[0..4].copy_from_slice(&1u32.to_le_bytes());
    let (shop, events) = shop_session_events(&[
        (ffxi_proto::map::s2c::SHOP_OPEN, shop_open_body(1)),
        (
            ffxi_proto::map::s2c::SHOP_LIST,
            shop_list_body(0, true, &[(4096, 100)]),
        ),
        (ffxi_proto::map::s2c::LOGIN, login),
    ]);

    assert!(shop.open.is_none(), "the window did not survive the zone");
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::ShopClosed)),
        "close is announced so the viewer drops its window"
    );
}

#[test]
fn shop_pages_accumulate_at_their_offsets_instead_of_replacing() {
    let page = |offset: u16, last: bool, rows: &[(u16, u32)]| {
        let mut data = vec![0u8; 4 + 12 * rows.len()];
        data[0..2].copy_from_slice(&offset.to_le_bytes());
        data[2] = if last { 0x89 } else { 0x00 };
        for (i, (item_no, price)) in rows.iter().enumerate() {
            let off = 4 + i * 12;
            data[off..off + 4].copy_from_slice(&price.to_le_bytes());
            data[off + 4..off + 6].copy_from_slice(&item_no.to_le_bytes());
        }
        decode_shop_list(&data).expect("decoded")
    };

    let mut shop = ShopState::default();
    merge_shop_page(&mut shop, page(0, false, &[(4096, 100), (4097, 200)]));
    assert!(!shop.complete);
    merge_shop_page(
        &mut shop,
        page(2, true, &[(SHOP_ITEM_THIRD_NO_PINNED, 300)]),
    );

    assert!(shop.complete, "Flags 0x89 marks the final page");
    let listed: Vec<(u8, u16)> = shop
        .items
        .iter()
        .map(|i| (i.shop_index, i.item_no))
        .collect();
    assert_eq!(
        listed,
        vec![(0, 4096), (1, 4097), (2, SHOP_ITEM_THIRD_NO_PINNED),]
    );
}

#[test]
fn shop_rows_past_the_client_table_are_dropped() {
    let over = crate::state::SHOP_TABLE_CAPACITY as u16;
    let mut data = vec![0u8; 4 + 12];
    data[0..2].copy_from_slice(&over.to_le_bytes());
    data[4..8].copy_from_slice(&1u32.to_le_bytes());
    data[8..10].copy_from_slice(&4096u16.to_le_bytes());

    let mut shop = ShopState::default();
    merge_shop_page(&mut shop, decode_shop_list(&data).expect("decoded"));
    assert!(shop.items.is_empty());
}

#[test]
fn shop_sell_ignores_a_completed_sale_packet() {
    let mut body = vec![0u8; 12];
    body[0..4].copy_from_slice(&250u32.to_le_bytes());
    body[4] = 9;
    // research/XiPackets server 0x003D Type 1 = sale, not a price to confirm.
    body[5] = 1;
    assert_eq!(decode_shop_sell(&body), None);
    body[5] = 0;
    assert_eq!(decode_shop_sell(&body), Some((250, 9, 0)));
}

#[test]
fn shop_open_reports_the_expected_row_count() {
    let mut body = vec![0u8; 4];
    body[0..2].copy_from_slice(&37u16.to_le_bytes());
    assert_eq!(decode_shop_open(&body), Some(37));
    assert_eq!(decode_shop_open(&body[..1]), None);
}

#[test]
fn buffcancel_packet_layout_matches_server_struct() {
    let buf = build_subpacket_buffcancel(0x1234, 40);
    assert_eq!(buf.len(), 8);
    let hdr = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr & framing::SUBPACKET_OPCODE_MASK,
        codec::BUFFCANCEL,
        "opcode"
    );
    assert_eq!(
        (hdr >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        2,
        "size in words"
    );
    assert_eq!(
        u16::from_le_bytes(buf[4..6].try_into().unwrap()),
        40,
        "BuffNo"
    );
    assert_eq!(&buf[6..8], &[0u8; 2], "padding00");
}

#[test]
fn shop_buy_packet_layout_matches_server_struct() {
    let buf = build_subpacket_shop_buy(0xABCD, 5, 12, 3);
    assert_eq!(buf.len(), 16);
    let hdr = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::SHOP_BUY
    );
    assert_eq!(
        (hdr >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        4
    );
    assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), 5, "qty");
    assert_eq!(
        u16::from_le_bytes(buf[8..10].try_into().unwrap()),
        12,
        "shop_no"
    );
    assert_eq!(
        u16::from_le_bytes(buf[10..12].try_into().unwrap()),
        3,
        "shop_index zero-extended to u16"
    );
    assert_eq!(buf[12], 0, "PropertyItemIndex = LOC_INVENTORY");
    assert_eq!(&buf[13..16], &[0u8; 3], "padding");
}

#[test]
fn shop_sell_req_packet_layout_matches_server_struct() {
    let buf = build_subpacket_shop_sell_req(0xABCD, 7, 4096, 11);
    assert_eq!(buf.len(), 12, "header (4) + body (8)");
    let hdr = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::SHOP_SELL_REQ,
        "opcode in low 9 bits"
    );
    assert_eq!(
        (hdr >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        3,
        "size_words=3"
    );
    assert_eq!(
        u16::from_le_bytes([buf[2], buf[3]]),
        0xABCD,
        "sync echoed in header"
    );
    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        7,
        "ItemNum"
    );
    assert_eq!(
        u16::from_le_bytes(buf[8..10].try_into().unwrap()),
        4096,
        "ItemNo"
    );
    assert_eq!(buf[10], 11, "ItemIndex");
    assert_eq!(buf[11], 0, "padding");
}

#[test]
fn shop_sell_set_packet_layout_matches_server_struct() {
    let buf = build_subpacket_shop_sell_set(0xBEEF);
    assert_eq!(buf.len(), 8, "header (4) + body (4)");
    let hdr = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::SHOP_SELL_SET,
        "opcode in low 9 bits"
    );
    assert_eq!(
        (hdr >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        2,
        "size_words=2"
    );
    assert_eq!(
        u16::from_le_bytes([buf[2], buf[3]]),
        0xBEEF,
        "sync echoed in header"
    );
    assert_eq!(
        u16::from_le_bytes([buf[4], buf[5]]),
        1,
        "SellFlag must be 1 to pass the server validator"
    );
    assert_eq!(&buf[6..8], &[0u8; 2], "padding");
}

#[test]
fn shop_sell_decode_reads_price_slot_count() {
    let mut body = vec![0u8; 12];
    body[0..4].copy_from_slice(&1250u32.to_le_bytes());
    body[4] = 9;
    body[8..12].copy_from_slice(&12u32.to_le_bytes());
    assert_eq!(decode_shop_sell(&body), Some((1250, 9, 12)));
    assert_eq!(decode_shop_sell(&body[..11]), None, "short body rejected");
}

#[test]
fn event_decoders_reject_short_bodies() {
    assert!(decode_event_0x032(&[0u8; 15]).is_none());
    assert!(decode_event_0x033(&[0u8; 107]).is_none());
    assert!(decode_event_0x034(&[0u8; 47]).is_none());
}

#[test]
fn camp_packet_layout_matches_server_struct() {
    for (mode, want) in [
        (HealMode::Toggle, 0u32),
        (HealMode::On, 1),
        (HealMode::Off, 2),
    ] {
        let buf = build_subpacket_camp(0xBEEF, mode);
        assert_eq!(buf.len(), 8, "header (4) + body (4)");
        let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
        assert_eq!(
            hdr_word & framing::SUBPACKET_OPCODE_MASK,
            codec::CAMP,
            "opcode in low 9 bits"
        );
        assert_eq!(
            (hdr_word >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
            2,
            "size_words=2"
        );
        assert_eq!(
            u16::from_le_bytes([buf[2], buf[3]]),
            0xBEEF,
            "sync echoed in header"
        );
        assert_eq!(
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            want,
            "Mode LE for {mode:?}"
        );
    }
}

#[test]
fn gameok_packet_layout_matches_server_struct() {
    let buf = build_subpacket_gameok(0xBEEF);
    assert_eq!(buf.len(), 12, "header (4) + body (8)");
    let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr_word & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::GAMEOK,
        "opcode in low 9 bits"
    );
    assert_eq!(
        (hdr_word >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        3,
        "size_words=3"
    );
    assert_eq!(
        u16::from_le_bytes([buf[2], buf[3]]),
        0xBEEF,
        "sync echoed in header"
    );
    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        0,
        "ClientState must be 0 to pass the server validator"
    );
    assert_eq!(
        u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        0,
        "DebugClientFlg/unused must be 0 to pass the server validator"
    );
}

#[test]
fn equip_inspect_packet_layout_matches_server_struct() {
    let buf = build_subpacket_equip_inspect(0xABCD, 0x1234_5678, 42, 1);
    assert_eq!(buf.len(), 16, "header (4) + body (12)");

    let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr_word & framing::SUBPACKET_OPCODE_MASK,
        codec::EQUIP_INSPECT,
        "opcode in low 9 bits"
    );
    assert_eq!(
        (hdr_word >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        4,
        "size_words=4"
    );
    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        0x1234_5678,
        "UniqueNo LE"
    );
    assert_eq!(
        u32::from_le_bytes(buf[8..12].try_into().unwrap()),
        42,
        "ActIndex zero-extended to u32 LE"
    );
    assert_eq!(buf[12], 1, "Kind=CheckName");
    assert_eq!(&buf[13..16], &[0u8; 3], "padding00");
}

#[test]
fn bazaar_packet_layouts_match_server_structs() {
    // GP_CLI_COMMAND_BAZAAR_LIST (vendor/server/src/map/packets/c2s/0x105_bazaar_list.cpp).
    let list = build_subpacket_bazaar_list(0xABCD, 0x1234_5678, 42);
    assert_eq!(list.len(), 12, "header (4) + body (8)");
    let hdr = u16::from_le_bytes([list[0], list[1]]);
    assert_eq!(
        hdr & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::BAZAAR_LIST,
        "opcode BAZAAR_LIST"
    );
    assert_eq!(
        (hdr >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        3,
        "size_words=3"
    );
    assert_eq!(u16::from_le_bytes([list[2], list[3]]), 0xABCD, "sync");
    assert_eq!(
        u32::from_le_bytes(list[4..8].try_into().unwrap()),
        0x1234_5678,
        "UniqueNo LE"
    );
    assert_eq!(u16::from_le_bytes([list[8], list[9]]), 42, "ActIndex LE");
    assert_eq!(&list[10..12], &[0u8; 2], "padding00");

    // GP_CLI_COMMAND_BAZAAR_BUY (vendor/server/src/map/packets/c2s/0x106_bazaar_buy.cpp).
    let buy = build_subpacket_bazaar_buy(0xBEEF, 7, 12);
    assert_eq!(buy.len(), 12, "header (4) + body (8)");
    let hdr = u16::from_le_bytes([buy[0], buy[1]]);
    assert_eq!(
        hdr & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::BAZAAR_BUY,
        "opcode BAZAAR_BUY"
    );
    assert_eq!(buy[4], 7, "BazaarItemIndex");
    assert_eq!(&buy[5..8], &[0u8; 3], "padding00");
    assert_eq!(
        u32::from_le_bytes(buy[8..12].try_into().unwrap()),
        12,
        "BuyNum LE"
    );

    // GP_CLI_COMMAND_BAZAAR_EXIT (vendor/server/src/map/packets/c2s/0x104_bazaar_exit.cpp) is header-only.
    let exit = build_subpacket_bazaar_exit(0x0042);
    assert_eq!(exit.len(), 4, "header only");
    let hdr = u16::from_le_bytes([exit[0], exit[1]]);
    assert_eq!(
        hdr & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::BAZAAR_EXIT,
        "opcode BAZAAR_EXIT"
    );
    assert_eq!(
        (hdr >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        1,
        "size_words=1"
    );
    assert_eq!(u16::from_le_bytes([exit[2], exit[3]]), 0x0042, "sync");
}

#[test]
fn equip_set_packet_layout_matches_server_struct() {
    // GP_CLI_COMMAND_EQUIP_SET (vendor/server/src/map/packets/c2s/0x050_equip_set.h):
    // PropertyItemIndex(u8), EquipKind(u8), Category(u8).
    let buf = build_subpacket_equip_set(0xBEEF, 7, 10, 0);
    assert_eq!(buf.len(), 8, "header (4) + body (4)");
    let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr_word & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::EQUIP_SET,
        "opcode EQUIP_SET"
    );
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
    assert_eq!(buf[4], 7, "PropertyItemIndex = container_index (slotID)");
    assert_eq!(buf[5], 10, "EquipKind = equip_slot (Waist)");
    assert_eq!(buf[6], 0, "Category = container (LOC_INVENTORY)");
}

#[test]
fn item_stack_packet_layout_matches_server_struct() {
    // GP_CLI_COMMAND_ITEM_STACK (vendor/server/src/map/packets/c2s/0x03a_item_stack.h):
    // a single u32 Category (container id) after the 4-byte subpacket header.
    let buf = build_subpacket_item_stack(0xCAFE, 0, false).unwrap();
    assert_eq!(buf.len(), 8, "header (4) + Category u32 (4)");
    let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr_word & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::ITEM_STACK,
        "opcode ITEM_STACK"
    );
    assert_eq!(
        (hdr_word >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        2,
        "size_words = 2 (8 bytes)"
    );
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xCAFE, "sync");
    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        0,
        "Category = container (LOC_INVENTORY = 0)"
    );

    let buf = build_subpacket_item_stack(0, 1, false).unwrap();
    assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), 1);
}

#[test]
fn item_move_packet_layout_matches_server_struct() {
    // GP_CLI_COMMAND_ITEM_MOVE (vendor/server/src/map/packets/c2s/0x029_item_move.h):
    // ItemNum u32, Category1 u8, Category2 u8, ItemIndex1 u8, ItemIndex2 u8.
    let buf = build_subpacket_item_move(0xBEEF, 12, 0, 1, 7, None);
    assert_eq!(buf.len(), 12, "header (4) + ItemNum (4) + 4 bytes");
    let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr_word & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::ITEM_MOVE,
        "opcode ITEM_MOVE"
    );
    assert_eq!(
        (hdr_word >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        3,
        "size_words = 3 (12 bytes)"
    );
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        12,
        "ItemNum = quantity"
    );
    assert_eq!(buf[8], 0, "Category1 = from LOC_INVENTORY");
    assert_eq!(buf[9], 1, "Category2 = to LOC_MOGSAFE");
    assert_eq!(buf[10], 7, "ItemIndex1 = from slot");
    // The server treats ItemIndex2 < 82 as a stack-merge target; 0xFF asks
    // for a free slot (0x029_item_move.cpp process).
    assert_eq!(buf[11], 0xFF, "ItemIndex2 = auto slot");

    let buf = build_subpacket_item_move(0, 1, 2, 0, 3, Some(9));
    assert_eq!(buf[11], 9, "explicit ItemIndex2 = stack merge slot");
}

#[test]
fn item_dump_packet_layout_matches_server_struct() {
    // GP_CLI_COMMAND_ITEM_DUMP (vendor/server/src/map/packets/c2s/0x028_item_dump.h):
    // ItemNum u32, Category u8, ItemIndex u8; packet_system.cpp packetSizeRange
    // accepts exactly sizeof rounded up to four = 12 bytes.
    let buf = build_subpacket_item_dump(0xBEEF, 12, 0, 7);
    assert_eq!(buf.len(), 12, "header (4) + ItemNum (4) + 2 bytes + pad");
    let hdr_word = u16::from_le_bytes([buf[0], buf[1]]);
    assert_eq!(
        hdr_word & framing::SUBPACKET_OPCODE_MASK,
        ffxi_proto::map::c2s::ITEM_DUMP,
        "opcode ITEM_DUMP"
    );
    assert_eq!(
        (hdr_word >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
        3,
        "size_words = 3 (12 bytes)"
    );
    assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 0xBEEF, "sync");
    assert_eq!(
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
        12,
        "ItemNum = quantity"
    );
    assert_eq!(buf[8], 0, "Category = LOC_INVENTORY");
    assert_eq!(buf[9], 7, "ItemIndex = slot");
    assert_eq!(&buf[10..12], &[0, 0], "padding");
}

/// Pins every 0x04D encoding to LSB's PacketValidator rules
/// (vendor/server/src/map/packets/c2s/0x04d_pbx.cpp validate): a field the
/// validator mustEquals is hard-coded, unused numerics are -1, and
/// Result/ResParam1-3 are zero — any drift is a silent server-side drop.
#[test]
fn pbx_packet_layout_matches_lsb_validator() {
    use crate::state::{DeliveryBoxNo, DeliveryBoxOp as Op};
    use ffxi_proto::map::pbx::{boxno, command};

    let fields = |op: &Op| {
        let buf = build_subpacket_pbx(0xBEEF, op);
        assert_eq!(buf.len(), 32, "GP_CLI_COMMAND_PBX is 32 bytes");
        let hdr = u16::from_le_bytes(buf[0..2].try_into().unwrap());
        assert_eq!(
            hdr & framing::SUBPACKET_OPCODE_MASK,
            ffxi_proto::map::c2s::PBX,
            "opcode PBX"
        );
        assert_eq!(
            (hdr >> framing::SUBPACKET_SIZE_WORDS_SHIFT) & framing::SUBPACKET_SIZE_WORDS_MASK,
            8,
            "size_words = 8"
        );
        assert_eq!(&buf[12..16], &[0, 0, 0, 0], "Result/ResParam1-3 zero");
        (
            buf[4],
            buf[5] as i8,
            buf[6] as i8,
            buf[7] as i8,
            i32::from_le_bytes(buf[8..12].try_into().unwrap()),
            buf[16..32].to_vec(),
        )
    };

    let no_name = vec![0u8; 16];

    let (cmd, b, pw, iw, st, name) = fields(&Op::PostOpen);
    assert_eq!(
        (cmd, b, pw, iw, st),
        (command::POST_OPEN, boxno::NONE, -1, -1, -1)
    );
    assert_eq!(name, no_name);

    let (cmd, b, pw, iw, st, _) = fields(&Op::Work {
        box_no: DeliveryBoxNo::Incoming,
    });
    assert_eq!(
        (cmd, b, pw, iw, st),
        (command::WORK, boxno::INCOMING, -1, -1, -1)
    );

    let (cmd, b, pw, iw, st, _) = fields(&Op::Check {
        box_no: DeliveryBoxNo::Outgoing,
    });
    assert_eq!(
        (cmd, b, pw, iw, st),
        (command::CHECK, boxno::OUTGOING, -1, -1, -1)
    );

    // Recv: BoxNo pinned Incoming, ItemWorkNo pinned 1.
    let (cmd, b, pw, iw, st, _) = fields(&Op::Recv { slot: 3 });
    assert_eq!(
        (cmd, b, pw, iw, st),
        (command::RECV, boxno::INCOMING, 3, 1, -1)
    );

    let (cmd, b, pw, iw, st, name) = fields(&Op::Set {
        slot: 2,
        inventory_slot: 11,
        quantity: 12,
        recipient: "Atti".into(),
    });
    assert_eq!(
        (cmd, b, pw, iw, st),
        (command::SET, boxno::OUTGOING, 2, 11, 12)
    );
    assert_eq!(&name[..5], b"Atti\0", "NUL-terminated TargetName");

    let (cmd, b, pw, iw, st, _) = fields(&Op::Send { slot: 2 });
    assert_eq!(
        (cmd, b, pw, iw, st),
        (command::SEND, boxno::OUTGOING, 2, -1, -1)
    );

    let (cmd, b, pw, ..) = fields(&Op::Cancel { slot: 4 });
    assert_eq!((cmd, b, pw), (command::CANCEL, boxno::OUTGOING, 4));

    let (cmd, b, pw, ..) = fields(&Op::Accept { slot: 5 });
    assert_eq!((cmd, b, pw), (command::ACCEPT, boxno::INCOMING, 5));

    let (cmd, b, pw, ..) = fields(&Op::Reject { slot: 6 });
    assert_eq!((cmd, b, pw), (command::REJECT, boxno::INCOMING, 6));

    let (cmd, b, pw, ..) = fields(&Op::Get {
        box_no: DeliveryBoxNo::Outgoing,
        slot: 7,
    });
    assert_eq!((cmd, b, pw), (command::GET, boxno::OUTGOING, 7));

    let (cmd, b, pw, ..) = fields(&Op::Clear {
        box_no: DeliveryBoxNo::Incoming,
        slot: 0,
    });
    assert_eq!((cmd, b, pw), (command::CLEAR, boxno::INCOMING, 0));

    let (cmd, b, pw, iw, st, name) = fields(&Op::Query {
        recipient: "Nicotine".into(),
    });
    assert_eq!(
        (cmd, b, pw, iw, st),
        (command::QUERY, boxno::NONE, -1, -1, -1)
    );
    assert_eq!(&name[..9], b"Nicotine\0");

    let (cmd, b, ..) = fields(&Op::Confirm);
    assert_eq!((cmd, b), (command::CONFIRM, boxno::NONE));

    let (cmd, b, ..) = fields(&Op::DeliOpen);
    assert_eq!((cmd, b), (command::DELI_OPEN, boxno::NONE));

    // PostClose: BoxNo pinned None regardless of which box is closing.
    let (cmd, b, ..) = fields(&Op::PostClose {
        box_no: DeliveryBoxNo::Outgoing,
    });
    assert_eq!((cmd, b), (command::POST_CLOSE, boxno::NONE));

    // A 15-char name (FFXI max) still leaves its NUL terminator in place.
    let (.., name) = fields(&Op::Query {
        recipient: "Abcdefghijklmnop".into(),
    });
    assert_eq!(name[15], 0, "TargetName[15] stays NUL");
}

#[test]
fn item_stack_throttle_is_per_container() {
    use std::time::{Duration, Instant};
    let mut last = std::collections::HashMap::new();
    let t0 = Instant::now();
    assert!(item_stack_allowed(&mut last, 0, t0), "first send passes");
    assert!(
        !item_stack_allowed(&mut last, 0, t0 + Duration::from_millis(500)),
        "second within the interval is throttled"
    );
    assert!(
        item_stack_allowed(&mut last, 1, t0 + Duration::from_millis(500)),
        "a different container is independent"
    );
    assert!(
        item_stack_allowed(&mut last, 0, t0 + ITEM_STACK_MIN_INTERVAL),
        "passes again once the interval has elapsed"
    );
}

#[test]
fn item_stack_interval_clears_server_window() {
    // LSB trips at faster than 1/sec; the client margin must stay >= 1s.
    assert!(ITEM_STACK_MIN_INTERVAL >= std::time::Duration::from_secs(1));
}

#[test]
fn equip_set_unequip_uses_zero_slot_index() {
    // LSB unequips a slot when PropertyItemIndex (slotID) is 0, regardless of
    // container: vendor/server/src/map/utils/charutils.cpp EquipItem
    // ("slotID of zero = unequip"). The re-select-to-unequip path encodes this.
    let buf = build_subpacket_equip_set(0, 0, 10, 0);
    assert_eq!(buf[4], 0, "slotID 0 = unequip");
    assert_eq!(buf[5], 10, "still targets the equip slot being cleared");
}

#[test]
fn item_use_with_nonzero_category_writes_full_u32() {
    let buf = build_subpacket_item_use(0, 0, 0, 8, 0);
    assert_eq!(
        u32::from_le_bytes(buf[16..20].try_into().unwrap()),
        8,
        "Category u32 LE"
    );
}

#[test]
fn chat_std_decoder_maps_each_channel() {
    use ffxi_proto::map::chat_kind as k;
    let cases = [
        (k::SAY, ChatChannel::Say),
        (k::SHOUT, ChatChannel::Shout),
        (k::TELL, ChatChannel::Tell),
        (k::PARTY, ChatChannel::Party),
        (k::LINKSHELL, ChatChannel::Linkshell),
        (k::YELL, ChatChannel::Yell),
        (k::SYSTEM_1, ChatChannel::System),
        (k::SYSTEM_3, ChatChannel::System),
        (k::EMOTION, ChatChannel::Emote),
        (k::NS_PARTY, ChatChannel::Party),
        (k::LINKSHELL2, ChatChannel::Linkshell),
        (200u8, ChatChannel::Other),
    ];
    for (kind, expected) in cases {
        let mut body = vec![0u8; 4 + 15];
        body[0] = kind;
        body.extend_from_slice(b"Hello there");
        body.push(0);
        let line = decode_chat_std(&body, &ffxi_proto::autotranslate::FrozenNames)
            .expect("decoder accepts well-formed body");
        assert_eq!(line.channel, expected, "kind {kind} → {expected:?}");
        assert_eq!(line.text, "Hello there");
    }
}

#[test]
fn chat_std_decoder_extracts_sender_and_message() {
    let mut body = vec![0u8; 4 + 15];
    body[0] = ffxi_proto::map::chat_kind::SAY;
    body[4..10].copy_from_slice(b"Sylvie");

    body.extend_from_slice(b"hi all");
    body.push(0);
    let line = decode_chat_std(&body, &ffxi_proto::autotranslate::FrozenNames).unwrap();
    assert_eq!(line.sender, "Sylvie");
    assert_eq!(line.text, "hi all");
    assert_eq!(line.channel, ChatChannel::Say);
}

#[test]
fn chat_std_decoder_rejects_truncated_body() {
    assert!(decode_chat_std(&[0u8; 5], &ffxi_proto::autotranslate::FrozenNames).is_none());
    assert!(decode_chat_std(&[0u8; 18], &ffxi_proto::autotranslate::FrozenNames).is_none());
    assert!(decode_chat_std(&[0u8; 19], &ffxi_proto::autotranslate::FrozenNames).is_some());
}

fn chat_std_body(kind: u8, sender: &str, message: &str) -> Vec<u8> {
    let mut body = vec![0u8; 4 + 15];
    body[0] = kind;
    let s = sender.as_bytes();
    body[4..4 + s.len()].copy_from_slice(s);
    body.extend_from_slice(message.as_bytes());
    body.push(0);
    body
}

#[test]
fn ns_chat_kind_blanks_sender() {
    // NS_SAY carries a sender name in the packet, but retail shows the text
    // unattributed — the decoder must drop it so the HUD omits the prefix.
    let body = chat_std_body(
        ffxi_proto::map::chat_kind::NS_SAY,
        "Oldman",
        "You can set this.",
    );
    let line = decode_chat_std(&body, &ffxi_proto::autotranslate::FrozenNames).unwrap();
    assert_eq!(line.sender, "");
    assert_eq!(line.text, "You can set this.");
    assert_eq!(line.channel, ChatChannel::Say);
    // A plain SAY from the same NPC keeps its attribution.
    let say = chat_std_body(ffxi_proto::map::chat_kind::SAY, "Oldman", "hi");
    assert_eq!(
        decode_chat_std(&say, &ffxi_proto::autotranslate::FrozenNames)
            .unwrap()
            .sender,
        "Oldman"
    );
}

#[test]
fn custom_menu_decodes_title_and_options() {
    let body = chat_std_body(
        MESSAGE_GMPROMPT,
        CUSTOM_MENU_SENDER,
        r#""Set this as your current home point?""Yes""No""#,
    );
    let (title, options) = decode_custom_menu(&body, &ffxi_proto::autotranslate::FrozenNames)
        .expect("customMenu decodes");
    assert_eq!(title, "Set this as your current home point?");
    assert_eq!(options, vec!["Yes".to_string(), "No".to_string()]);
}

#[test]
fn custom_menu_gated_on_type_and_sender() {
    // Right sender, wrong type (a plain say) is an ordinary chat line.
    let say = chat_std_body(0, CUSTOM_MENU_SENDER, r#""Title""Yes""#);
    assert!(decode_custom_menu(&say, &ffxi_proto::autotranslate::FrozenNames).is_none());
    // Right type, ordinary sender is not a menu either.
    let other = chat_std_body(MESSAGE_GMPROMPT, "Oldman", r#""Title""Yes""#);
    assert!(decode_custom_menu(&other, &ffxi_proto::autotranslate::FrozenNames).is_none());
}

// Mirror the server's HandleCustomMenu extraction (luautils.cpp NA path):
// find `: Result (`, take the tail, drop the trailing `)`.
fn server_extract_result(selection: &str) -> Option<String> {
    let pos = selection.find(CUSTOM_MENU_RESULT_MARKER)?;
    let mut tail = selection[pos + CUSTOM_MENU_RESULT_MARKER.len()..].to_string();
    tail.pop(); // trailing ')'
    Some(tail)
}

#[test]
fn custom_menu_reply_round_trips_through_server_parser() {
    let reply = custom_menu_reply("Zeid", "Set this as your current home point?", Some("Yes"));
    assert_eq!(server_extract_result(&reply).as_deref(), Some("Yes"));

    // Cancel takes the onCancelled branch: the "Canceled." marker is present.
    let cancel = custom_menu_reply("Zeid", "Set this as your current home point?", None);
    assert!(cancel.contains(&format!("{CUSTOM_MENU_RESULT_MARKER}{CUSTOM_MENU_CANCEL})")));
    assert_eq!(
        server_extract_result(&cancel).as_deref(),
        Some(CUSTOM_MENU_CANCEL)
    );
}

#[test]
fn system_message_substitutes_seconds() {
    let raw = "Executing logout in <seconds> seconds. Cancel healing to remain logged in.";
    let s = substitute_system_placeholders(raw, 30, 0);
    assert_eq!(
        s,
        "Executing logout in 30 seconds. Cancel healing to remain logged in.",
    );
}

#[test]
fn system_message_unknown_id_falls_through() {
    let line = build_system_message_line(decode::SystemMessage {
        para: 7,
        para2: 42,
        message_id: 0xBEEF,
    });
    assert!(line.text.contains("msg #48879"), "{}", line.text);
    assert!(line.text.contains("para=7,42"), "{}", line.text);
    assert!(matches!(line.channel, ChatChannel::System));
}

#[test]
fn system_message_executing_logout_full_line() {
    let line = build_system_message_line(decode::SystemMessage {
        para: 25,
        para2: 0,
        message_id: 7,
    });
    assert!(
        line.text.starts_with("Executing logout in 25 seconds."),
        "{}",
        line.text
    );
    assert!(matches!(line.channel, ChatChannel::System));
}

#[test]
pub(super) fn bootstrap_acceptance_contract() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for scenario in [
                BootstrapReply::Silent,
                BootstrapReply::MalformedLogin,
                BootstrapReply::OtherLoginWithSelfPosition,
                BootstrapReply::SelfPositionOnly,
                BootstrapReply::SelfLogin,
                BootstrapReply::DelayedSelfLogin,
            ] {
                bootstrap_scenario(scenario).await;
            }
        });
}

const FIXTURE_PLAYER_PINNED: u32 = 17_455_719;
const FIXTURE_SEED: [u8; 20] = [0; 20];
const FIXTURE_POSITION: [f32; 3] = [2.15, -2.1, 3.25];
const BOOTSTRAP_DATAGRAMS: usize = 2;
const FIXTURE_CASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

fn fixture_packet(opcode: u16, body: &[u8]) -> Vec<u8> {
    let words = framing::subpacket_size_words(body.len() + framing::SUBPACKET_HEADER_SIZE);
    let mut out = build_subpacket_header(opcode, words, 1).to_vec();
    out.extend(body);
    out
}

/// A self 0x00A / 0x00D body (vendor/server/src/map/packets/s2c/0x00a_login.cpp):
/// unique_no, send flags, and the three position floats in wire order
/// (x, height, north).
fn login_fixture_body(player: u32, position: [f32; 3]) -> Vec<u8> {
    const LOGIN_BODY_LEN: usize = 48;
    const SEND_FLAGS: usize = 6;
    const POSITION_X: usize = 8;
    const POSITION_HEIGHT: usize = 12;
    const POSITION_NORTH: usize = 16;
    let mut body = vec![0; LOGIN_BODY_LEN];
    body[..4].copy_from_slice(&player.to_le_bytes());
    body[SEND_FLAGS] = 1;
    for (offset, value) in [POSITION_X, POSITION_HEIGHT, POSITION_NORTH]
        .into_iter()
        .zip(position)
    {
        body[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    body
}

fn fixture_config() -> Config {
    Config {
        server: "127.0.0.1".into(),
        map_host_override: None,
        auth_port: 0,
        data_port: 0,
        view_port: 0,
        user: "bootstrap-fixture".into(),
        password: String::new(),
        char_selection: CharSelection::Id(FIXTURE_PLAYER_PINNED),
        initial_state: None,
        playonline_session: None,
        user_driven_events: true,
        dat_root: None,
    }
}

fn fixture_bootstrap() -> BootstrapArgs<'static> {
    BootstrapArgs {
        char_id: FIXTURE_PLAYER_PINNED,
        char_name: "Bootstrap",
        account_name: "bootstrap-fixture",
        ticket: [0; 16],
        version: 0,
        platform: *b"WIN\0",
        cli_lang: 0,
    }
}

#[derive(Debug, Clone, Copy)]
enum BootstrapReply {
    Silent,
    MalformedLogin,
    OtherLoginWithSelfPosition,
    SelfPositionOnly,
    SelfLogin,
    DelayedSelfLogin,
}

/// Drives one bootstrap scenario against a fake map server. The bootstrap
/// does not gate on the s2c 0x00A LOGIN (vendor/server/src/map/packets/s2c/0x00a_login.cpp);
/// it is tracked for voyage timing and processed opportunistically, so a
/// requested disconnect completes cleanly regardless of the scenario, and the
/// fake server's cooperation drives its behavior, not the acceptance. The
/// session claims InZone / blowfish-Accepted and seeds a position only when
/// a self position seed lands, so acceptance is pinned by the saw_in_zone /
/// saw_accepted / saw_seed assertions below. Both map clients bind an
/// ephemeral local port so tests do not inherit the FFXI_MAP_LOCAL_PORT
/// Docker/WSL2 DNAT pin, which would collide on the pinned port. Post-
/// bootstrap packets (GROUP_LIST_REQ, CLISTATUS) are sent unconditionally, so
/// every scenario sends more than the two bootstraps.
async fn bootstrap_scenario(scenario: BootstrapReply) {
    use ffxi_proto::map::s2c;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Duration;

    const PLAYER: u32 = FIXTURE_PLAYER_PINNED;
    const SEED: [u8; 20] = FIXTURE_SEED;
    const POSITION: [f32; 3] = FIXTURE_POSITION;
    const EXPECTED_BOOTSTRAPS: usize = BOOTSTRAP_DATAGRAMS;
    const CASE_TIMEOUT: Duration = FIXTURE_CASE_TIMEOUT;
    const DELAYED_LOGIN: Duration = Duration::from_millis(900);

    let packet = fixture_packet;
    let server_cooperates = matches!(
        scenario,
        BootstrapReply::SelfLogin | BootstrapReply::DelayedSelfLogin
    );
    let position_seeded = matches!(
        scenario,
        BootstrapReply::OtherLoginWithSelfPosition
            | BootstrapReply::SelfPositionOnly
            | BootstrapReply::SelfLogin
            | BootstrapReply::DelayedSelfLogin
    );
    let self_body = login_fixture_body(PLAYER, POSITION);
    let login = packet(s2c::LOGIN, &self_body);
    let self_position = packet(s2c::CHAR_PC, &self_body);
    let initial = match scenario {
        BootstrapReply::Silent => None,
        BootstrapReply::MalformedLogin => Some(packet(s2c::LOGIN, &[0; 4])),
        BootstrapReply::OtherLoginWithSelfPosition => {
            let mut wrong = self_body.clone();
            wrong[..4].copy_from_slice(&(PLAYER + 1).to_le_bytes());
            let mut payload = packet(s2c::LOGIN, &wrong);
            payload.extend(&self_position);
            Some(payload)
        }
        BootstrapReply::SelfPositionOnly | BootstrapReply::DelayedSelfLogin => Some(self_position),
        BootstrapReply::SelfLogin => Some(login.clone()),
    };
    let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let outgoing = Arc::new(AtomicUsize::new(0));
    let observed = outgoing.clone();
    let fake = tokio::spawn(async move {
        let mut bytes = vec![0; ffxi_proto::map::MAX_DATAGRAM];
        let mut peer = None;
        loop {
            let (size, client) = server.recv_from(&mut bytes).await.unwrap();
            let count = observed.fetch_add(1, Ordering::SeqCst) + 1;
            if count <= EXPECTED_BOOTSTRAPS {
                assert_eq!(size, map_client::BOOTSTRAP_DATAGRAM_SIZE);
                let payload = &bytes[framing::FFXI_HEADER_SIZE
                    ..framing::FFXI_HEADER_SIZE + map_client::GP_CLI_LOGIN_SIZE];
                let sent = framing::walk_sub_packets(payload).next().unwrap().unwrap();
                assert_eq!(sent.opcode, ffxi_proto::map::c2s::LOGIN);
            }
            if count == 1 {
                peer = Some(
                    MapClient::connect_with_local(client, SEED, "0.0.0.0:0")
                        .await
                        .unwrap(),
                );
                if let Some(ref payload) = initial {
                    peer.as_ref()
                        .unwrap()
                        .send_encrypted(payload, 1, 0)
                        .await
                        .unwrap();
                }
                if matches!(scenario, BootstrapReply::DelayedSelfLogin) {
                    tokio::time::sleep(DELAYED_LOGIN).await;
                    peer.as_ref()
                        .unwrap()
                        .send_encrypted(&login, 2, 0)
                        .await
                        .unwrap();
                }
            } else if !matches!(scenario, BootstrapReply::Silent) {
                if server_cooperates {
                    peer.as_ref()
                        .unwrap()
                        .send_encrypted(&[], count as u16 + 1, 0)
                        .await
                        .unwrap();
                } else {
                    server.send_to(&[0], client).await.unwrap();
                }
            }
        }
    });
    let mut map = MapClient::connect_with_local(address, SEED, "0.0.0.0:0")
        .await
        .unwrap();
    let cfg = fixture_config();
    let auth = crate::auth_client::AuthSession {
        account_id: 1,
        session_hash: [0; 16],
        auth_code: crate::auth_client::LobbyAuthCode::NONE,
    };
    let bootstrap = fixture_bootstrap();
    let (commands, mut command_rx) = mpsc::channel(1);
    commands.send(AgentCommand::Disconnect).await.unwrap();
    let (events, mut event_rx) = broadcast::channel(256);
    let outcome = tokio::time::timeout(
        CASE_TIMEOUT,
        run_map_session(
            &cfg,
            &auth,
            &bootstrap,
            &mut map,
            None,
            1,
            None,
            &mut command_rx,
            &events,
            None,
        ),
    )
    .await
    .expect("bootstrap must complete within its bounded deadline");
    tokio::task::yield_now().await;
    fake.abort();
    assert!(
        fake.await.unwrap_err().is_cancelled(),
        "fake map server panicked"
    );
    assert!(outcome.is_ok(), "{scenario:?}: {outcome:?}");
    let mut saw_in_zone = false;
    let mut saw_accepted = false;
    let mut saw_seed = false;
    while let Ok(event) = event_rx.try_recv() {
        match event {
            AgentEvent::StageChanged {
                stage: Stage::InZone,
            } => saw_in_zone = true,
            AgentEvent::Diagnostics { diagnostics } => {
                saw_accepted |= diagnostics.blowfish_status == Some(BlowfishStatus::Accepted);
            }
            AgentEvent::EntityUpserted {
                entity,
                pos_present: true,
            } if entity.id == PLAYER => {
                saw_seed |= entity.pos
                    == Vec3 {
                        x: POSITION[0],
                        y: POSITION[2],
                        z: POSITION[1],
                    };
            }
            _ => {}
        }
    }
    assert_eq!(saw_in_zone, position_seeded, "{scenario:?}");
    assert_eq!(saw_accepted, position_seeded, "{scenario:?}");
    assert_eq!(saw_seed, position_seeded, "{scenario:?}");
    assert!(
        outgoing.load(Ordering::SeqCst) > EXPECTED_BOOTSTRAPS,
        "{scenario:?}: post-bootstrap packets must follow the two bootstraps"
    );
}

#[test]
pub(super) fn bootstrap_enterzone_contract() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(enterzone_in_gameok_reply());
}

/// LSB answers 0x00C GAMEOK with 0x008 ENTERZONE (vendor/server/src/map/packets/
/// c2s/0x00c_gameok.cpp GP_CLI_COMMAND_GAMEOK::process), which the bootstrap's
/// post-send drain consumes before the keepalive loop exists. The loop must
/// still send exactly one 0x011 ZONE_TRANSITION and then 0x01A SendResRdy,
/// the request LSB spawns the Mog House Moogle on (SpawnConditionalNPCs).
/// Both map clients bind an ephemeral local port so the test does not inherit
/// the FFXI_MAP_LOCAL_PORT Docker/WSL2 DNAT pin, which would make both sides
/// of the loopback bind the same port.
async fn enterzone_in_gameok_reply() {
    use ffxi_proto::map::{c2s, s2c};
    use std::sync::{Arc, Mutex};

    const ACTION_ID: std::ops::Range<usize> = 6..8;
    let send_res_rdy = crate::state::ActionKind::SendResRdy.action_id();

    let login = fixture_packet(
        s2c::LOGIN,
        &login_fixture_body(FIXTURE_PLAYER_PINNED, FIXTURE_POSITION),
    );
    let enterzone = fixture_packet(s2c::ENTERZONE, &[0; 4]);
    let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let address = server.local_addr().unwrap();
    let sent: Arc<Mutex<Vec<(u16, u16)>>> = Arc::new(Mutex::new(Vec::new()));
    let observed = sent.clone();
    let (commands, mut command_rx) = mpsc::channel(1);
    let release = commands.clone();
    let fake = tokio::spawn(async move {
        let mut bytes = vec![0; ffxi_proto::map::MAX_DATAGRAM];
        let mut peer: Option<MapClient> = None;
        let mut count = 0usize;
        let mut released = false;
        loop {
            let (size, client) = server.recv_from(&mut bytes).await.unwrap();
            count += 1;
            if count == 1 {
                let connected = MapClient::connect_with_local(client, FIXTURE_SEED, "0.0.0.0:0")
                    .await
                    .unwrap();
                connected.send_encrypted(&login, 1, 0).await.unwrap();
                peer = Some(connected);
            }
            if count <= BOOTSTRAP_DATAGRAMS {
                continue;
            }
            let peer = peer.as_ref().unwrap();
            let datagram = peer
                .decode_datagram(bytes[..size].to_vec(), client)
                .unwrap();
            for sub in framing::walk_sub_packets(&datagram[framing::FFXI_HEADER_SIZE..]).flatten() {
                let action = if sub.opcode == c2s::ACTION {
                    u16::from_le_bytes(sub.data[ACTION_ID].try_into().unwrap())
                } else {
                    0
                };
                observed.lock().unwrap().push((sub.opcode, action));
            }
            let stamp = (count - 1) as u16;
            let reply: &[u8] = if count == BOOTSTRAP_DATAGRAMS + 1 {
                &enterzone
            } else {
                &[]
            };
            peer.send_encrypted(reply, stamp, 0).await.unwrap();
            let done = observed
                .lock()
                .unwrap()
                .iter()
                .any(|&(opcode, action)| opcode == c2s::ACTION && action == send_res_rdy);
            if done && !released {
                released = true;
                let _ = release.send(AgentCommand::Disconnect).await;
            }
        }
    });
    let mut map = MapClient::connect_with_local(address, FIXTURE_SEED, "0.0.0.0:0")
        .await
        .unwrap();
    let cfg = fixture_config();
    let auth = crate::auth_client::AuthSession {
        account_id: 1,
        session_hash: [0; 16],
        auth_code: crate::auth_client::LobbyAuthCode::NONE,
    };
    let bootstrap = fixture_bootstrap();
    let (events, _event_rx) = broadcast::channel(256);
    let outcome = tokio::time::timeout(
        FIXTURE_CASE_TIMEOUT,
        run_map_session(
            &cfg,
            &auth,
            &bootstrap,
            &mut map,
            None,
            1,
            None,
            &mut command_rx,
            &events,
            None,
        ),
    )
    .await
    .expect("a bootstrap 0x008 must still lead to 0x011 and SendResRdy");
    fake.abort();
    assert!(
        fake.await.unwrap_err().is_cancelled(),
        "fake map server panicked"
    );
    assert!(outcome.is_ok(), "{outcome:?}");
    let sent = sent.lock().unwrap().clone();
    let gameok = sent
        .iter()
        .position(|&(opcode, _)| opcode == c2s::GAMEOK)
        .expect("GAMEOK");
    let transitions: Vec<usize> = sent
        .iter()
        .enumerate()
        .filter(|(_, &(opcode, _))| opcode == c2s::ZONE_TRANSITION)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(transitions.len(), 1, "one 0x011 per zone-in: {sent:?}");
    let res_rdy = sent
        .iter()
        .position(|&(opcode, action)| opcode == c2s::ACTION && action == send_res_rdy)
        .expect("SendResRdy");
    assert!(
        gameok < transitions[0] && transitions[0] < res_rdy,
        "GAMEOK, then ZONE_TRANSITION, then SendResRdy: {sent:?}"
    );
}

#[test]
fn party_moghouse_flag_never_announces_a_mog_house_transition() {
    const MOGHOUSE_FLG_ATTR: usize = 21;
    const MOGHOUSE_FLG_LIST: usize = 23;
    const ATTR_LEN: usize = 32;
    const LIST_LEN: usize = 52;

    for (opcode, len, flag_at) in [
        (
            ffxi_proto::map::s2c::GROUP_ATTR,
            ATTR_LEN,
            MOGHOUSE_FLG_ATTR,
        ),
        (
            ffxi_proto::map::s2c::GROUP_LIST,
            LIST_LEN,
            MOGHOUSE_FLG_LIST,
        ),
    ] {
        let mut body = vec![0u8; len];
        body[flag_at] = 1;

        let events = sub_packet_events(opcode, &body);
        assert!(
            events.iter().any(
                |e| matches!(e, AgentEvent::PartyMemberUpdated { member } if member.in_mog_house)
            ),
            "{opcode:#05X}: the wire flag still reaches the party member"
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ChatLine { .. })),
            "{opcode:#05X}: only the 0x00A myroom cluster may announce a Mog House transition"
        );
    }
}

#[test]
fn mog_transition_lines_are_devhud_only() {
    let (tx, mut rx) = broadcast::channel(4);
    let mut was = false;
    note_mog_transition(true, &mut was, &tx);
    note_mog_transition(true, &mut was, &tx);
    note_mog_transition(false, &mut was, &tx);

    let mut texts = Vec::new();
    while let Ok(AgentEvent::ChatLine { line }) = rx.try_recv() {
        assert_eq!(line.channel, ChatChannel::Debug);
        assert!(line.text.is_ascii(), "{}", line.text);
        texts.push(line.text);
    }
    assert_eq!(texts.len(), 2, "an unchanged state announces nothing");
}

#[test]
fn bag_capacity_line_is_devhud_only() {
    const MAIN_BAG_WIRE_CAP: u8 = 31;
    // vendor/server/src/map/packets/s2c/0x01c_item_max.cpp GP_SERV_COMMAND_ITEM_MAX
    const ITEM_MAX_BODY_LEN: usize = 96;

    let mut body = vec![0u8; ITEM_MAX_BODY_LEN];
    body[0] = MAIN_BAG_WIRE_CAP;

    let lines: Vec<_> = sub_packet_events(ffxi_proto::map::s2c::ITEM_MAX, &body)
        .into_iter()
        .filter_map(|e| match e {
            AgentEvent::ChatLine { line } => Some(line),
            _ => None,
        })
        .collect();

    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].channel, ChatChannel::Debug);
    assert_eq!(
        lines[0].text,
        format!("Bag capacities: c0={}", MAIN_BAG_WIRE_CAP - 1)
    );
    assert!(lines[0].text.is_ascii(), "{}", lines[0].text);
}

/// A frame with a speaker resolves to that entity's name; a speakerless frame
/// gets a blank header (retail's no-speaker lines); an unresolvable index
/// prints retail's missing-entity marker rather than guessing the trigger NPC.
#[test]
fn speaker_attribution_resolves_the_frame_speaker_not_the_trigger() {
    let mut target_cache: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
    target_cache.insert(7u16, 0x010E_60D5u32);
    let mut name_cache: std::collections::HashMap<u32, String> = std::collections::HashMap::new();
    name_cache.insert(0x010E_60D5u32, "Curilla".to_string());

    let mut d = crate::state::DialogState {
        npc_name: None,
        speaker_index: Some(7),
        ..Default::default()
    };
    super::attribute_event_speaker(&mut d, &target_cache, &name_cache);
    assert_eq!(d.npc_name.as_deref(), Some("Curilla"));

    let mut d = crate::state::DialogState {
        npc_name: None,
        speaker_index: None,
        ..Default::default()
    };
    super::attribute_event_speaker(&mut d, &target_cache, &name_cache);
    assert_eq!(d.npc_name.as_deref(), Some(""));

    let mut d = crate::state::DialogState {
        npc_name: None,
        speaker_index: Some(99),
        ..Default::default()
    };
    super::attribute_event_speaker(&mut d, &target_cache, &name_cache);
    assert_eq!(d.npc_name.as_deref(), Some("???"));
}

#[test]
fn chat_packet_resolves_autotranslate_from_the_install() {
    let Some(root) = ffxi_dat::archive::open_test_install() else {
        return;
    };
    let expected = match root.profile().name() {
        "horizonxi-2023" => "{Onion Greataxe} {Zeruhn report}",
        "retail-2026-09" => "{Mandau} {Zeruhn report}",
        _ => return,
    };
    let names = ffxi_dat::autotranslate_names::InstalledNames::open_from_root(&root);
    // vendor/server/src/map/packets/s2c/0x017_chat_std.h GP_SERV_COMMAND_CHAT_STD::PacketData.
    const CHAT_TEXT_OFFSET: usize = 19;
    let mut body = vec![0; CHAT_TEXT_OFFSET];
    body.extend_from_slice(&[
        0xFD, 0x07, 0x02, 0x47, 0x5F, 0xFD, b' ', 0xFD, 0x15, 0x02, 0xFF, 0x01, 0xFD, 0,
    ]);
    let events = sub_packet_events_with_names(ffxi_proto::map::s2c::CHAT, &body, &names);
    assert!(
        matches!(events.as_slice(), [AgentEvent::ChatLine { line }] if line.text == expected),
        "{events:?}"
    );
}

#[test]
fn shop_appraisals_use_the_latest_slot_and_quantity() {
    let mut shop = ShopSession::default();
    let request = |quantity, item_index| PendingShopAppraisal {
        item_no: 4096,
        quantity,
        item_index,
    };
    shop.pending_sell = Some(request(1, 1));
    shop.pending_sell = Some(request(12, 1));
    assert_eq!(shop.appraisal_request(1).unwrap().quantity, 12);
    assert_eq!(shop.appraisal_request(1).unwrap().quantity, 12);
    shop.retire_appraisals();
    assert!(shop.appraisal_request(1).is_none());
    shop.pending_sell = Some(request(10, 2));
    assert!(shop.appraisal_request(1).is_none());
    let second = shop.appraisal_request(2).unwrap();
    assert_eq!((second.item_index, second.quantity), (2, 10));
}

#[test]
fn shop_missing_appraisals_do_not_block_later_requests() {
    let mut shop = ShopSession::default();
    for item_index in [1, 1, 2] {
        shop.retire_appraisals();
        shop.pending_sell = Some(PendingShopAppraisal {
            item_no: 4096,
            quantity: 1,
            item_index,
        });
        assert_eq!(
            shop.appraisal_request(item_index).unwrap().item_index,
            item_index
        );
    }
}

#[test]
fn shop_raw_appraisal_ignores_other_slots_and_survives_duplicate_unit_quotes() {
    let body = |slot| {
        let mut packet = vec![0; 12];
        packet[..4].copy_from_slice(&20u32.to_le_bytes());
        packet[4] = slot;
        packet
    };
    let (shop, events) = shop_session_events_from(
        ShopSession {
            open: Some(ShopState::default()),
            pending_sell: Some(PendingShopAppraisal {
                item_no: 4096,
                quantity: 10,
                item_index: 2,
            }),
            ..Default::default()
        },
        &[
            (ffxi_proto::map::s2c::SHOP_SELL, body(1)),
            (ffxi_proto::map::s2c::SHOP_SELL, body(2)),
            (ffxi_proto::map::s2c::SHOP_SELL, body(2)),
        ],
    );
    let quotes: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ShopSellAppraisal {
                item_index, count, ..
            } => Some((*item_index, *count)),
            _ => None,
        })
        .collect();
    assert_eq!(quotes, vec![(2, 10), (2, 10)]);
    let sale = shop.open.unwrap().pending_sale.unwrap();
    assert_eq!((sale.item_index, sale.count), (2, 10));
}

/// An equipped enchanted item is already permanently locked — 0x020's lockFlagFor
/// (vendor/server/src/map/packets/s2c/0x020_item_attr.cpp) answers NoDrop for anything
/// equipped — so a plain "is it locked" read cannot tell a use in flight from a
/// shield sitting on the arm. Only the NoSelect flag marks the slot as spoken for.
#[test]
fn only_the_no_select_flag_marks_a_slot_an_action_owns() {
    const NO_SELECT: u8 = ffxi_proto::decode::lock_flg::NO_SELECT;
    assert!(slot_unselectable(NO_SELECT));
    for other in 0..=u8::MAX {
        if other != NO_SELECT {
            assert!(!slot_unselectable(other), "lock flag {other:#04X}");
        }
    }
}

/// Two targets of one skill: the hit with a level is a knockback, the miss
/// with the same level is not (vendor/server/src/map/packets/s2c/0x028_battle2.cpp
/// GP_SERV_COMMAND_BATTLE2::pack field order; research/xim
/// EffectRoutineInstance.kt handleKnockBackRoutine skips a miss).
#[test]
fn battle2_knockbacks_collect_landed_levels_per_target() {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(2, 6);
    w.write(2, 4);
    w.write(11, 4);
    w.write(0, 32);
    w.write(0, 32);

    for (target, resolution, level) in [(0xBEEFu64, 0u64, 3u64), (0xF00Du64, 1u64, 3u64)] {
        w.write(target, 32);
        w.write(1, 4);
        w.write(resolution, 3);
        w.write(0, 2);
        w.write(0, 12);
        w.write(0, 5);
        w.write(0, 2);
        w.write(level, 3);
        w.write(40, 17);
        w.write(1, 10);
        w.write(0, 31);
        w.write(0, 1);
        w.write(0, 1);
    }
    let data = w.into_bytes();

    let (actor_id, hits) = super::decode_battle2_knockbacks(&data).expect("one knockback");
    assert_eq!(actor_id, 0xCAFE);
    assert_eq!(
        hits,
        vec![crate::state::KnockbackHit {
            target_id: 0xBEEF,
            level: 3
        }]
    );
}

#[test]
fn battle2_knockbacks_are_none_without_a_level() {
    let mut w = BattleBitWriter::new(8);
    w.write(0xCAFEu64, 32);
    w.write(1, 6);
    w.write(1, 4);
    w.write(1, 4);
    w.write(0, 32);
    w.write(0, 32);
    w.write(0xBEEFu64, 32);
    w.write(1, 4);
    w.write(0, 3);
    w.write(0, 2);
    w.write(0, 12);
    w.write(0, 5);
    w.write(0, 2);
    w.write(0, 3);
    w.write(40, 17);
    w.write(1, 10);
    w.write(0, 31);
    w.write(0, 1);
    w.write(0, 1);
    assert!(super::decode_battle2_knockbacks(&w.into_bytes()).is_none());
}
