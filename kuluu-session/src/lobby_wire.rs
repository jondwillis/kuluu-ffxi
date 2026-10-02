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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LobbyWire {
    LandSandBoat {
        session_hash: [u8; SESSION_HASH_LEN],
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
            }
        }
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
            Self::LandSandBoat { session_hash } => *session_hash,
            Self::PlayOnline { passwd, .. } => *passwd,
        }
    }

    /// Writes the header identifer of a complete IXFF packet.
    pub fn seal(&self, packet: &mut [u8]) {
        let field = HEADER_IDENTIFIER_OFFSET..HEADER_IDENTIFIER_OFFSET + HEADER_IDENTIFIER_LEN;
        match self {
            Self::LandSandBoat { session_hash } => packet[field].copy_from_slice(session_hash),
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
    pub fn on_key_advancing_reply(&mut self) {
        if let Self::PlayOnline { md5_key, .. } = self {
            *md5_key = next_md5_key(*md5_key);
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
