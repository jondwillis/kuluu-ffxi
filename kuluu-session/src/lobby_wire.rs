//! What a lobby session puts into each packet, for the two kinds of lobby
//! Kuluu talks to. LandSandBoat identifies the connection by the auth
//! server's session hash in every header
//! (vendor/server/src/login/login_helpers.cpp getHashFromPacket). Retail's
//! header field is an MD5 of the packet with that field zeroed
//! (research/XiPackets/lobby/Header.md), and packets that carry a `passwd`
//! take the PlayOnline-issued 16 bytes hashed with a rolling md5key
//! (research/XiPackets/lobby/Notes.md Passwords;
//! research/XIClient/src/XIClient/source/Network/Lobby/LobbyClient.cpp
//! rapMakeMD5 and rapGetMD5NextKey).

use ffxi_proto::md5::md5;

use crate::auth_client::{AuthSession, LobbyAuthCode, LOBBY_AUTH_CODE_LEN, SESSION_HASH_LEN};

pub const HEADER_IDENTIFIER_OFFSET: usize = 0x0C;
pub const HEADER_IDENTIFIER_LEN: usize = SESSION_HASH_LEN;

/// research/XiPackets/lobby/C2S_0x0007_RequestSelectChr.md unknown0000: the
/// client means to send a DLL checksum but sends its table index, always 3.
/// research/XIClient LoginStateMachine.cpp HandleLogin sets field_1A2C = 3.
pub const SELECT_DLL_HASH_INDEX: u32 = 3;
/// The second word retail folds into the 0x07 checksum, GlobalStruct field_20
/// as research/XIClient/src/XIClient/source/GlobalStruct.cpp initialises it.
pub const SELECT_DLL_HASH_SEED: u32 = 0xFF16_00AF;

// vendor/server/src/login/view_session.cpp stepsKeyOnRequest
const STEPPED_LOBBY_REQUESTS: [u8; 8] = [0x07, 0x14, 0x1F, 0x21, 0x22, 0x24, 0x28, 0x2B];
// vendor/server/src/login/view_session.cpp stepsKeyOnError
const STEPPED_LOBBY_ERROR_REQUESTS: [u8; 4] = [0x22, 0x24, 0x28, 0x2B];
/// session_t::keyCounter before any adoption or step
/// (vendor/server/src/login/session.h).
const LOBBY_KEY_COUNTER_UNSET: u32 = 0;
/// vendor/server/src/login/data_session.cpp data_session::read_func case 0xA2:
/// the server hands map the lobby counter plus this many steps, because the
/// client hashes its counter after one more step for the 0x0B and two at zone
/// connect.
pub const HANDOFF_KEY_COUNTER_STEPS: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LobbyWire {
    LandSandBoat {
        session_hash: [u8; SESSION_HASH_LEN],
        /// The counter vendor/server/src/login/view_session.cpp follows the
        /// client's lobby key with, mirrored step for step so the handoff word
        /// can be predicted (vendor/server/src/login/data_session.cpp case 0xA2).
        lobby_counter: u32,
        /// session_t::keyRequest — only an error reply to these steps the counter.
        key_request: Option<u8>,
    },
    PlayOnline {
        passwd: [u8; SESSION_HASH_LEN],
        auth_code: LobbyAuthCode,
        md5_key: u32,
    },
}

impl LobbyWire {
    pub fn for_session(auth: &AuthSession) -> Self {
        if auth.is_playonline() {
            Self::PlayOnline {
                passwd: auth.session_hash,
                auth_code: auth.auth_code,
                md5_key: 0,
            }
        } else {
            Self::LandSandBoat {
                session_hash: auth.session_hash,
                lobby_counter: LOBBY_KEY_COUNTER_UNSET,
                key_request: None,
            }
        }
    }

    /// A brand-new LSB lobby connection: epoch counter, no key request seen.
    pub fn fresh_lsb_session(session_hash: [u8; SESSION_HASH_LEN]) -> Self {
        Self::for_session(&AuthSession {
            account_id: 0,
            session_hash,
            auth_code: LobbyAuthCode::NONE,
        })
    }

    pub fn is_playonline(&self) -> bool {
        matches!(self, Self::PlayOnline { .. })
    }

    pub fn auth_code(&self) -> LobbyAuthCode {
        match self {
            Self::LandSandBoat { .. } => LobbyAuthCode::NONE,
            Self::PlayOnline { auth_code, .. } => *auth_code,
        }
    }

    /// The 16 bytes the data-port 0xA1 carries at offset 12
    /// (vendor/server/src/login/data_session.cpp data_session::read_func).
    pub fn data_identifier(&self) -> [u8; SESSION_HASH_LEN] {
        match self {
            Self::LandSandBoat { session_hash, .. } => *session_hash,
            Self::PlayOnline { passwd, .. } => *passwd,
        }
    }

    /// The 0x05 reply landed. view_session::do_write's command-0x05 arm sets
    /// keyCounter from offset 0x1C of the REPLY buffer it is writing — its own
    /// `packet.key`, a fixed server word (vendor/server/src/login/view_session.cpp
    /// view_session::read_func case 0x26) — never from anything in the client's
    /// request, so the mirror adopts exactly what came on the wire plus one.
    pub fn begin_lobby_counter(&mut self, key_field_on_wire: u32) {
        if let Self::LandSandBoat { lobby_counter, .. } = self {
            *lobby_counter = key_field_on_wire.wrapping_add(1);
        }
    }

    /// Record a view request for the counter mirror: vendor/server/src/login/
    /// view_session.cpp view_session::read_func stores keyRequest and steps on
    /// the codes in STEPPED_LOBBY_REQUESTS.
    pub fn note_view_request(&mut self, code: u8) {
        if let Self::LandSandBoat {
            lobby_counter,
            key_request,
            ..
        } = self
        {
            *key_request = Some(code);
            if STEPPED_LOBBY_REQUESTS.contains(&code) {
                *lobby_counter = lobby_counter.wrapping_add(1);
            }
        }
    }

    /// A 0x04 error frame steps only when it answers a stepping request
    /// (vendor/server/src/login/view_session.cpp view_session::do_write's
    /// command-0x04 arm).
    pub fn note_error_frame(&mut self) {
        if let Self::LandSandBoat {
            lobby_counter,
            key_request,
            ..
        } = self
        {
            if matches!(key_request, Some(code) if STEPPED_LOBBY_ERROR_REQUESTS.contains(code)) {
                *lobby_counter = lobby_counter.wrapping_add(1);
            }
        }
    }

    /// The 20 bytes LSB seeds the map blowfish with after the 0xA2 handoff:
    /// MD5 of the account session hash (which replaces the loader's key3
    /// placeholder, vendor/server/src/login/data_session.cpp case 0xA2) and
    /// the counter word at that instant plus HANDOFF_KEY_COUNTER_STEPS.
    pub fn lsb_map_seed(&self) -> Option<[u8; 20]> {
        let Self::LandSandBoat {
            session_hash,
            lobby_counter,
            ..
        } = self
        else {
            return None;
        };
        let mut seed = [0u8; 20];
        seed[..SESSION_HASH_LEN].copy_from_slice(&md5(session_hash));
        let word = lobby_counter.wrapping_add(HANDOFF_KEY_COUNTER_STEPS);
        seed[SESSION_HASH_LEN..].copy_from_slice(&word.to_le_bytes());
        Some(seed)
    }

    /// Writes the header identifer of a complete IXFF packet.
    pub fn seal(&self, packet: &mut [u8]) {
        let field = HEADER_IDENTIFIER_OFFSET..HEADER_IDENTIFIER_OFFSET + HEADER_IDENTIFIER_LEN;
        match self {
            Self::LandSandBoat { session_hash, .. } => packet[field].copy_from_slice(session_hash),
            Self::PlayOnline { .. } => {
                packet[field.clone()].fill(0);
                let digest = md5(packet);
                packet[field].copy_from_slice(&digest);
            }
        }
    }

    /// research/XIClient LobbyClient.cpp ntLoginAnalyzePacket
    /// LOBBY_IN_LOGIN_GOOD: the 0x05 reply's key seeds the md5key, advanced once.
    pub fn on_login_good(&mut self, key: u32) {
        if let Self::PlayOnline { md5_key, .. } = self {
            *md5_key = next_md5_key(key);
        }
    }

    /// research/XIClient LobbyClient.cpp ntLoginAnalyzePacket advances the
    /// md5key on every 0x03, 0x0B, 0x20 and 0x23 reply.
    /// The same reply set vendor/server/src/login/view_session.cpp
    /// stepsKeyOnReply uses for LSB's lobby key counter.
    pub fn on_key_advancing_reply(&mut self) {
        match self {
            Self::PlayOnline { md5_key, .. } => *md5_key = next_md5_key(*md5_key),
            Self::LandSandBoat { lobby_counter, .. } => {
                *lobby_counter = lobby_counter.wrapping_add(1)
            }
        }
    }

    /// The `passwd` field of a packet that carries one, or `None` when the
    /// lobby is LandSandBoat, which reads no such field. Consumes one md5key
    /// step, as research/XIClient LobbyClient.cpp ntLoginMakePacket does after
    /// rapMakeMD5.
    pub fn next_passwd(&mut self) -> Option<[u8; SESSION_HASH_LEN]> {
        match self {
            Self::LandSandBoat { .. } => None,
            Self::PlayOnline {
                passwd, md5_key, ..
            } => {
                let field = passwd_field(passwd, *md5_key);
                *md5_key = next_md5_key(*md5_key);
                Some(field)
            }
        }
    }

    /// research/XIClient LobbyClient.cpp ntLoginMakePacket LOBBY_OUT_CHARSELECT:
    /// MD5 over the authCode, the passwd field just written, the two DLL hash
    /// words and the selected content id.
    pub fn select_checksum(
        &self,
        passwd_field: &[u8; SESSION_HASH_LEN],
        ffxi_id: u32,
    ) -> Option<[u8; SESSION_HASH_LEN]> {
        let Self::PlayOnline { auth_code, .. } = self else {
            return None;
        };
        let mut block = Vec::with_capacity(LOBBY_AUTH_CODE_LEN + SESSION_HASH_LEN + 12);
        block.extend_from_slice(&auth_code.0);
        block.extend_from_slice(passwd_field);
        block.extend_from_slice(&SELECT_DLL_HASH_INDEX.to_le_bytes());
        block.extend_from_slice(&SELECT_DLL_HASH_SEED.to_le_bytes());
        block.extend_from_slice(&ffxi_id.to_le_bytes());
        Some(md5(&block))
    }
}

/// research/XIClient LobbyClient.cpp GetMD5NextKey.
fn next_md5_key(key: u32) -> u32 {
    key.wrapping_add(1)
}

/// research/XIClient LobbyClient.cpp rapMakeMD5 with the hashing bit set:
/// MD5 over the 16-byte passwd followed by the little-endian md5key.
fn passwd_field(passwd: &[u8; SESSION_HASH_LEN], md5_key: u32) -> [u8; SESSION_HASH_LEN] {
    let mut block = [0u8; SESSION_HASH_LEN + 4];
    block[..SESSION_HASH_LEN].copy_from_slice(passwd);
    block[SESSION_HASH_LEN..].copy_from_slice(&md5_key.to_le_bytes());
    md5(&block)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: [u8; SESSION_HASH_LEN] = [0x5A; SESSION_HASH_LEN];
    const AUTH_CODE_BYTE_MASK: u8 = 0x3C;

    /// vendor/server/src/login/view_session.cpp view_session::read_func case
    /// 0x26 `packet.key` — the word every server 0x05 reply carries at offset
    /// 0x1C and the mirror adopts from.
    const SERVER_LOBBY_KEY_ON_WIRE: u32 = 0xAD5DE04F;

    fn pol_session() -> AuthSession {
        let mut code = [0u8; LOBBY_AUTH_CODE_LEN];
        for (i, b) in code.iter_mut().enumerate() {
            *b = i as u8 ^ AUTH_CODE_BYTE_MASK;
        }
        AuthSession {
            account_id: 9,
            session_hash: PASSWD,
            auth_code: LobbyAuthCode(code),
        }
    }

    fn lsb_session() -> AuthSession {
        AuthSession {
            account_id: 9,
            session_hash: PASSWD,
            auth_code: LobbyAuthCode::NONE,
        }
    }

    /// vendor/server/src/login/view_session.cpp do_write adopts the login's
    /// counter word +1, read_func steps on select, data_session.cpp case 0xA2
    /// adds HANDOFF_KEY_COUNTER_STEPS; kuluu must predict that exact moment.
    #[test]
    fn a_plain_handoff_seed_follows_the_servers_counter_arithmetic() {
        let mut wire = LobbyWire::for_session(&lsb_session());
        assert_eq!(wire.lsb_map_seed().unwrap()[16..20], (3u32).to_le_bytes()); // untouched: 0 + 3
        wire.note_view_request(0x26);
        wire.begin_lobby_counter(SERVER_LOBBY_KEY_ON_WIRE);
        let after_key = wire.lsb_map_seed().unwrap();
        assert_eq!(
            after_key[16..20],
            (SERVER_LOBBY_KEY_ON_WIRE + 1 + HANDOFF_KEY_COUNTER_STEPS).to_le_bytes()
        );
        assert_eq!(&after_key[..16], &md5(&PASSWD));

        wire.on_key_advancing_reply(); // chr_info2 (0x20) reply
        wire.note_view_request(0x07); // select
        let seed = wire.lsb_map_seed().unwrap();
        assert_eq!(
            u32::from_le_bytes(seed[16..20].try_into().unwrap()),
            SERVER_LOBBY_KEY_ON_WIRE + 1 + 1 + 1 + HANDOFF_KEY_COUNTER_STEPS
        );
    }

    /// The counter word wraps rather than overflowing: LSB adds its handoff steps with C's unsigned
    /// arithmetic, so a loader key at the top of the range still predicts the same seed
    /// (vendor/server/src/login/data_session.cpp case 0xA2).
    #[test]
    fn the_handoff_word_wraps_at_the_top_of_the_range() {
        let mut wire = LobbyWire::for_session(&lsb_session());
        const NEAR_TOP: u32 = u32::MAX - HANDOFF_KEY_COUNTER_STEPS;
        wire.note_view_request(0x26);
        wire.begin_lobby_counter(NEAR_TOP);
        let seed = wire.lsb_map_seed().unwrap();
        assert_eq!(
            u32::from_le_bytes(seed[16..20].try_into().unwrap()),
            0,
            "{NEAR_TOP:#010X} + 1 adopted, then + {HANDOFF_KEY_COUNTER_STEPS} handoff steps is
             exactly 2^32, which wraps to zero"
        );
    }

    /// vendor/server/src/login/view_session.cpp stepsKeyOnError excludes the
    /// register request while name check is included; the retry re-steps as a
    /// fresh stepped request.
    #[test]
    fn create_flow_steps_requests_replies_and_the_error_arm() {
        let mut wire = LobbyWire::for_session(&lsb_session());
        wire.note_view_request(0x26);
        wire.begin_lobby_counter(SERVER_LOBBY_KEY_ON_WIRE);
        wire.on_key_advancing_reply(); // chr_info2
        let step = |w: &mut LobbyWire| {
            u32::from_le_bytes(w.lsb_map_seed().unwrap()[16..20].try_into().unwrap())
        };
        assert_eq!(
            step(&mut wire),
            SERVER_LOBBY_KEY_ON_WIRE + 1 + 1 + HANDOFF_KEY_COUNTER_STEPS
        );

        wire.note_view_request(0x22); // name check request
        wire.on_key_advancing_reply(); // its ixff result (0x03) reply
        wire.note_view_request(0x21); // register request
        assert_eq!(
            step(&mut wire),
            SERVER_LOBBY_KEY_ON_WIRE + 5 + HANDOFF_KEY_COUNTER_STEPS
        );

        wire.note_error_frame(); // 0x04 answering 0x21: no step (stepsKeyOnError)
        assert_eq!(
            step(&mut wire),
            SERVER_LOBBY_KEY_ON_WIRE + 5 + HANDOFF_KEY_COUNTER_STEPS
        );

        wire.note_view_request(0x21); // retry as a fresh request
        wire.on_key_advancing_reply();
        assert_eq!(
            step(&mut wire),
            SERVER_LOBBY_KEY_ON_WIRE + 7 + HANDOFF_KEY_COUNTER_STEPS
        );

        let mut err_wire = LobbyWire::for_session(&lsb_session());
        err_wire.note_view_request(0x22);
        err_wire.note_error_frame(); // answering 0x22: steps
        assert_eq!(
            step(&mut err_wire),
            LOBBY_KEY_COUNTER_UNSET + HANDOFF_KEY_COUNTER_STEPS + 1 + 1
        );
    }

    #[test]
    fn a_playonline_handoff_has_no_predicted_seed() {
        let wire = LobbyWire::for_session(&AuthSession {
            account_id: 9,
            session_hash: PASSWD,
            auth_code: LobbyAuthCode([0xAB; LOBBY_AUTH_CODE_LEN]),
        });
        assert!(wire.lsb_map_seed().is_none());
    }

    fn packet(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn an_lsb_seal_is_the_session_hash_verbatim() {
        let wire = LobbyWire::for_session(&lsb_session());
        let mut buf = packet(0x44);
        wire.seal(&mut buf);
        assert_eq!(
            &buf[HEADER_IDENTIFIER_OFFSET..HEADER_IDENTIFIER_OFFSET + 16],
            &PASSWD
        );
        assert_eq!(wire.data_identifier(), PASSWD);
        assert!(!wire.is_playonline());
    }

    /// research/XiPackets/lobby/Header.md: the identifer is the MD5 of the
    /// full packet with the identifer nulled.
    #[test]
    fn a_playonline_seal_is_the_md5_of_the_packet_with_the_field_nulled() {
        let wire = LobbyWire::for_session(&pol_session());
        let mut buf = packet(0x98);
        let mut nulled = buf.clone();
        nulled[HEADER_IDENTIFIER_OFFSET..HEADER_IDENTIFIER_OFFSET + 16].fill(0);
        wire.seal(&mut buf);
        assert_eq!(
            &buf[HEADER_IDENTIFIER_OFFSET..HEADER_IDENTIFIER_OFFSET + 16],
            &md5(&nulled)
        );
        assert_eq!(
            &buf[..HEADER_IDENTIFIER_OFFSET],
            &nulled[..HEADER_IDENTIFIER_OFFSET]
        );
        assert_eq!(
            &buf[HEADER_IDENTIFIER_OFFSET + 16..],
            &nulled[HEADER_IDENTIFIER_OFFSET + 16..]
        );
        assert!(wire.is_playonline());
    }

    #[test]
    fn sealing_is_idempotent_on_retail() {
        let wire = LobbyWire::for_session(&pol_session());
        let mut once = packet(0x58);
        wire.seal(&mut once);
        let mut twice = once.clone();
        wire.seal(&mut twice);
        assert_eq!(once, twice);
    }

    #[test]
    fn the_md5key_seeds_from_the_login_reply_and_steps_per_use() {
        let mut wire = LobbyWire::for_session(&pol_session());
        wire.on_login_good(0xCF75_87BB);
        let first = wire.next_passwd().unwrap();
        assert_eq!(first, passwd_field(&PASSWD, 0xCF75_87BC));
        wire.on_key_advancing_reply();
        let third = wire.next_passwd().unwrap();
        assert_eq!(third, passwd_field(&PASSWD, 0xCF75_87BE));
        assert_ne!(first, third);
    }

    #[test]
    fn the_md5key_wraps_rather_than_overflowing() {
        let mut wire = LobbyWire::for_session(&pol_session());
        wire.on_login_good(u32::MAX);
        assert_eq!(wire.next_passwd().unwrap(), passwd_field(&PASSWD, 0));
    }

    #[test]
    fn lsb_carries_no_passwd_and_no_checksum() {
        let mut wire = LobbyWire::for_session(&lsb_session());
        wire.on_login_good(1);
        assert_eq!(wire.next_passwd(), None);
        assert_eq!(wire.select_checksum(&PASSWD, 5), None);
        assert_eq!(wire.auth_code(), LobbyAuthCode::NONE);
    }

    #[test]
    fn the_select_checksum_binds_the_auth_code_passwd_and_content_id() {
        let session = pol_session();
        let wire = LobbyWire::for_session(&session);
        let field = [0x11u8; SESSION_HASH_LEN];
        let sum = wire.select_checksum(&field, 0x0001_0203).unwrap();

        let mut block = Vec::new();
        block.extend_from_slice(&session.auth_code.0);
        block.extend_from_slice(&field);
        block.extend_from_slice(&SELECT_DLL_HASH_INDEX.to_le_bytes());
        block.extend_from_slice(&SELECT_DLL_HASH_SEED.to_le_bytes());
        block.extend_from_slice(&0x0001_0203u32.to_le_bytes());
        assert_eq!(sum, md5(&block));

        assert_ne!(sum, wire.select_checksum(&field, 0x0001_0204).unwrap());
        assert_ne!(sum, wire.select_checksum(&[0x12; 16], 0x0001_0203).unwrap());
    }
}
