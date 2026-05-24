//! Lobby flow — owns both the data (54230) and view (54001) TLS sockets and
//! runs the IXFF binary handshake from "auth done" to "we know the map server
//! IP/port and the 20-byte session key seed that derives our Blowfish key".
//!
//! Wire protocol mostly inferred from `server/src/login/data_session.cpp` and
//! `view_session.cpp`. v1 covers the happy path only (no char create/delete).

use anyhow::{bail, Context, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use ffxi_proto::login::IXFF_TERMINATOR;

use crate::auth_client::AuthSession;

// NOTE on transport: only the auth port (54231) negotiates TLS in LSB. The
// data (54230) and view (54001) ports use plain TCP — see
// `handler_session.cpp::do_read()` which calls `socket_.next_layer().
// async_read_some(...)`, bypassing the SSL layer. `auth_session` overrides
// `start()` to perform `async_handshake`; `data_session` and `view_session`
// inherit `handler_session::start()` which does *not* handshake. This is
// silently true at runtime — easy to miss from the per-port handler<T>
// template scaffolding.

/// The data-port 0xA1 response is a fixed 328-byte buffer.
const DATA_CHARLIST_SIZE: usize = 0x148;

/// View-port packet header total (mirrors `packet_t`: 4+4+4+16).
const IXFF_HEADER_SIZE: usize = 28;

const DATA_CMD_CHAR_LIST: u8 = 0xA1;
const DATA_CMD_HANDOFF: u8 = 0xA2;

const VIEW_CMD_REGISTER: u32 = 0x00;
const VIEW_CMD_SELECT: u32 = 0x07;
/// Delete-character view opcode — view_session.cpp:92. Server decodes a
/// `lpkt_deletechr` (login_packets.h:170-189) from the read buffer.
const VIEW_CMD_DELETE_CHAR: u32 = 0x14;
/// Real "register character" command — see view_session.cpp:160. The
/// previous `VIEW_CMD_CREATE = 0x01` value was wrong; the server has no
/// case 0x01 in its dispatch switch, which is why creation hung forever.
const VIEW_CMD_REGISTER_CHAR: u32 = 0x21;
/// "Name check + Gold World Pass" — view_session.cpp:197. Must precede
/// the 0x21 because the server reads the character name from
/// `session.requestedNewCharacterName`, which is *only* populated by the
/// 0x22 success path (login_helpers.cpp:220).
const VIEW_CMD_NAME_CHECK: u32 = 0x22;
// Server response: lpkt_next_login command on view socket.
const VIEW_RESP_NEXT_LOGIN: u32 = 0x0B;

/// Size of `lpkt_next_login` (header 28 + payload 0x48 - hmm, server sets
/// `packet_size = 0x48` which is the *whole* struct). We trust the field on
/// read.
const NEXT_LOGIN_PACKET_SIZE: u32 = 0x48;

#[derive(Debug, Clone)]
pub struct CharListEntry {
    pub content_id: u32,
    pub char_id_main: u16,
    pub world_id: u8,
    pub char_id_extra: u8,
}

#[derive(Debug, Clone)]
pub struct CharList {
    pub characters: Vec<CharListEntry>,
}

/// Parsed entry from the view-port `lpkt_chr_info2` push. Carries the full
/// `u32` charid (the data-port 0xA1 only gives a 16-bit `char_id_main`,
/// inadequate for accounts with millions-range ids) and the user-visible
/// name. Empty slots arrive with status=0x01 and a space-prefixed name —
/// `list_characters` filters those out before returning.
///
/// Appearance fields (`race` .. `ranged`) are sourced from the embedded
/// `TC_OPERATION_MAKE` block (see `vendor/server/src/login/login_packets.h`).
/// They power the launcher's 3D character preview — char-select can spawn
/// each character's model without waiting for the map server.
#[derive(Debug, Clone)]
pub struct CharSlot {
    pub char_id: u32,
    pub name: String,
    pub status: u16,
    /// FFXI race byte (1=Hume M, 2=Hume F, …, 8=Galka). Low byte of
    /// `TC_OPERATION_MAKE::mon_no`; high byte is a separate enum we
    /// don't currently consume.
    pub race: u8,
    /// `TC_OPERATION_MAKE::face_no` — face id, 0..=7 typically.
    pub face: u8,
    /// `GrapIDTbl[0..=7]` — the 8 equipment slots, in canonical
    /// order. Each entry is a slot-tagged item-model id (e.g.,
    /// `0x2065` = body slot, item-model 0x65). `0x*000` sentinels
    /// mean the slot is empty.
    pub head: u16,
    pub body: u16,
    pub hands: u16,
    pub legs: u16,
    pub feet: u16,
    pub main: u16,
    pub sub: u16,
    pub ranged: u16,
    /// The character's saved zone id, reconstructed from
    /// `TC_OPERATION_MAKE::zone_no` (low byte) and `zone_no2` bit 0
    /// (bit 8). See `vendor/server/src/login/data_session.cpp:185-186`
    /// — LSB packs the 9-bit zone id across these two `uint8` fields.
    /// Used by the launcher's 3D backdrop to load the character's
    /// saved zone behind the char-select screen.
    pub zone_id: u16,
}

#[derive(Debug, Clone)]
pub struct MapHandoff {
    pub char_id: u32,
    pub character_name: String,
    pub server_ip: u32, // network byte order from server's perspective; LE u32 here
    pub server_port: u16,
    /// The 20-byte key3 we sent (after server-side adjustments are NOT
    /// applied here; map server reads from DB so we must use what we sent).
    pub session_key_seed: [u8; 20],
}

pub struct LobbyClient {
    pub host: String,
    pub data_port: u16,
    pub view_port: u16,
}

/// A live lobby session: both TLS-TCP sockets remain open after `open()`
/// so multiple operations (select / create / delete) share one server-side
/// `session_t` lifecycle. Drop the handle to close cleanly without picking
/// a character — the server's `handle_error` path will null both slots and
/// remove the entry from `authenticatedSessions_`.
pub struct LobbyHandle {
    view: TcpStream,
    data: TcpStream,
    /// Populated character slots only (empty slots from the server are
    /// filtered out at construction time).
    chars: Vec<CharSlot>,
    session_hash: [u8; 16],
}

impl LobbyHandle {
    pub fn chars(&self) -> &[CharSlot] {
        &self.chars
    }

    /// Create a new character on this account, then refresh the lobby's
    /// char list — all on the live sockets. Use this instead of
    /// [`LobbyClient::create_character`] when you already hold a live
    /// `LobbyHandle`: opening a second lobby session with the same
    /// `session_hash` races server-side `session_t` ownership of the
    /// view/data slots and the new open()'s 0xA1 reply ends up either
    /// silently dropped or routed to the old socket (surfaces as
    /// "reading 0xA1 char list" failure). LSB keys `session_t` by
    /// `session_hash`, not by socket — one hash, one live pair.
    ///
    /// On success returns `self` with `chars` updated to include the
    /// newly-created character. On failure the handle is consumed and
    /// the caller should `LobbyClient::open` afresh to recover.
    pub async fn create_character(
        mut self,
        auth: &AuthSession,
        spec: &CharCreateSpec,
    ) -> Result<Self> {
        // ---- 0x22 name check ----
        let name_check = build_view_name_check(&spec.name, &self.session_hash);
        self.view.write_all(&name_check).await?;
        self.view.flush().await?;
        tracing::debug!(name = %spec.name, "lobby handle: 0x22 name check sent");
        read_create_reply(&mut self.view, "name check")
            .await
            .context("0x22 name check response")?;

        // ---- 0x21 register character ----
        let register_char = build_view_register_char(
            spec.race,
            spec.job,
            spec.nation,
            spec.size,
            spec.face,
            &self.session_hash,
        );
        self.view.write_all(&register_char).await?;
        self.view.flush().await?;
        tracing::debug!(
            race = spec.race,
            job = spec.job,
            nation = spec.nation,
            size = spec.size,
            face = spec.face,
            "lobby handle: 0x21 register sent"
        );
        read_create_reply(&mut self.view, "register character")
            .await
            .context("0x21 register character response")?;

        // ---- refresh char list via another 0xA1 on the existing data
        // socket. The server's `data->addCharIntoCharInfo(charInfo)` call
        // inside 0x21 (view_session.cpp:172) updates the data_session's
        // cached characterInfoResponse, so this 0xA1 round-trip returns
        // the list *with* the new character.
        // account_id must match `session.accountID` server-side
        // (data_session.cpp:83). 0 silently fails the comparison and the
        // server never replies — the 100ms-then-read-EOF that we saw
        // before this fix.
        let req_a1 = build_data_a1(auth.account_id, 0, &self.session_hash);
        self.data.write_all(&req_a1).await?;
        self.data.flush().await?;
        // Same race-mitigation rationale as `open()` before its 0xA1.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = read_data_charlist(&mut self.data)
            .await
            .context("reading 0xA1 char-list refresh after create")?;
        let slots = parse_view_chr_info2(&mut self.view)
            .await
            .context("reading chr_info2 refresh after create")?;
        self.chars = slots
            .into_iter()
            .filter(|s| s.char_id != 0 && !s.name.starts_with(' '))
            .collect();
        tracing::info!(
            new_name = %spec.name,
            total = self.chars.len(),
            "lobby handle: char list refreshed after create"
        );
        Ok(self)
    }

    /// Delete a character on this account. Sends opcode 0x14
    /// (`lpkt_deletechr`) on the view socket, reads the 0x20 ACK reply,
    /// then refreshes `chars` via a fresh 0xA1 / chr_info2 round-trip
    /// (same pattern as `create_character`). LSB's delete handler doesn't
    /// actually validate the `passwd` field — only `ffxi_id` and the
    /// session's `accountID` are checked — so callers pass `[0u8; 16]`.
    pub async fn delete_character(mut self, auth: &AuthSession, char_id: u32) -> Result<Self> {
        let pkt = build_delete_char(char_id, 0, [0u8; 16], &self.session_hash);
        self.view.write_all(&pkt).await?;
        self.view.flush().await?;
        tracing::debug!(char_id, "lobby handle: 0x14 delete-char sent");

        // Server replies with a 0x20-byte ACK packet on the view socket
        // (`do_write(0x20)` in view_session.cpp:119). Drain it so the next
        // read aligns to the chr_info2 push that follows the 0xA1.
        let mut ack = [0u8; 0x20];
        self.view
            .read_exact(&mut ack)
            .await
            .context("reading 0x14 delete-char ack on view socket")?;

        let req_a1 = build_data_a1(auth.account_id, 0, &self.session_hash);
        self.data.write_all(&req_a1).await?;
        self.data.flush().await?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = read_data_charlist(&mut self.data)
            .await
            .context("reading 0xA1 char-list refresh after delete")?;
        let slots = parse_view_chr_info2(&mut self.view)
            .await
            .context("reading chr_info2 refresh after delete")?;
        self.chars = slots
            .into_iter()
            .filter(|s| s.char_id != 0 && !s.name.starts_with(' '))
            .collect();
        tracing::info!(
            char_id,
            total = self.chars.len(),
            "lobby handle: char list refreshed after delete"
        );
        Ok(self)
    }

    /// Run the second half of the lobby flow: view 0x07 select → wait for
    /// data 0x02 ack → data 0xA2 with key3 → read view `lpkt_next_login`.
    /// Consumes the handle (sockets close on drop after this returns).
    pub async fn select(
        mut self,
        char_id: u32,
        char_name: &str,
        key3: [u8; 20],
    ) -> Result<MapHandoff> {
        let req_select = build_view_select(char_id, char_name, &self.session_hash);
        self.view.write_all(&req_select).await?;
        self.view.flush().await?;
        tracing::debug!(char_id, "lobby: 0x07 select sent");

        let mut ack = [0u8; 5];
        self.data
            .read_exact(&mut ack)
            .await
            .context("reading 0x07 ack on data port")?;
        if ack[0] != 0x02 {
            bail!("expected 0x02 ack after view select, got {ack:?}");
        }
        tracing::debug!("lobby: 0x02 ack received");

        let req_a2 = build_data_a2(&key3);
        self.data.write_all(&req_a2).await?;
        self.data.flush().await?;
        tracing::debug!("lobby: 0xA2 handoff sent");

        // Same race-mitigation rationale as the sleep in `open()` before
        // 0xA1: LSB's 0xA2 handler writes lpkt_next_login to
        // view_session->buffer_, but the view socket's read state and
        // the data session's write must serialize cleanly. Locally
        // observed: ~4ms is enough on a warm server, but cold/contended
        // races see the next_login push silently dropped. 50ms is a
        // generous nudge that costs nothing on the happy path.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let handoff = read_lpkt_next_login(&mut self.view, char_id, &key3).await?;
        tracing::debug!(
            server_port = handoff.server_port,
            "lobby: lpkt_next_login received"
        );
        Ok(handoff)
    }
}

impl LobbyClient {
    pub fn new(host: impl Into<String>, data_port: u16, view_port: u16) -> Self {
        Self {
            host: host.into(),
            data_port,
            view_port,
        }
    }

    /// Open a lobby session: connect both sockets, register the view
    /// session, fire `0xA1`, parse both responses, and hand back a
    /// `LobbyHandle` that *retains* the live sockets. Subsequent
    /// operations (`select`, `create_character`, `delete_character`)
    /// reuse those sockets so the server's `session_t::view_session`
    /// and `session_t::data_session` slots stay coherent across the
    /// entire flow — closing and reopening between steps races the
    /// server's async cleanup and routes responses to dead pointers
    /// (the bug that surfaced as `early eof`).
    pub async fn open(&self, auth: &AuthSession) -> Result<LobbyHandle> {
        let mut view = self.connect(self.view_port).await?;
        tracing::debug!("lobby: view socket connected");
        let mut data = self.connect(self.data_port).await?;
        tracing::debug!("lobby: data socket connected");

        let register = ixff_header(
            IXFF_HEADER_SIZE as u32,
            VIEW_CMD_REGISTER,
            &auth.session_hash,
        );
        view.write_all(&register).await?;
        view.flush().await?;
        tracing::debug!("lobby: VIEW_CMD_REGISTER sent");

        // Race-condition mitigation: LSB's data_session::read_func handles
        // 0xA1 by writing chr_info2 to `session.view_session->buffer_`, but
        // `session.view_session` is only populated when view_session::
        // read_func has run for the first time. If we send 0xA1 before
        // the server's asio reactor has dispatched VIEW_CMD_REGISTER on
        // the view socket, the data handler sees a null view_session and
        // never sends chr_info2 — client then blocks forever in
        // `parse_view_chr_info2`. 100ms is a generous nudge; LSB
        // typically dispatches the view read in under 1ms locally, but
        // the kernel-level ordering of two TCP sockets isn't guaranteed.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let req_a1 = build_data_a1(auth.account_id, 0, &auth.session_hash);
        data.write_all(&req_a1).await?;
        data.flush().await?;
        tracing::debug!(
            account_id = auth.account_id,
            "lobby: 0xA1 char-list request sent"
        );

        let charlist = read_data_charlist(&mut data).await?;
        tracing::debug!(
            count = charlist.characters.len(),
            "lobby: 0xA1 char-list received"
        );
        let slots = parse_view_chr_info2(&mut view).await?;
        // Drop empty slots (server marks them with status=0x01 and a leading
        // space byte in `character_name`, see view_session.cpp:222).
        let chars: Vec<CharSlot> = slots
            .into_iter()
            .filter(|s| s.char_id != 0 && !s.name.starts_with(' '))
            .collect();
        tracing::debug!(populated = chars.len(), "lobby: chr_info2 parsed");

        Ok(LobbyHandle {
            view,
            data,
            chars,
            session_hash: auth.session_hash,
        })
    }

    /// Convenience wrapper for the original "open + select" flow. Most
    /// callers should prefer `open()` to keep the sockets alive across
    /// multiple operations; this exists for the MCP path that does the
    /// whole handshake in one shot inside `session::run`.
    pub async fn handshake(
        &self,
        auth: &AuthSession,
        char_id: u32,
        char_name: &str,
        _search_server_ip: u32,
        key3: [u8; 20],
    ) -> Result<MapHandoff> {
        let handle = self.open(auth).await?;
        if handle.chars.is_empty() {
            bail!("no characters found for account");
        }
        handle.select(char_id, char_name, key3).await
    }

    /// Open lobby, resolve char_name to char_id from the roster, then select.
    /// Returns the resolved char_id alongside the handoff so the caller
    /// (session::run) can store it for the map bootstrap.
    pub async fn handshake_by_name(
        &self,
        auth: &AuthSession,
        char_name: &str,
        key3: [u8; 20],
    ) -> Result<(u32, MapHandoff)> {
        let handle = self.open(auth).await?;
        if handle.chars.is_empty() {
            bail!("no characters found for account");
        }
        let slot = handle
            .chars()
            .iter()
            .find(|c| c.name == char_name)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no character named '{char_name}' on account (have: {:?})",
                    handle
                        .chars()
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                )
            })?;
        let handoff = handle.select(slot.char_id, &slot.name, key3).await?;
        Ok((slot.char_id, handoff))
    }

    /// Run the LSB character-creation flow on an already-open lobby view
    /// socket. Two packets, in order:
    ///   1. 0x22 (name check) — server validates name, stashes it in
    ///      `session.requestedNewCharacterName`. Reply is 0x20 bytes on
    ///      success (result byte = 0x03 at buf[8]) or 0x24 bytes on
    ///      failure (result = 0x04, errorCode u16 at buf[32]).
    ///   2. 0x21 (register character) — server reads race/job/nation/
    ///      size/face at fixed packet offsets (see login_helpers.cpp:216
    ///      and view_session.cpp:160). Reply is again 0x20 / closed
    ///      socket on failure.
    pub async fn create_character(&self, auth: &AuthSession, spec: &CharCreateSpec) -> Result<()> {
        // The chr_info2 view push is triggered by the *data* port's 0xA1
        // handler, not by VIEW_CMD_REGISTER alone — see data_session.cpp
        // around line 281 where it writes into `viewSession->buffer_`.
        // So we run the full open() flow (which connects both sockets,
        // registers the view, fires 0xA1, drains chr_info2), then keep
        // only the view socket for the 0x22/0x21 exchange.
        let handle = self.open(auth).await?;
        let mut view = handle.view;
        tracing::debug!("create_character: lobby open complete");

        // ---- 0x22 name check ----
        let name_check = build_view_name_check(&spec.name, &auth.session_hash);
        view.write_all(&name_check).await?;
        view.flush().await?;
        tracing::debug!(name = %spec.name, "create_character: 0x22 name check sent");
        read_create_reply(&mut view, "name check")
            .await
            .context("0x22 name check response")?;

        // ---- 0x21 register character ----
        let register_char = build_view_register_char(
            spec.race,
            spec.job,
            spec.nation,
            spec.size,
            spec.face,
            &auth.session_hash,
        );
        view.write_all(&register_char).await?;
        view.flush().await?;
        tracing::debug!(
            race = spec.race,
            job = spec.job,
            nation = spec.nation,
            size = spec.size,
            face = spec.face,
            "create_character: 0x21 register sent"
        );
        read_create_reply(&mut view, "register character")
            .await
            .context("0x21 register character response")?;

        Ok(())
    }

    async fn connect(&self, port: u16) -> Result<TcpStream> {
        TcpStream::connect((self.host.as_str(), port))
            .await
            .with_context(|| format!("TCP connect to {}:{}", self.host, port))
    }
}

fn ixff_header(packet_size: u32, command: u32, session_hash: &[u8; 16]) -> [u8; 28] {
    let mut buf = [0u8; 28];
    buf[0..4].copy_from_slice(&packet_size.to_le_bytes());
    buf[4..8].copy_from_slice(&IXFF_TERMINATOR.to_le_bytes());
    buf[8..12].copy_from_slice(&command.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);
    buf
}

fn build_data_a1(account_id: u32, search_server_ip: u32, session_hash: &[u8; 16]) -> Vec<u8> {
    // Layout (per data_session.cpp:80–87):
    //   [0]    code 0xA1
    //   [1..5] account_id   (LE u32)
    //   [5..9] search_server_ip (LE u32)
    //   [9..12] padding
    //   [12..28] session_hash
    let mut buf = vec![0u8; 28];
    buf[0] = DATA_CMD_CHAR_LIST;
    buf[1..5].copy_from_slice(&account_id.to_le_bytes());
    buf[5..9].copy_from_slice(&search_server_ip.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);
    buf
}

fn build_data_a2(key3: &[u8; 20]) -> Vec<u8> {
    // Layout (per data_session.cpp:287–291):
    //   [0]     code 0xA2
    //   [1..21] key3 (20 bytes)
    // Server falls back to the connection's stored sessionHash; we don't
    // need to embed the hash again on this packet.
    let mut buf = vec![0u8; 28];
    buf[0] = DATA_CMD_HANDOFF;
    buf[1..21].copy_from_slice(key3);
    buf
}

fn build_view_select(char_id: u32, char_name: &str, session_hash: &[u8; 16]) -> Vec<u8> {
    // Layout (per view_session.cpp:54–59 + login_packets.h::packet_t):
    //   [0..4]   packet_size = 0x44 (size of the full select packet)
    //   [4..8]   IXFF terminator
    //   [8..12]  command = 0x07
    //   [12..28] session_hash
    //   [28..32] char_id (LE u32)
    //   [32..36] char_id_world (LE u32) — server reads the name at offset 36
    //   [36..52] character_name (16 bytes, NUL-padded)
    let packet_size = 0x44u32; // 68 bytes
    let mut buf = vec![0u8; packet_size as usize];
    buf[0..4].copy_from_slice(&packet_size.to_le_bytes());
    buf[4..8].copy_from_slice(&IXFF_TERMINATOR.to_le_bytes());
    buf[8..12].copy_from_slice(&VIEW_CMD_SELECT.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);
    buf[28..32].copy_from_slice(&char_id.to_le_bytes());
    // ffxi_id_world (lower 16 bits of char_id) — populate for completeness.
    buf[32..36].copy_from_slice(&((char_id & 0xFFFF) as u32).to_le_bytes());
    let name_bytes = char_name.as_bytes();
    let n = name_bytes.len().min(15);
    buf[36..36 + n].copy_from_slice(&name_bytes[..n]);
    buf
}

/// Build an `lpkt_deletechr` view packet (opcode 0x14). Layout per
/// `vendor/server/src/login/login_packets.h:170-189` with default struct
/// packing (no `#pragma pack` in that header) — total 52 bytes, not the
/// 40 the design note suggested:
///   [0..4]   packet_size = 0x34 (52)
///   [4..8]   IXFF terminator
///   [8..12]  command = 0x14
///   [12..28] identifer[16] — IXFF session-hash slot; reuse the auth
///            `session_hash` here, matching every other view packet
///            (see `ixff_header` + `build_view_select`).
///   [28..32] ffxi_id (content id, LE u32)
///   [32..36] ffxi_id_world (LE u32)
///   [36..52] passwd[16] — current account password, NUL-padded.
///            Server re-validates this against `accounts.password` via
///            view_session.cpp:128-134's account-ownership check (the
///            destructive op deliberately demands a fresh password).
pub fn build_delete_char(
    content_id: u32,
    world_id: u32,
    passwd: [u8; 16],
    session_hash: &[u8; 16],
) -> Vec<u8> {
    let packet_size = 0x34u32; // 52
    let mut buf = vec![0u8; packet_size as usize];
    buf[0..4].copy_from_slice(&packet_size.to_le_bytes());
    buf[4..8].copy_from_slice(&IXFF_TERMINATOR.to_le_bytes());
    buf[8..12].copy_from_slice(&VIEW_CMD_DELETE_CHAR.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);
    buf[28..32].copy_from_slice(&content_id.to_le_bytes());
    buf[32..36].copy_from_slice(&world_id.to_le_bytes());
    buf[36..52].copy_from_slice(&passwd);
    buf
}

/// Spec for `LobbyClient::create_character`. Only the fields the server
/// actually reads in `loginHelpers::createCharacter` (login_helpers.cpp:216)
/// are exposed: gender, tail, and body are derived server-side from
/// race+size, not taken from the wire.
#[derive(Debug, Clone)]
pub struct CharCreateSpec {
    pub name: String,
    /// 1..=8 — 1 HumeM, 2 HumeF, 3 ElvaanM, 4 ElvaanF, 5 TaruM, 6 TaruF,
    /// 7 Mithra, 8 Galka. (login_helpers.cpp:228.)
    pub race: u8,
    /// Starting main job; server clamps to 1..=6 (WAR/MNK/WHM/BLM/RDM/THF).
    /// login_helpers.cpp:247.
    pub job: u8,
    /// 0 San d'Oria, 1 Bastok, 2 Windurst. login_helpers.cpp:259.
    pub nation: u8,
    /// 0..=2 (Small/Medium/Large). login_helpers.cpp:234.
    pub size: u8,
    /// 0..=15. login_helpers.cpp:240.
    pub face: u8,
}

/// Build a 0x22 name-check view packet. Server reads the name at packet
/// offset 32 (= payload offset 4) for up to 15 bytes, NUL-terminated.
/// See view_session.cpp:212.
fn build_view_name_check(name: &str, session_hash: &[u8; 16]) -> Vec<u8> {
    // Pick a comfortable round-up that covers offsets through 32+16=48.
    // 0x40 (64) is what LSB's own buffers tend to round to and what the
    // server's do_write contracts handle without complaint.
    let packet_size = 0x40u32;
    let mut buf = vec![0u8; packet_size as usize];
    buf[0..4].copy_from_slice(&packet_size.to_le_bytes());
    buf[4..8].copy_from_slice(&IXFF_TERMINATOR.to_le_bytes());
    buf[8..12].copy_from_slice(&VIEW_CMD_NAME_CHECK.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);
    // Name at packet offset 32 (payload offset 4) — view_session.cpp:212
    // `std::memcpy(CharName, buffer_.data() + 32, PacketNameLength - 1);`
    let name_bytes = name.as_bytes();
    let n = name_bytes.len().min(15);
    buf[32..32 + n].copy_from_slice(&name_bytes[..n]);
    buf
}

/// Build a 0x21 register-character view packet. Field offsets are
/// load-bearing — the server reads them as fixed positions in the
/// buffer (login_helpers.cpp:216–266), not as a packed struct:
///   race   @ packet offset 48  (login_helpers.cpp:224)
///   mjob   @ packet offset 50  (login_helpers.cpp:247)
///   nation @ packet offset 54  (login_helpers.cpp:259)
///   size   @ packet offset 57  (login_helpers.cpp:225)
///   face   @ packet offset 60  (login_helpers.cpp:226)
/// Name is taken from `session.requestedNewCharacterName`, set by the
/// preceding 0x22 — NOT from this packet's body.
fn build_view_register_char(
    race: u8,
    job: u8,
    nation: u8,
    size: u8,
    face: u8,
    session_hash: &[u8; 16],
) -> Vec<u8> {
    let packet_size = 0x40u32; // covers through offset 60+1=61
    let mut buf = vec![0u8; packet_size as usize];
    buf[0..4].copy_from_slice(&packet_size.to_le_bytes());
    buf[4..8].copy_from_slice(&IXFF_TERMINATOR.to_le_bytes());
    buf[8..12].copy_from_slice(&VIEW_CMD_REGISTER_CHAR.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);
    buf[48] = race;
    buf[50] = job;
    buf[54] = nation;
    buf[57] = size;
    buf[60] = face;
    buf
}

/// Read and validate a 0x22 / 0x21 reply. Server writes a size byte at
/// `buf[0]` (the rest of the size u32 is zero-padded by memset, so the
/// LE u32 we read is the same value): 0x20 on success with result=0x03
/// at buf[8], 0x24 on failure with result=0x04 and `loginErrors::errorCode`
/// at buf[32] (login_helpers.cpp:59). On 0x21 failure the server just
/// closes the socket (view_session.cpp:166) — we surface that as EOF.
async fn read_create_reply(stream: &mut TcpStream, stage: &str) -> Result<()> {
    let mut size_bytes = [0u8; 4];
    stream
        .read_exact(&mut size_bytes)
        .await
        .with_context(|| format!("reading {stage} reply size (server may have closed socket)"))?;
    let size = u32::from_le_bytes(size_bytes) as usize;
    if size != 0x20 && size != 0x24 {
        bail!("{stage}: implausible reply size {size:#x} (want 0x20 or 0x24)");
    }
    let mut rest = vec![0u8; size - 4];
    stream
        .read_exact(&mut rest)
        .await
        .with_context(|| format!("reading {stage} reply body"))?;
    // After the 4-byte size we just consumed:
    //   rest[0..4] = "IXFF" terminator
    //   rest[4..8] = result byte at index 4 (= original buf[8])
    let term = u32::from_le_bytes(rest[0..4].try_into().unwrap());
    if term != IXFF_TERMINATOR {
        bail!("{stage}: bad terminator {term:#x}");
    }
    let result = rest[4]; // buf[8]
    match (size, result) {
        (0x20, 0x03) => Ok(()),
        (0x24, 0x04) => {
            // errorCode is at packet offset 32 (= rest[28..30]) per
            // login_helpers.cpp:74 `ref<uint16>(packet, 32) = errorCode;`.
            let err = u16::from_le_bytes(rest[28..30].try_into().unwrap());
            bail!(
                "{stage}: server rejected with loginErrors code {err} ({})",
                login_error_name(err)
            );
        }
        _ => bail!("{stage}: unexpected reply size={size:#x} result={result:#x}"),
    }
}

/// Decode a `loginErrors::errorCode` (login_errors.h). Returned alongside
/// the numeric code so logs are self-explanatory.
fn login_error_name(code: u16) -> &'static str {
    match code {
        305 => "UNABLE_TO_CONNECT_TO_WORLD_SERVER",
        313 => "CHARACTER_NAME_UNAVAILABLE",
        201 => "CHARACTER_ALREADY_LOGGED_IN",
        314 => "FAILED_TO_REGISTER_WITH_THE_NAME_SERVER",
        321 => "CHARACTERS_PARAMETERS_ARE_INCORRECT",
        331 => "GAMES_DATA_HAS_BEEN_UPDATED",
        332 => "COULD_NOT_CONNECT_TO_LOBBY_SERVER",
        _ => "unknown",
    }
}

async fn read_data_charlist(stream: &mut TcpStream) -> Result<CharList> {
    let mut buf = vec![0u8; DATA_CHARLIST_SIZE];
    stream
        .read_exact(&mut buf)
        .await
        .context("reading 0xA1 char list")?;
    if buf[0] != 0x03 {
        bail!("expected 0x03 char-list response code, got {:#x}", buf[0]);
    }
    let count = buf[1] as usize;
    let mut chars = Vec::with_capacity(count);
    for i in 0..count.min(16) {
        let off = 16 * (i + 1);
        let entry = CharListEntry {
            content_id: u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()),
            char_id_main: u16::from_le_bytes(buf[off + 4..off + 6].try_into().unwrap()),
            world_id: buf[off + 6],
            char_id_extra: buf[off + 7],
        };
        chars.push(entry);
    }
    Ok(CharList { characters: chars })
}

/// Parse the size-prefixed `lpkt_chr_info2` push that the server emits to
/// the view socket immediately after handling a data-port 0xA1. Each slot
/// is 76 bytes (`lpkt_chr_info_sub2`); we only extract the fields the
/// launcher cares about — `ffxi_id`, `status`, and `character_name`.
async fn parse_view_chr_info2(stream: &mut TcpStream) -> Result<Vec<CharSlot>> {
    tracing::debug!("lobby: waiting for chr_info2 packet on view socket");
    let mut size_bytes = [0u8; 4];
    stream
        .read_exact(&mut size_bytes)
        .await
        .context("reading lpkt_chr_info2 size header — server may not be sending chr_info2")?;
    let size = u32::from_le_bytes(size_bytes) as usize;
    if size < IXFF_HEADER_SIZE || size > 64 * 1024 {
        bail!("implausible lpkt_chr_info2 size {size}");
    }
    let mut rest = vec![0u8; size - 4];
    stream
        .read_exact(&mut rest)
        .await
        .context("reading lpkt_chr_info2 body")?;

    // Body layout after the 4-byte size we already consumed:
    //   rest[0..4]   terminator
    //   rest[4..8]   command (0x20)
    //   rest[8..24]  identifier (16B)
    //   rest[24..28] characters: u32
    //   rest[28..]   lpkt_chr_info_sub2 [characters]   (76B each)
    if rest.len() < 28 {
        bail!("chr_info2 body too short ({})", rest.len());
    }
    let count = u32::from_le_bytes(rest[24..28].try_into().unwrap()) as usize;
    // 4 ffxi_id + 2 ffxi_id_world + 2 worldid + 2 status + 1 bitfield + 1
    // ffxi_id_world_tbl + 16 character_name + 16 world_name + 96
    // TC_OPERATION_MAKE. The TC_OPERATION_MAKE size is the easy thing to
    // get wrong: the *full* struct in `login_packets.h` runs through
    // skill1..skill9, ChatCounter, PartyCounter, FirstLoginDate, etc. — not
    // the 32-byte truncation I had on first read. Verified empirically by
    // hex-dump: a 16-slot response is 28 + 16*140 = 2268 bytes, matching
    // the wire size when the server fills empty slots.
    const SUB2_SIZE: usize = 140;
    let needed = 28 + count * SUB2_SIZE;
    if rest.len() < needed {
        bail!(
            "chr_info2 body short: have {} bytes, need {} for {} slot(s)",
            rest.len(),
            needed,
            count
        );
    }
    // Diagnostic — emitted at TRACE so it stays out of normal output. Useful
    // when a populated slot looks wrong (off-by-N alignment, unexpected
    // padding, server reusing a stale `data_session`'s `characterInfoResponse`
    // because of session-hash collision). Set RUST_LOG=ffxi_client=trace to see.
    if tracing::enabled!(tracing::Level::TRACE) {
        let total = 28 + count * SUB2_SIZE;
        let hex: String = rest[..rest.len().min(total)]
            .chunks(16)
            .enumerate()
            .map(|(i, chunk)| {
                let off = i * 16;
                let bytes: String = chunk.iter().map(|b| format!("{b:02x} ")).collect();
                format!("  {off:04x}: {bytes}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        tracing::trace!(count, total_bytes = rest.len(), "chr_info2 raw:\n{hex}");
    }
    let mut slots = Vec::with_capacity(count);
    for i in 0..count {
        let off = 28 + i * SUB2_SIZE;
        let char_id = u32::from_le_bytes(rest[off..off + 4].try_into().unwrap());
        let status = u16::from_le_bytes(rest[off + 8..off + 10].try_into().unwrap());
        let name_bytes = &rest[off + 12..off + 28];
        let nul = name_bytes.iter().position(|&b| b == 0).unwrap_or(16);
        let name = String::from_utf8_lossy(&name_bytes[..nul]).into_owned();

        // TC_OPERATION_MAKE block: sub2 layout is
        //   4 ffxi_id + 2 ffxi_id_world + 2 worldid + 2 status +
        //   1 bitfield + 1 ffxi_id_world_tbl + 16 char_name +
        //   16 world_name + 96 TC_OPERATION_MAKE
        // → TC_OPERATION_MAKE starts at sub2 offset 44. Field
        // layout via `vendor/server/src/login/login_packets.h:85`:
        //   off+ 0..2  uint16 mon_no         (low byte = race)
        //   off+ 2..4  uint8 mjob_no, sjob_no
        //   off+ 4..6  uint16 face_no
        //   off+ 6..12 town, gen_flag, hair, size, world_no
        //   off+12..28 uint16 GrapIDTbl[8]
        //
        // GrapIDTbl ordering for chr_info2 is **shifted by one** vs
        // the in-game `EntityLook::Equipped` block — LSB writes
        // [0]=face, [1]=head, [2]=body, [3]=hands, [4]=legs,
        // [5]=feet, [6]=main, [7]=sub (no ranged).
        // See `vendor/server/src/login/data_session.cpp:191-197`.
        //
        // Values stored in chr_info2 are **raw database item-model
        // ids** (12-bit), NOT the slot-tagged form CHAR_PC uses
        // (high nibble = slot tag). `dat_vos2::spawn_equipped`
        // calls `resolve_equipment_slot` which needs the slot tag,
        // so we OR it in here per canonical slot order (head=1,
        // body=2, … sub=7).
        //
        // Empty / dummy slots arrive with zero TC_OPERATION_MAKE
        // — appearance fields then read as zero and the model
        // won't render; the launcher should treat zero `race` as
        // "no preview".
        let tc = off + 44;
        let mon_no = u16::from_le_bytes(rest[tc..tc + 2].try_into().unwrap());
        let race = (mon_no & 0xFF) as u8;
        let face_u16 = u16::from_le_bytes(rest[tc + 4..tc + 6].try_into().unwrap());
        let face = (face_u16 & 0xFF) as u8;
        let grap = |i: usize| -> u16 {
            let o = tc + 12 + i * 2;
            u16::from_le_bytes(rest[o..o + 2].try_into().unwrap())
        };
        // Slot-tag the raw item-model id with its canonical slot
        // index (1=head, 2=body, ... 7=sub) so
        // `resolve_equipment_slot` can route to the right tier
        // formula. We mask the low 12 bits in case LSB ever
        // started shipping pre-tagged ids — keeps the OR
        // idempotent.
        let tag = |slot_idx: u16, raw: u16| -> u16 { (slot_idx << 12) | (raw & 0x0FFF) };
        // GrapIDTbl[0] is `face` (also redundantly stored in
        // face_no above) — skip it; we want index 1 onward.
        let head = tag(1, grap(1));
        let body = tag(2, grap(2));
        let hands = tag(3, grap(3));
        let legs = tag(4, grap(4));
        let feet = tag(5, grap(5));
        let main = tag(6, grap(6));
        let sub = tag(7, grap(7));
        // chr_info2 doesn't carry ranged — leave it empty. Items
        // with id 0 get sentinel-rejected by
        // resolve_equipment_slot so this is safe.
        let ranged = 0u16;

        // TC_OPERATION_MAKE layout: zone_no at struct-offset 28
        // (after mon_no/mjob/sjob/face/town/gen/hair/size/world +
        // GrapIDTbl[8]=16B), zone_no2 at struct-offset 34 (after
        // mjob_level/open_flag/GMCallCounter/version/skill1). LSB
        // writes `zone_no = zone & 0xFF; zone_no2 = (zone >> 8) & 1;`.
        let zone_no = rest[tc + 28];
        let zone_no2 = rest[tc + 34];
        let zone_id = (zone_no as u16) | (((zone_no2 & 0x01) as u16) << 8);

        slots.push(CharSlot {
            char_id,
            name,
            status,
            race,
            face,
            head,
            body,
            hands,
            legs,
            feet,
            main,
            sub,
            ranged,
            zone_id,
        });
    }
    Ok(slots)
}

async fn read_lpkt_next_login(
    stream: &mut TcpStream,
    char_id: u32,
    key3: &[u8; 20],
) -> Result<MapHandoff> {
    let mut buf = vec![0u8; NEXT_LOGIN_PACKET_SIZE as usize];
    stream
        .read_exact(&mut buf)
        .await
        .context("reading lpkt_next_login")?;

    let packet_size = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    if packet_size != NEXT_LOGIN_PACKET_SIZE {
        bail!(
            "lpkt_next_login: unexpected packet_size {:#x} (want {:#x})",
            packet_size,
            NEXT_LOGIN_PACKET_SIZE
        );
    }
    let term = u32::from_le_bytes(buf[4..8].try_into().unwrap());
    if term != IXFF_TERMINATOR {
        bail!("lpkt_next_login: bad terminator {term:#x}");
    }
    let cmd = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    if cmd != VIEW_RESP_NEXT_LOGIN {
        bail!("lpkt_next_login: unexpected command {cmd:#x}");
    }

    let resp_char_id = u32::from_le_bytes(buf[28..32].try_into().unwrap());
    if resp_char_id != char_id {
        bail!("lpkt_next_login char_id {resp_char_id:#x} != requested {char_id:#x}");
    }

    let name_bytes = &buf[36..52];
    let nul = name_bytes.iter().position(|&b| b == 0).unwrap_or(16);
    let character_name = String::from_utf8_lossy(&name_bytes[..nul]).into_owned();

    // server_id at [52..56], server_ip at [56..60], server_port at [60..64].
    let server_ip = u32::from_le_bytes(buf[56..60].try_into().unwrap());
    let server_port = u32::from_le_bytes(buf[60..64].try_into().unwrap()) as u16;

    Ok(MapHandoff {
        char_id,
        character_name,
        server_ip,
        server_port,
        session_key_seed: *key3,
    })
}
