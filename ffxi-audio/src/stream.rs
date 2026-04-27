use crate::adpcm::{decode_block_into, ChannelState};
use crate::header::AudioHeader;
use crate::Result;

pub struct AdpcmStream<'a> {
    pub header: AudioHeader,
    data: &'a [u8],
    states: Vec<ChannelState>,
    loop_snapshot: Option<Vec<ChannelState>>,
    current_block: u32,
    block_bytes_per_channel: usize,
    block_bytes_total: usize,
}

impl<'a> AdpcmStream<'a> {
    pub fn new(data: &'a [u8], header: AudioHeader) -> Self {
        let channels = header.channels as usize;
        let block_bytes_per_channel = 1 + (header.block_size / 2) as usize;
        let block_bytes_total = block_bytes_per_channel * channels;
        Self {
            states: (0..channels).map(|_| ChannelState::default()).collect(),
            loop_snapshot: None,
            current_block: 0,
            block_bytes_per_channel,
            block_bytes_total,
            data,
            header,
        }
    }

    pub fn next_block_interleaved(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        out.clear();
        if self.current_block >= self.header.sample_blocks {
            return Ok(false);
        }

        if self.header.loop_start > 0 && self.current_block == (self.header.loop_start - 1) as u32 {
            self.loop_snapshot = Some(self.states.clone());
        }
        let channels = self.header.channels as usize;
        let frames = self.header.block_size as usize;
        let base = self.current_block as usize * self.block_bytes_total;
        if base + self.block_bytes_total > self.data.len() {
            return Ok(false);
        }
        let mut scratch: Vec<Vec<f32>> =
            (0..channels).map(|_| Vec::with_capacity(frames)).collect();
        for (ch, scratch_chan) in scratch.iter_mut().enumerate() {
            let off = base + ch * self.block_bytes_per_channel;
            let slice = &self.data[off..off + self.block_bytes_per_channel];
            decode_block_into(
                slice,
                self.header.block_size,
                &mut self.states[ch],
                scratch_chan,
            )?;
        }
        let actual_frames = scratch.iter().map(|v| v.len()).min().unwrap_or(0);

        out.extend((0..actual_frames).flat_map(|i| scratch.iter().map(move |c| c[i])));
        self.current_block += 1;
        Ok(true)
    }

    pub fn reset_loop(&mut self) {
        if let Some(snap) = &self.loop_snapshot {
            self.states = snap.clone();
            self.current_block = (self.header.loop_start - 1).max(0) as u32;
        } else {
            for s in &mut self.states {
                *s = ChannelState::default();
            }
            self.current_block = 0;
        }
    }
}
