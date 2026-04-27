//! Map-server UDP client. v1 brings a character all the way from "got map IP
//! and port from the lobby" through "spawned in zone, sending position
//! keepalives". The wire details mirror `server/src/map/map_networking.cpp`.

use std::net::SocketAddr;

use anyhow::{Context, Result, anyhow, bail};
use tokio::net::UdpSocket;

use ffxi_proto::{blowfish, framing, md5, zlib};

/// `sizeof(GP_CLI_LOGIN)` — see `server/src/map/packets/c2s/0x00a_login.h`.
/// Layout: 4 bytes header + 88 bytes body = 92 bytes (no trailing padding;
/// the largest field alignment is 4 and 92 is 4-aligned).
pub const GP_CLI_LOGIN_SIZE: usize = 92;

/// Total wire size of an unencrypted bootstrap 0x00A datagram:
/// FFXI header (28) + GP_CLI_LOGIN (92) + MD5 trailer (16) = 136.
pub const BOOTSTRAP_DATAGRAM_SIZE: usize = framing::FFXI_HEADER_SIZE + GP_CLI_LOGIN_SIZE + 16;

#[derive(Debug, Clone)]
pub struct BootstrapArgs<'a> {
    pub char_id: u32,
    pub char_name: &'a str,
    pub account_name: &'a str,
    /// 16-byte session_hash from the auth response. Server stores it in
    /// `Ticket[]` but does not currently validate it; safe to pass through.
    pub ticket: [u8; 16],
    /// Client version (cosmetic; server doesn't gate on it for v1 flow).
    pub version: u32,
    /// 4-byte platform tag, e.g. `b"PC\0\0"` or `b"PS2\0"`.
    pub platform: [u8; 4],
    /// Client language (`uCliLang`). 0 is fine; xiloader sends 0.
    pub cli_lang: u16,
}

pub struct MapClient {
    socket: UdpSocket,
    server: SocketAddr,
    blowfish: blowfish::State,
    decompress_table: zlib::DecompressTable,
    compress_table: zlib::CompressTable,
    /// Snapshot of the 16-byte MD5(key_seed) used to seed Blowfish. The
    /// server stores this in `accounts_sessions.session_key` (20 bytes), then
    /// MD5s to derive the cipher; our `seed` is the 20-byte original.
    pub seed: [u8; 20],
}

impl MapClient {
    pub async fn connect(server: SocketAddr, seed: [u8; 20]) -> Result<Self> {
        Self::connect_with_local(server, seed, "0.0.0.0:0").await
    }

    pub async fn connect_with_local(
        server: SocketAddr,
        seed: [u8; 20],
        local: &str,
    ) -> Result<Self> {
        let socket = UdpSocket::bind(local)
            .await
            .with_context(|| format!("UDP bind {local}"))?;
        tracing::info!(local_addr = %socket.local_addr()?, "UDP socket bound");
        let blowfish = derive_blowfish(&seed);
        let decompress_table = zlib::DecompressTable::new()
            .map_err(|e| anyhow!("decompress table init: {e}"))?;
        let compress_table = zlib::CompressTable::new()
            .map_err(|e| anyhow!("compress table init: {e}"))?;
        Ok(Self {
            socket,
            server,
            blowfish,
            decompress_table,
            compress_table,
            seed,
        })
    }

    /// Build and send the unencrypted GP_CLI_COMMAND_LOGIN bootstrap packet.
    /// Send an *encrypted* bundle of sub-packets. The payload should be a
    /// concatenation of sub-packet bodies (each starts with its own 4-byte
    /// `GP_CLI_HEADER` of `id|size_words|sync`, followed by body bytes).
    pub async fn send_encrypted(
        &self,
        sub_packets_payload: &[u8],
        bundle_seq: u16,
        ack_server_seq: u16,
    ) -> Result<()> {
        // 1) FFXI custom-zlib compress.
        let (compressed_bits, compressed) = self
            .compress_table
            .compress(sub_packets_payload)
            .map_err(|e| anyhow!("compress: {e}"))?;

        // 2) Build the post-header tail: compressed bytes + u32 bit count + 16-byte MD5.
        let mut tail = compressed;
        tail.extend_from_slice(&(compressed_bits as u32).to_le_bytes());
        let digest = ffxi_proto::md5::md5(&tail);
        tail.extend_from_slice(&digest);

        // 3) Build the wire frame: 28-byte header + tail.
        let mut frame = vec![0u8; framing::FFXI_HEADER_SIZE + tail.len()];
        frame[0..2].copy_from_slice(&bundle_seq.to_le_bytes());
        frame[2..4].copy_from_slice(&ack_server_seq.to_le_bytes());
        frame[4..6].copy_from_slice(&bundle_seq.to_le_bytes());
        // bytes 6..28 stay zero (server overwrites timestamp at offset 8 on send).
        frame[framing::FFXI_HEADER_SIZE..].copy_from_slice(&tail);

        // 4) Encrypt u32 pairs from offset 28.
        framing::encrypt_in_place(&mut frame, &self.blowfish);

        let n = self
            .socket
            .send_to(&frame, &self.server)
            .await
            .context("UDP send_to (encrypted)")?;
        if n != frame.len() {
            bail!("partial encrypted UDP send: {n} of {}", frame.len());
        }
        Ok(())
    }

    pub async fn send_bootstrap(&self, args: &BootstrapArgs<'_>) -> Result<()> {
        let datagram = build_bootstrap_packet(args)?;
        debug_assert_eq!(datagram.len(), BOOTSTRAP_DATAGRAM_SIZE);
        let n = self
            .socket
            .send_to(&datagram, &self.server)
            .await
            .context("UDP send_to")?;
        if n != datagram.len() {
            bail!("partial UDP send: {n} of {}", datagram.len());
        }
        Ok(())
    }

    /// Receive one UDP datagram, decrypt it in place, return the decompressed
    /// (header + payload) bytes. Caller walks sub-packets from
    /// `bytes[FFXI_HEADER_SIZE..]`.
    pub async fn recv_decrypted(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; ffxi_proto::map::MAX_DATAGRAM];
        let (n, src) = self
            .socket
            .recv_from(&mut buf)
            .await
            .context("UDP recv_from")?;
        tracing::info!(bytes = n, src = %src, "recv");
        buf.truncate(n);

        if n < framing::MIN_FRAME_SIZE {
            bail!("undersized response datagram: {n} bytes");
        }

        // Decrypt u32 pairs from offset 28 onward.
        framing::decrypt_in_place(&mut buf, &self.blowfish);

        // The compressed payload size lives in the 4 bytes immediately before
        // the trailing 16-byte MD5 — `buf[n-20 .. n-16]` (matches
        // map_networking.cpp:371).
        if n < framing::FFXI_HEADER_SIZE + 20 {
            bail!("response too short to hold size+md5 trailer: {n}");
        }
        let size_off = n - 4 - 16;
        // The u32 here is the *bit* count of the compressed payload, not the
        // byte count — see `server/src/common/zlib.h::zlib_compressed_size`
        // and `zlib_compress()` returning `read + 8`. Convert with `(bits+7)/8`.
        let compressed_bits =
            u32::from_le_bytes(buf[size_off..size_off + 4].try_into().unwrap()) as usize;
        let compressed_bytes = (compressed_bits + 7) / 8;
        let compressed_end = framing::FFXI_HEADER_SIZE + compressed_bytes;
        if compressed_end > size_off {
            bail!(
                "compressed payload {compressed_bytes} bytes ({compressed_bits} bits) overruns size field at {size_off}"
            );
        }

        // Verify trailing MD5 over [28 .. n-16].
        let body_range = framing::FFXI_HEADER_SIZE..(n - 16);
        let trailer: [u8; 16] = buf[n - 16..n].try_into().unwrap();
        let computed = md5::md5(&buf[body_range.clone()]);
        if computed != trailer {
            bail!(
                "MD5 trailer mismatch — got {} expected {}",
                hex16(&computed),
                hex16(&trailer)
            );
        }
        let payload_bits = compressed_bits.saturating_sub(8); // strip the magic byte
        let decompressed = self
            .decompress_table
            .decompress(
                &buf[framing::FFXI_HEADER_SIZE..compressed_end],
                payload_bits,
            )
            .map_err(|e| anyhow!("decompress: {e}"))?;
        let mut out = Vec::with_capacity(framing::FFXI_HEADER_SIZE + decompressed.len());
        out.extend_from_slice(&buf[..framing::FFXI_HEADER_SIZE]);
        out.extend_from_slice(&decompressed);
        Ok(out)
    }

    pub fn server_addr(&self) -> SocketAddr {
        self.server
    }
}

/// Apply the same key rotation the server runs in
/// `MapSession::incrementBlowfish()` — `blowfish.key[4] += 2`, where `key` is
/// `uint32[5]` covering the 20-byte session_key. We mirror this by reading
/// the last 4 bytes of the seed as a little-endian u32, adding 2 with
/// wrapping semantics, and writing back. **The server does this AFTER
/// sending the encrypted `0x00B`** — i.e. the 0x00B is enciphered with the
/// *current* key; the next bootstrap on the new map address must use the
/// rotated key.
pub fn rotate_session_key_seed(seed: &mut [u8; 20]) {
    let cur = u32::from_le_bytes(seed[16..20].try_into().unwrap());
    let next = cur.wrapping_add(2);
    seed[16..20].copy_from_slice(&next.to_le_bytes());
}

/// Derive the Blowfish state from the 20-byte session-key seed stored in
/// `accounts_sessions.session_key`.
///
/// Mirrors `server/src/map/map_session.cpp::initBlowfish` exactly:
///
/// ```cpp
/// md5(key[20], hash, 20);
/// for (i=0; i<16; ++i) if (hash[i]==0) { memset(hash+i, 0, 16-i); break; }
/// blowfish_init(hash, /*keybytes=*/16, P, S);
/// ```
///
/// Two subtleties that bit me on the first attempt:
/// 1. **`keybytes` is hardcoded to 16**, not the position of the first NUL.
///    The cycling XOR in `blowfish_init` therefore wraps modulo 16 even
///    though some trailing key bytes are now zero.
/// 2. The NUL-then-memset behavior means: if `hash[5]` is the first zero,
///    `hash[5..16]` all become zero. This *does* differ from a raw
///    truncated-at-NUL key in the cycling XOR.
fn derive_blowfish(seed: &[u8; 20]) -> blowfish::State {
    let mut hash = md5::md5(seed);
    if let Some(zero_at) = hash.iter().position(|&b| b == 0) {
        for b in &mut hash[zero_at..] {
            *b = 0;
        }
    }
    blowfish::State::new(&hash)
}

fn build_bootstrap_packet(args: &BootstrapArgs<'_>) -> Result<Vec<u8>> {
    let mut frame = vec![0u8; BOOTSTRAP_DATAGRAM_SIZE];

    // FFXI bundle header.
    //
    // [0..2]   SmallPD_Code — bundle-level "this packet's sequence" cap.
    //          The parse() loop in `map_networking.cpp:425` skips any
    //          sub-packet whose sequence > SmallPD_Code, AND <= client_packet_id
    //          (which the bootstrap path resets to 0). So both must be > 0
    //          and ≤ SmallPD_Code or the only sub-packet (0x00A) is silently
    //          dropped, leaving the server idle and us never receiving the
    //          login response.
    // [2..4]   sync_in — our view of the server's last sent packet id (0 here).
    // [4..6]   sync_out — outgoing seq (mirror of SmallPD_Code).
    // [6..28]  scratch / padding — leave zero.
    frame[0..2].copy_from_slice(&1u16.to_le_bytes());
    frame[4..6].copy_from_slice(&1u16.to_le_bytes());

    // GP_CLI_LOGIN body (offset 28..120 = 28..28+92).
    let body = &mut frame[framing::FFXI_HEADER_SIZE..framing::FFXI_HEADER_SIZE + GP_CLI_LOGIN_SIZE];

    // GP_CLI_HEADER (4 bytes): id (9 bits) + size (7 bits) | sync u16.
    // size_value = sizeof(GP_CLI_LOGIN) / 4 = 23. Verify the bit-fields:
    //   wire_byte_0 = id & 0xFF                              // = 0x0A
    //   wire_byte_1 = ((id >> 8) & 1) | ((size << 1) & 0xFE) // = 0x2E
    let id: u16 = 0x00A;
    let size_words: u16 = (GP_CLI_LOGIN_SIZE / 4) as u16; // 23
    let header_word = id | (size_words << 9);
    body[0..2].copy_from_slice(&header_word.to_le_bytes());
    // sync at offset 2..4 of sub-packet header — must be in (client_packet_id,
    // SmallPD_Code], i.e. > 0 and ≤ 1. Use 1 to match our bundle SmallPD_Code.
    body[2..4].copy_from_slice(&1u16.to_le_bytes());

    // GP_CLI_LOGIN payload (offsets 4..92 of the struct).
    // [4]   LoginPacketCheck u8 — computed below.
    // [5]   padding00 u8 = 0.
    // [6..8]  unknown00 u16 (MyPort) = 0.
    // [8..12] unknown01 u32 (MyIP) = 0.
    // [12..16] UniqueNo u32 = char_id.
    body[12..16].copy_from_slice(&args.char_id.to_le_bytes());
    // [16..34] GrapIDTbl[9] u16 — leave zero for v1.
    // [34..49] sName[15] (15 bytes; NUL truncates).
    write_fixed(&mut body[34..49], args.char_name.as_bytes());
    // [49..64] sAccunt[15].
    write_fixed(&mut body[49..64], args.account_name.as_bytes());
    // [64..80] Ticket[16] — session_hash from auth.
    body[64..80].copy_from_slice(&args.ticket);
    // [80..84] Ver u32.
    body[80..84].copy_from_slice(&args.version.to_le_bytes());
    // [84..88] sPlatform[4].
    body[84..88].copy_from_slice(&args.platform);
    // [88..90] uCliLang u16.
    body[88..90].copy_from_slice(&args.cli_lang.to_le_bytes());
    // [90..92] dammyArea u16 = 0.

    // Compute LoginPacketCheck = sum of bytes [8..92) of GP_CLI_LOGIN
    // (i.e. starting at `unknown01`), low byte. Server expects this in [4].
    let sum: u32 = body[8..GP_CLI_LOGIN_SIZE].iter().map(|&b| b as u32).sum();
    body[4] = sum as u8;

    // 16-byte MD5 trailer over [28..28+92) (the GP_CLI_LOGIN bytes).
    let payload_range = framing::FFXI_HEADER_SIZE..(framing::FFXI_HEADER_SIZE + GP_CLI_LOGIN_SIZE);
    let digest = md5::md5(&frame[payload_range]);
    frame[BOOTSTRAP_DATAGRAM_SIZE - 16..].copy_from_slice(&digest);

    Ok(frame)
}

fn write_fixed(dst: &mut [u8], src: &[u8]) {
    let n = src.len().min(dst.len());
    dst[..n].copy_from_slice(&src[..n]);
}

fn hex16(b: &[u8; 16]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[allow(dead_code)]
fn _unused() -> Result<()> {
    Err(anyhow!("compiler-quietener"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotate_session_key_seed_increments_last_u32_by_two() {
        // bytes [16..20] = 0x00010203 (LE) = 0x03020100 numerically.
        let mut seed = [0u8; 20];
        seed[16] = 0x00;
        seed[17] = 0x01;
        seed[18] = 0x02;
        seed[19] = 0x03;
        rotate_session_key_seed(&mut seed);
        // u32 LE was 0x03020100, +2 = 0x03020102 → bytes [02 01 02 03].
        assert_eq!(&seed[16..20], &[0x02, 0x01, 0x02, 0x03]);
        // Other bytes untouched.
        assert!(seed[..16].iter().all(|&b| b == 0));
    }

    #[test]
    fn rotate_session_key_seed_wraps_at_u32_max() {
        let mut seed = [0u8; 20];
        seed[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        rotate_session_key_seed(&mut seed);
        // u32::MAX + 2 wraps to 1.
        assert_eq!(&seed[16..20], &1u32.to_le_bytes());
    }
}
