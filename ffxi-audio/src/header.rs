//! `.spw` / `.bgw` container header parsing.
//!
//! Both formats are 48-byte fixed headers. Field layout taken from
//! `vendor/lotus-ffxi/ffxi/audio/ffxi_audio.cppm:46-79`.

use crate::{AudioError, Result};

/// Sample codec selector stored in the header at offset 0x0C (BGM) /
/// 0x0C (SFX). Matches `SampleFormat` enum in lotus-ffxi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    Adpcm = 0,
    Pcm = 1,
    Atrac3 = 3,
}

impl SampleFormat {
    pub fn from_u32(v: u32) -> Result<Self> {
        match v {
            0 => Ok(Self::Adpcm),
            1 => Ok(Self::Pcm),
            3 => Ok(Self::Atrac3),
            other => Err(AudioError::UnknownFormat(other)),
        }
    }
}

/// Common-shape audio header. Both SeWave (`.spw`) and BGMStream
/// (`.bgw`) headers fit into this — the wire layouts differ by
/// field offsets but the semantics are identical.
#[derive(Debug, Clone)]
pub struct AudioHeader {
    pub format: SampleFormat,
    pub channels: u8,
    /// Samples per channel per decoded ADPCM block (called `blocksize`
    /// in lotus). For PCM, the number of i16 samples read per channel
    /// chunk.
    pub block_size: u32,
    /// Total number of blocks in the file. Total samples per channel
    /// = `sample_blocks * block_size`.
    pub sample_blocks: u32,
    /// Loop entry, in *blocks*, 1-indexed (matching the file's wire
    /// encoding — vgmstream subtracts 1 before turning it into a
    /// sample position). `<= 0` means "no loop" — SPW files use
    /// `0xFFFFFFFF` (signed `-1`) as the sentinel, BGW files use
    /// `0`. Stored as `i32` so the sentinel survives.
    pub loop_start: i32,
    /// `sample_rate_high + sample_rate_low` (lotus stores it split;
    /// the sum is the actual rate, typically 24000 or 44100).
    pub sample_rate: f32,
    /// True for `.bgw` (BGMStream — interleaved output expected),
    /// false for `.spw` (SeWave — per-channel).
    pub is_streaming: bool,
}

impl AudioHeader {
    /// Total decoded sample count per channel.
    pub fn total_samples_per_channel(&self) -> u32 {
        self.sample_blocks * self.block_size
    }
}

const SPW_MAGIC: &[u8; 6] = b"SeWave";
const BGW_MAGIC: &[u8; 8] = b"BGMStrea"; // first 8 chars of "BGMStream"

/// Parse the magic bytes and dispatch to the right header parser.
/// Returns the parsed header plus the byte offset where the encoded
/// audio data begins (always 48).
pub fn parse_any(bytes: &[u8]) -> Result<(AudioHeader, usize)> {
    if bytes.len() < 48 {
        return Err(AudioError::HeaderTooShort {
            need: 48,
            have: bytes.len(),
        });
    }
    if &bytes[0..6] == SPW_MAGIC {
        Ok((parse_spw(bytes)?, 48))
    } else if &bytes[0..8] == BGW_MAGIC {
        Ok((parse_bgw(bytes)?, 48))
    } else {
        let mut tag = [0u8; 8];
        tag.copy_from_slice(&bytes[..8]);
        Err(AudioError::UnknownMagic(tag))
    }
}

/// SeWave (`.spw`) layout (lotus `SoundEffectHeader`, 48 bytes):
///
/// ```text
/// 0x00  char[8]   "SeWave\0\0"
/// 0x08  u32       size
/// 0x0C  u32       sample_format  (SampleFormat)
/// 0x10  u32       id
/// 0x14  u32       sample_blocks
/// 0x18  u32       loop_start
/// 0x1C  u32       sample_rate_high
/// 0x20  u32       sample_rate_low
/// 0x24  u32       unknown1
/// 0x28  u8        unknown2
/// 0x29  u8        unknown3
/// 0x2A  u8        channels
/// 0x2B  u8        blocksize
/// 0x2C  u32       unknown4
/// ```
pub fn parse_spw(bytes: &[u8]) -> Result<AudioHeader> {
    let format = SampleFormat::from_u32(u32_le(bytes, 0x0C))?;
    let sample_blocks = u32_le(bytes, 0x14);
    let loop_start = i32_le(bytes, 0x18);
    let sr_high = u32_le(bytes, 0x1C);
    let sr_low = u32_le(bytes, 0x20);
    let channels = bytes[0x2A];
    let block_size = bytes[0x2B] as u32;
    validate(channels, block_size)?;
    Ok(AudioHeader {
        format,
        channels,
        block_size,
        sample_blocks,
        loop_start,
        sample_rate: decode_sample_rate(sr_high, sr_low),
        is_streaming: false,
    })
}

/// BGMStream (`.bgw`) layout (lotus `BGMHeader`, 48 bytes):
///
/// ```text
/// 0x00  char[12]  "BGMStream\0\0\0"
/// 0x0C  u32       sample_format
/// 0x10  u32       size
/// 0x14  u32       id
/// 0x18  u32       sample_blocks
/// 0x1C  u32       loop_start
/// 0x20  u32       sample_rate_high
/// 0x24  u32       sample_rate_low
/// 0x28  u32       unknown1
/// 0x2C  u8        unknown2
/// 0x2D  u8        unknown3
/// 0x2E  u8        channels
/// 0x2F  u8        blocksize
/// ```
pub fn parse_bgw(bytes: &[u8]) -> Result<AudioHeader> {
    let format = SampleFormat::from_u32(u32_le(bytes, 0x0C))?;
    let sample_blocks = u32_le(bytes, 0x18);
    let loop_start = i32_le(bytes, 0x1C);
    let sr_high = u32_le(bytes, 0x20);
    let sr_low = u32_le(bytes, 0x24);
    let channels = bytes[0x2E];
    let block_size = bytes[0x2F] as u32;
    validate(channels, block_size)?;
    Ok(AudioHeader {
        format,
        channels,
        block_size,
        sample_blocks,
        loop_start,
        sample_rate: decode_sample_rate(sr_high, sr_low),
        is_streaming: true,
    })
}

/// FFXI obfuscates the sample rate as two u32s that have to be
/// summed with wrap then masked. The vgmstream reference
/// (`src/meta/bgw.c`) is the authoritative source for this
/// encoding: `rate = (low + high) & 0x7FFFFFFF` with the add
/// allowed to wrap as u32. For music101.bgw the sum-with-carry
/// produces 0xAC44 = 44100. Lotus' direct C++ add happens to work
/// because unsigned wrap is defined; the naive Rust `+` panics in
/// debug, hence the explicit `wrapping_add`.
#[inline]
fn decode_sample_rate(high: u32, low: u32) -> f32 {
    (high.wrapping_add(low) & 0x7FFF_FFFF) as f32
}

fn validate(channels: u8, block_size: u32) -> Result<()> {
    if channels == 0 || channels > 8 {
        return Err(AudioError::InvalidChannels(channels));
    }
    if block_size == 0 || block_size > 1024 {
        return Err(AudioError::InvalidBlockSize(block_size));
    }
    Ok(())
}

#[inline]
fn u32_le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
fn i32_le(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_short_input() {
        assert!(matches!(
            parse_any(&[0u8; 10]),
            Err(AudioError::HeaderTooShort { need: 48, have: 10 })
        ));
    }

    #[test]
    fn rejects_unknown_magic() {
        let mut buf = [0u8; 48];
        buf[..8].copy_from_slice(b"XXXX____");
        assert!(matches!(parse_any(&buf), Err(AudioError::UnknownMagic(_))));
    }

    #[test]
    fn parses_synthetic_spw() {
        let mut buf = [0u8; 48];
        buf[..6].copy_from_slice(b"SeWave");
        // format = ADPCM (0) at 0x0C — already zero
        // sample_blocks = 100 @ 0x14
        buf[0x14..0x18].copy_from_slice(&100u32.to_le_bytes());
        // loop_start = 0 @ 0x18 (no loop)
        // sample_rate split @ 0x1C/0x20: 22050 = 22000 + 50
        buf[0x1C..0x20].copy_from_slice(&22000u32.to_le_bytes());
        buf[0x20..0x24].copy_from_slice(&50u32.to_le_bytes());
        buf[0x2A] = 1; // mono
        buf[0x2B] = 28; // 28 samples / block (typical)
        let h = parse_spw(&buf).unwrap();
        assert_eq!(h.format, SampleFormat::Adpcm);
        assert_eq!(h.channels, 1);
        assert_eq!(h.block_size, 28);
        assert_eq!(h.sample_blocks, 100);
        assert_eq!(h.sample_rate, 22050.0);
        assert!(!h.is_streaming);
    }

    #[test]
    fn parses_synthetic_bgw() {
        let mut buf = [0u8; 48];
        buf[..8].copy_from_slice(b"BGMStrea");
        buf[8..12].copy_from_slice(b"m\0\0\0");
        // format = ADPCM @ 0x0C
        buf[0x18..0x1C].copy_from_slice(&500u32.to_le_bytes()); // blocks
        buf[0x1C..0x20].copy_from_slice(&10u32.to_le_bytes()); // loop block 10
        buf[0x20..0x24].copy_from_slice(&44100u32.to_le_bytes());
        buf[0x2E] = 2; // stereo
        buf[0x2F] = 28;
        let h = parse_bgw(&buf).unwrap();
        assert_eq!(h.channels, 2);
        assert_eq!(h.block_size, 28);
        assert_eq!(h.sample_blocks, 500);
        assert_eq!(h.loop_start, 10);
        assert_eq!(h.sample_rate, 44100.0);
        assert!(h.is_streaming);
    }
}
