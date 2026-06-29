use std::net::SocketAddr;

use anyhow::{anyhow, bail, Context, Result};
use tokio::net::UdpSocket;

use ffxi_proto::{blowfish, framing, md5, zlib};

pub const GP_CLI_LOGIN_SIZE: usize = 92;

pub const BOOTSTRAP_DATAGRAM_SIZE: usize = framing::FFXI_HEADER_SIZE + GP_CLI_LOGIN_SIZE + 16;

#[derive(Debug, Clone)]
pub struct BootstrapArgs<'a> {
    pub char_id: u32,
    pub char_name: &'a str,
    pub account_name: &'a str,

    pub ticket: [u8; 16],

    pub version: u32,

    pub platform: [u8; 4],

    pub cli_lang: u16,
}

pub struct MapClient {
    socket: UdpSocket,
    server: SocketAddr,
    blowfish: blowfish::State,
    decompress_table: zlib::DecompressTable,
    compress_table: zlib::CompressTable,

    bytes_sent: std::sync::atomic::AtomicU64,
    bytes_recv: std::sync::atomic::AtomicU64,

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
        let decompress_table =
            zlib::DecompressTable::new().map_err(|e| anyhow!("decompress table init: {e}"))?;
        let compress_table =
            zlib::CompressTable::new().map_err(|e| anyhow!("compress table init: {e}"))?;
        Ok(Self {
            socket,
            server,
            blowfish,
            decompress_table,
            compress_table,
            bytes_sent: std::sync::atomic::AtomicU64::new(0),
            bytes_recv: std::sync::atomic::AtomicU64::new(0),
            seed,
        })
    }

    pub async fn send_encrypted(
        &self,
        sub_packets_payload: &[u8],
        bundle_seq: u16,
        ack_server_seq: u16,
    ) -> Result<()> {
        let (compressed_bits, compressed) = self
            .compress_table
            .compress(sub_packets_payload)
            .map_err(|e| anyhow!("compress: {e}"))?;

        let mut tail = compressed;
        tail.extend_from_slice(&(compressed_bits as u32).to_le_bytes());
        let digest = ffxi_proto::md5::md5(&tail);
        tail.extend_from_slice(&digest);

        let mut frame = vec![0u8; framing::FFXI_HEADER_SIZE + tail.len()];
        frame[0..2].copy_from_slice(&bundle_seq.to_le_bytes());
        frame[2..4].copy_from_slice(&ack_server_seq.to_le_bytes());
        frame[4..6].copy_from_slice(&bundle_seq.to_le_bytes());

        frame[framing::FFXI_HEADER_SIZE..].copy_from_slice(&tail);

        framing::encrypt_in_place(&mut frame, &self.blowfish);

        let n = self
            .socket
            .send_to(&frame, &self.server)
            .await
            .context("UDP send_to (encrypted)")?;
        if n != frame.len() {
            bail!("partial encrypted UDP send: {n} of {}", frame.len());
        }
        self.bytes_sent
            .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
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
        self.bytes_sent
            .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub async fn recv_decrypted(&self) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; ffxi_proto::map::MAX_DATAGRAM];
        let (n, src) = self
            .socket
            .recv_from(&mut buf)
            .await
            .context("UDP recv_from")?;
        buf.truncate(n);
        self.bytes_recv
            .fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);

        if n < framing::MIN_FRAME_SIZE {
            bail!("undersized response datagram: {n} bytes");
        }

        framing::decrypt_in_place(&mut buf, &self.blowfish);

        if n < framing::FFXI_HEADER_SIZE + 20 {
            bail!("response too short to hold size+md5 trailer: {n}");
        }
        let size_off = n - 4 - 16;

        let compressed_bits =
            u32::from_le_bytes(buf[size_off..size_off + 4].try_into().unwrap()) as usize;
        let compressed_bytes = compressed_bits.div_ceil(8);
        let compressed_end = framing::FFXI_HEADER_SIZE + compressed_bytes;
        if compressed_end > size_off {
            bail!(
                "compressed payload {compressed_bytes} bytes ({compressed_bits} bits) overruns size field at {size_off}"
            );
        }

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
        let payload_bits = compressed_bits.saturating_sub(8);
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

        let opcodes: Vec<String> = framing::walk_sub_packets(&decompressed)
            .filter_map(|r| r.ok().map(|s| format!("0x{:03x}", s.opcode)))
            .collect();
        if !opcodes.is_empty() {
            tracing::info!(
                bytes = n,
                src = %src,
                sub_count = opcodes.len(),
                sub_opcodes = opcodes.join(" "),
                "recv"
            );
        }

        Ok(out)
    }

    pub fn server_addr(&self) -> SocketAddr {
        self.server
    }

    pub fn traffic_totals(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering;
        (
            self.bytes_sent.load(Ordering::Relaxed),
            self.bytes_recv.load(Ordering::Relaxed),
        )
    }

    pub fn retarget(&mut self, new_server: SocketAddr, new_seed: [u8; 20]) {
        self.server = new_server;
        self.seed = new_seed;
        self.blowfish = derive_blowfish(&new_seed);
    }
}

pub fn rotate_session_key_seed(seed: &mut [u8; 20]) {
    let cur = u32::from_le_bytes(seed[16..20].try_into().unwrap());
    let next = cur.wrapping_add(2);
    seed[16..20].copy_from_slice(&next.to_le_bytes());
}

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

    frame[0..2].copy_from_slice(&1u16.to_le_bytes());
    frame[4..6].copy_from_slice(&1u16.to_le_bytes());

    let body = &mut frame[framing::FFXI_HEADER_SIZE..framing::FFXI_HEADER_SIZE + GP_CLI_LOGIN_SIZE];

    let id: u16 = 0x00A;
    let size_words: u16 = (GP_CLI_LOGIN_SIZE / 4) as u16;
    let header_word = id | (size_words << 9);
    body[0..2].copy_from_slice(&header_word.to_le_bytes());

    body[2..4].copy_from_slice(&1u16.to_le_bytes());

    body[12..16].copy_from_slice(&args.char_id.to_le_bytes());

    write_fixed(&mut body[34..49], args.char_name.as_bytes());

    write_fixed(&mut body[49..64], args.account_name.as_bytes());

    body[64..80].copy_from_slice(&args.ticket);

    body[80..84].copy_from_slice(&args.version.to_le_bytes());

    body[84..88].copy_from_slice(&args.platform);

    body[88..90].copy_from_slice(&args.cli_lang.to_le_bytes());

    let sum: u32 = body[8..GP_CLI_LOGIN_SIZE].iter().map(|&b| b as u32).sum();
    body[4] = sum as u8;

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
        let mut seed = [0u8; 20];
        seed[16] = 0x00;
        seed[17] = 0x01;
        seed[18] = 0x02;
        seed[19] = 0x03;
        rotate_session_key_seed(&mut seed);

        assert_eq!(&seed[16..20], &[0x02, 0x01, 0x02, 0x03]);

        assert!(seed[..16].iter().all(|&b| b == 0));
    }

    #[test]
    fn rotate_session_key_seed_wraps_at_u32_max() {
        let mut seed = [0u8; 20];
        seed[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        rotate_session_key_seed(&mut seed);

        assert_eq!(&seed[16..20], &1u32.to_le_bytes());
    }

    #[tokio::test]
    async fn retarget_preserves_local_socket_port() {
        let server_a: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let server_b: SocketAddr = "127.0.0.2:2".parse().unwrap();
        let seed_a = [1u8; 20];
        let seed_b = [2u8; 20];
        let mut client = MapClient::connect(server_a, seed_a).await.unwrap();
        let local_before = client.socket.local_addr().unwrap();
        client.retarget(server_b, seed_b);
        let local_after = client.socket.local_addr().unwrap();
        assert_eq!(
            local_before, local_after,
            "retarget must keep the same local (ip, port)"
        );
        assert_eq!(client.server, server_b, "retarget updates server");
        assert_eq!(client.seed, seed_b, "retarget updates seed");
    }
}
