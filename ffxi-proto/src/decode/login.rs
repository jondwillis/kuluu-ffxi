use super::*;

/// s2c 0x00A Mog House cluster. Body offsets follow
/// vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN LoginState; `login_state` values are
/// the SAVE_LOGIN_STATE enum (h:50-59). `map_number` is the MH interior MODEL id
/// (GetMogHouseModelID, 0x00a_login.cpp), NOT a zone id; `mog_zone_flag` is
/// only assigned in the non-MH branch (.cpp: CanUseMisc(MISC_MOGMENU)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerLoginMyroom {
    pub login_state: u32,
    pub sub_map_number: u8,
    pub map_number: u16,
    pub exit_bit: u8,
    pub mog_zone_flag: u8,
}

impl ServerLoginMyroom {
    pub const LOGIN_STATE_MYROOM: u32 = 1;
    pub const LOGIN_STATE_GAME: u32 = 2;

    /// MyroomMapNumber sentinel "not in a Mog House" (0x00a_login.cpp non-MH branch).
    pub(crate) const MYROOM_NONE: u16 = 0x01FF;

    /// MyroomSubMapNumber value while on the MH second floor (0x00a_login.cpp MH branch).
    pub const SUB_MAP_2F: u8 = 0x02;

    /// LSB reuses LoginState MYROOM + this MyroomMapNumber for ZONE_FERETORY
    /// (Monstrosity), which is not a Mog House server-side
    /// (0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN sets it with no m_moghouseID).
    pub(crate) const MYROOM_FERETORY: u16 = 0x02D9;

    pub(crate) const LOGIN_STATE_OFFSET: usize = 0x7C;
    pub(crate) const SUB_MAP_NUMBER_OFFSET: usize = 0xA4;
    pub(crate) const MAP_NUMBER_OFFSET: usize = 0xA6;
    pub(crate) const EXIT_BIT_OFFSET: usize = 0xAA;
    pub(crate) const MOG_ZONE_FLAG_OFFSET: usize = 0xAB;
    pub(crate) const MIN_LEN: usize = Self::MOG_ZONE_FLAG_OFFSET + 1;

    fn decode(body: &[u8]) -> Option<Self> {
        if body.len() < Self::MIN_LEN {
            return None;
        }
        Some(Self {
            login_state: u32::from_le_bytes(
                body[Self::LOGIN_STATE_OFFSET..Self::LOGIN_STATE_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ),
            sub_map_number: body[Self::SUB_MAP_NUMBER_OFFSET],
            map_number: u16::from_le_bytes(
                body[Self::MAP_NUMBER_OFFSET..Self::MAP_NUMBER_OFFSET + 2]
                    .try_into()
                    .unwrap(),
            ),
            exit_bit: body[Self::EXIT_BIT_OFFSET],
            mog_zone_flag: body[Self::MOG_ZONE_FLAG_OFFSET],
        })
    }

    /// The MH interior model id, only when the server actually placed the player
    /// in a Mog House: LoginState MYROOM excluding the [`Self::MYROOM_NONE`]
    /// sentinel and the [`Self::MYROOM_FERETORY`] alias.
    pub fn myroom_model(&self) -> Option<u16> {
        (self.login_state == Self::LOGIN_STATE_MYROOM
            && self.map_number != Self::MYROOM_NONE
            && self.map_number != Self::MYROOM_FERETORY)
            .then_some(self.map_number)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ServerLogin {
    pub voyage: Option<ZoneInVoyage>,
    pub unique_no: u32,
    pub act_index: u16,
    pub zone_no: u16,

    pub game_time: Option<u32>,

    pub pos_head: PosHead,

    pub music_num: Option<[u16; 5]>,

    pub myroom: Option<ServerLoginMyroom>,

    /// `SubMapNumber` — `PChar->loc.boundary`, the server's authoritative
    /// initial sub-area for this zone-in (vendor/server/src/map/packets/s2c/
    /// 0x00a_login.h field order; see [`Self::SUB_AREA_OFFSET`]). Distinct
    /// from [`ServerLoginMyroom::sub_map_number`] (`MyroomSubMapNumber`,
    /// offset 0xA4, u8) — that is a different field and would truncate a
    /// sub-area id (sub-areas run 293..640).
    pub sub_area: Option<u16>,

    pub zone_in_event: Option<ZoneInEvent>,

    /// The character's own appearance. LSB never sends a 0x00D CHAR_PC about a
    /// player to that player (vendor/server/src/map/zone_entities.cpp
    /// `CZoneEntities::UpdateEntityPacket` skips `PCurrentChar == PEntity`), so
    /// this `GrapIDTbl` — written at 0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN with the same slot
    /// tagging as CHAR_PC — plus 0x051 GRAP_LIST are self's only look sources.
    pub look: Option<LookData>,

    /// `DeadCounter` — the same `60 * (6min + GetTimeUntilDeathHomepoint())`
    /// encoding 0x037 puts in `dead_counter1`
    /// (vendor/server/src/map/packets/s2c/0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN deadRemaining). `None` only
    /// when the body stopped short of the field. Read it through
    /// [`Self::seconds_until_homepoint`], which applies the `hpp == 0` KO gate
    /// the raw counter needs.
    pub dead_counter: Option<u32>,

    /// Weather in force as the character zones in.
    ///
    /// The server sends 0x057 WEATHER only when the weather *changes*
    /// (vendor/server/src/map/zone.cpp CZone::SetWeather is its sole construction site, a
    /// CHAR_INZONE broadcast), so this is the only weather a zoning character
    /// receives until the next change — which on a Vana'diel schedule can be a
    /// long wait.
    pub weather: Option<ZoneInWeather>,
}

/// Weather in force at zone-in, from the 0x00A weather block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneInWeather {
    /// `WeatherNumber` — the LSB weather id, same discriminant order as 0x057.
    /// This is the weather actually in force: retail's `FUNC_ZoneSetUp` calls
    /// `XiArea_SetWeather` twice, with the `*2` slot first and this one second
    /// (research/XiPackets/world/server/0x000A/README.md "WeatherNumber`, `WeatherNumber2`, `WeatherTime`, `WeatherTime2`, `WeatherOffsetTime").
    pub weather_number: u16,
    /// `WeatherNumber2` — the weather being transitioned *from*, not the
    /// incoming side: retail assigns it to `PreviousWeatherNumber`
    /// (research/XIClient/src/XIClient/source/Game/Net/Packets/s2c/0x00A.cpp S2C::RecvLogin,
    /// whose `field_NN` names run +4 ahead of the true payload offsets).
    pub previous_weather_number: u16,
    /// `WeatherTime` — `zone->GetWeatherChangeTime()`, retail's
    /// `CurrentWeatherStartTime` (0x00A.cpp S2C::RecvLogin), in **Earth seconds since the
    /// Vana'diel epoch**: 0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN assigns
    /// `zone->GetWeatherChangeTime()`, which zone.cpp CZone::SetWeather sets from
    /// `earth_time::vanadiel_timestamp()`
    /// (vendor/server/src/common/earth_time.h timestamp).
    pub weather_time: u32,
    /// `WeatherTime2` — retail's `PreviousWeatherStartTime` (0x00A.cpp S2C::RecvLogin).
    pub previous_weather_time: u32,
    /// `WeatherOffsetTime` — two packed u16s: the low half is retail's
    /// `CurrentWeatherOffsetTime`, the high half its `PreviousWeatherOffsetTime`
    /// (0x00A.cpp S2C::RecvLogin,96; `FUNC_ZoneSetUp` feeds `HIWORD` to the previous pass).
    pub offset_time: u32,
}

impl ZoneInWeather {
    /// LSB never writes the previous-weather slots —
    /// vendor/server/src/map/packets/s2c/0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN sets only
    /// `WeatherNumber`/`WeatherTime` under a `// TODO: Previous weather` — so
    /// they arrive zeroed. Weather id 0 is a real weather (`fine`), so a
    /// consumer must gate on this rather than read 0 as clear skies.
    pub fn has_previous(&self) -> bool {
        self.previous_weather_number != 0 || self.previous_weather_time != 0
    }
}

/// Zone-in cutscene carried inside s2c 0x00A LOGIN: when `currentEvent` is
/// already set at zone-in (e.g. the new-character intro, a Mog House 2F unlock
/// CS), LSB delivers it via the login packet instead of a 0x032/0x034 push
/// (vendor/server/src/map/packets/s2c/0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN csid). The client
/// must answer with 0x05B `End` (`EventPara` = this `event_para`) or the char
/// stays InEvent server-side — zonelines/logout rejected — and the CS re-fires
/// on every subsequent login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneInEvent {
    /// `EventNum`: the zone id the event belongs to.
    pub event_num: u16,
    /// `EventPara`: the cutscene/event id (`currentEvent->eventId`).
    pub event_para: u16,
    /// `EventMode`: `currentEvent->eventFlags` low half.
    pub event_mode: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneInVoyage {
    pub start: u32,
    pub duration: u16,
    pub reverse: bool,
    pub route: u8,
}

impl ZoneInVoyage {
    // vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN ShipStart/ShipEnd
    // FFXiMain.dll (2026-09-09 install) VA 0x100FA0F7 reconstructs end = start + duration.
    pub fn decode(body: &[u8]) -> Option<Self> {
        const START: usize = 0x74;
        const DURATION: usize = 0x78;
        const REVERSE: usize = 0x23;
        const ROUTE: usize = 0x26;
        const REVERSE_MASK: u8 = 4;
        const ROUTE_SHIFT: u8 = 3;
        const ROUTE_MASK: u8 = 3;
        let start = u32::from_le_bytes(body.get(START..START + 4)?.try_into().ok()?);
        let duration = u16::from_le_bytes(body.get(DURATION..DURATION + 2)?.try_into().ok()?);
        (start != 0 && duration != 0).then_some(Self {
            start,
            duration,
            reverse: body[REVERSE] & REVERSE_MASK != 0,
            route: (body[ROUTE] >> ROUTE_SHIFT) & ROUTE_MASK,
        })
    }
}

impl ServerLogin {
    pub(crate) const SIZE: usize = 48;

    pub(crate) const MUSIC_NUM_OFFSET: usize = 0x52;
    pub(crate) const MUSIC_NUM_SIZE: usize = 5 * 2;

    pub(crate) const GAME_TIME_OFFSET: usize = 0x38;

    // vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN EventNo — GrapIDTbl[9] u16
    // runs to MusicNum[5] u16 at MUSIC_NUM_OFFSET, which lands SubMapNumber
    // immediately before EVENT_NUM_OFFSET below; pinning the neighbours proves
    // both offsets (see `grap_id_tbl_offset_abuts_music_num` and
    // `sub_area_offset_sits_between_music_num_and_event_num`).
    pub(crate) const GRAP_ID_TBL_OFFSET: usize = 0x40;

    pub(crate) const SUB_AREA_OFFSET: usize = 0x5C;

    pub(crate) const EVENT_NUM_OFFSET: usize = 0x5E;
    pub(crate) const EVENT_PARA_OFFSET: usize = 0x60;
    pub(crate) const EVENT_MODE_OFFSET: usize = 0x62;
    // vendor/server/src/map/packets/s2c/0x00a_login.h — WeatherNumber,
    // WeatherNumber2, WeatherTime, WeatherTime2, WeatherOffsetTime, immediately
    // after EventMode. The offset chain is pinned at both ends by constants this
    // decoder already uses: MusicNum[5] at 0x52 runs to SubMapNumber at 0x5C,
    // and past the weather block sit ShipStart/ShipEnd/IsMonstrosity, landing
    // exactly on LOGIN_STATE_OFFSET 0x7C.
    pub const WEATHER_NUMBER_OFFSET: usize = 0x64;
    pub const WEATHER_NUMBER2_OFFSET: usize = 0x66;
    pub const WEATHER_TIME_OFFSET: usize = 0x68;
    pub const WEATHER_TIME2_OFFSET: usize = 0x6C;
    pub const WEATHER_OFFSET_TIME_OFFSET: usize = 0x70;

    /// `DeadCounter`, between `PlayTime` and `MyroomSubMapNumber` in
    /// vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN LoginState. The chain from
    /// `LoginState` @0x7C runs `name[16]`, `certificate[2]`, `unknown9C`,
    /// `ZoneSubNo`, `PlayTime`, `DeadCounter` — landing on
    /// `ServerLoginMyroom::SUB_MAP_NUMBER_OFFSET`, which the const assert
    /// below pins. Retail's own struct agrees: `field_A4` (its names run +4
    /// ahead of the payload offsets, per
    /// `static_assert(offsetof(GP_SERV_LOGIN, field_A8) == 0xA4)`) is the u32 it
    /// divides by 60 into the death deadline
    /// (research/XIClient/src/XIClient/source/Game/Net/Packets/s2c/0x00A.cpp S2C::RecvLogin,
    /// .../include/Game/Net/Packets/s2c/0x00A.h).
    pub const DEAD_COUNTER_OFFSET: usize = 0xA0;

    /// `PosHead.server_status` while a zone-in event is pending — the packet's
    /// event fields are only written then, and event id 0 is a real cutscene
    /// (Bastok Markets intro), so presence keys off the status byte
    /// (0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN, ANIMATION_EVENT in
    /// vendor/server/src/map/entities/baseentity.h ANIMATIONTYPE ANIMATION_EVENT).
    pub(crate) const SERVER_STATUS_EVENT: u8 = 4;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }

        let zone_u32 = u32::from_le_bytes(body[44..48].try_into().unwrap());
        let pos_head = PosHead::decode(&body[..PosHead::SIZE_WITH_BT_TARGET])?;
        let game_time = if body.len() >= Self::GAME_TIME_OFFSET + 4 {
            Some(u32::from_le_bytes(
                body[Self::GAME_TIME_OFFSET..Self::GAME_TIME_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            ))
        } else {
            None
        };

        let music_num = if body.len() >= Self::MUSIC_NUM_OFFSET + Self::MUSIC_NUM_SIZE {
            let base = Self::MUSIC_NUM_OFFSET;
            Some([
                u16::from_le_bytes([body[base], body[base + 1]]),
                u16::from_le_bytes([body[base + 2], body[base + 3]]),
                u16::from_le_bytes([body[base + 4], body[base + 5]]),
                u16::from_le_bytes([body[base + 6], body[base + 7]]),
                u16::from_le_bytes([body[base + 8], body[base + 9]]),
            ])
        } else {
            None
        };
        let sub_area = (body.len() >= Self::SUB_AREA_OFFSET + 2).then(|| {
            u16::from_le_bytes(
                body[Self::SUB_AREA_OFFSET..Self::SUB_AREA_OFFSET + 2]
                    .try_into()
                    .unwrap(),
            )
        });
        let zone_in_event = (pos_head.server_status == Self::SERVER_STATUS_EVENT
            && body.len() >= Self::EVENT_MODE_OFFSET + 2)
            .then(|| ZoneInEvent {
                event_num: u16::from_le_bytes(
                    body[Self::EVENT_NUM_OFFSET..Self::EVENT_NUM_OFFSET + 2]
                        .try_into()
                        .unwrap(),
                ),
                event_para: u16::from_le_bytes(
                    body[Self::EVENT_PARA_OFFSET..Self::EVENT_PARA_OFFSET + 2]
                        .try_into()
                        .unwrap(),
                ),
                event_mode: u16::from_le_bytes(
                    body[Self::EVENT_MODE_OFFSET..Self::EVENT_MODE_OFFSET + 2]
                        .try_into()
                        .unwrap(),
                ),
            });
        let dead_counter = (body.len() >= Self::DEAD_COUNTER_OFFSET + 4).then(|| {
            u32::from_le_bytes(
                body[Self::DEAD_COUNTER_OFFSET..Self::DEAD_COUNTER_OFFSET + 4]
                    .try_into()
                    .unwrap(),
            )
        });
        let weather = (body.len() >= Self::WEATHER_OFFSET_TIME_OFFSET + 4).then(|| {
            let u16_at = |off: usize| u16::from_le_bytes(body[off..off + 2].try_into().unwrap());
            let u32_at = |off: usize| u32::from_le_bytes(body[off..off + 4].try_into().unwrap());
            ZoneInWeather {
                weather_number: u16_at(Self::WEATHER_NUMBER_OFFSET),
                previous_weather_number: u16_at(Self::WEATHER_NUMBER2_OFFSET),
                weather_time: u32_at(Self::WEATHER_TIME_OFFSET),
                previous_weather_time: u32_at(Self::WEATHER_TIME2_OFFSET),
                offset_time: u32_at(Self::WEATHER_OFFSET_TIME_OFFSET),
            }
        });
        Ok(Self {
            voyage: ZoneInVoyage::decode(body),
            unique_no: pos_head.unique_no,
            act_index: pos_head.act_index,
            zone_no: zone_u32 as u16,
            game_time,
            pos_head,
            music_num,
            myroom: ServerLoginMyroom::decode(body),
            sub_area,
            zone_in_event,
            look: LookData::decode_grap_id_tbl(body, Self::GRAP_ID_TBL_OFFSET),
            dead_counter,
            weather,
        })
    }

    /// Seconds until the forced home-point warp, or `None` when the character is
    /// not KO'd (or the body stopped short of `DeadCounter`).
    ///
    /// `PosHead.HpMax` is `PChar->GetHPP()`
    /// (vendor/server/src/map/packets/s2c/0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN), and `GetHPP()`
    /// clamps a living character to at least 1, so `hpp == 0` is the same true KO
    /// sentinel the 0x037 path uses — and just as load-bearing, since LSB fills
    /// `DeadCounter` for a living character too.
    pub fn seconds_until_homepoint(&self) -> Option<u32> {
        self.dead_counter
            .filter(|_| self.pos_head.hpp == 0)
            .map(dead_counter_seconds_until_homepoint)
    }
}

const _: () = assert!(
    ServerLogin::DEAD_COUNTER_OFFSET + 4 == ServerLoginMyroom::SUB_MAP_NUMBER_OFFSET,
    "DeadCounter abuts MyroomSubMapNumber in GP_SERV_COMMAND_LOGIN"
);

#[derive(Debug, Clone, Copy)]
pub struct ServerLogout {
    pub logout_state: u32,
    pub new_server_ip: u32,
    pub new_server_port: u16,
    pub error_code: u32,
}

impl ServerLogout {
    pub(crate) const SIZE: usize = 24;

    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        if body.len() < Self::SIZE {
            return Err(DecodeError::Truncated(Self::SIZE, body.len()));
        }
        Ok(Self {
            logout_state: u32::from_le_bytes(body[0..4].try_into().unwrap()),
            new_server_ip: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            new_server_port: u32::from_le_bytes(body[8..12].try_into().unwrap()) as u16,
            error_code: u32::from_le_bytes(body[20..24].try_into().unwrap()),
        })
    }

    pub fn is_zone_change(&self) -> bool {
        self.logout_state == 2
    }
}

#[cfg(test)]
mod server_login_tests {
    // The weather block sits between EventMode and ShipStart in
    // vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN WeatherNumber. Pin the offsets
    // against the two constants that bracket it, so a future field insertion
    // cannot silently slide weather onto the ship or event fields.
    #[test]
    fn zone_in_weather_offsets_sit_between_event_mode_and_login_state() {
        use super::ServerLogin as L;
        assert_eq!(L::WEATHER_NUMBER_OFFSET, L::EVENT_MODE_OFFSET + 2);
        assert_eq!(L::WEATHER_NUMBER2_OFFSET, L::WEATHER_NUMBER_OFFSET + 2);
        assert_eq!(L::WEATHER_TIME_OFFSET, L::WEATHER_NUMBER2_OFFSET + 2);
        assert_eq!(L::WEATHER_TIME2_OFFSET, L::WEATHER_TIME_OFFSET + 4);
        assert_eq!(L::WEATHER_OFFSET_TIME_OFFSET, L::WEATHER_TIME2_OFFSET + 4);
        // ShipStart u32, ShipEnd u16, IsMonstrosity u16, then LoginState.
        assert_eq!(
            L::WEATHER_OFFSET_TIME_OFFSET + 4 + 4 + 2 + 2,
            super::ServerLoginMyroom::LOGIN_STATE_OFFSET
        );
    }

    /// Pins SUB_AREA_OFFSET against the two already-pinned neighbours it sits
    /// between in GP_SERV_COMMAND_LOGIN::PacketData
    /// (vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN ntTime): MusicNum[5]
    /// (MUSIC_NUM_OFFSET, 10 bytes) then SubMapNumber (u16) then EventNum
    /// (EVENT_NUM_OFFSET). If a field were inserted ahead of SubMapNumber,
    /// this chain — not just the standalone constant — would break.
    #[test]
    fn sub_area_offset_sits_between_music_num_and_event_num() {
        use super::ServerLogin as L;
        assert_eq!(L::SUB_AREA_OFFSET, L::MUSIC_NUM_OFFSET + L::MUSIC_NUM_SIZE);
        assert_eq!(L::EVENT_NUM_OFFSET, L::SUB_AREA_OFFSET + 2);
    }

    use super::*;

    #[test]
    fn server_login_decodes_zone_no() {
        let mut buf = vec![0u8; ServerLogin::SIZE];
        buf[0..4].copy_from_slice(&0x0123_4567u32.to_le_bytes());
        buf[4..6].copy_from_slice(&0x00FFu16.to_le_bytes());
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.unique_no, 0x0123_4567);
        assert_eq!(l.act_index, 0x00FF);
        assert_eq!(l.zone_no, 230);
    }

    #[test]
    fn server_login_zone_in_event_keys_off_status_byte_not_event_id() {
        let mut buf = vec![0u8; 0x100];
        buf[44..48].copy_from_slice(&234u32.to_le_bytes());
        buf[ServerLogin::EVENT_NUM_OFFSET..ServerLogin::EVENT_NUM_OFFSET + 2]
            .copy_from_slice(&234u16.to_le_bytes());
        // Bastok Markets intro cutscene is event id 0 — a zeroed EventPara must
        // still decode as an event when the status byte says so.
        buf[ServerLogin::EVENT_MODE_OFFSET..ServerLogin::EVENT_MODE_OFFSET + 2]
            .copy_from_slice(&32u16.to_le_bytes());

        let no_event = ServerLogin::decode(&buf).unwrap();
        assert_eq!(no_event.zone_in_event, None);

        buf[27] = ServerLogin::SERVER_STATUS_EVENT;
        let with_event = ServerLogin::decode(&buf).unwrap();
        assert_eq!(
            with_event.zone_in_event,
            Some(ZoneInEvent {
                event_num: 234,
                event_para: 0,
                event_mode: 32,
            })
        );
    }

    /// The dead-counter field is only readable if the offset chain from the
    /// already-pinned LoginState is intact, so pin the chain LSB declares
    /// (vendor/server/src/map/packets/s2c/0x00a_login.h): LoginState u32,
    /// name[16], certificate[2] i32, unknown9C u16, ZoneSubNo u16, PlayTime u32,
    /// DeadCounter u32, MyroomSubMapNumber u8.
    #[test]
    fn dead_counter_offset_sits_between_login_state_and_myroom_sub_map() {
        use super::ServerLogin as L;
        use super::ServerLoginMyroom as M;
        assert_eq!(
            L::DEAD_COUNTER_OFFSET,
            M::LOGIN_STATE_OFFSET + 4 + 16 + 8 + 2 + 2 + 4
        );
        assert_eq!(L::DEAD_COUNTER_OFFSET + 4, M::SUB_MAP_NUMBER_OFFSET);
    }

    #[test]
    fn server_login_dead_counter_only_reads_as_a_timer_while_ko() {
        let mut buf = vec![0u8; 0x100];
        const REMAINING_SECS: u32 = 1800;
        buf[ServerLogin::DEAD_COUNTER_OFFSET..ServerLogin::DEAD_COUNTER_OFFSET + 4]
            .copy_from_slice(
                &(DEAD_COUNTER_UNITS_PER_SECOND * (DEAD_COUNTER_PADDING_SECS + REMAINING_SECS))
                    .to_le_bytes(),
            );
        // Neighbouring PlayTime / MyroomSubMapNumber must not bleed in.
        buf[ServerLogin::DEAD_COUNTER_OFFSET - 4..ServerLogin::DEAD_COUNTER_OFFSET]
            .copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        buf[ServerLoginMyroom::SUB_MAP_NUMBER_OFFSET] = 0xAB;

        // PosHead.HpMax is GetHPP(); a living character carries the very same
        // counter, so hpp alone decides whether it means anything.
        const FULL_HP_PCT: u8 = 100;
        buf[PosHead::HPP_OFFSET] = FULL_HP_PCT;
        let alive = ServerLogin::decode(&buf).unwrap();
        assert_eq!(
            alive.dead_counter,
            Some(DEAD_COUNTER_UNITS_PER_SECOND * (DEAD_COUNTER_PADDING_SECS + REMAINING_SECS))
        );
        assert_eq!(alive.seconds_until_homepoint(), None);

        buf[PosHead::HPP_OFFSET] = 0;
        let ko = ServerLogin::decode(&buf).unwrap();
        assert_eq!(ko.seconds_until_homepoint(), Some(REMAINING_SECS));
    }

    /// Both carriers of the counter share one encoding, so the 0x00A value must
    /// convert exactly like the 0x037 one
    /// (vendor/server/src/map/packets/char_status.cpp CCharStatusPacket::CCharStatusPacket deadRemaining vs
    /// vendor/server/src/map/packets/s2c/0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN deadRemaining).
    #[test]
    fn server_login_dead_counter_matches_char_status_conversion() {
        for remaining in [0u32, 1, 599, 3600] {
            let raw = DEAD_COUNTER_UNITS_PER_SECOND * (DEAD_COUNTER_PADDING_SECS + remaining);
            let mut buf = vec![0u8; 0x100];
            buf[ServerLogin::DEAD_COUNTER_OFFSET..ServerLogin::DEAD_COUNTER_OFFSET + 4]
                .copy_from_slice(&raw.to_le_bytes());
            let login = ServerLogin::decode(&buf).unwrap();
            let mut status_body = vec![0u8; CharStatus::MIN_LEN];
            status_body[CharStatus::DEAD_COUNTER1_OFFSET..CharStatus::DEAD_COUNTER1_OFFSET + 4]
                .copy_from_slice(&raw.to_le_bytes());
            let status = CharStatus::decode(&status_body).unwrap();
            assert_eq!(
                login.seconds_until_homepoint(),
                Some(status.seconds_until_homepoint())
            );
            assert_eq!(login.seconds_until_homepoint(), Some(remaining));
        }
    }

    #[test]
    fn server_login_dead_counter_absent_when_body_stops_short() {
        let buf = vec![0u8; ServerLogin::DEAD_COUNTER_OFFSET + 3];
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.dead_counter, None);
        assert_eq!(l.seconds_until_homepoint(), None);
    }

    #[test]
    fn server_login_truncated_errors() {
        let buf = vec![0u8; ServerLogin::SIZE - 1];
        assert!(matches!(
            ServerLogin::decode(&buf),
            Err(DecodeError::Truncated(48, _))
        ));
    }

    #[test]
    fn server_login_myroom_cluster_roundtrips() {
        let mut buf = vec![0u8; 0x100];
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        buf[ServerLoginMyroom::LOGIN_STATE_OFFSET..ServerLoginMyroom::LOGIN_STATE_OFFSET + 4]
            .copy_from_slice(&ServerLoginMyroom::LOGIN_STATE_MYROOM.to_le_bytes());
        buf[ServerLoginMyroom::SUB_MAP_NUMBER_OFFSET] = ServerLoginMyroom::SUB_MAP_2F;
        buf[ServerLoginMyroom::MAP_NUMBER_OFFSET..ServerLoginMyroom::MAP_NUMBER_OFFSET + 2]
            .copy_from_slice(&617u16.to_le_bytes());
        buf[ServerLoginMyroom::EXIT_BIT_OFFSET] = 3;
        buf[ServerLoginMyroom::MOG_ZONE_FLAG_OFFSET] = 1;

        let l = ServerLogin::decode(&buf).unwrap();
        let myroom = l.myroom.expect("full-size body carries the cluster");
        assert_eq!(myroom.login_state, ServerLoginMyroom::LOGIN_STATE_MYROOM);
        assert_eq!(myroom.sub_map_number, ServerLoginMyroom::SUB_MAP_2F);
        assert_eq!(myroom.map_number, 617);
        assert_eq!(myroom.exit_bit, 3);
        assert_eq!(myroom.mog_zone_flag, 1);
        assert_eq!(myroom.myroom_model(), Some(617));
    }

    #[test]
    fn server_login_truncated_body_yields_no_myroom() {
        let mut buf = vec![0u8; ServerLoginMyroom::MIN_LEN - 1];
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.zone_no, 230);
        assert!(l.myroom.is_none());
    }

    #[test]
    fn server_login_decodes_sub_area() {
        let mut buf = vec![0u8; 0x100];
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        // Sub-area ids run 293..640, well past a u8 — pin one to prove the
        // u16 width, not just the offset.
        buf[ServerLogin::SUB_AREA_OFFSET..ServerLogin::SUB_AREA_OFFSET + 2]
            .copy_from_slice(&400u16.to_le_bytes());
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.sub_area, Some(400));
    }

    #[test]
    fn server_login_short_body_yields_no_sub_area() {
        let buf = vec![0u8; ServerLogin::SUB_AREA_OFFSET + 1];
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.sub_area, None);
    }

    // Distinct sentinels in every weather field: an offset-arithmetic test
    // cannot catch a current/previous pair swapped inside the constructor.
    #[test]
    fn zone_in_weather_decodes_current_and_previous_slots() {
        let mut buf = vec![0u8; 0x104];
        let put16 = |b: &mut Vec<u8>, off: usize, v: u16| {
            b[off..off + 2].copy_from_slice(&v.to_le_bytes());
        };
        let put32 = |b: &mut Vec<u8>, off: usize, v: u32| {
            b[off..off + 4].copy_from_slice(&v.to_le_bytes());
        };
        put16(&mut buf, ServerLogin::WEATHER_NUMBER_OFFSET, 6);
        put16(&mut buf, ServerLogin::WEATHER_NUMBER2_OFFSET, 2);
        put32(&mut buf, ServerLogin::WEATHER_TIME_OFFSET, 0x1111_1111);
        put32(&mut buf, ServerLogin::WEATHER_TIME2_OFFSET, 0x2222_2222);
        put32(
            &mut buf,
            ServerLogin::WEATHER_OFFSET_TIME_OFFSET,
            0x3333_4444,
        );

        let w = ServerLogin::decode(&buf).unwrap().weather.unwrap();
        assert_eq!(w.weather_number, 6);
        assert_eq!(w.previous_weather_number, 2);
        assert_eq!(w.weather_time, 0x1111_1111);
        assert_eq!(w.previous_weather_time, 0x2222_2222);
        assert_eq!(w.offset_time, 0x3333_4444);
        assert!(w.has_previous());
    }

    // vendor/server/src/map/packets/s2c/0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN writes only
    // WeatherNumber/WeatherTime, so the previous slots arrive zeroed and a
    // cross-fade must not read 0 as weather id 0 (`fine`).
    #[test]
    fn lsb_zeroed_previous_weather_slots_are_not_weather_id_zero() {
        let mut buf = vec![0u8; 0x104];
        buf[ServerLogin::WEATHER_NUMBER_OFFSET..ServerLogin::WEATHER_NUMBER_OFFSET + 2]
            .copy_from_slice(&4u16.to_le_bytes());
        buf[ServerLogin::WEATHER_TIME_OFFSET..ServerLogin::WEATHER_TIME_OFFSET + 4]
            .copy_from_slice(&1234u32.to_le_bytes());

        let w = ServerLogin::decode(&buf).unwrap().weather.unwrap();
        assert_eq!(w.weather_number, 4);
        assert_eq!(w.weather_time, 1234);
        assert_eq!(w.previous_weather_number, 0);
        assert_eq!(w.previous_weather_time, 0);
        assert!(!w.has_previous());
    }

    #[test]
    fn short_body_yields_no_zone_in_weather() {
        let buf = vec![0u8; ServerLogin::WEATHER_OFFSET_TIME_OFFSET + 3];
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.weather, None);
        assert!(l.sub_area.is_some());
    }

    #[test]
    fn server_login_myroom_jeuno_model_decodes() {
        let mut buf = vec![0u8; 0x100];
        buf[44..48].copy_from_slice(&243u32.to_le_bytes());
        buf[ServerLoginMyroom::LOGIN_STATE_OFFSET..ServerLoginMyroom::LOGIN_STATE_OFFSET + 4]
            .copy_from_slice(&ServerLoginMyroom::LOGIN_STATE_MYROOM.to_le_bytes());
        buf[ServerLoginMyroom::MAP_NUMBER_OFFSET..ServerLoginMyroom::MAP_NUMBER_OFFSET + 2]
            .copy_from_slice(&0x0100u16.to_le_bytes());

        let myroom = ServerLogin::decode(&buf).unwrap().myroom.unwrap();
        assert_eq!(myroom.myroom_model(), Some(0x0100));
    }

    #[test]
    fn server_login_myroom_model_gated_on_state_and_sentinel() {
        let mut buf = vec![0u8; 0x100];
        buf[ServerLoginMyroom::LOGIN_STATE_OFFSET..ServerLoginMyroom::LOGIN_STATE_OFFSET + 4]
            .copy_from_slice(&ServerLoginMyroom::LOGIN_STATE_GAME.to_le_bytes());
        buf[ServerLoginMyroom::MAP_NUMBER_OFFSET..ServerLoginMyroom::MAP_NUMBER_OFFSET + 2]
            .copy_from_slice(&ServerLoginMyroom::MYROOM_NONE.to_le_bytes());
        let myroom = ServerLogin::decode(&buf).unwrap().myroom.unwrap();
        assert_eq!(myroom.login_state, ServerLoginMyroom::LOGIN_STATE_GAME);
        assert_eq!(myroom.myroom_model(), None, "GAME state carries no model");

        buf[ServerLoginMyroom::LOGIN_STATE_OFFSET..ServerLoginMyroom::LOGIN_STATE_OFFSET + 4]
            .copy_from_slice(&ServerLoginMyroom::LOGIN_STATE_MYROOM.to_le_bytes());
        let myroom = ServerLogin::decode(&buf).unwrap().myroom.unwrap();
        assert_eq!(
            myroom.myroom_model(),
            None,
            "MYROOM with the 0x01FF sentinel carries no model"
        );

        buf[ServerLoginMyroom::MAP_NUMBER_OFFSET..ServerLoginMyroom::MAP_NUMBER_OFFSET + 2]
            .copy_from_slice(&ServerLoginMyroom::MYROOM_FERETORY.to_le_bytes());
        let myroom = ServerLogin::decode(&buf).unwrap().myroom.unwrap();
        assert_eq!(
            myroom.myroom_model(),
            None,
            "Feretory MYROOM alias is not a Mog House"
        );
    }

    /// Pins the myroom cluster to LSB's GP_SERV_COMMAND_LOGIN PacketData layout
    /// (vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN ntTime; body offsets, no
    /// sub-packet header) so an offset edit can't pass the roundtrip tests, which
    /// build buffers through these same consts.
    #[test]
    fn myroom_cluster_offsets_and_sentinels_match_lsb_login_layout() {
        assert_eq!(ServerLoginMyroom::LOGIN_STATE_OFFSET, 0x7C);
        assert_eq!(ServerLoginMyroom::SUB_MAP_NUMBER_OFFSET, 0xA4);
        assert_eq!(ServerLoginMyroom::MAP_NUMBER_OFFSET, 0xA6);
        assert_eq!(ServerLoginMyroom::EXIT_BIT_OFFSET, 0xAA);
        assert_eq!(ServerLoginMyroom::MOG_ZONE_FLAG_OFFSET, 0xAB);
        assert_eq!(ServerLoginMyroom::LOGIN_STATE_MYROOM, 1, "SAVE_LOGIN_STATE");
        assert_eq!(ServerLoginMyroom::LOGIN_STATE_GAME, 2, "SAVE_LOGIN_STATE");
        assert_eq!(ServerLoginMyroom::MYROOM_NONE, 0x01FF);
        assert_eq!(ServerLoginMyroom::SUB_MAP_2F, 0x02);
        assert_eq!(ServerLoginMyroom::MYROOM_FERETORY, 0x02D9);
    }

    #[test]
    fn server_login_carries_pos_head_for_spawn_seed() {
        let mut buf = vec![0u8; ServerLogin::SIZE];
        buf[0..4].copy_from_slice(&0x0123_4567u32.to_le_bytes());
        buf[4..6].copy_from_slice(&0x00FFu16.to_le_bytes());
        buf[7] = 96;
        buf[8..12].copy_from_slice(&(-115.5f32).to_le_bytes());
        buf[12..16].copy_from_slice(&(7.25f32).to_le_bytes());
        buf[16..20].copy_from_slice(&(280.0f32).to_le_bytes());
        buf[24] = 40;
        buf[25] = 40;
        buf[44..48].copy_from_slice(&230u32.to_le_bytes());
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(l.pos_head.x, -115.5);
        assert_eq!(l.pos_head.z, 7.25);
        assert_eq!(l.pos_head.y, 280.0);
        assert_eq!(l.pos_head.dir, 96);
        assert_eq!(l.pos_head.speed, 40);
        assert_eq!(l.pos_head.speed_base, 40);
    }

    // vendor/server/src/map/packets/s2c/0x00a_login.h GP_SERV_COMMAND_LOGIN EventNo — GrapIDTbl[9] sits
    // immediately before MusicNum[5]; pin the abutment so a field insertion
    // cannot slide the table without failing here.
    #[test]
    fn grap_id_tbl_offset_abuts_music_num() {
        use super::ServerLogin as L;
        assert_eq!(L::GRAP_ID_TBL_OFFSET, 0x40);
        assert_eq!(
            L::GRAP_ID_TBL_OFFSET + LookData::GRAP_ID_TBL_LEN,
            L::MUSIC_NUM_OFFSET
        );
    }

    #[test]
    fn server_login_carries_self_look_from_grap_id_tbl() {
        let mut buf = vec![0u8; ServerLogin::MUSIC_NUM_OFFSET];
        // 0x00a_login.cpp GP_SERV_COMMAND_LOGIN::GP_SERV_COMMAND_LOGIN — slot0 = face | race << 8, slot i tagged +0x{i}000.
        let slots: [u16; LookData::GRAP_ID_TBL_SLOTS] = [
            0x0507, 0x1011, 0x2022, 0x3033, 0x4044, 0x5055, 0x6066, 0x7077, 0x8088,
        ];
        for (i, v) in slots.iter().enumerate() {
            let off = ServerLogin::GRAP_ID_TBL_OFFSET + i * 2;
            buf[off..off + 2].copy_from_slice(&v.to_le_bytes());
        }
        let l = ServerLogin::decode(&buf).unwrap();
        assert_eq!(
            l.look,
            Some(LookData::Equipped {
                face: 0x07,
                race: 0x05,
                head: 0x011,
                body: 0x022,
                hands: 0x033,
                legs: 0x044,
                feet: 0x055,
                main: 0x066,
                sub: 0x077,
                ranged: 0x088,
            })
        );
    }

    #[test]
    fn server_login_without_grap_id_tbl_has_no_look() {
        let buf = vec![0u8; ServerLogin::MUSIC_NUM_OFFSET];
        assert_eq!(ServerLogin::decode(&buf).unwrap().look, None);
    }
}

#[cfg(test)]
mod server_logout_tests {
    use super::*;

    #[test]
    fn server_logout_zone_change() {
        let mut buf = vec![0u8; ServerLogout::SIZE];
        buf[0..4].copy_from_slice(&2u32.to_le_bytes());
        buf[4..8].copy_from_slice(&0x6F00_A8C0u32.to_le_bytes());
        buf[8..12].copy_from_slice(&54230u32.to_le_bytes());
        let l = ServerLogout::decode(&buf).unwrap();
        assert!(l.is_zone_change());
        assert_eq!(l.new_server_port, 54230);
        assert_eq!(l.new_server_ip, 0x6F00_A8C0);
    }
}

#[cfg(test)]
mod voyage_tests {
    use super::{ServerLogin, ZoneInVoyage};

    #[test]
    fn voyage_fields_decode_independently_and_reject_incomplete_timing() {
        const START: usize = 116;
        const DURATION: usize = 120;
        const FLAGS: usize = 35;
        const ROUTE: usize = 38;
        const FULL: usize = 122;
        const START_VALUE: u32 = 0x1200_3400;
        const DURATION_VALUE: u16 = 897;
        const ALL_FLAGS: u8 = u8::MAX;
        const OTHER_REVERSE_FLAGS: u8 = 0xFB;
        const OTHER_ROUTE_FLAGS: u8 = 0xE7;
        const ROUTE_SHIFT: u8 = 3;
        const ROUTE_COUNT: u8 = 4;
        let mut body = [0u8; FULL];
        body[START..START + 4].copy_from_slice(&START_VALUE.to_le_bytes());
        body[DURATION..].copy_from_slice(&DURATION_VALUE.to_le_bytes());
        for reverse in [false, true] {
            for route in 0..ROUTE_COUNT {
                body[FLAGS] = if reverse {
                    ALL_FLAGS
                } else {
                    OTHER_REVERSE_FLAGS
                };
                body[ROUTE] = OTHER_ROUTE_FLAGS | route << ROUTE_SHIFT;
                let expected = Some(ZoneInVoyage {
                    start: START_VALUE,
                    duration: DURATION_VALUE,
                    reverse,
                    route,
                });
                assert_eq!(ZoneInVoyage::decode(&body), expected);
                assert_eq!(ServerLogin::decode(&body).unwrap().voyage, expected);
            }
        }
        for length in 0..FULL {
            assert_eq!(ZoneInVoyage::decode(&body[..length]), None);
        }
        body[DURATION..].fill(0);
        assert_eq!(ZoneInVoyage::decode(&body), None);
        body[DURATION..].copy_from_slice(&DURATION_VALUE.to_le_bytes());
        body[START..START + 4].fill(0);
        assert_eq!(ZoneInVoyage::decode(&body), None);
        assert_eq!(ZoneInVoyage::decode(&[0; FULL]), None);
    }
}
