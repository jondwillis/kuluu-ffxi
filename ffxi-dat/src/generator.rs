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

/// A FFXI-faithful dynamic point light decoded from a particle `Generator`
/// whose particle is flagged as a point light.
///
/// Values are pre-composed to XIM's runtime form (`Particle.kt:418-426`,
/// cross-referenced from `research/xim`):
///   * `range`        = `range · rangeMultiplier`
///   * `attenuation`  = `1 / (theta · thetaMultiplier)` — the *quadratic*
///     coefficient in `1/(c + l·d + q·d²)`; const/linear are 0 (the
///     consumer adds the `0.5` const dampen for actors, `GLDrawer.kt`).
///   * `color`        = particle base color (0..1) `· 2`, so the warm
///     channels can exceed 1 like XIM's `withMultiplied(2f)`.
///   * `base_position`= the particle's local offset from the emitter; the
///     world light position is the zone object's transform applied to this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointLightDef {
    pub range: f32,
    pub attenuation: f32,
    pub color: [f32; 4],
    pub base_position: [f32; 3],
}

impl Generator {
    /// `linkedDataType` (`StandardParticleSetup` byte at payload+29) that
    /// marks a particle as a point light. XIM `LinkedDataType.PointLight`.
    const LINKED_DATA_POINT_LIGHT: u8 = 0x47;

    /// Parse a Generator body for a dynamic point light. Walks the same
    /// particle-initializer block stream as [`Generator::parse`] (each block
    /// is `[opcode:u8, size_in_words:u8 & 0x1F, ...]`, `opcode 0x00` ends the
    /// stream), collecting the three blocks XIM uses for point lights:
    ///   * `0x01` StandardParticleSetup → `linkedDataType` (gate) + basePosition
    ///   * `0x16` ColorSetup            → particle color (defaults to white)
    ///   * `0x58` PointLightParamsInitializer → range/theta/multipliers
    ///
    /// Returns `Ok(None)` when the generator isn't a point light or lacks the
    /// `0x58` params block. Reuses the header offsets of [`Generator::parse`].
    ///
    /// NOTE: the block size field is read as `byte1 & 0x1F` (XIM's
    /// `(opCodeConfig >> 8) & 0x1F`), wider than [`Generator::parse`]'s
    /// `& 0x0F`; both agree for the small sizes seen in practice.
    pub fn parse_point_light(body: &[u8]) -> Result<Option<PointLightDef>> {
        const HEADER_LEN: usize = 0x80;
        if body.len() < HEADER_LEN {
            return Err(DatError::TruncatedChunk {
                offset: 0,
                needed: HEADER_LEN,
                available: body.len(),
            });
        }
        let creation_offset = u32_le(body, 0x74) as usize;
        if creation_offset < 16 || creation_offset - 16 >= body.len() {
            return Ok(None);
        }
        let mut cursor = creation_offset - 16;

        let mut is_point_light = false;
        let mut base_position = [0.0f32; 3];
        let mut color = [1.0f32; 4]; // default white; ColorSetup overrides
        // (range, theta, range_mult, theta_mult) with the multipliers mapped.
        let mut params: Option<(f32, f32, f32, f32)> = None;

        while cursor + 4 <= body.len() {
            let opcode = body[cursor];
            if opcode == 0x00 {
                break; // end of the initializer section
            }
            let size_words = (body[cursor + 1] & 0x1F) as usize;
            if size_words == 0 {
                break; // malformed: a real block is at least the 4-byte header
            }
            let block_len = size_words * 4;
            let payload = cursor + 4;
            if cursor + block_len > body.len() {
                break;
            }
            match opcode {
                0x01 => {
                    // basePosition (3×f32) @ +16, linkedDataType @ +29.
                    if payload + 28 <= body.len() {
                        base_position = [
                            f32_le(body, payload + 16),
                            f32_le(body, payload + 20),
                            f32_le(body, payload + 24),
                        ];
                    }
                    if payload + 30 <= body.len() {
                        is_point_light = body[payload + 29] == Self::LINKED_DATA_POINT_LIGHT;
                    }
                }
                0x16 => {
                    // ColorSetup: RGBA bytes (XIM `nextRGBA`), normalized 0..1.
                    if payload + 4 <= body.len() {
                        color = [
                            body[payload] as f32 / 255.0,
                            body[payload + 1] as f32 / 255.0,
                            body[payload + 2] as f32 / 255.0,
                            body[payload + 3] as f32 / 255.0,
                        ];
                    }
                }
                0x58 => {
                    // PointLightParamsInitializer: range, theta, rangeMult,
                    // thetaMult (4×f32); the two multipliers are remapped.
                    if payload + 16 <= body.len() {
                        params = Some((
                            f32_le(body, payload),
                            f32_le(body, payload + 4),
                            map_multiplier(f32_le(body, payload + 8)),
                            map_multiplier(f32_le(body, payload + 12)),
                        ));
                    }
                }
                _ => {}
            }
            cursor += block_len;
        }

        if !is_point_light {
            return Ok(None);
        }
        let Some((range, theta, range_mult, theta_mult)) = params else {
            return Ok(None);
        };
        let denom = theta * theta_mult;
        let attenuation = if denom.abs() > 1e-9 { 1.0 / denom } else { 0.0 };
        Ok(Some(PointLightDef {
            range: range * range_mult,
            attenuation,
            color: [color[0] * 2.0, color[1] * 2.0, color[2] * 2.0, color[3]],
            base_position,
        }))
    }
}

/// XIM's `mapMultiplier` (`ParticleInitializers.kt:969-977`): the stored
/// range/theta multiplier is encoded so `x >= 0` means `2^x`, `-1 <= x < 0`
/// means `1 + x` (a linear ramp down to 0), and anything below `-1` is `0`.
fn map_multiplier(base: f32) -> f32 {
    if base >= 0.0 {
        2.0f32.powf(base)
    } else if base >= -1.0 {
        1.0 + base
    } else {
        0.0
    }
}

#[inline]
fn u32_le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
fn f32_le(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
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

    /// Append one initializer block `[opcode, size_words, 0, 0] + payload`,
    /// padded so the whole block is `size_words * 4` bytes.
    fn push_block(buf: &mut Vec<u8>, opcode: u8, size_words: u8, payload: &[u8]) {
        let block_len = size_words as usize * 4;
        let start = buf.len();
        buf.push(opcode);
        buf.push(size_words);
        buf.push(0);
        buf.push(0);
        buf.extend_from_slice(payload);
        buf.resize(start + block_len, 0);
    }

    #[test]
    fn parses_point_light_generator() {
        let mut body = make_body();

        // 0x01 StandardParticleSetup: basePosition @ +16, linkedDataType @ +29.
        let mut setup = vec![0u8; 32];
        setup[16..20].copy_from_slice(&1.0f32.to_le_bytes());
        setup[20..24].copy_from_slice(&2.0f32.to_le_bytes());
        setup[24..28].copy_from_slice(&3.0f32.to_le_bytes());
        setup[29] = Generator::LINKED_DATA_POINT_LIGHT; // the gate parse checks
        push_block(&mut body, 0x01, 9, &setup);

        // 0x16 ColorSetup: warm RGBA.
        push_block(&mut body, 0x16, 2, &[255, 128, 64, 255]);

        // 0x58 PointLightParamsInitializer: range, theta, rangeMult, thetaMult.
        let mut params = Vec::new();
        params.extend_from_slice(&4.0f32.to_le_bytes()); // range
        params.extend_from_slice(&2.0f32.to_le_bytes()); // theta
        params.extend_from_slice(&0.0f32.to_le_bytes()); // rangeMult → 2^0 = 1
        params.extend_from_slice(&0.0f32.to_le_bytes()); // thetaMult → 1
        push_block(&mut body, 0x58, 5, &params);

        body.push(0x00); // terminate the stream

        let pl = Generator::parse_point_light(&body).unwrap().unwrap();
        assert_eq!(pl.range, 4.0, "range = range * rangeMult");
        assert_eq!(pl.attenuation, 0.5, "atten = 1/(theta * thetaMult)");
        assert_eq!(pl.base_position, [1.0, 2.0, 3.0]);
        // color = particle color (0..1) * 2; warm channels exceed 1.
        assert_eq!(pl.color[0], 2.0);
        assert!((pl.color[1] - (128.0 / 255.0) * 2.0).abs() < 1e-6);
        assert_eq!(pl.color[3], 1.0, "alpha is not doubled");
    }

    #[test]
    fn non_point_light_generator_is_none() {
        let mut body = make_body();
        let mut setup = vec![0u8; 32];
        setup[29] = 0x3D; // Audio
        push_block(&mut body, 0x01, 9, &setup);
        push_block(&mut body, 0x16, 2, &[255, 255, 255, 255]);
        body.push(0x00);
        assert!(Generator::parse_point_light(&body).unwrap().is_none());
    }

    #[test]
    fn point_light_without_params_is_none() {
        // linkedDataType says PointLight but no 0x58 block is present.
        let mut body = make_body();
        let mut setup = vec![0u8; 32];
        setup[29] = 0x47;
        push_block(&mut body, 0x01, 9, &setup);
        body.push(0x00);
        assert!(Generator::parse_point_light(&body).unwrap().is_none());
    }

    #[test]
    fn map_multiplier_branches() {
        assert_eq!(map_multiplier(0.0), 1.0); // 2^0
        assert_eq!(map_multiplier(1.0), 2.0); // 2^1
        assert_eq!(map_multiplier(-0.5), 0.5); // 1 + x
        assert_eq!(map_multiplier(-2.0), 0.0); // below -1 clamps to 0
    }
}
