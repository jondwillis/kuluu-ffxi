//! Raw int16 PCM normaliser. Port of
//! `vendor/lotus-ffxi/ffxi/audio/pcm.cppm`.
//!
//! Wire format (after the 48-byte header):
//! ```text
//! Repeated until EOF:
//!   For each channel:
//!     int16[block_size] little-endian
//! ```
//! Channels are laid out block-by-block in channel-major order, just
//! like the ADPCM SeWave variant.

use crate::header::AudioHeader;
use crate::Result;

pub fn decode_interleaved(data: &[u8], h: &AudioHeader) -> Result<Vec<f32>> {
    let channels = h.channels as usize;
    let block_size = h.block_size as usize;
    let bytes_per_channel_block = block_size * 2;
    let bytes_per_block = bytes_per_channel_block * channels;
    let blocks = data.len() / bytes_per_block;

    let mut per_channel: Vec<Vec<f32>> = (0..channels)
        .map(|_| Vec::with_capacity(blocks * block_size))
        .collect();

    for block_i in 0..blocks {
        let base = block_i * bytes_per_block;
        for (ch, chan) in per_channel.iter_mut().enumerate() {
            let chunk_base = base + ch * bytes_per_channel_block;
            for s in 0..block_size {
                let off = chunk_base + s * 2;
                let val = i16::from_le_bytes([data[off], data[off + 1]]);
                chan.push(val as f32 / 32768.0);
            }
        }
    }

    let frames = per_channel.iter().map(|v| v.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(frames * channels);
    // Frame-interleave: emit frame i of every channel before advancing.
    out.extend((0..frames).flat_map(|i| per_channel.iter().map(move |c| c[i])));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::SampleFormat;

    #[test]
    fn mono_passthrough() {
        let h = AudioHeader {
            format: SampleFormat::Pcm,
            channels: 1,
            block_size: 4,
            sample_blocks: 1,
            loop_start: 0,
            sample_rate: 22050.0,
            is_streaming: false,
        };
        // 4 samples: i16 [0, 16384, -16384, 32767]
        let mut data = Vec::new();
        for v in [0i16, 16384, -16384, 32767] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let out = decode_interleaved(&data, &h).unwrap();
        assert_eq!(out.len(), 4);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-4);
        assert!((out[2] - -0.5).abs() < 1e-4);
        assert!((out[3] - (32767.0 / 32768.0)).abs() < 1e-4);
    }

    #[test]
    fn stereo_interleaves_correctly() {
        let h = AudioHeader {
            format: SampleFormat::Pcm,
            channels: 2,
            block_size: 2,
            sample_blocks: 1,
            loop_start: 0,
            sample_rate: 22050.0,
            is_streaming: false,
        };
        // One block: L0,L1 then R0,R1
        let mut data = Vec::new();
        for v in [100i16, 200, 300, 400] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        let out = decode_interleaved(&data, &h).unwrap();
        // Output interleaved: L0,R0,L1,R1 → 100,300,200,400 / 32768
        assert_eq!(out.len(), 4);
        let s = |x: f32| (x * 32768.0).round() as i16;
        assert_eq!(s(out[0]), 100);
        assert_eq!(s(out[1]), 300);
        assert_eq!(s(out[2]), 200);
        assert_eq!(s(out[3]), 400);
    }
}
