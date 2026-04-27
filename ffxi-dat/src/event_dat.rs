//! Parser for FFXI per-zone **event / cutscene DAT** files — the compiled
//! bytecode every NPC and interactable in a zone is scripted with. The server
//! only sends a trigger (map packet 0x32); the client runs the local bytecode.
//!
//! Container layout per atom0s/XiEvents `Event DAT Structures.md`
//! (`research/XiEvents/`), reversed from PS2-beta DWARF symbols:
//!
//! ```text
//! eventheader_t { u32 BlockCount; u32 BlockSizes[BlockCount]; }
//! eventblock_t  { u32 Actornumber; u32 TagCount;
//!                 u16 TagOffset[TagCount]; u16 EvectExecNum[TagCount];
//!                 u32 ImedCount; u32 ImidData[ImedCount];
//!                 u32 EventDataSize; u8 EventData[align4(EventDataSize)]; }
//! ```
//!
//! This module parses the container only; the bytecode VM that interprets
//! `event_data` lives elsewhere.

/// `Actornumber` for the zone/player block — events not bound to a specific
/// entity (zone-in cutscenes, menu flows). Per XiEvents `Event DAT Structures`.
pub const ZONE_PLAYER_ACTOR: u32 = 0x7FFF_FFFF;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EventDatError {
    #[error("truncated event DAT: need {need} bytes at offset {at}, have {len}")]
    Truncated { at: usize, need: usize, len: usize },
    #[error("block {index} declares size {declared} but file has {remaining} bytes left")]
    BlockOverrun {
        index: usize,
        declared: usize,
        remaining: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBlock {
    /// Entity server id this block scripts; [`ZONE_PLAYER_ACTOR`] for zone events.
    pub actor: u32,
    /// `EvectExecNum` — event ids, parallel to [`Self::event_offsets`].
    pub event_ids: Vec<u16>,
    /// `TagOffset` — byte offset into [`Self::event_data`] where each event's
    /// bytecode begins. Parallel to [`Self::event_ids`].
    pub event_offsets: Vec<u16>,
    /// `ImidData` — immediate/reference table opcodes index into (event ids,
    /// item ids, string ids for DAT lookups, …).
    pub references: Vec<u32>,
    /// `EventData` — the raw bytecode, trimmed to the true unaligned
    /// `EventDataSize` (file padding to 4 bytes dropped).
    pub event_data: Vec<u8>,
}

impl EventBlock {
    /// Byte offset into [`Self::event_data`] where `event_id`'s bytecode starts,
    /// or `None` if this block has no such event. The VM enters here and follows
    /// the bytecode (which may jump anywhere within `event_data`), so callers run
    /// over the whole `event_data` from this offset rather than a fixed slice.
    pub fn event_entry(&self, event_id: u16) -> Option<usize> {
        self.event_ids
            .iter()
            .position(|&id| id == event_id)
            .and_then(|i| self.event_offsets.get(i).map(|&o| o as usize))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDat {
    pub blocks: Vec<EventBlock>,
}

impl EventDat {
    pub fn parse(buf: &[u8]) -> Result<Self, EventDatError> {
        let mut cur = Cursor::new(buf);
        let block_count = cur.u32()? as usize;
        let block_sizes: Vec<usize> = (0..block_count)
            .map(|_| cur.u32().map(|v| v as usize))
            .collect::<Result<_, _>>()?;

        let mut blocks = Vec::with_capacity(block_count);
        for (index, &size) in block_sizes.iter().enumerate() {
            let start = cur.pos;
            let slice = buf
                .get(start..start + size)
                .ok_or(EventDatError::BlockOverrun {
                    index,
                    declared: size,
                    remaining: buf.len().saturating_sub(start),
                })?;
            blocks.push(parse_block(slice)?);
            // BlockSizes is authoritative for the block boundary, so trailing
            // alignment padding past EventData is skipped naturally.
            cur.pos = start + size;
        }
        Ok(Self { blocks })
    }

    pub fn block_for_actor(&self, actor: u32) -> Option<&EventBlock> {
        self.blocks.iter().find(|b| b.actor == actor)
    }

    /// The zone/player event block ([`ZONE_PLAYER_ACTOR`]), if present.
    pub fn zone_block(&self) -> Option<&EventBlock> {
        self.block_for_actor(ZONE_PLAYER_ACTOR)
    }
}

fn parse_block(slice: &[u8]) -> Result<EventBlock, EventDatError> {
    let mut cur = Cursor::new(slice);
    let actor = cur.u32()?;
    let tag_count = cur.u32()? as usize;
    let event_offsets = (0..tag_count)
        .map(|_| cur.u16())
        .collect::<Result<_, _>>()?;
    let event_ids = (0..tag_count)
        .map(|_| cur.u16())
        .collect::<Result<_, _>>()?;
    let imed_count = cur.u32()? as usize;
    let references = (0..imed_count)
        .map(|_| cur.u32())
        .collect::<Result<_, _>>()?;
    let data_size = cur.u32()? as usize;
    let event_data = cur.bytes(data_size)?.to_vec();
    Ok(EventBlock {
        actor,
        event_ids,
        event_offsets,
        references,
        event_data,
    })
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], EventDatError> {
        let slice = self
            .buf
            .get(self.pos..self.pos + n)
            .ok_or(EventDatError::Truncated {
                at: self.pos,
                need: n,
                len: self.buf.len(),
            })?;
        self.pos += n;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, EventDatError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, EventDatError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], EventDatError> {
        self.take(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn align4(n: usize) -> usize {
        (n + 3) & !3
    }

    /// Build one `eventblock_t` body (without the size prefix), 4-byte padded.
    fn block_bytes(
        actor: u32,
        events: &[(u16, u16)], // (event_id, tag_offset)
        references: &[u32],
        event_data: &[u8],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&actor.to_le_bytes());
        b.extend_from_slice(&(events.len() as u32).to_le_bytes());
        for (_, off) in events {
            b.extend_from_slice(&off.to_le_bytes());
        }
        for (id, _) in events {
            b.extend_from_slice(&id.to_le_bytes());
        }
        b.extend_from_slice(&(references.len() as u32).to_le_bytes());
        for r in references {
            b.extend_from_slice(&r.to_le_bytes());
        }
        b.extend_from_slice(&(event_data.len() as u32).to_le_bytes());
        b.extend_from_slice(event_data);
        b.resize(align4(b.len()), 0); // EventData 4-byte alignment
        b
    }

    fn dat_bytes(blocks: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
        for blk in blocks {
            out.extend_from_slice(&(blk.len() as u32).to_le_bytes());
        }
        for blk in blocks {
            out.extend_from_slice(blk);
        }
        out
    }

    #[test]
    fn parses_two_blocks_with_unaligned_event_data() {
        // 3-byte event_data forces 1 byte of alignment padding, so the second
        // block must still parse — i.e. the boundary tracking is correct.
        let b0 = block_bytes(
            0x0100_0042,
            &[(5, 0), (9, 2)],
            &[0xDEAD_BEEF, 0x1234],
            &[0xAA, 0xBB, 0xCC],
        );
        let b1 = block_bytes(ZONE_PLAYER_ACTOR, &[(1, 0)], &[], &[0x42, 0x43, 0x44, 0x45]);
        let dat = EventDat::parse(&dat_bytes(&[b0, b1])).expect("parse");

        assert_eq!(dat.blocks.len(), 2);
        let blk = &dat.blocks[0];
        assert_eq!(blk.actor, 0x0100_0042);
        assert_eq!(blk.event_ids, vec![5, 9]);
        assert_eq!(blk.event_offsets, vec![0, 2]);
        assert_eq!(blk.references, vec![0xDEAD_BEEF, 0x1234]);
        assert_eq!(blk.event_data, vec![0xAA, 0xBB, 0xCC]);

        assert_eq!(blk.event_entry(5), Some(0));
        assert_eq!(blk.event_entry(9), Some(2));
        assert_eq!(blk.event_entry(404), None);
    }

    #[test]
    fn finds_zone_player_block() {
        let b0 = block_bytes(0x01, &[(0, 0)], &[], &[0x00]);
        let b1 = block_bytes(ZONE_PLAYER_ACTOR, &[(7, 0)], &[], &[0x01, 0x02]);
        let dat = EventDat::parse(&dat_bytes(&[b0, b1])).expect("parse");
        assert_eq!(dat.zone_block().map(|b| b.actor), Some(ZONE_PLAYER_ACTOR));
        assert_eq!(
            dat.block_for_actor(0x01).map(|b| b.event_data.len()),
            Some(1)
        );
        assert_eq!(dat.block_for_actor(0xDEAD), None);
    }

    #[test]
    fn truncated_header_errors() {
        assert!(matches!(
            EventDat::parse(&[0x01, 0x00]),
            Err(EventDatError::Truncated { .. })
        ));
    }

    #[test]
    fn block_overrun_errors() {
        // Header claims one block of 0x40 bytes, but no block bytes follow.
        let mut buf = Vec::new();
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&0x40u32.to_le_bytes());
        assert!(matches!(
            EventDat::parse(&buf),
            Err(EventDatError::BlockOverrun { index: 0, .. })
        ));
    }
}
