//! FFXI map-server packet framing.
//!
//! A map-server UDP datagram has the wire shape:
//!
//! ```text
//!   [ 0..28 ]      FFXI header (28 bytes).
//!                  Only the first 14 bytes are semantically meaningful;
//!                  the rest is scratch / padding used during encryption.
//!   [28..N-16]     Payload — zlib-compressed sub-packet bundle.
//!   [N-16..N]      Trailing MD5 over [28..N-16] (verifies before decompress).
//! ```
//!
//! Encryption is FFXI-Blowfish on u32 pairs starting at byte offset 28, for as
//! many full u32 pairs as fit within `(PacketSize / 4) & !1` words; the very
//! last 0–7 bytes of payload may be left unencrypted depending on alignment
//! (matches `server/src/map/map_networking.cpp:683–720`).
//!
//! NOTE: encrypt/decrypt and the precise boundary between "payload size" and
//! "frame size" are sensitive to ground-truth capture. The functions below
//! follow the reading of LSB source as documented in the plan; they will be
//! validated against captured vectors before being relied upon.

use crate::blowfish;

pub const FFXI_HEADER_SIZE: usize = 28;
pub const MD5_TRAILER_SIZE: usize = 16;

/// Minimum byte count a valid frame can have: header + at least one sub-packet
/// header (4 bytes) + MD5 trailer.
pub const MIN_FRAME_SIZE: usize = FFXI_HEADER_SIZE + 4 + MD5_TRAILER_SIZE;

/// FFXI map header — first 14 bytes of every encrypted frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Header {
    /// Bundle-level packet ID + size word (low 9 bits = opcode, high 7 = words).
    pub id_and_size: u16,
    /// "sync_in" — last server packet ID the client received (in C→S);
    /// in S→C, the server's view of the client's last server-ack.
    pub sync_in: u16,
    /// "sync_out" — sender's outgoing packet sequence ID.
    pub sync_out: u16,
    /// Reserved padding (zero on send).
    pub reserved0: u16,
    /// Vana'diel timestamp.
    pub timestamp: u32,
    /// Compression-size info written during compression (server-side).
    pub size_or_reserved: u32,
}

impl Header {
    pub fn read(buf: &[u8]) -> Self {
        debug_assert!(buf.len() >= FFXI_HEADER_SIZE);
        Self {
            id_and_size: u16::from_le_bytes(buf[0..2].try_into().unwrap()),
            sync_in: u16::from_le_bytes(buf[2..4].try_into().unwrap()),
            sync_out: u16::from_le_bytes(buf[4..6].try_into().unwrap()),
            reserved0: u16::from_le_bytes(buf[6..8].try_into().unwrap()),
            timestamp: u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            size_or_reserved: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        }
    }

    pub fn write(&self, buf: &mut [u8]) {
        debug_assert!(buf.len() >= FFXI_HEADER_SIZE);
        buf[0..2].copy_from_slice(&self.id_and_size.to_le_bytes());
        buf[2..4].copy_from_slice(&self.sync_in.to_le_bytes());
        buf[4..6].copy_from_slice(&self.sync_out.to_le_bytes());
        buf[6..8].copy_from_slice(&self.reserved0.to_le_bytes());
        buf[8..12].copy_from_slice(&self.timestamp.to_le_bytes());
        buf[12..16].copy_from_slice(&self.size_or_reserved.to_le_bytes());
        // Bytes 16..28 are scratch — leave untouched / zero.
        for b in &mut buf[16..FFXI_HEADER_SIZE] {
            *b = 0;
        }
    }

    /// Bundle opcode (low 9 bits of `id_and_size`).
    pub fn opcode(&self) -> u16 {
        self.id_and_size & 0x1FF
    }
}

/// One sub-packet inside a decrypted bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubPacket<'a> {
    /// Sub-packet opcode (low 9 bits of the sub-packet's first u16).
    pub opcode: u16,
    /// Per-sub-packet sequence number / ack.
    pub sequence: u16,
    /// Payload bytes following the 4-byte sub-packet header. May be empty.
    pub data: &'a [u8],
}

/// Walk `payload` (the decompressed inner bundle) and yield sub-packets.
///
/// Sub-packet header layout (matches LSB):
/// ```text
///   [u8]  type_lo            // low byte of opcode
///   [u8]  size_words_x2      // sub-packet length in 4-byte words, LSB
///                            //   (`actual_size_bytes = (this & 0xFE) << 1`)
///   [u16] sequence (LE)
///   [..]  payload data
/// ```
///
/// Returns an iterator that lazily walks; mismatches between the implied
/// length and `payload.len()` surface as `WalkError::Truncated`.
pub fn walk_sub_packets(payload: &[u8]) -> SubPacketWalker<'_> {
    SubPacketWalker { rest: payload }
}

#[derive(Debug, Clone)]
pub struct SubPacketWalker<'a> {
    rest: &'a [u8],
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WalkError {
    #[error("truncated sub-packet: have {have} bytes, header claims {want}")]
    Truncated { have: usize, want: usize },
    #[error("zero-length sub-packet — would loop forever")]
    ZeroSize,
}

impl<'a> Iterator for SubPacketWalker<'a> {
    type Item = Result<SubPacket<'a>, WalkError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.len() < 4 {
            return if self.rest.is_empty() {
                None
            } else {
                Some(Err(WalkError::Truncated {
                    have: self.rest.len(),
                    want: 4,
                }))
            };
        }
        // LSB stores the size as: byte_at_offset_1 with low bit cleared, then
        // multiply by 2 to get bytes (i.e. 4-byte-word-aligned size). See
        // map_networking.cpp:419: `SmallPD_Size = ref<uint8>(SmallPD_ptr,1) & 0xFE`,
        // and pointer advance `SmallPD_ptr + SmallPD_Size * 2`.
        let opcode = (self.rest[0] as u16) | (((self.rest[1] as u16) & 0x01) << 8);
        let size_bytes = ((self.rest[1] as usize) & 0xFE) << 1;
        if size_bytes == 0 {
            return Some(Err(WalkError::ZeroSize));
        }
        if size_bytes > self.rest.len() {
            return Some(Err(WalkError::Truncated {
                have: self.rest.len(),
                want: size_bytes,
            }));
        }
        let sequence = u16::from_le_bytes([self.rest[2], self.rest[3]]);
        let sub = SubPacket {
            opcode,
            sequence,
            data: &self.rest[4..size_bytes],
        };
        self.rest = &self.rest[size_bytes..];
        Some(Ok(sub))
    }
}

/// Decrypt the encrypted region of a UDP frame in place.
///
/// `frame` must be the full datagram. The header (`[0..28]`) is left untouched;
/// bytes from offset 28 onward, in u32 pairs, are deciphered.
pub fn decrypt_in_place(frame: &mut [u8], state: &blowfish::State) {
    if frame.len() <= FFXI_HEADER_SIZE {
        return;
    }
    let payload_words = (frame.len() - FFXI_HEADER_SIZE) / 4;
    let pair_count = payload_words & !1; // round down to even
    for j in (0..pair_count).step_by(2) {
        let off = FFXI_HEADER_SIZE + j * 4;
        let (l, r) = read_u32_pair(&frame[off..off + 8]);
        let mut xl = l;
        let mut xr = r;
        blowfish::decipher(&mut xl, &mut xr, &state.p, &state.s);
        write_u32_pair(&mut frame[off..off + 8], xl, xr);
    }
}

/// Encrypt the encrypted region of a UDP frame in place. Mirror of `decrypt_in_place`.
pub fn encrypt_in_place(frame: &mut [u8], state: &blowfish::State) {
    if frame.len() <= FFXI_HEADER_SIZE {
        return;
    }
    let payload_words = (frame.len() - FFXI_HEADER_SIZE) / 4;
    let pair_count = payload_words & !1;
    for j in (0..pair_count).step_by(2) {
        let off = FFXI_HEADER_SIZE + j * 4;
        let (l, r) = read_u32_pair(&frame[off..off + 8]);
        let mut xl = l;
        let mut xr = r;
        blowfish::encipher(&mut xl, &mut xr, &state.p, &state.s);
        write_u32_pair(&mut frame[off..off + 8], xl, xr);
    }
}

#[inline]
fn read_u32_pair(buf: &[u8]) -> (u32, u32) {
    (
        u32::from_le_bytes(buf[0..4].try_into().unwrap()),
        u32::from_le_bytes(buf[4..8].try_into().unwrap()),
    )
}

#[inline]
fn write_u32_pair(buf: &mut [u8], l: u32, r: u32) {
    buf[0..4].copy_from_slice(&l.to_le_bytes());
    buf[4..8].copy_from_slice(&r.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trip() {
        let h = Header {
            id_and_size: 0x012A,
            sync_in: 0x1234,
            sync_out: 0x5678,
            reserved0: 0,
            timestamp: 0xDEAD_BEEF,
            size_or_reserved: 0xCAFE_BABE,
        };
        let mut buf = [0u8; FFXI_HEADER_SIZE];
        h.write(&mut buf);
        let parsed = Header::read(&buf);
        assert_eq!(parsed, h);
        assert_eq!(parsed.opcode(), 0x12A);
    }

    #[test]
    fn encrypt_decrypt_round_trip() {
        let key = b"ffxi-test-key";
        let st = blowfish::State::new(key);

        // Build a 28-byte header + 32-byte payload + 16-byte trailer = 76 bytes.
        let mut frame = vec![0u8; FFXI_HEADER_SIZE + 32 + 16];
        // Sentinel header so we can confirm it isn't disturbed.
        frame[0..2].copy_from_slice(&0x012Au16.to_le_bytes());
        // Distinct payload.
        for (i, b) in frame[FFXI_HEADER_SIZE..FFXI_HEADER_SIZE + 32 + 16]
            .iter_mut()
            .enumerate()
        {
            *b = (i as u8).wrapping_mul(7);
        }
        let original = frame.clone();

        encrypt_in_place(&mut frame, &st);
        assert_ne!(frame, original, "encrypt should change the buffer");
        // Header should be untouched.
        assert_eq!(&frame[..FFXI_HEADER_SIZE], &original[..FFXI_HEADER_SIZE]);

        decrypt_in_place(&mut frame, &st);
        assert_eq!(frame, original, "round-trip should restore original");
    }

    #[test]
    fn sub_packet_walker_basic() {
        // Two sub-packets, each 8 bytes long (size_words_x2 = 4).
        // Layout: [opcode_lo, size_byte, seq_lo, seq_hi, data...]
        let mut payload = vec![];
        // Sub-packet #1: opcode=0x015, size=8 bytes, seq=0x0042, data 4 bytes.
        payload.extend_from_slice(&[0x15, 0x04, 0x42, 0x00, 0xAA, 0xBB, 0xCC, 0xDD]);
        // Sub-packet #2: opcode=0x00A, size=8 bytes, seq=0x0043, data 4 bytes.
        payload.extend_from_slice(&[0x0A, 0x04, 0x43, 0x00, 0x11, 0x22, 0x33, 0x44]);

        let subs: Vec<_> = walk_sub_packets(&payload).collect::<Result<_, _>>().unwrap();
        assert_eq!(subs.len(), 2);
        assert_eq!(subs[0].opcode, 0x015);
        assert_eq!(subs[0].sequence, 0x0042);
        assert_eq!(subs[0].data, &[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(subs[1].opcode, 0x00A);
        assert_eq!(subs[1].sequence, 0x0043);
        assert_eq!(subs[1].data, &[0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn sub_packet_walker_zero_size_is_error() {
        let payload = [0u8; 4];
        let mut walker = walk_sub_packets(&payload);
        let first = walker.next().unwrap();
        assert_eq!(first, Err(WalkError::ZeroSize));
    }

    #[test]
    fn sub_packet_walker_truncated() {
        // Claims 16 bytes but only 8 present.
        let payload = [0x15, 0x08, 0x42, 0x00, 0, 0, 0, 0];
        let mut walker = walk_sub_packets(&payload);
        let first = walker.next().unwrap();
        assert!(matches!(first, Err(WalkError::Truncated { .. })));
    }
}
