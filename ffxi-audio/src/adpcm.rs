//! 4-bit ADPCM decoder, ported from
//! `vendor/lotus-ffxi/ffxi/audio/adpcm.cppm` (SFX, channel-major)
//! and `adpcmstream.cppm` (BGM, frame-interleaved).
//!
//! Block layout (per block):
//!
//! ```text
//! For each channel in [0..channels):
//!   1 byte:  header
//!     bits 0..3 = scale_low (scale = 12 - low)
//!     bits 4..7 = filter_index ∈ [0, 5)  (≥5 → silent block)
//!   (block_size / 2) bytes: packed nibble-pairs
//!     low nibble decoded first, then high
//! ```
//!
//! 5-entry filter tables (`filter0`/`filter1`) are PS2 ADPCM
//! coefficients — see `adpcm.cppm:37-38`.

use crate::header::AudioHeader;
use crate::{AudioError, Result};

const FILTER0: [i32; 5] = [0x0000, 0x00F0, 0x01CC, 0x0188, 0x01E8];
const FILTER1: [i32; 5] = [0x0000, 0x0000, -0x00D0, -0x00DC, -0x00F0];

/// Per-channel decoder state. `[prev1, prev2]` in lotus' indexing
/// (the most recent decoded sample is at index 0).
#[derive(Debug, Clone, Default)]
pub struct ChannelState {
    pub prev1: i32,
    pub prev2: i32,
}

/// Decode one ADPCM block for a single channel, *appending*
/// `block_size` samples to `out` as f32 in the range [-1.0, 1.0].
///
/// `block` is the (1 + block_size/2) byte slice for *this channel
/// only*. Returns silently (no samples emitted) for an invalid
/// filter index ≥ 5, matching lotus behaviour at `adpcm.cppm:97`.
pub fn decode_block_into(
    block: &[u8],
    block_size: u32,
    state: &mut ChannelState,
    out: &mut Vec<f32>,
) -> Result<()> {
    let half = (block_size / 2) as usize;
    if block.len() < 1 + half {
        return Err(AudioError::TruncatedBlock {
            offset: 0,
            need: 1 + half,
            have: block.len(),
        });
    }
    let hdr = block[0];
    let scale = 0x0Ci32 - (hdr & 0x0F) as i32;
    let filter_index = (hdr >> 4) as usize;
    if filter_index >= 5 {
        // lotus silently skips: the channel produces no samples for
        // this block. Empirically only seen in malformed files.
        return Ok(());
    }
    let f0 = FILTER0[filter_index];
    let f1 = FILTER1[filter_index];

    for sample_i in 0..half {
        let sample_byte = block[1 + sample_i];
        for nibble in 0..2 {
            let mut value = ((sample_byte >> (4 * nibble)) & 0x0F) as i32;
            if value >= 8 {
                value -= 16;
            }
            // Mirror lotus' `value <<= scale`. `scale` is always
            // non-negative in well-formed files (low_nibble ≤ 12).
            value <<= scale.max(0);
            value += (state.prev1 * f0 + state.prev2 * f1) / 256;
            let clamped = value.clamp(-0x8000, 0x7FFF);
            state.prev2 = state.prev1;
            state.prev1 = clamped;
            out.push((clamped as i16) as f32 / 32768.0);
        }
    }
    Ok(())
}

/// Decode a SeWave (`.spw`) body into a fully interleaved f32 buffer
/// (`L0,R0,L1,R1,...`). Internally the SeWave decoder is
/// channel-major (matches lotus `ADPCM` class which stores
/// `Vec<Vec<float>>`), so this function decodes per-channel first
/// then interleaves. For mono files the interleave step is a no-op.
pub fn decode_channel_major_then_interleave(
    data: &[u8],
    h: &AudioHeader,
) -> Result<Vec<f32>> {
    let channels = h.channels as usize;
    let block_size = h.block_size;
    let block_bytes_per_channel = 1 + (block_size / 2) as usize;
    let block_bytes_total = block_bytes_per_channel * channels;
    let frames_per_block = block_size as usize;
    let blocks = (data.len() / block_bytes_total).min(h.sample_blocks as usize);

    let mut per_channel: Vec<Vec<f32>> = (0..channels)
        .map(|_| Vec::with_capacity(blocks * frames_per_block))
        .collect();
    let mut states: Vec<ChannelState> = (0..channels).map(|_| ChannelState::default()).collect();

    for block_i in 0..blocks {
        let block_base = block_i * block_bytes_total;
        for ch in 0..channels {
            let off = block_base + ch * block_bytes_per_channel;
            let slice = &data[off..off + block_bytes_per_channel];
            decode_block_into(slice, block_size, &mut states[ch], &mut per_channel[ch])?;
        }
    }

    // Interleave. All channel vectors are the same length unless a
    // filter-skip happened, in which case lotus would also be short
    // — accept the shorter common length.
    let frames = per_channel.iter().map(|v| v.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(frames * channels);
    for i in 0..frames {
        for ch in 0..channels {
            out.push(per_channel[ch][i]);
        }
    }
    Ok(out)
}

/// Decode a BGMStream (`.bgw`) body into an interleaved f32 buffer.
/// This mirrors `ADPCMStream::getNextBlock` which writes
/// `output[((sample*2)+nibble)*channels + channel]` — the resulting
/// vector is already frame-interleaved.
pub fn decode_interleaved(data: &[u8], h: &AudioHeader) -> Result<Vec<f32>> {
    let channels = h.channels as usize;
    let block_size = h.block_size;
    let block_bytes_per_channel = 1 + (block_size / 2) as usize;
    let block_bytes_total = block_bytes_per_channel * channels;
    let frames_per_block = block_size as usize;
    let blocks = (data.len() / block_bytes_total).min(h.sample_blocks as usize);

    let mut out = Vec::with_capacity(blocks * frames_per_block * channels);
    let mut states: Vec<ChannelState> = (0..channels).map(|_| ChannelState::default()).collect();

    // Per-channel temporary buffer for one block.
    let mut tmp: Vec<f32> = Vec::with_capacity(frames_per_block);

    for block_i in 0..blocks {
        let block_base = block_i * block_bytes_total;
        // Decode this block per-channel into `tmp` slots, then weave.
        // Allocate a scratch (channels × frames_per_block) matrix.
        let mut scratch: Vec<Vec<f32>> = (0..channels)
            .map(|_| Vec::with_capacity(frames_per_block))
            .collect();
        for ch in 0..channels {
            let off = block_base + ch * block_bytes_per_channel;
            let slice = &data[off..off + block_bytes_per_channel];
            tmp.clear();
            decode_block_into(slice, block_size, &mut states[ch], &mut tmp)?;
            scratch[ch].extend_from_slice(&tmp);
        }
        let frames = scratch.iter().map(|v| v.len()).min().unwrap_or(0);
        for i in 0..frames {
            for ch in 0..channels {
                out.push(scratch[ch][i]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block whose header says "silent filter" (filter_index = 0,
    /// scale = 12) and all-zero nibbles should produce a run of
    /// zeros equal to `block_size` samples.
    #[test]
    fn silent_block_yields_zeros() {
        let block_size = 28u32;
        let mut block = vec![0u8; 1 + (block_size / 2) as usize];
        block[0] = 0x00; // filter 0, scale low = 0 → scale = 12
        let mut st = ChannelState::default();
        let mut out = Vec::new();
        decode_block_into(&block, block_size, &mut st, &mut out).unwrap();
        assert_eq!(out.len(), block_size as usize);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(st.prev1, 0);
        assert_eq!(st.prev2, 0);
    }

    /// A block with filter_index = 0 and a single +1 nibble at the
    /// first position should push the predictor by (1 << 12) = 4096.
    /// Verifies the per-sample math matches lotus' formula exactly.
    #[test]
    fn first_nibble_predictor_step_output() {
        let block_size = 28u32;
        let mut block = vec![0u8; 1 + (block_size / 2) as usize];
        block[0] = 0x00; // scale = 12, filter = 0
        block[1] = 0x01; // low nibble = 1, high nibble = 0
        let mut st = ChannelState::default();
        let mut out = Vec::new();
        decode_block_into(&block, block_size, &mut st, &mut out).unwrap();
        // With filter 0 (zero coefficients), value = 1 << 12 = 4096.
        // First sample (low nibble) = 4096 / 32768 ≈ 0.125.
        let expected = 4096.0_f32 / 32768.0;
        assert!((out[0] - expected).abs() < 1e-6, "got {}", out[0]);
        // Second sample (high nibble) = 0 shifted, plus predictor
        // term (filter 0 = zero), = 0.
        assert_eq!(out[1], 0.0);
        // The full block runs to 28 samples — subsequent zero-nibbles
        // do not assert on final state here; see
        // `state_after_minimal_two_sample_block` for that.
    }

    /// Same nibble pair as `first_nibble_predictor_step_output` but
    /// in a minimal `block_size = 2` block (one sample byte = 2
    /// samples). After exactly two samples, state should land at
    /// (prev1=0, prev2=4096): low-nibble decode set prev1=4096,
    /// high-nibble decode shifted prev1=4096 → prev2 and set
    /// prev1=0. This is what the original (oversized) state check
    /// was trying to verify.
    #[test]
    fn state_after_minimal_two_sample_block() {
        let block_size = 2u32;
        let mut block = vec![0u8; 1 + (block_size / 2) as usize];
        block[0] = 0x00; // scale = 12, filter = 0
        block[1] = 0x01; // low = 1, high = 0
        let mut st = ChannelState::default();
        let mut out = Vec::new();
        decode_block_into(&block, block_size, &mut st, &mut out).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(st.prev1, 0);
        assert_eq!(st.prev2, 4096);
    }

    #[test]
    fn invalid_filter_index_is_no_op() {
        let block_size = 28u32;
        let mut block = vec![0u8; 1 + (block_size / 2) as usize];
        block[0] = 0x50; // filter index 5 → invalid
        let mut st = ChannelState::default();
        let mut out = Vec::new();
        decode_block_into(&block, block_size, &mut st, &mut out).unwrap();
        assert!(out.is_empty());
    }
}
