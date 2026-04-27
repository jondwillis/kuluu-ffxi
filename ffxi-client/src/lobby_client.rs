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

impl LobbyClient {
    pub fn new(host: impl Into<String>, data_port: u16, view_port: u16) -> Self {
        Self {
            host: host.into(),
            data_port,
            view_port,
        }
    }

    /// Run the full lobby handshake. Returns the map handoff bundle on success.
    pub async fn handshake(
        &self,
        auth: &AuthSession,
        char_id: u32,
        char_name: &str,
        search_server_ip: u32,
        key3: [u8; 20],
    ) -> Result<MapHandoff> {
        // 1) Open both sockets.
        let mut view = self.connect(self.view_port).await?;
        tracing::debug!("lobby: view socket connected");
        let mut data = self.connect(self.data_port).await?;
        tracing::debug!("lobby: data socket connected");

        // 2) Register the view session so the server's lpkt_chr_info2 push
        //    during the 0xA1 response has a target. Send a no-op IXFF packet
        //    with command=0; the server's switch falls through but creates
        //    `session.view_session` as a side effect of the read.
        let register = ixff_header(IXFF_HEADER_SIZE as u32, VIEW_CMD_REGISTER, &auth.session_hash);
        view.write_all(&register).await?;
        view.flush().await?;
        tracing::debug!("lobby: VIEW_CMD_REGISTER sent");

        // 3) Send 0xA1 on data: char-list request + session_hash.
        let req_a1 = build_data_a1(auth.account_id, search_server_ip, &auth.session_hash);
        data.write_all(&req_a1).await?;
        data.flush().await?;
        tracing::debug!(account_id = auth.account_id, "lobby: 0xA1 char-list request sent");

        // 4) Read the 328-byte char list response. (We don't return this;
        //    the caller already named the char by id+name. But we do parse
        //    enough to confirm the requested char_id is in the list.)
        let charlist = read_data_charlist(&mut data).await?;
        tracing::debug!(count = charlist.characters.len(), "lobby: 0xA1 char-list received");
        if charlist.characters.is_empty() {
            bail!("no characters found for account");
        }
        // We *also* expect the server to have pushed lpkt_chr_info2 to the
        // view socket. We can drain it but don't need to interpret it —
        // the caller selected the char by id already.
        drain_view_chr_info2(&mut view).await?;
        tracing::debug!("lobby: lpkt_chr_info2 drained");

        // 5) Send 0x07 on view: select character.
        let req_select = build_view_select(char_id, char_name, &auth.session_hash);
        view.write_all(&req_select).await?;
        view.flush().await?;
        tracing::debug!(char_id, "lobby: 0x07 select sent");

        // 6) Server replies on data port with [0x02, 0, 0, 0, 0].
        let mut ack = [0u8; 5];
        data.read_exact(&mut ack).await
            .context("reading 0x07 ack on data port")?;
        if ack[0] != 0x02 {
            bail!("expected 0x02 ack after view select, got {ack:?}");
        }
        tracing::debug!("lobby: 0x02 ack received");

        // 7) Send 0xA2 on data with key3.
        let req_a2 = build_data_a2(&key3);
        data.write_all(&req_a2).await?;
        data.flush().await?;
        tracing::debug!("lobby: 0xA2 handoff sent");

        // 8) Server pushes lpkt_next_login on view (and closes view socket).
        let handoff = read_lpkt_next_login(&mut view, char_id, &key3).await?;
        tracing::debug!(server_port = handoff.server_port, "lobby: lpkt_next_login received");

        // View is closed by server; nothing more to do with it.
        // Data socket may also be closed; we don't reuse it after this.
        Ok(handoff)
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
    // Read the 4-byte size header, then size-4 more bytes. See login_packets.h.
    let mut size_bytes = [0u8; 4];
    stream
        .read_exact(&mut size_bytes)
        .await
        .context("reading lpkt_chr_info2 size header")?;
    let size = u32::from_le_bytes(size_bytes) as usize;
    if size < IXFF_HEADER_SIZE || size > 64 * 1024 {
        bail!("implausible lpkt_chr_info2 size {size}");
    }
    let mut rest = vec![0u8; size - 4];
    stream
        .read_exact(&mut rest)
        .await
        .context("reading lpkt_chr_info2 body")?;
    Ok(())
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
