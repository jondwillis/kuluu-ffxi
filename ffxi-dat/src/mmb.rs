//! MMB entity-model decryptor + parser.
//!
//! FFXI MMB chunks are encrypted on disk using two XOR/swap passes keyed
//! by a byte at offset 5 of the chunk body. The algorithm and key tables
//! are ported from `FFXI-NavMesh-Builder/Common/dat/Types/MMB.cs` and
//! `KeyTables.cs` (GPL-3, © 2021 Xenonsmurf, workspace-compatible).
//!
//! Header layout (decrypted; bytes 0..8 are never encrypted):
//!   offset 0..4   ASCII tag (typically "MMB ")
//!   offset 4      version-byte (decryption is enabled iff `data[3] >= 5`)
//!   offset 5      key-table index byte (`data[5] ^ 0xf0` selects keys)
//!   offset 6..8   feature flags; `0xFFFF` triggers the second (block-swap) pass
//!
//! Two-pass scheme:
//!   Pass 1 (always, when version>=5): XOR each byte at pos>=8 with a
//!     pseudorandom mask derived from KeyTable[data[5] ^ 0xf0].
//!   Pass 2 (when flag==0xFFFF): conditionally swap 8-byte blocks across
//!     a midpoint, gated by parity of a key2 register that ticks each loop.

use crate::{DatError, Result};

pub(crate) mod keys;

/// Errors specific to MMB decryption / parsing.
#[derive(Debug, thiserror::Error)]
pub enum MmbError {
    #[error("MMB buffer too small: need at least 8 bytes for header, got {0}")]
    TooSmall(usize),
}

impl From<MmbError> for DatError {
    fn from(e: MmbError) -> Self {
        DatError::Mmb(format!("{e}"))
    }
}

/// Decrypt an MMB blob in place. The first 8 bytes (header) are
/// unchanged; remaining bytes are XORed and possibly block-swapped per
/// the MMB scheme.
///
/// `data` should be the *body* of an MMB chunk — exactly what would be
/// passed to `Mmb.DecodeMmb` in C#. Returns `Ok(())` on success even when
/// no decryption was needed (`data[3] < 5`), since the on-disk
/// representation already matches the cleartext form in that case.
pub fn decrypt_in_place(data: &mut [u8]) -> Result<()> {
    if data.len() < 8 {
        return Err(MmbError::TooSmall(data.len()).into());
    }

    // Pass 1: XOR every byte from offset 8 onward.
    if data[3] >= 5 {
        let key_seed = keys::KEY_TABLE[(data[5] ^ 0xf0) as usize] as i32;
        let mut key: i32 = key_seed;
        let mut key_count: i32 = 0;
        let len = data.len();

        for pos in 8..len {
            // C# (signed int 32 arithmetic; we mirror with i32):
            //   x = ((key & 0xFF) << 8) | (key & 0xFF)
            //   key += ++keyCount
            //   data[pos] ^= (byte)(x >> (key & 7))
            //   key += ++keyCount
            let key_low = key & 0xFF;
            let x = (key_low << 8) | key_low;
            key_count = key_count.wrapping_add(1);
            key = key.wrapping_add(key_count);

            let shift = (key & 7) as u32;
            // Per C#: `(byte)(x >> shift)` — C# `>>` on int is arithmetic, but
            // since x is non-negative (max 0xFFFF) this matches a logical shift.
            let mask = ((x >> shift) & 0xFF) as u8;
            data[pos] ^= mask;

            key_count = key_count.wrapping_add(1);
            key = key.wrapping_add(key_count);
        }
    }

    // Pass 2: block-swap, only when flag at offset 6..8 is 0xFFFF.
    if data[6] == 0xFF && data[7] == 0xFF {
        let len = data.len();
        let mut key1: i32 = (data[5] ^ 0xf0) as i32;
        let mut key2: i32 = keys::KEY_TABLE_2[key1 as usize] as i32;
        // C#: var len2 = ((len - 8) & ~0xf) >> 1;
        let len2 = (((len - 8) & !0xf) >> 1) as i32;

        let mut offset1: usize = 8;
        let mut offset2: usize = (8 + len2) as usize;

        let mut i: i32 = 0;
        while i < len2 {
            if (key2 & 1) == 1 {
                // C# does the swap awkwardly through an intermediate
                // tmp buffer using BlockCopy. The intent is straight
                // 8-byte swap between [offset1..+8] and [offset2..+8].
                let mut tmp = [0u8; 8];
                tmp.copy_from_slice(&data[offset1..offset1 + 8]);
                let (left, right) = data.split_at_mut(offset2);
                left[offset1..offset1 + 8].copy_from_slice(&right[..8]);
                right[..8].copy_from_slice(&tmp);
            }

            key1 = key1.wrapping_add(9);
            key2 = key2.wrapping_add(key1);
            offset1 += 8;
            offset2 += 8;
            i += 8;
        }
    }

    Ok(())
}

/// Returns a fresh decrypted copy.
pub fn decrypt(data: &[u8]) -> Result<Vec<u8>> {
    let mut buf = data.to_vec();
    decrypt_in_place(&mut buf)?;
    Ok(buf)
}

/// Decoded MMB file header (the first 32 bytes of the *decrypted* body).
///
/// Layout (observed empirically against real MMBs — kabu helmet, flat
/// item, clod texture, etc.):
///
///   offset 0..4   crypto preamble (two bytes vary per file — possibly
///                 a hash or salt; not yet decoded)
///   offset 3      `version` (u8) — controls decryption mode
///   offset 5      `key_index` (u8) — XOR'd with 0xF0 to index KEY_TABLE
///   offset 6..8   `feature_flags` (u16 LE) — 0xFFFF triggers block-swap pass
///   offset 8..24  16-byte ASCII `asset_name`, space-padded
///                 (may contain a NUL splitting it into two 8-char halves;
///                 e.g. "tensaka\0kabuse_m" for the "tensa kabuto" helmet)
///   offset 24..32 8-byte padding (observed as `20 20 ...` spaces)
///
/// Beyond offset 32 the format is sub-record-based: u32 counts, vec3/vec4
/// floats for bounding boxes, and 16-byte typed sub-record names like
/// "model   <name>" or "test00  tex0    ". Decoding these is Phase 3 work.
#[derive(Debug, Clone)]
pub struct MmbHeader<'a> {
    pub version: u8,
    pub key_index: u8,
    pub feature_flags: u16,
    pub asset_name: &'a [u8],
    /// Body following the 32-byte header (sub-records etc.).
    pub payload: &'a [u8],
}

impl<'a> MmbHeader<'a> {
    pub fn parse(decrypted: &'a [u8]) -> Result<Self> {
        if decrypted.len() < 32 {
            return Err(MmbError::TooSmall(decrypted.len()).into());
        }
        Ok(Self {
            version: decrypted[3],
            key_index: decrypted[5],
            feature_flags: u16::from_le_bytes([decrypted[6], decrypted[7]]),
            asset_name: &decrypted[8..24],
            payload: &decrypted[32..],
        })
    }

    /// Asset name as a UTF-8 String. NULs become `.` and trailing spaces
    /// are trimmed.
    pub fn asset_name_str(&self) -> String {
        let raw: String = self
            .asset_name
            .iter()
            .map(|&b| if b == 0 { '.' } else { b as char })
            .collect();
        raw.trim_end().to_string()
    }
}

/// One decoded vertex from an MMB sub-record body. **36 bytes** on disk
/// (corrected from earlier 40-byte hypothesis — the trailing 4 bytes I
/// thought were "weights" were actually vertex N+1's pos.x leaking
/// in. The 36-byte stride was confirmed by checking unit-vector
/// magnitude of vertex N+1's normal: with 32B stride normal mag was
/// 17.5 (broken); with 36B stride normal mag was 1.0 (clean unit vec)).
///
/// Helmet/accessory MMBs (`con_*` variants) use this skinning-free
/// layout. Body-equipment MMBs that need bone weights likely use a
/// 40+ byte stride; that's a different sub-record class for later.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MmbVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub rgba: [u8; 4],
    pub uv: [f32; 2],
}

/// A `"model   "` sub-record inside an MMB payload. Each sub-record is a
/// named mesh group; a single MMB often has 1-4 of them (one per
/// material or LOD).
///
/// Layout discovered empirically:
///   offset 0..8     b"model   " tag
///   offset 8..16    8-byte ASCII variant name (e.g. "con_wi1 ")
///   offset 16..20   u32 LE — *probably* vertex count
///   offset 20..48   28 bytes of metadata (sub-mesh header: bbox center+extent + flags?)
///   offset 48..     payload (vertex/triangle data — format TBD)
///
/// The walker uses a string search for the `b"model   "` tag rather than
/// stride arithmetic, since the exact stride between sub-records is not
/// yet fully decoded.
#[derive(Debug, Clone, Copy)]
pub struct MmbSubRecord<'a> {
    /// Absolute offset of the tag in the *payload* slice this was
    /// found in.
    pub offset: usize,
    /// 8-byte tag immediately preceding `variant_name`. `b"model   "`
    /// for the standard 36-byte-stride vertex/index sub-record format;
    /// other tags (e.g. `b"clod    "`, asset-name prefixes) indicate
    /// alternate body layouts that [`parse_vertices`] cannot decode —
    /// filter those out at the caller.
    pub tag: &'a [u8],
    pub variant_name: &'a [u8],
    pub count: u32,
    /// Bytes immediately after the count word and before the next
    /// sub-record (or end of payload). Includes the 28-byte sub-mesh
    /// header followed by vertex/triangle data we haven't decoded yet.
    pub body: &'a [u8],
}

impl<'a> MmbSubRecord<'a> {
    /// Find sub-records in `payload` by scanning for 16-byte aligned
    /// "tag + variant" headers. Two real-world tag patterns:
    ///   - `"model   <variant>"` — kabu-style accessory MMBs
    ///   - `"<prefix>   <variant>"` — clod-style cloth MMBs (prefix
    ///                                 matches the asset shortname)
    ///
    /// Detection: at a 16-byte aligned offset, the first 4 bytes are
    /// ASCII alphanumerics/underscore/space and the byte at offset+16
    /// is followed by a plausible count u32. We start from `start_at`
    /// to skip the 32-byte MMB file header.
    pub fn find_all(payload: &'a [u8]) -> Vec<MmbSubRecord<'a>> {
        let mut starts: Vec<usize> = Vec::new();
        let mut i = 0;
        while i + 24 <= payload.len() {
            let tag_word = &payload[i..i + 8];
            let variant = &payload[i + 8..i + 16];
            // Heuristic: the tag must be 8 bytes of ASCII (graphic +
            // space + NUL acceptable), and the variant must be too.
            // Tag's first 4 bytes are typically alphanumeric (no spaces
            // up front), variants can have NULs.
            // Sanity-check the count u32 — real MMB element counts fit
            // in a u16; >65535 is a strong "this is a coincidental ASCII
            // window, not a real record" signal.
            let count = u32::from_le_bytes([
                payload[i + 16],
                payload[i + 17],
                payload[i + 18],
                payload[i + 19],
            ]);
            if is_ascii_tag(tag_word) && is_ascii_variant(variant) && count > 0 && count <= 0xFFFF {
                starts.push(i);
                i += 20;
                continue;
            }
            // Sub-records are 4-byte aligned but NOT necessarily 16-byte
            // aligned — empirically kabu's 2nd sub-record sits at
            // payload offset 14168 (= 0x3758, 8-aligned).
            i += 4;
        }

        starts
            .iter()
            .enumerate()
            .map(|(idx, &start)| {
                let end = starts.get(idx + 1).copied().unwrap_or(payload.len());
                let count = u32::from_le_bytes([
                    payload[start + 16],
                    payload[start + 17],
                    payload[start + 18],
                    payload[start + 19],
                ]);
                MmbSubRecord {
                    offset: start,
                    tag: &payload[start..start + 8],
                    variant_name: &payload[start + 8..start + 16],
                    count,
                    body: &payload[start + 20..end],
                }
            })
            .collect()
    }

    /// Triangle strip indices following the vertex block (36-byte
    /// stride per vertex). Each u16 is a vertex index.
    ///
    /// The *first* u16 after the vertex block is a strip-length
    /// header (empirically: kabu con_wi1 has header=775 — well above
    /// the 348-vertex count, while remaining indices are all 0..347).
    /// We return the raw indices INCLUDING the header — callers that
    /// want the strip should use `parse_triangle_list` which handles
    /// the header skip and restart-marker logic.
    pub fn parse_triangle_strip(&self) -> Vec<u16> {
        let vert_bytes = self.count as usize * 36;
        if vert_bytes >= self.body.len() {
            return Vec::new();
        }
        let leftover = &self.body[vert_bytes..];
        leftover
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect()
    }

    /// Convert the triangle strip to a flat triangle-list with proper
    /// FFXI strip handling:
    ///   - First u16 is a strip-length header — skip it.
    ///   - `(a, a, a)` triple marks a strip terminator. After it,
    ///     winding state resets and a new strip begins.
    ///   - `(a, a)` doublets within a strip flip winding (standard
    ///     tri-strip restart pattern). Triangles like `(a, a, b)` and
    ///     `(a, b, b)` are emitted as degenerate (zero-area, GPU
    ///     discards them safely).
    ///   - Within a strip, winding alternates each step.
    ///
    /// All resulting triangles use the *raw* index values. The caller's
    /// GPU pipeline should disable back-face culling, since FFXI strip
    /// winding doesn't always match the OpenGL CCW front-face
    /// convention.
    pub fn parse_triangle_list(&self) -> Vec<[u16; 3]> {
        let strip = self.parse_triangle_strip();
        if strip.len() < 4 {
            return Vec::new();
        }
        // Skip the first u16 (strip-length header).
        let strip = &strip[1..];

        let mut out = Vec::with_capacity(strip.len().saturating_sub(2));
        let mut i = 0;
        let mut flip = false;
        while i + 2 < strip.len() {
            let a = strip[i];
            let b = strip[i + 1];
            let c = strip[i + 2];

            // Triple-repeat = strip terminator → reset winding, advance
            // past the marker so the next iteration starts fresh.
            if a == b && b == c {
                flip = false;
                i += 1;
                continue;
            }
            // Strict degenerate check (any two equal): emit nothing,
            // advance by 1, but DO flip winding (treats degenerate
            // pairs as winding-flip restarts within an open strip).
            if a == b || b == c || a == c {
                flip = !flip;
                i += 1;
                continue;
            }
            let tri = if flip { [a, c, b] } else { [a, b, c] };
            out.push(tri);
            flip = !flip;
            i += 1;
        }
        out
    }

    /// Decode vertex data starting at `body` offset 0 with the observed
    /// 36-byte stride. Returns `None` if `body` is too short or `count`
    /// would overflow.
    ///
    /// 36-byte vertex layout:
    ///   offset 0..12   vec3 position
    ///   offset 12..24  vec3 normal (unit magnitude verified)
    ///   offset 24..28  u8 × 4 RGBA
    ///   offset 28..36  vec2 UV
    pub fn parse_vertices(&self) -> Option<Vec<MmbVertex>> {
        const STRIDE: usize = 36;
        let needed = self.count as usize * STRIDE;
        if needed > self.body.len() {
            return None;
        }
        let mut out = Vec::with_capacity(self.count as usize);
        for i in 0..self.count as usize {
            let off = i * STRIDE;
            let pos = [
                f32::from_le_bytes(self.body[off..off + 4].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 4..off + 8].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 8..off + 12].try_into().ok()?),
            ];
            let normal = [
                f32::from_le_bytes(self.body[off + 12..off + 16].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 16..off + 20].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 20..off + 24].try_into().ok()?),
            ];
            let rgba = [
                self.body[off + 24],
                self.body[off + 25],
                self.body[off + 26],
                self.body[off + 27],
            ];
            let uv = [
                f32::from_le_bytes(self.body[off + 28..off + 32].try_into().ok()?),
                f32::from_le_bytes(self.body[off + 32..off + 36].try_into().ok()?),
            ];
            out.push(MmbVertex {
                pos,
                normal,
                rgba,
                uv,
            });
        }
        Some(out)
    }

    pub fn variant_name_str(&self) -> String {
        self.variant_name
            .iter()
            .map(|&b| if b == 0 { '.' } else { b as char })
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// Tag bytes (8 bytes; e.g. `b"model   "` or `b"clod    "`).
    pub fn tag<'b>(&'b self, payload: &'a [u8]) -> &'a [u8]
    where
        'a: 'b,
    {
        &payload[self.offset..self.offset + 8]
    }
}

/// True if all 8 bytes are ASCII alphanumeric, underscore, or space.
/// First byte must NOT be space (filters out padding-only regions).
fn is_ascii_tag(b: &[u8]) -> bool {
    if b.len() != 8 || b[0] == b' ' {
        return false;
    }
    b.iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b' ')
}

/// True if all 8 bytes are valid for a variant name (alphanumeric,
/// underscore, space, or NUL padding). Empty (all-NUL) is rejected.
fn is_ascii_variant(b: &[u8]) -> bool {
    if b.len() != 8 || b.iter().all(|&c| c == 0) {
        return false;
    }
    b.iter()
        .all(|&c| c == 0 || c.is_ascii_alphanumeric() || c == b'_' || c == b' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_below_5_is_noop_for_pass1() {
        // data[3] = 4 → skip pass 1. data[6..8] ≠ 0xFFFF → skip pass 2.
        // Result: bytes after offset 8 are unchanged.
        let mut buf = vec![b'M', b'M', b'B', 4, 0x12, 0x34, 0x00, 0x00, 0xAA, 0xBB];
        decrypt_in_place(&mut buf).unwrap();
        assert_eq!(&buf[8..], &[0xAA, 0xBB]);
    }

    #[test]
    fn too_small_errors() {
        let mut buf = [0u8; 4];
        let err = decrypt_in_place(&mut buf).unwrap_err();
        assert!(matches!(err, DatError::Mmb(_)));
    }

    #[test]
    fn round_trip_is_identity_when_version_below_5() {
        // Decrypt is a noop on version<5 + flag!=0xFFFF, so re-running
        // gives the same bytes back.
        let original = vec![
            b'M', b'M', b'B', 4, 0x12, 0x34, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 7, 8,
        ];
        let once = decrypt(&original).unwrap();
        let twice = decrypt(&once).unwrap();
        assert_eq!(once, twice);
        assert_eq!(once, original);
    }

    #[test]
    fn pass1_xor_is_involutive() {
        // Pass 1 is a pure XOR with deterministic pad → applying decrypt
        // twice should restore the original bytes (the algorithm is its
        // own inverse for pass 1 when pass 2 is disabled).
        let mut bytes = vec![b'M', b'M', b'B', 5, 0xAA, 0x42, 0x00, 0x00];
        bytes.extend((0..64).map(|i| i as u8));
        let original = bytes.clone();

        decrypt_in_place(&mut bytes).unwrap();
        assert_ne!(bytes[8..], original[8..], "pass 1 should change bytes");
        decrypt_in_place(&mut bytes).unwrap();
        assert_eq!(bytes, original, "applying twice should be identity");
    }

    /// First 64 bytes of the *decrypted* MMB chunk at file_id 115,
    /// chunk index 18 ("kabu" helmet) — pulled empirically against a
    /// real install. Used to lock the algorithm and the header parser
    /// against drive-by changes. No copyrighted assets baked in — just
    /// 64 bytes of header structure (asset name + bbox floats).
    const KABU_DECRYPTED_HEAD: [u8; 64] = [
        0xe4, 0x72, 0x00, 0x05, 0x01, 0xb7, 0x47, 0x9f, 0x74, 0x65, 0x6e, 0x73, 0x61, 0x6b, 0x61,
        0x00, 0x6b, 0x61, 0x62, 0x75, 0x73, 0x65, 0x5f, 0x6d, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20,
        0x20, 0x20, 0x01, 0x00, 0x00, 0x00, 0xe6, 0xee, 0x3a, 0xc2, 0x34, 0x22, 0xca, 0x41, 0xb3,
        0xaa, 0x76, 0xc2, 0x99, 0xaa, 0xf6, 0xc1, 0xcd, 0xcc, 0x50, 0xc2, 0x00, 0x00, 0xd4, 0x41,
        0x40, 0x00, 0x00, 0x00,
    ];

    /// Synthesize a multi-sub-record MMB-payload that mimics the kabu
    /// layout for the walker test. Uses non-ASCII bytes for the payload
    /// regions so the walker doesn't mistake them for tags.
    fn synth_payload() -> Vec<u8> {
        let mut buf = Vec::new();
        // Sub-record 1: tag "model   " + variant "con_wi1 " + count + body
        // (real kabu data pads variants with trailing spaces, not NULs;
        // see hex dump at offset 0x60 of mmb-115-18.bin: `... 31 20`)
        buf.extend_from_slice(b"model   ");
        buf.extend_from_slice(b"con_wi1 ");
        buf.extend_from_slice(&348u32.to_le_bytes()); // count
        buf.extend(std::iter::repeat_n(0xFFu8, 28)); // sub-mesh header (non-ASCII)
        buf.extend(std::iter::repeat_n(0xFEu8, 192)); // vertex data (non-ASCII, 16-aligned region)
                                                      // Sub-record 2: clod-style variant naming
        buf.extend_from_slice(b"clod    ");
        buf.extend_from_slice(b"clod_a01");
        buf.extend_from_slice(&71u32.to_le_bytes()); // count
        buf.extend(std::iter::repeat_n(0xFFu8, 28));
        buf.extend(std::iter::repeat_n(0xFDu8, 48));
        buf
    }

    #[test]
    fn sub_record_walker_finds_mixed_tag_kinds() {
        let payload = synth_payload();
        let records = MmbSubRecord::find_all(&payload);
        assert_eq!(
            records.len(),
            2,
            "should find both 'model   ' and 'clod    ' headers"
        );
        assert_eq!(records[0].variant_name_str(), "con_wi1");
        assert_eq!(records[0].count, 348);
        assert_eq!(&records[0].tag(&payload), b"model   ");
        assert_eq!(records[1].variant_name_str(), "clod_a01");
        assert_eq!(records[1].count, 71);
        assert_eq!(&records[1].tag(&payload), b"clod    ");
    }

    #[test]
    fn parses_real_kabu_header() {
        let h = MmbHeader::parse(&KABU_DECRYPTED_HEAD).unwrap();
        assert_eq!(h.version, 0x05);
        assert_eq!(h.key_index, 0xB7);
        assert_eq!(h.feature_flags, 0x9F47); // not the 0xFFFF block-swap trigger
        assert_eq!(h.asset_name_str(), "tensaka.kabuse_m");
        // First payload u32 = 1 (likely sub-model count).
        let count = u32::from_le_bytes([h.payload[0], h.payload[1], h.payload[2], h.payload[3]]);
        assert_eq!(count, 1);
        // Next 12 bytes look like a vec3 bbox-min (-46.73, 25.27, -61.67).
        let f0 = f32::from_le_bytes([h.payload[4], h.payload[5], h.payload[6], h.payload[7]]);
        let f1 = f32::from_le_bytes([h.payload[8], h.payload[9], h.payload[10], h.payload[11]]);
        let f2 = f32::from_le_bytes([h.payload[12], h.payload[13], h.payload[14], h.payload[15]]);
        assert!((f0 + 46.73).abs() < 0.1, "f0 was {f0}");
        assert!((f1 - 25.27).abs() < 0.1, "f1 was {f1}");
        assert!((f2 + 61.67).abs() < 0.1, "f2 was {f2}");
    }
}
