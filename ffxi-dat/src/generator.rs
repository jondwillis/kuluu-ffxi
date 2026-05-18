//! `Generator` chunk parser — kind `0x05`. Particle/sound/model
//! spawner. Closes the Scheduler→Generator→Sep chain for runtime
//! SFX trigger resolution.
//!
//! Full layout is documented in
//! `vendor/lotus-ffxi/ffxi/dat/generator.cppm` — ~140 fields across
//! a `GeneratorHeader` plus a 70-opcode bytecode for particle
//! behaviour. This Rust port intentionally only extracts the two
//! fields we need for SE wiring:
//!
//!   - `effect_type` (byte 29 of the `0x01` creation-command) —
//!     `0x3D` means "this generator produces a Sep-referenced
//!     sound" (lotus `GeneratorComponent::Type::Sound`).
//!   - `id` (4 chars at offset 8 of the `0x01` creation-command) —
//!     the 4-char name of the sibling chunk (a `Sep` for Sound
//!     generators) whose SE id will play.
//!
//! Header layout (`GeneratorHeader`, packed 2-byte aligned):
//!
//! ```text
//! 0x00 u8  flags1
//! 0x01 u8  bone_point
//! 0x02 u8  flags2
//! 0x03 u8  flags3
//! 0x04 u32[3] unknown1               (12 bytes)
//! 0x10 f32[16] unknown2              (64 bytes)
//! 0x50 u32 flags4
//! 0x54 u32[4] unknown3               (16 bytes)
//! 0x64 u16 unknown4
//! 0x66 u16 interval
//! 0x68 u8  occurences
//! 0x69 u8  flags5
//! 0x6A u16 unknown5
//! 0x6C u32 flags6
//! 0x70 u32 unknown_command_offset
//! 0x74 u32 creation_command_offset   ← where we scan for opcode 0x01
//! 0x78 u32 tick_command_offset
//! 0x7C u32 expiry_command_offset
//! ```
//!
//! Lotus' offset semantics: command offsets are relative to the
//! enclosing DAT *file*, not the chunk body, and lotus subtracts 16
//! (the chunk-header size) before using them. We replicate that.

use crate::{DatError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generator {
    pub name: [u8; 4],
    /// `0x3D` = Sound (lotus `Type::Sound`). Other observed values:
    /// `0x3C` model-ring, `0x3B` model-d3m, etc. We only need to
    /// recognize Sound for SE wiring; other types are surfaced as
    /// the raw byte for callers that care.
    pub effect_type: u8,
    /// 4-char name of the sibling chunk this generator references.
    /// For `effect_type == 0x3D`, this names a `Sep` child whose
    /// `se_id` is the .spw to play.
    pub id: [u8; 4],
}

impl Generator {
    pub fn is_sound(&self) -> bool {
        self.effect_type == 0x3D
    }

    /// Parse a Generator chunk body. Returns `Ok(None)` if the
    /// chunk is structurally valid but doesn't contain a `0x01`
    /// creation command (some Generators only have tick/expiry
    /// commands and don't fire a primary effect).
    pub fn parse(name: [u8; 4], body: &[u8]) -> Result<Option<Self>> {
        // Header is 0x80 bytes (see field offsets above; the field
        // at 0x7C is the last u32, ending at 0x80). Lotus' packed
        // struct sums to 0x80 with no padding.
        const HEADER_LEN: usize = 0x80;
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }
        let creation_offset = u32_le(body, 0x74) as usize;
        let tick_offset = u32_le(body, 0x78) as usize;
        // Lotus: `buffer + creation_command_offset - 16`. The minus
        // 16 backs out the chunk header that's NOT in our `body`
        // slice (we receive the body after `chunk.rs` already
        // stripped the header), so the corrected offset is
        // `creation_offset - 16`.
        if creation_offset < 16 || creation_offset - 16 >= body.len() {
            return Ok(None);
        }
        let creation_start = creation_offset - 16;
        let creation_end = if tick_offset >= 16 && tick_offset - 16 <= body.len() {
            tick_offset - 16
        } else {
            body.len()
        };

        // Walk the bytecode looking for opcode 0x01.
        let mut cursor = creation_start;
        while cursor + 4 <= creation_end {
            let data_type = body[cursor];
            let data_size_nibble = (body[cursor + 1] & 0x0F) as usize;
            let advance = data_size_nibble.saturating_mul(4);
            if data_type == 0x00 {
                break;
            }
            if data_type == 0x01 && advance >= 32 && cursor + 4 + 32 <= body.len() {
                // Lotus reads from `data2 + 8` for id (4 bytes),
                // `data2 + 29` for effect_type. `data2` is set to
                // `cursor + 4` (after consuming the type+size+pad).
                let payload = cursor + 4;
                let id = [
                    body[payload + 8],
                    body[payload + 9],
                    body[payload + 10],
                    body[payload + 11],
                ];
                let effect_type = body[payload + 29];
                return Ok(Some(Self {
                    name,
                    effect_type,
                    id,
                }));
            }
            // Lotus' loop: `data2 += 4` (skipping type+size+pad
            // header) and then `data2 += (data_size - 1) *
            // sizeof(uint32_t)`. Net advance = `data_size * 4`.
            if advance == 0 {
                break; // guard against infinite loop on malformed data
            }
            cursor = cursor.saturating_add(advance);
        }
        Ok(None)
    }
}

#[inline]
fn u32_le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_body() -> Vec<u8> {
        // 128-byte header, with creation_command_offset = 0x80 + 16
        // (so post-subtract = 0x80) and tick at 0x80+16+0x30. This
        // emulates a real Generator header where commands start
        // right after the header in the file.
        let mut buf = vec![0u8; 0x80];
        // creation_command_offset at 0x74
        buf[0x74..0x78].copy_from_slice(&(0x80u32 + 16).to_le_bytes());
        // tick_command_offset at 0x78
        buf[0x78..0x7C].copy_from_slice(&(0x80u32 + 16 + 0x30).to_le_bytes());
        buf
    }

    #[test]
    fn parses_sound_generator() {
        let mut body = make_body();
        // Append a 0x01 creation command:
        //   byte 0: 0x01 (type)
        //   byte 1: data_size_nibble=9 → 36 bytes total advance
        //   bytes 2-3: pad
        //   payload 0..32 (relative to cursor+4):
        //     0..8   billboard/pos_flags/zeros
        //     8..12  id "snd0"
        //     12..28 pos/etc
        //     28..29 zero
        //     29     effect_type = 0x3D (Sound)
        //     30..32 lifetime
        // Total: 4 + (9-1)*4 = 4 + 32 = 36 bytes
        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09; // data_size_nibble
        // Payload starts at cmd[4]; id at +8..+12, effect_type at +29
        cmd[4 + 8..4 + 12].copy_from_slice(b"snd0");
        cmd[4 + 29] = 0x3D;
        body.extend_from_slice(&cmd);
        // Fill out to tick_command_offset
        body.resize(body.len().max(0x80 + 0x30), 0);

        let g = Generator::parse(*b"gen0", &body).unwrap().unwrap();
        assert_eq!(g.name, *b"gen0");
        assert_eq!(g.id, *b"snd0");
        assert_eq!(g.effect_type, 0x3D);
        assert!(g.is_sound());
    }

    #[test]
    fn non_sound_generator_still_parses() {
        let mut body = make_body();
        let mut cmd = vec![0u8; 36];
        cmd[0] = 0x01;
        cmd[1] = 0x09;
        cmd[4 + 8..4 + 12].copy_from_slice(b"ring");
        cmd[4 + 29] = 0x3B; // model-d3m, not Sound
        body.extend_from_slice(&cmd);
        body.resize(body.len().max(0x80 + 0x30), 0);

        let g = Generator::parse(*b"gen1", &body).unwrap().unwrap();
        assert!(!g.is_sound());
        assert_eq!(g.effect_type, 0x3B);
    }

    #[test]
    fn missing_0x01_returns_none() {
        let body = make_body();
        // No 0x01 command appended.
        let g = Generator::parse(*b"empt", &body).unwrap();
        assert!(g.is_none());
    }

    #[test]
    fn truncated_header_errors() {
        let body = vec![0u8; 0x40];
        assert!(matches!(
            Generator::parse(*b"shrt", &body),
            Err(DatError::TruncatedChunk { needed: 0x80, .. })
        ));
    }
}
