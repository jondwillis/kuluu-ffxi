use crate::{AudioError, Result};

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

#[derive(Debug, Clone)]
pub struct AudioHeader {
    pub format: SampleFormat,
    pub channels: u8,

    pub block_size: u32,

    pub sample_blocks: u32,

    pub loop_start: i32,

    pub sample_rate: f32,

    pub is_streaming: bool,
}

impl AudioHeader {
    pub fn total_samples_per_channel(&self) -> u32 {
        self.sample_blocks * self.block_size
    }
}

const SPW_MAGIC: &[u8; 6] = b"SeWave";
const BGW_MAGIC: &[u8; 8] = b"BGMStrea";

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

        buf[0x14..0x18].copy_from_slice(&100u32.to_le_bytes());

        buf[0x1C..0x20].copy_from_slice(&22000u32.to_le_bytes());
        buf[0x20..0x24].copy_from_slice(&50u32.to_le_bytes());
        buf[0x2A] = 1;
        buf[0x2B] = 28;
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

        buf[0x18..0x1C].copy_from_slice(&500u32.to_le_bytes());
        buf[0x1C..0x20].copy_from_slice(&10u32.to_le_bytes());
        buf[0x20..0x24].copy_from_slice(&44100u32.to_le_bytes());
        buf[0x2E] = 2;
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
