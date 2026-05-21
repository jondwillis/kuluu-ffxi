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
///   offset 8..32  24-byte ASCII `asset_name`, space-padded. Logical
///                 layout is 8-byte zone_prefix + up to 16-byte
///                 base/variant name (e.g. `tshimono` + `wall_id01`,
///                 `tshimono` + `house_p4_m`). Some assets use a NUL
///                 separator inside the prefix region (e.g.
///                 `tensaka\0kabuse_m`).
///                 NOTE: an earlier revision read only [8..24] under
///                 the belief that bytes 24..32 were padding. They are
///                 not — that read silently truncated 17+ char names
///                 like `tshimonowall_id01` to `tshimonowall_id0`,
///                 collapsing every numbered/LOD variant in a zone
///                 onto a single asset_name and breaking placement
///                 → chunk resolution. Trust the full 24 bytes.
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
            asset_name: &decrypted[8..32],
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

/// One mesh sub-record inside an MMB payload. Cross-referenced against
/// lotus-ffxi `mmb.cppm::SMMBModelHeader`, the layout is:
///
///   offset 0..16    `textureName[16]` (ASCII, NUL- or space-padded)
///   offset 16..18   `vertexsize: u16` — vertex count for this submesh
///   offset 18..20   `blending: u16`   — alpha/blend mode flags
///   offset 20..     `vertexsize × Vertex` then `u16 num_indices`,
///                   2 bytes pad, then indices.
///
/// The walker is a heuristic scanner (string search for ASCII-looking
/// 16-byte textureName windows) rather than a structural walk —
/// adequate for now because every plausible texture-name window is also
/// followed by a small vertexsize. The top 16 bits of the u32 at
/// offset 16 are `blending`, not part of `count`; mask with `0xFFFF`
/// when validating.
///
/// Naming: our internal `tag` is the FIRST 8 bytes of textureName, and
/// `variant_name` is the NEXT 8 bytes. They form one logical name —
/// use [`MmbSubRecord::texture_name_str`] to get the cleaned full name.
/// The historical filter `tag.starts_with(b"model")` only matched the
/// top-level `SMMBHeader.imgID` (which has a `"model   "` prefix); it
/// silently dropped every per-submesh textureName (which has no prefix)
/// and caused all submeshes to fall back to the first IMG, regardless
/// of material. Don't reintroduce that filter.
#[derive(Debug, Clone, Copy)]
pub struct MmbSubRecord<'a> {
    /// Absolute offset of the tag in the *payload* slice this was
    /// found in.
    pub offset: usize,
    /// First 8 bytes of the 16-byte `textureName` field. Despite the
    /// historical name, this is NOT a sub-record-type tag — it's the
    /// first half of a texture name. Combine with `variant_name` for
    /// the full 16 bytes (or call [`texture_name_str`]).
    pub tag: &'a [u8],
    /// Bytes 8..16 of the 16-byte `textureName` field. Combine with
    /// `tag` for the full name.
    pub variant_name: &'a [u8],
    /// Vertex count for this submesh. Low 16 bits of the u32 at
    /// offset 16 of the textureName record; the high 16 bits are
    /// `blending` and stored separately. Always `<= 0xFFFF`.
    pub count: u32,
    /// Blending/alpha-mode bits — top 16 bits of the same u32 that
    /// carries `count`. `& 0x8000` flags transparent geometry per
    /// lotus's `mesh->has_transparency = mmb_mesh.blending & 0x8000`.
    pub blending: u16,
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
            //
            // The u32 at offset 16 is actually `{vertexsize: u16,
            // blending: u16}`. Only the LOW 16 bits are the count;
            // the high 16 bits are alpha-blend flags (often 0x8000
            // for transparent geometry). Validate just the low half.
            let vertexsize = u16::from_le_bytes([payload[i + 16], payload[i + 17]]) as u32;
            if is_ascii_tag(tag_word)
                && is_ascii_variant(variant)
                && vertexsize > 0
                && vertexsize <= 0xFFFF
            {
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
                let vertexsize = u16::from_le_bytes([payload[start + 16], payload[start + 17]]);
                let blending = u16::from_le_bytes([payload[start + 18], payload[start + 19]]);
                MmbSubRecord {
                    offset: start,
                    tag: &payload[start..start + 8],
                    variant_name: &payload[start + 8..start + 16],
                    count: vertexsize as u32,
                    blending,
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
        // Layout (cross-checked against lotus-ffxi `mmb.cppm` parseModel):
        //
        //   [vertex block: count*36 bytes]
        //   [u16 num_indices]           strip-length header
        //   [u16 pad]                   skipped — advance is 4 bytes after count
        //   [num_indices × u16]         the strip indices
        //   [u16, u16 trailing align]   2 u16s past the last sliding window
        //   [optional u16 odd-pad]      if num_indices is odd
        //
        // Walk `num_indices - 2` sliding 3-index windows; emit one tri
        // per window using iteration-parity for winding. Skip when
        // `i1 == i2 || i2 == i3` (degenerate). DO NOT treat `i1 == i3`
        // as degenerate — that's a legitimate bowtie pinch in a strip.
        //
        // Fixes vs the previous Rust implementation:
        //   1. 4-byte header skip (was 2) — pad u16 is no longer
        //      consumed as the strip's first index.
        //   2. Strip bounded by `num_indices` — trailing alignment
        //      padding no longer leaks in as fake indices (the source
        //      of "stretched tris across the mesh" on bulletin-board
        //      and wall assets in zones 230 / 232).
        //   3. Degenerate test no longer rejects `i1 == i3` — restores
        //      legitimate pinch tris that were silently dropped
        //      (the source of small triangular holes).
        //   4. Winding parity comes from the iteration index, not a
        //      toggle state — degenerates no longer flip downstream
        //      winding (the source of mixed inside-out faces near
        //      pinches and seams).
        //   5. Removed the `(a, a, a)` "triple terminator" rule — not
        //      part of the actual strip protocol per lotus/LSB.
        const HEADER_BYTES: usize = 4;
        let vert_bytes = self.count as usize * 36;
        if vert_bytes + HEADER_BYTES > self.body.len() {
            return Vec::new();
        }
        let count_off = vert_bytes;
        let num = u16::from_le_bytes([self.body[count_off], self.body[count_off + 1]]) as usize;
        if num < 3 {
            return Vec::new();
        }
        let strip_off = count_off + HEADER_BYTES;
        let bytes_needed = num * 2;
        if strip_off + bytes_needed > self.body.len() {
            return Vec::new();
        }
        let read = |i: usize| -> u16 {
            let p = strip_off + i * 2;
            u16::from_le_bytes([self.body[p], self.body[p + 1]])
        };
        let mut out = Vec::with_capacity(num.saturating_sub(2));
        for i in 0..num - 2 {
            let i1 = read(i);
            let i2 = read(i + 1);
            let i3 = read(i + 2);
            if i1 == i2 || i2 == i3 {
                continue;
            }
            let tri = if i % 2 == 1 {
                [i2, i1, i3]
            } else {
                [i1, i2, i3]
            };
            out.push(tri);
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

    /// Texture name for IMG pairing — bytes 8..16 of the 16-byte
    /// textureName field, NUL-terminated and space-trimmed.
    ///
    /// The 16-byte textureName field is laid out as
    /// `<8-byte type-tag><8-byte name>`. The type-tag is `"model   "`
    /// for standard meshes (`tag` field; what callers filter on).
    /// The 8-byte `name` half is the texture identifier and matches
    /// IMG's `extract_texture_name` output. Verified against real
    /// city-zone data (DAT 330 chunk 252 in tshimonorig_06): MMB
    /// records contain `"model   jimeni_0"` and the matching IMG
    /// chunks register names like `"jimeni_0"`.
    ///
    /// Use this — not `variant_name_str` — to pair against IMG names.
    /// `variant_name_str` maps NUL to `.`, producing strings like
    /// `kabe_3..` that fail to match `kabe_3` from the IMG side
    /// (short names <8 chars are NUL-padded).
    pub fn texture_name_str(&self) -> String {
        let s: String = self
            .variant_name
            .iter()
            .map(|&b| {
                if (0x20..0x7f).contains(&b) {
                    b as char
                } else {
                    '\0'
                }
            })
            .take_while(|&c| c != '\0')
            .collect();
        s.trim_end().to_string()
    }

    /// Tag bytes (8 bytes; e.g. `b"model   "` or `b"clod    "`).
    pub fn tag<'b>(&'b self, payload: &'a [u8]) -> &'a [u8]
    where
        'a: 'b,
    {
        &payload[self.offset..self.offset + 8]
    }
}

/// True if all 8 bytes are valid for the first half of a textureName
/// (alphanumeric, underscore, space, or NUL padding). First byte must
/// NOT be space or NUL — those would mean the textureName starts blank
/// (padding-only region). NULs are allowed in trailing positions to
/// support short names like `s_kabe2\0`.
fn is_ascii_tag(b: &[u8]) -> bool {
    if b.len() != 8 || b[0] == b' ' || b[0] == 0 {
        return false;
    }
    b.iter()
        .all(|&c| c == 0 || c.is_ascii_alphanumeric() || c == b'_' || c == b' ')
}

/// True if all 8 bytes are valid for the second half of a textureName
/// (alphanumeric, underscore, space, or NUL padding). All-NUL is
/// ALLOWED here — short texture names (≤ 8 chars) legitimately leave
/// bytes 8..16 as NUL padding. The previous rejection of all-NUL
/// silently dropped every short-named texture (e.g. `s_kabe2\0\0…`),
/// which was a major source of missing/mistextured zone props.
fn is_ascii_variant(b: &[u8]) -> bool {
    if b.len() != 8 {
        return false;
    }
    b.iter()
        .all(|&c| c == 0 || c.is_ascii_alphanumeric() || c == b'_' || c == b' ')
}

/// One mesh produced by the structural [`parse_models`] walk —
/// equivalent to `lotus-ffxi::FFXI::MMB::Mesh`. Carries everything the
/// renderer needs without any of the heuristic-scanner uncertainty.
#[derive(Debug, Clone)]
pub struct MmbModel {
    /// Last 8 bytes of the 16-byte textureName field (the bare name
    /// portion; the first 8 bytes are always the `"model   "` type
    /// tag for this record kind). Matches IMG's
    /// [`crate::texture::extract_texture_name`] output exactly.
    pub texture_name: String,
    pub blending: u16,
    pub vertices: Vec<MmbVertex>,
    /// Already a triangle list (strip decoding is done internally
    /// when the source topology was a strip).
    pub indices: Vec<u16>,
}

/// Structurally walk an MMB chunk and return every per-model record
/// — the way lotus-ffxi's `MMB(buffer)` constructor does it.
///
/// Why this exists alongside [`MmbSubRecord::find_all`]: the heuristic
/// scanner walks the payload looking for 16-byte windows of ASCII text
/// that *look* like textureNames. It silently misses real records when
/// adjacent vertex data doesn't produce a clean ASCII boundary, when
/// a false-positive match shadows the next real record, or when an
/// MMB uses the 48-byte `SMMBBlockVertex2` stride (d3==2 files). For
/// city buildings the scanner can find just 1–2 of 5–10 actual models.
///
/// The structural walk follows the documented layout (lotus mmb.cppm
/// lines 235-413):
///   - SMMBHEAD (16 B) at decrypted[0..16] — `head->id == "MMB"`
///     selects type-1 (16-bit strip topology) vs type-2 (uses head2).
///   - SMMBHEAD2 also at decrypted[0..16] — type-2 layout. The `d3`
///     byte (decrypted[8] after the packed MMBSize:24+d1:8 prefix)
///     selects 48-byte SMMBBlockVertex2 if == 2, else 36-byte
///     SMMBBlockVertex.
///   - SMMBHeader at decrypted[16..64]: imgID(16) + pieces(4) +
///     6×f32 bbox(24) + offsetBlockHeader(4).
///   - Block-offset list at decrypted[64..]:
///     * if `offsetBlockHeader == 0 && pieces != 0`: 8×u32 pointers,
///       NULs filtered out.
///     * if `offsetBlockHeader == 0 && pieces == 0`: implicit, one
///       block lives directly at offset 64.
///     * if `offsetBlockHeader != 0`: that's the first block; any
///       additional pointers come from the 4..N words between offset
///       64 and offsetBlockHeader.
///   - For each piece: SMMBBlockHeader(32 B) = numModel(4) +
///     6×f32(24) + numFace(4). Followed by `numModel` inline
///     SMMBModelHeader records (20 B each), each immediately
///     followed by `vertexsize × Vertex` and then a u16 num_indices
///     + 2 B pad + indices (u16 × num_indices), plus an odd-pad u16
///     if num_indices is odd.
pub fn parse_models(decrypted: &[u8]) -> Vec<MmbModel> {
    const SMMB_HEAD_SIZE: usize = 16;
    const SMMB_HEADER_SIZE: usize = 48; // imgID(16)+pieces(4)+bbox(24)+offset(4)

    if decrypted.len() < SMMB_HEAD_SIZE + SMMB_HEADER_SIZE {
        return Vec::new();
    }

    // SMMBHEAD: ASCII id at bytes 0..3 distinguishes the two file
    // shapes. Type-1 files start with "MMB"; type-2 (more common in
    // zone DATs) don't.
    let is_v1 = &decrypted[0..3] == b"MMB";

    // For type-2 (SMMBHEAD2), the layout is:
    //   MMBSize: u24, d1: u8, d3: u8, d4: u8, d5: u8, d6: u8, name[8]
    // packed. `d3` sits at decrypted[4]. d3 == 2 means the file uses
    // the wide 48-byte SMMBBlockVertex2 layout (cloth/dressable
    // assets); otherwise the standard 36-byte SMMBBlockVertex.
    let d3 = if is_v1 { 0 } else { decrypted[4] };
    let vertex_stride: usize = if d3 == 2 { 48 } else { 36 };

    // SMMBHeader starts at offset 16.
    let header_off = SMMB_HEAD_SIZE;
    let pieces = u32::from_le_bytes([
        decrypted[header_off + 16],
        decrypted[header_off + 17],
        decrypted[header_off + 18],
        decrypted[header_off + 19],
    ]) as usize;
    let offset_block_header = u32::from_le_bytes([
        decrypted[header_off + 44],
        decrypted[header_off + 45],
        decrypted[header_off + 46],
        decrypted[header_off + 47],
    ]) as usize;

    // Block-offset list — see function docs for the three shapes.
    let mut offset_list: Vec<usize> = Vec::new();
    let mut cursor = header_off + SMMB_HEADER_SIZE; // 64
    if offset_block_header == 0 {
        if pieces != 0 {
            // 8 candidate u32 offsets follow the SMMBHeader; non-zero
            // ones are real block offsets.
            for _ in 0..8 {
                if cursor + 4 > decrypted.len() {
                    break;
                }
                let po = u32::from_le_bytes([
                    decrypted[cursor],
                    decrypted[cursor + 1],
                    decrypted[cursor + 2],
                    decrypted[cursor + 3],
                ]) as usize;
                if po != 0 {
                    offset_list.push(po);
                }
                cursor += 4;
            }
        } else {
            // Implicit single block directly after the header.
            offset_list.push(cursor);
        }
    } else {
        offset_list.push(offset_block_header);
        if offset_block_header > cursor {
            let pad = offset_block_header - cursor;
            for _ in (0..pad).step_by(4) {
                if cursor + 4 > decrypted.len() {
                    break;
                }
                let po = u32::from_le_bytes([
                    decrypted[cursor],
                    decrypted[cursor + 1],
                    decrypted[cursor + 2],
                    decrypted[cursor + 3],
                ]) as usize;
                if po != 0 {
                    offset_list.push(po);
                }
                cursor += 4;
            }
        }
    }

    let mut models: Vec<MmbModel> = Vec::new();
    for piece_idx in 0..pieces {
        let piece_off = match offset_list.get(piece_idx).copied() {
            Some(o) => o,
            None => break,
        };
        if piece_off + 32 > decrypted.len() {
            break;
        }
        let num_model = u32::from_le_bytes([
            decrypted[piece_off],
            decrypted[piece_off + 1],
            decrypted[piece_off + 2],
            decrypted[piece_off + 3],
        ]) as usize;
        // lotus debug-breaks on numModel > 50; we accept up to 100 as
        // a sanity bound (real assets max out around 30-40).
        if num_model > 100 {
            break;
        }
        let mut off = piece_off + 32; // skip SMMBBlockHeader

        for _ in 0..num_model {
            if off + 20 > decrypted.len() {
                break;
            }
            // SMMBModelHeader: textureName[16] + vertexsize:u16 + blending:u16
            let texture_name = {
                // Last 8 bytes of textureName (matches IMG's
                // extract_texture_name output).
                let name_bytes = &decrypted[off + 8..off + 16];
                let s: String = name_bytes
                    .iter()
                    .map(|&b| {
                        if (0x20..0x7f).contains(&b) {
                            b as char
                        } else {
                            '\0'
                        }
                    })
                    .take_while(|&c| c != '\0')
                    .collect();
                s.trim_end().to_string()
            };
            let vertexsize =
                u16::from_le_bytes([decrypted[off + 16], decrypted[off + 17]]) as usize;
            let blending = u16::from_le_bytes([decrypted[off + 18], decrypted[off + 19]]);
            off += 20;

            // Parse vertices at the file's stride.
            let vert_bytes = vertexsize * vertex_stride;
            if off + vert_bytes > decrypted.len() {
                break;
            }
            let mut vertices = Vec::with_capacity(vertexsize);
            for vi in 0..vertexsize {
                let vo = off + vi * vertex_stride;
                let pos = [
                    f32::from_le_bytes([
                        decrypted[vo],
                        decrypted[vo + 1],
                        decrypted[vo + 2],
                        decrypted[vo + 3],
                    ]),
                    f32::from_le_bytes([
                        decrypted[vo + 4],
                        decrypted[vo + 5],
                        decrypted[vo + 6],
                        decrypted[vo + 7],
                    ]),
                    f32::from_le_bytes([
                        decrypted[vo + 8],
                        decrypted[vo + 9],
                        decrypted[vo + 10],
                        decrypted[vo + 11],
                    ]),
                ];
                // d3==2 inserts a 12-byte displacement field before
                // the normal; advance past it. Layout (lotus mmb.cppm
                // SMMBBlockVertex2):
                //   x,y,z (12) + dx,dy,dz (12) + hx,hy,hz (12) +
                //   color (4) + u,v (8) = 48
                let normal_base = if d3 == 2 { vo + 24 } else { vo + 12 };
                let normal = [
                    f32::from_le_bytes([
                        decrypted[normal_base],
                        decrypted[normal_base + 1],
                        decrypted[normal_base + 2],
                        decrypted[normal_base + 3],
                    ]),
                    f32::from_le_bytes([
                        decrypted[normal_base + 4],
                        decrypted[normal_base + 5],
                        decrypted[normal_base + 6],
                        decrypted[normal_base + 7],
                    ]),
                    f32::from_le_bytes([
                        decrypted[normal_base + 8],
                        decrypted[normal_base + 9],
                        decrypted[normal_base + 10],
                        decrypted[normal_base + 11],
                    ]),
                ];
                let color_base = normal_base + 12;
                let rgba = [
                    decrypted[color_base],
                    decrypted[color_base + 1],
                    decrypted[color_base + 2],
                    decrypted[color_base + 3],
                ];
                let uv_base = color_base + 4;
                let uv = [
                    f32::from_le_bytes([
                        decrypted[uv_base],
                        decrypted[uv_base + 1],
                        decrypted[uv_base + 2],
                        decrypted[uv_base + 3],
                    ]),
                    f32::from_le_bytes([
                        decrypted[uv_base + 4],
                        decrypted[uv_base + 5],
                        decrypted[uv_base + 6],
                        decrypted[uv_base + 7],
                    ]),
                ];
                vertices.push(MmbVertex {
                    pos,
                    normal,
                    rgba,
                    uv,
                });
            }
            off += vert_bytes;

            // u16 num_indices, then 2 bytes pad (offset += 4 in lotus).
            if off + 4 > decrypted.len() {
                break;
            }
            let num_indices = u16::from_le_bytes([decrypted[off], decrypted[off + 1]]) as usize;
            off += 4;

            // Topology: triangle-list when type-1 or (type-2 with
            // d3==2); otherwise triangle-strip. Convert strip → list
            // inline so the downstream renderer doesn't need to know.
            let mut indices: Vec<u16> = Vec::new();
            let is_list = is_v1 || (!is_v1 && d3 == 2);
            if off + num_indices * 2 > decrypted.len() {
                break;
            }
            if is_list {
                for i in 0..num_indices {
                    let p = off + i * 2;
                    indices.push(u16::from_le_bytes([decrypted[p], decrypted[p + 1]]));
                }
            } else if num_indices >= 3 {
                // Strip with parity-based winding flip (lotus mmb.cppm:381-403).
                // Drop triangles where i1==i2 or i2==i3 (degenerate);
                // keep i1==i3 (legitimate pinch).
                for i in 0..(num_indices - 2) {
                    let p = off + i * 2;
                    let i1 = u16::from_le_bytes([decrypted[p], decrypted[p + 1]]);
                    let i2 = u16::from_le_bytes([decrypted[p + 2], decrypted[p + 3]]);
                    let i3 = u16::from_le_bytes([decrypted[p + 4], decrypted[p + 5]]);
                    if i1 == i2 || i2 == i3 {
                        continue;
                    }
                    if i % 2 == 1 {
                        indices.extend_from_slice(&[i2, i1, i3]);
                    } else {
                        indices.extend_from_slice(&[i1, i2, i3]);
                    }
                }
            }
            off += num_indices * 2;
            if num_indices % 2 != 0 {
                off += 2; // odd-pad align
            }

            if !indices.is_empty() && !vertices.is_empty() {
                models.push(MmbModel {
                    texture_name,
                    blending,
                    vertices,
                    indices,
                });
            }
        }
    }

    models
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
    fn texture_name_str_returns_name_half_only() {
        // Real per-submesh textureName layout is
        // `<"model   "><8-char name>`. The IMG side registers the bare
        // name ("kabe_3"), so `texture_name_str` must return only the
        // second half — with NUL termination (the historical
        // `variant_name_str` mapped NUL -> '.' giving "kabe_3.." which
        // never matched).
        let mut buf = Vec::new();
        buf.extend_from_slice(b"model   "); // tag
        buf.extend_from_slice(b"kabe_3\0\0"); // name (6 chars + 2 NUL)
        buf.extend_from_slice(&8u16.to_le_bytes()); // vertexsize
        buf.extend_from_slice(&0u16.to_le_bytes()); // blending
        buf.extend(std::iter::repeat_n(0xFFu8, 64));
        let recs = MmbSubRecord::find_all(&buf);
        assert_eq!(recs.len(), 1, "scanner should accept the model record");
        assert_eq!(
            recs[0].texture_name_str(),
            "kabe_3",
            "NUL-padded short name should compare equal to IMG-side bare name"
        );
        // And an exact 8-char name should round-trip without truncation.
        let mut buf2 = Vec::new();
        buf2.extend_from_slice(b"model   ");
        buf2.extend_from_slice(b"jimeni_0"); // exact 8 chars, no NUL
        buf2.extend_from_slice(&8u16.to_le_bytes());
        buf2.extend_from_slice(&0u16.to_le_bytes());
        buf2.extend(std::iter::repeat_n(0xFFu8, 64));
        let recs2 = MmbSubRecord::find_all(&buf2);
        assert_eq!(recs2[0].texture_name_str(), "jimeni_0");
    }

    #[test]
    fn scanner_accepts_records_with_nonzero_blending() {
        // Regression: the previous count check rejected any record
        // where `blending != 0` because it read the u32 high bits as
        // part of count and required count <= 0xFFFF. A transparent-
        // glass wall with blending=0x8000 would silently disappear.
        let mut buf = Vec::new();
        buf.extend_from_slice(b"glass01\0\0\0\0\0\0\0\0\0"); // textureName[16]
        buf.extend_from_slice(&42u16.to_le_bytes()); // vertexsize
        buf.extend_from_slice(&0x8000u16.to_le_bytes()); // blending = transparent
        buf.extend(std::iter::repeat_n(0xFFu8, 64));
        let recs = MmbSubRecord::find_all(&buf);
        assert_eq!(
            recs.len(),
            1,
            "non-zero blending must not reject the record"
        );
        assert_eq!(recs[0].count, 42);
        assert_eq!(recs[0].blending, 0x8000);
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
