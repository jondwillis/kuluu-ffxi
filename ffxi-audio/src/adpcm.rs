use crate::header::AudioHeader;
use crate::{AudioError, Result};

const FILTER0: [i32; 5] = [0x0000, 0x00F0, 0x01CC, 0x0188, 0x01E8];
const FILTER1: [i32; 5] = [0x0000, 0x0000, -0x00D0, -0x00DC, -0x00F0];

#[derive(Debug, Clone, Default)]
pub struct ChannelState {
    pub prev1: i32,
    pub prev2: i32,
}

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

pub fn decode_channel_major_then_interleave(data: &[u8], h: &AudioHeader) -> Result<Vec<f32>> {
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
        for (ch, chan) in per_channel.iter_mut().enumerate() {
            let off = block_base + ch * block_bytes_per_channel;
            let slice = &data[off..off + block_bytes_per_channel];
            decode_block_into(slice, block_size, &mut states[ch], chan)?;
        }
    }

    let frames = per_channel.iter().map(|v| v.len()).min().unwrap_or(0);
    let mut out = Vec::with_capacity(frames * channels);

    out.extend((0..frames).flat_map(|i| per_channel.iter().map(move |c| c[i])));
    Ok(out)
}

pub fn decode_interleaved(data: &[u8], h: &AudioHeader) -> Result<Vec<f32>> {
    let channels = h.channels as usize;
    let block_size = h.block_size;
    let block_bytes_per_channel = 1 + (block_size / 2) as usize;
    let block_bytes_total = block_bytes_per_channel * channels;
    let frames_per_block = block_size as usize;
    let blocks = (data.len() / block_bytes_total).min(h.sample_blocks as usize);

    let mut out = Vec::with_capacity(blocks * frames_per_block * channels);
    let mut states: Vec<ChannelState> = (0..channels).map(|_| ChannelState::default()).collect();

    let mut tmp: Vec<f32> = Vec::with_capacity(frames_per_block);

    for block_i in 0..blocks {
        let block_base = block_i * block_bytes_total;

        let mut scratch: Vec<Vec<f32>> = (0..channels)
            .map(|_| Vec::with_capacity(frames_per_block))
            .collect();
        for (ch, scratch_chan) in scratch.iter_mut().enumerate() {
            let off = block_base + ch * block_bytes_per_channel;
            let slice = &data[off..off + block_bytes_per_channel];
            tmp.clear();
            decode_block_into(slice, block_size, &mut states[ch], &mut tmp)?;
            scratch_chan.extend_from_slice(&tmp);
        }
        let frames = scratch.iter().map(|v| v.len()).min().unwrap_or(0);

        out.extend((0..frames).flat_map(|i| scratch.iter().map(move |c| c[i])));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silent_block_yields_zeros() {
        let block_size = 28u32;
        let mut block = vec![0u8; 1 + (block_size / 2) as usize];
        block[0] = 0x00;
        let mut st = ChannelState::default();
        let mut out = Vec::new();
        decode_block_into(&block, block_size, &mut st, &mut out).unwrap();
        assert_eq!(out.len(), block_size as usize);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(st.prev1, 0);
        assert_eq!(st.prev2, 0);
    }

    #[test]
    fn first_nibble_predictor_step_output() {
        let block_size = 28u32;
        let mut block = vec![0u8; 1 + (block_size / 2) as usize];
        block[0] = 0x00;
        block[1] = 0x01;
        let mut st = ChannelState::default();
        let mut out = Vec::new();
        decode_block_into(&block, block_size, &mut st, &mut out).unwrap();

        let expected = 4096.0_f32 / 32768.0;
        assert!((out[0] - expected).abs() < 1e-6, "got {}", out[0]);

        assert_eq!(out[1], 0.0);
    }

    #[test]
    fn state_after_minimal_two_sample_block() {
        let block_size = 2u32;
        let mut block = vec![0u8; 1 + (block_size / 2) as usize];
        block[0] = 0x00;
        block[1] = 0x01;
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
        block[0] = 0x50;
        let mut st = ChannelState::default();
        let mut out = Vec::new();
        decode_block_into(&block, block_size, &mut st, &mut out).unwrap();
        assert!(out.is_empty());
    }
}
