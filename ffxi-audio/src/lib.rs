//! FFXI audio container parsing and decoding.
//!
//! Two container formats live on disk under
//! `{install}/sound{,2..15}/win/`:
//!
//! - `.spw` ("SeWave") — sound effects: ADPCM or PCM, channel-major.
//! - `.bgw` ("BGMStream") — streaming music: ADPCM, frame-interleaved
//!   on output, with a loop point given in *blocks*.
//!
//! Decoder math (4-bit ADPCM with 5-entry filter tables, PS2-derived)
//! is a direct Rust port of `vendor/lotus-ffxi/ffxi/audio/adpcm.cppm`
//! and `adpcmstream.cppm` (GPL-3, license-compatible with this
//! workspace).
//!
//! `ATRAC3` (SampleFormat=3) is recognized but not decoded; only a
//! handful of files use it and ATRAC3 implementation is deferred.

pub mod adpcm;
pub mod header;
pub mod path;
pub mod pcm;
pub mod stream;

pub use header::{parse_any, parse_bgw, parse_spw, AudioHeader, SampleFormat};
pub use path::{find_audio, AudioKind};

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("file too short for header: need {need} bytes, have {have}")]
    HeaderTooShort { need: usize, have: usize },

    #[error("unknown audio magic: {0:?}")]
    UnknownMagic([u8; 8]),

    #[error("unknown SampleFormat value: {0}")]
    UnknownFormat(u32),

    #[error("ATRAC3 not supported (file id may be one of the rare ATRAC3 cutscene tracks)")]
    Atrac3Unsupported,

    #[error("truncated audio block at byte {offset}: need {need}, have {have}")]
    TruncatedBlock {
        offset: usize,
        need: usize,
        have: usize,
    },

    #[error("invalid channel count {0}")]
    InvalidChannels(u8),

    #[error("invalid block size {0}")]
    InvalidBlockSize(u32),
}

pub type Result<T> = std::result::Result<T, AudioError>;

/// Fully decoded audio with metadata. Samples are interleaved
/// (`L0,R0,L1,R1,...`) for ≥1 channel.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    pub samples: Vec<f32>,
    pub channels: u8,
    pub sample_rate: f32,
    /// Loop start, in *output samples per channel* (not bytes, not
    /// blocks). `None` if the file does not loop.
    pub loop_start_sample: Option<u32>,
}

impl DecodedAudio {
    /// Total frames (samples per channel).
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }
}

/// Load and decode a `.spw` or `.bgw` file in one shot. Streaming
/// decode (incremental block-by-block, for engines that want to keep
/// the encoded data in memory and decode on demand) is in
/// [`stream::AdpcmStream`].
pub fn decode_file(path: impl AsRef<Path>) -> Result<DecodedAudio> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).map_err(|e| AudioError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    decode_bytes(&bytes)
}

pub fn decode_bytes(bytes: &[u8]) -> Result<DecodedAudio> {
    let (header, data_offset) = parse_any(bytes)?;
    let data = &bytes[data_offset..];

    let samples = match header.format {
        SampleFormat::Adpcm => {
            if header.is_streaming {
                adpcm::decode_interleaved(data, &header)?
            } else {
                adpcm::decode_channel_major_then_interleave(data, &header)?
            }
        }
        SampleFormat::Pcm => pcm::decode_interleaved(data, &header)?,
        SampleFormat::Atrac3 => return Err(AudioError::Atrac3Unsupported),
    };

    let loop_start_sample = if header.loop_start > 0 {
        // vgmstream: `loop_start_sample = (loop_start - 1) * block_align`
        // for ADPCM, and `(loop_start - 1)` for PCM (block_align=1).
        // `loop_start` in the file is 1-indexed block count.
        let zero_indexed = (header.loop_start - 1) as u32;
        Some(match header.format {
            SampleFormat::Adpcm => zero_indexed.saturating_mul(header.block_size),
            SampleFormat::Pcm => zero_indexed,
            SampleFormat::Atrac3 => unreachable!(),
        })
    } else {
        None
    };

    Ok(DecodedAudio {
        samples,
        channels: header.channels,
        sample_rate: header.sample_rate,
        loop_start_sample,
    })
}
