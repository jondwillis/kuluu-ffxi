use anyhow::{bail, Context, Result};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use ffxi_proto::login::IXFF_TERMINATOR;

use crate::auth_client::AuthSession;

const DATA_CHARLIST_SIZE: usize = 0x148;

const IXFF_HEADER_SIZE: usize = 28;

const DATA_CMD_CHAR_LIST: u8 = 0xA1;
const DATA_CMD_HANDOFF: u8 = 0xA2;

const VIEW_CMD_REGISTER: u32 = 0x00;
const VIEW_CMD_SELECT: u32 = 0x07;

const VIEW_CMD_DELETE_CHAR: u32 = 0x14;

const VIEW_CMD_REGISTER_CHAR: u32 = 0x21;

const VIEW_CMD_NAME_CHECK: u32 = 0x22;

const VIEW_RESP_NEXT_LOGIN: u32 = 0x0B;

const NEXT_LOGIN_PACKET_SIZE: u32 = 0x48;

const LOBBY_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

async fn lobby_io<T>(
    step: &'static str,
    fut: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    match tokio::time::timeout(LOBBY_IO_TIMEOUT, fut).await {
        Ok(inner) => inner,
        Err(_elapsed) => {
            tracing::warn!(
                step,
                secs = LOBBY_IO_TIMEOUT.as_secs(),
                "lobby step timed out waiting for server"
            );
            bail!(
                "lobby {step}: server did not respond within {}s",
                LOBBY_IO_TIMEOUT.as_secs()
            )
        }
    }
}

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
pub struct CharSlot {
    pub char_id: u32,
    pub name: String,
    pub status: u16,

    pub race: u8,

    pub face: u8,

    pub head: u16,
    pub body: u16,
    pub hands: u16,
    pub legs: u16,
    pub feet: u16,
    pub main: u16,
    pub sub: u16,
    pub ranged: u16,

    pub zone_id: u16,
}

#[derive(Debug, Clone)]
pub struct MapHandoff {
    pub char_id: u32,
    pub character_name: String,
    pub server_ip: u32,
    pub server_port: u16,

    pub session_key_seed: [u8; 20],
}

pub struct LobbyClient {
    pub host: String,
    pub data_port: u16,
    pub view_port: u16,
}

pub struct LobbyHandle {
    view: TcpStream,
    data: TcpStream,

    chars: Vec<CharSlot>,
    session_hash: [u8; 16],
}

impl LobbyHandle {
    pub fn chars(&self) -> &[CharSlot] {
        &self.chars
    }

    pub async fn create_character(
        mut self,
        auth: &AuthSession,
        spec: &CharCreateSpec,
    ) -> Result<Self> {
        let name_check = build_view_name_check(&spec.name, &self.session_hash);
        self.view.write_all(&name_check).await?;
        self.view.flush().await?;
        tracing::debug!(name = %spec.name, "lobby handle: 0x22 name check sent");
        read_create_reply(&mut self.view, "name check")
            .await
            .context("0x22 name check response")?;

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

        let req_a1 = build_data_a1(auth.account_id, 0, &self.session_hash);
        self.data.write_all(&req_a1).await?;
        self.data.flush().await?;

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

    pub async fn delete_character(mut self, auth: &AuthSession, char_id: u32) -> Result<Self> {
        let pkt = build_delete_char(char_id, 0, [0u8; 16], &self.session_hash);
        self.view.write_all(&pkt).await?;
        self.view.flush().await?;
        tracing::debug!(char_id, "lobby handle: 0x14 delete-char sent");

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

    pub async fn select(
        mut self,
        char_id: u32,
        char_name: &str,
        key3: [u8; 20],
    ) -> Result<MapHandoff> {
        let req_select = build_view_select(char_id, char_name, &self.session_hash);
        self.view.write_all(&req_select).await?;
        self.view.flush().await?;
        tracing::info!(char_id, "lobby: 0x07 select sent");

        let mut ack = [0u8; 5];
        lobby_io("0x02 ack (data)", async {
            self.data
                .read_exact(&mut ack)
                .await
                .context("reading 0x07 ack on data port")?;
            Ok(())
        })
        .await?;
        if ack[0] != 0x02 {
            bail!("expected 0x02 ack after view select, got {ack:?}");
        }
        tracing::info!("lobby: 0x02 ack received");

        let req_a2 = build_data_a2(&key3);
        self.data.write_all(&req_a2).await?;
        self.data.flush().await?;
        tracing::info!("lobby: 0xA2 handoff sent");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let handoff = lobby_io(
            "lpkt_next_login (view)",
            read_lpkt_next_login(&mut self.view, char_id, &key3),
        )
        .await?;
        tracing::info!(
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

    pub async fn open(&self, auth: &AuthSession) -> Result<LobbyHandle> {
        let mut view = self.connect(self.view_port).await?;
        tracing::info!("lobby: view socket connected");
        let mut data = self.connect(self.data_port).await?;
        tracing::info!("lobby: data socket connected");

        let register = ixff_header(
            IXFF_HEADER_SIZE as u32,
            VIEW_CMD_REGISTER,
            &auth.session_hash,
        );
        view.write_all(&register).await?;
        view.flush().await?;
        tracing::info!("lobby: VIEW_CMD_REGISTER sent");

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let req_a1 = build_data_a1(auth.account_id, 0, &auth.session_hash);
        data.write_all(&req_a1).await?;
        data.flush().await?;
        tracing::info!(
            account_id = auth.account_id,
            "lobby: 0xA1 char-list request sent"
        );

        let charlist = lobby_io("0xA1 char-list (data)", read_data_charlist(&mut data)).await?;
        tracing::info!(
            count = charlist.characters.len(),
            "lobby: 0xA1 char-list received"
        );
        let slots = lobby_io("chr_info2 (view)", parse_view_chr_info2(&mut view)).await?;

        let chars: Vec<CharSlot> = slots
            .into_iter()
            .filter(|s| s.char_id != 0 && !s.name.starts_with(' '))
            .collect();
        tracing::info!(populated = chars.len(), "lobby: chr_info2 parsed");

        Ok(LobbyHandle {
            view,
            data,
            chars,
            session_hash: auth.session_hash,
        })
    }

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

    pub async fn create_character(&self, auth: &AuthSession, spec: &CharCreateSpec) -> Result<()> {
        let handle = self.open(auth).await?;
        let mut view = handle.view;
        tracing::debug!("create_character: lobby open complete");

        let name_check = build_view_name_check(&spec.name, &auth.session_hash);
        view.write_all(&name_check).await?;
        view.flush().await?;
        tracing::debug!(name = %spec.name, "create_character: 0x22 name check sent");
        read_create_reply(&mut view, "name check")
            .await
            .context("0x22 name check response")?;

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
        lobby_io("TCP connect", async {
            TcpStream::connect((self.host.as_str(), port))
                .await
                .with_context(|| format!("TCP connect to {}:{}", self.host, port))
        })
        .await
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
    let mut buf = vec![0u8; 28];
    buf[0] = DATA_CMD_CHAR_LIST;
    buf[1..5].copy_from_slice(&account_id.to_le_bytes());
    buf[5..9].copy_from_slice(&search_server_ip.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);
    buf
}

fn build_data_a2(key3: &[u8; 20]) -> Vec<u8> {
    let mut buf = vec![0u8; 28];
    buf[0] = DATA_CMD_HANDOFF;
    buf[1..21].copy_from_slice(key3);
    buf
}

fn build_view_select(char_id: u32, char_name: &str, session_hash: &[u8; 16]) -> Vec<u8> {
    let packet_size = 0x44u32;
    let mut buf = vec![0u8; packet_size as usize];
    buf[0..4].copy_from_slice(&packet_size.to_le_bytes());
    buf[4..8].copy_from_slice(&IXFF_TERMINATOR.to_le_bytes());
    buf[8..12].copy_from_slice(&VIEW_CMD_SELECT.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);
    buf[28..32].copy_from_slice(&char_id.to_le_bytes());

    buf[32..36].copy_from_slice(&(char_id & 0xFFFF).to_le_bytes());
    let name_bytes = char_name.as_bytes();
    let n = name_bytes.len().min(15);
    buf[36..36 + n].copy_from_slice(&name_bytes[..n]);
    buf
}

pub fn build_delete_char(
    content_id: u32,
    world_id: u32,
    passwd: [u8; 16],
    session_hash: &[u8; 16],
) -> Vec<u8> {
    let packet_size = 0x34u32;
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

#[derive(Debug, Clone)]
pub struct CharCreateSpec {
    pub name: String,

    pub race: u8,

    pub job: u8,

    pub nation: u8,

    pub size: u8,

    pub face: u8,
}

fn build_view_name_check(name: &str, session_hash: &[u8; 16]) -> Vec<u8> {
    let packet_size = 0x40u32;
    let mut buf = vec![0u8; packet_size as usize];
    buf[0..4].copy_from_slice(&packet_size.to_le_bytes());
    buf[4..8].copy_from_slice(&IXFF_TERMINATOR.to_le_bytes());
    buf[8..12].copy_from_slice(&VIEW_CMD_NAME_CHECK.to_le_bytes());
    buf[12..28].copy_from_slice(session_hash);

    let name_bytes = name.as_bytes();
    let n = name_bytes.len().min(15);
    buf[32..32 + n].copy_from_slice(&name_bytes[..n]);
    buf
}

fn build_view_register_char(
    race: u8,
    job: u8,
    nation: u8,
    size: u8,
    face: u8,
    session_hash: &[u8; 16],
) -> Vec<u8> {
    let packet_size = 0x40u32;
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

    let term = u32::from_le_bytes(rest[0..4].try_into().unwrap());
    if term != IXFF_TERMINATOR {
        bail!("{stage}: bad terminator {term:#x}");
    }
    let result = rest[4];
    match (size, result) {
        (0x20, 0x03) => Ok(()),
        (0x24, 0x04) => {
            let err = u16::from_le_bytes(rest[28..30].try_into().unwrap());
            bail!(
                "{stage}: server rejected with loginErrors code {err} ({})",
                login_error_name(err)
            );
        }
        _ => bail!("{stage}: unexpected reply size={size:#x} result={result:#x}"),
    }
}

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

async fn parse_view_chr_info2(stream: &mut TcpStream) -> Result<Vec<CharSlot>> {
    tracing::debug!("lobby: waiting for chr_info2 packet on view socket");
    let mut size_bytes = [0u8; 4];
    stream
        .read_exact(&mut size_bytes)
        .await
        .context("reading lpkt_chr_info2 size header — server may not be sending chr_info2")?;
    let size = u32::from_le_bytes(size_bytes) as usize;
    if !(IXFF_HEADER_SIZE..=64 * 1024).contains(&size) {
        bail!("implausible lpkt_chr_info2 size {size}");
    }
    let mut rest = vec![0u8; size - 4];
    stream
        .read_exact(&mut rest)
        .await
        .context("reading lpkt_chr_info2 body")?;

    if rest.len() < 28 {
        bail!("chr_info2 body too short ({})", rest.len());
    }
    let count = u32::from_le_bytes(rest[24..28].try_into().unwrap()) as usize;

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

        let tc = off + 44;
        let mon_no = u16::from_le_bytes(rest[tc..tc + 2].try_into().unwrap());
        let race = (mon_no & 0xFF) as u8;
        let face_u16 = u16::from_le_bytes(rest[tc + 4..tc + 6].try_into().unwrap());
        let face = (face_u16 & 0xFF) as u8;
        let grap = |i: usize| -> u16 {
            let o = tc + 12 + i * 2;
            u16::from_le_bytes(rest[o..o + 2].try_into().unwrap())
        };

        let tag = |slot_idx: u16, raw: u16| -> u16 { (slot_idx << 12) | (raw & 0x0FFF) };

        let head = tag(1, grap(1));
        let body = tag(2, grap(2));
        let hands = tag(3, grap(3));
        let legs = tag(4, grap(4));
        let feet = tag(5, grap(5));
        let main = tag(6, grap(6));
        let sub = tag(7, grap(7));

        let ranged = 0u16;

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
