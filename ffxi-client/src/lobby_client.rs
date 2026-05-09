//! Lobby flow — owns both the data (54230) and view (54001) TLS sockets and
//! runs the IXFF binary handshake from "auth done" to "we know the map server
//! IP/port and the 20-byte session key seed that derives our Blowfish key".
//!
//! Wire protocol mostly inferred from `server/src/login/data_session.cpp` and
//! `view_session.cpp`. v1 covers the happy path only (no char create/delete).

use anyhow::{Context, Result, bail};
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
const VIEW_CMD_CREATE: u32 = 0x01;
const VIEW_CMD_SELECT: u32 = 0x07;
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
#[derive(Debug, Clone)]
pub struct CharSlot {
    pub char_id: u32,
    pub name: String,
    pub status: u16,
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
        tracing::debug!(server_port = handoff.server_port, "lobby: lpkt_next_login received");
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

        let register = ixff_header(IXFF_HEADER_SIZE as u32, VIEW_CMD_REGISTER, &auth.session_hash);
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
        tracing::debug!(account_id = auth.account_id, "lobby: 0xA1 char-list request sent");

        let charlist = read_data_charlist(&mut data).await?;
        tracing::debug!(count = charlist.characters.len(), "lobby: 0xA1 char-list received");
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

    pub async fn create_character(
        &self,
        auth: &AuthSession,
        name: &str,
        race: u8,
        job: u8,
        body_type: u8,
        gender: u8,
        face_type: u8,
        tail_type: u8,
    ) -> Result<()> {
        let mut view = self.connect(self.view_port).await?;
        let register = ixff_header(IXFF_HEADER_SIZE as u32, VIEW_CMD_REGISTER, &auth.session_hash);
        view.write_all(&register).await?;
        view.flush().await?;

        // Drain the initial lpkt_chr_info2 push from the register
        drain_view_chr_info2(&mut view).await?;

        let create = build_view_create(name, race, job, body_type, gender, face_type, tail_type, &auth.session_hash);
        tracing::debug!(payload_size = create.len(), "sending create packet");
        view.write_all(&create).await?;
        view.flush().await?;

        // Server responds with a size-prefixed packet
        let mut size_bytes = [0u8; 4];
        view.read_exact(&mut size_bytes).await
            .context("reading create response size")?;
        let size = u32::from_le_bytes(size_bytes) as usize;
        tracing::debug!(response_size = size, "received create response size");
        if size < IXFF_HEADER_SIZE || size > 64 * 1024 {
            bail!("implausible create response size {size}");
        }
        let mut body = vec![0u8; size - 4];
        view.read_exact(&mut body).await
            .context("reading create response body")?;
        tracing::debug!(response_len = body.len(), "received create response");
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

fn build_view_create(
    name: &str,
    race: u8,
    job: u8,
    body_type: u8,
    gender: u8,
    face_type: u8,
    tail_type: u8,
    session_hash: &[u8; 16],
) -> Vec<u8> {
    // Layout matches lpkt_chr_create from login_packets.h:
    // header(28) + name(16) + race(1) + job(1) + body_type(1) + gender(1) + face(1) + tail(1) = 50 bytes
    // packet_size = 50
    let payload_size = 22u32; // name(16) + race + job + body_type + gender + face + tail
    let mut payload = vec![0u8; payload_size as usize];
    // [0..16] character_name (NUL-padded)
    let name_bytes = name.as_bytes();
    let n = name_bytes.len().min(15);
    payload[0..n].copy_from_slice(&name_bytes[..n]);
    // [16] race
    payload[16] = race;
    // [17] job
    payload[17] = job;
    // [18] body_type
    payload[18] = body_type;
    // [19] gender
    payload[19] = gender;
    // [20] face_type
    payload[20] = face_type;
    // [21] tail_type
    payload[21] = tail_type;

    let packet_size = payload_size + IXFF_HEADER_SIZE as u32;
    let mut out = vec![0u8; packet_size as usize];
    out[0..4].copy_from_slice(&packet_size.to_le_bytes());
    out[4..8].copy_from_slice(&IXFF_TERMINATOR.to_le_bytes());
    out[8..12].copy_from_slice(&VIEW_CMD_CREATE.to_le_bytes());
    out[12..28].copy_from_slice(session_hash);
    out[28..].copy_from_slice(&payload);
    out
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

async fn drain_view_chr_info2(stream: &mut TcpStream) -> Result<()> {
    parse_view_chr_info2(stream).await.map(|_| ())
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
                let bytes: String =
                    chunk.iter().map(|b| format!("{b:02x} ")).collect();
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
        slots.push(CharSlot {
            char_id,
            name,
            status,
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
        bail!(
            "lpkt_next_login char_id {resp_char_id:#x} != requested {char_id:#x}"
        );
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
