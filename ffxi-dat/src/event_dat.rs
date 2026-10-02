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
/// entity (zone-in cutscenes, menu flows).
///
/// XiEvents `Event DAT Structures` gives 0x7FFFFFFF here and is wrong: every
/// retail zone ships 0x7FFFFFF0, which is also the local-player value XiEvents'
/// own `Event VM Functions`:463-466 resolves. Byte-checked on ROM/21/39, /52
/// and /54 — each holds exactly one block whose actor matches the 0x7F pattern,
/// and in all three it is 0x7FFFFFF0 at block 0.
pub const ZONE_PLAYER_ACTOR: u32 = 0x7FFF_FFF0;

/// An event id the engine dispatches for *any* requested id. Rare (4 across the
/// three zone DATs surveyed) and clusters on the master block.
pub const EVENT_ID_WILDCARD: u16 = 0xFFFE;

/// A placeholder occupying an otherwise-real entry offset. It cannot be
/// requested by event id, so an exact match on it must not resolve; but the
/// entry is still a valid REQSET target, because ReqSet indexes TagOffset by
/// tag byte (research/XiEvents/Event VM Functions.md XiEvent::ReqSet).
pub const EVENT_ID_PLACEHOLDER: u16 = 0xFFFF;

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
        // The placeholder is not a requestable id, so it must not reach the
        // wildcard either — a catch-all is for ids that could have been real.
        if event_id == EVENT_ID_PLACEHOLDER {
            return None;
        }
        self.event_entry_exact(event_id)
            .or_else(|| self.event_entry_exact(EVENT_ID_WILDCARD))
    }

    /// [`Self::event_entry`] without the [`EVENT_ID_WILDCARD`] fallback, so a
    /// caller searching several blocks can prefer a real entry in a later block
    /// over an earlier block's catch-all.
    pub fn event_entry_exact(&self, event_id: u16) -> Option<usize> {
        if event_id == EVENT_ID_PLACEHOLDER {
            return None;
        }
        self.event_ids
            .iter()
            .position(|&e| e == event_id)
            .and_then(|i| self.event_offsets.get(i).map(|&o| o as usize))
    }
}

/// Which block [`EventDat::block_for_event`] resolved an event on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventBlockSource {
    /// The entity the server named — the ordinary case.
    OwnBlock,
    /// The zone/player master block.
    ZoneMasterBlock,
    /// A different entity's block, which was the only one holding the id.
    SoleOwnerElsewhere,
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

    /// The block to run `event_id` on for the entity the server named.
    ///
    /// Normally that is the entity's own block. Servers do send pairs the DAT
    /// disagrees with, though: talking to a conquest outpost guard yields the
    /// *controlling* nation's event id against the guard entity that was
    /// targeted, and the retail DAT files those two on different blocks (all
    /// three nations' guards stand on one spot). Auto-releasing there loses the
    /// interaction entirely, so fall back — but only when the answer is not a
    /// guess:
    ///
    /// 1. the entity's own block, exact id;
    /// 2. the zone master block, exact id;
    /// 3. the single NPC block that owns the id, if exactly one does;
    /// 4. a wildcard entry, own block before master.
    ///
    /// Step 3 is bounded deliberately: 19% of (zone, event id) pairs are owned
    /// by more than one NPC block — one id in Northern San d'Oria has 81 owners
    /// — so picking "the first block that has it" would silently run an
    /// arbitrary NPC's script. Exact matches are tried everywhere before any
    /// wildcard because a wildcard is a catch-all and must not beat real data.
    ///
    /// This is our recovery policy for a server/DAT mismatch, not established
    /// retail behaviour: no tier-1/2 source for the lookup order exists
    /// (XIClient carries no event VM), so `resolved_elsewhere` is reported to
    /// the caller to log rather than applied silently.
    pub fn block_for_event(
        &self,
        actor: u32,
        event_id: u16,
    ) -> Option<(&EventBlock, EventBlockSource)> {
        let own = self.block_for_actor(actor);
        if let Some(b) = own.filter(|b| b.event_entry_exact(event_id).is_some()) {
            return Some((b, EventBlockSource::OwnBlock));
        }
        if let Some(b) = self
            .zone_block()
            .filter(|b| b.event_entry_exact(event_id).is_some())
        {
            return Some((b, EventBlockSource::ZoneMasterBlock));
        }
        let mut owners = self
            .blocks
            .iter()
            .filter(|b| b.actor != ZONE_PLAYER_ACTOR && b.event_entry_exact(event_id).is_some());
        if let (Some(b), None) = (owners.next(), owners.next()) {
            return Some((b, EventBlockSource::SoleOwnerElsewhere));
        }
        own.filter(|b| b.event_entry(event_id).is_some())
            .map(|b| (b, EventBlockSource::OwnBlock))
            .or_else(|| {
                self.zone_block()
                    .filter(|b| b.event_entry(event_id).is_some())
                    .map(|b| (b, EventBlockSource::ZoneMasterBlock))
            })
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

    // The synthesised blocks above are written with our own constant, so they
    // pass under any value. Pin the literal separately: retail ships 0x7FFFFFF0
    // in every zone event DAT, and a block built with XiEvents' documented
    // 0x7FFFFFFF must not resolve as the master.
    #[test]
    fn zone_player_actor_is_the_retail_literal() {
        const ZONE_PLAYER_ACTOR_PINNED: u32 = 0x7FFF_FFF0;
        assert_eq!(ZONE_PLAYER_ACTOR, ZONE_PLAYER_ACTOR_PINNED);
        let dat = EventDat::parse(&dat_bytes(&[block_bytes(
            0x7FFF_FFFF,
            &[(1, 0)],
            &[],
            &[0x00],
        )]))
        .expect("parse");
        assert_eq!(dat.zone_block(), None);
    }

    // Repro of the conquest outpost guard: the server names the guard entity it
    // targeted but the id of the *controlling* nation's guard, and the retail DAT
    // files those on different blocks. Zone 109 has 32763 only on Mesachedeau
    // (0x0106D286) while the trigger arrives on Souun (0x0106D287).
    #[test]
    fn event_resolves_on_the_sole_owner_when_the_named_entity_lacks_it() {
        let dat = EventDat::parse(&dat_bytes(&[
            block_bytes(ZONE_PLAYER_ACTOR, &[(EVENT_ID_WILDCARD, 0)], &[], &[0]),
            block_bytes(0x0106_D286, &[(32763, 0)], &[], &[0]),
            block_bytes(0x0106_D287, &[(32761, 0)], &[], &[0]),
        ]))
        .expect("parse");

        // The named entity owns it -> its own block, never anyone else's.
        let (b, src) = dat.block_for_event(0x0106_D287, 32761).expect("own");
        assert_eq!(b.actor, 0x0106_D287);
        assert_eq!(src, EventBlockSource::OwnBlock);

        // It does not -> the one block that does, and NOT the master wildcard,
        // which would otherwise swallow it and play nothing.
        let (b, src) = dat.block_for_event(0x0106_D287, 32763).expect("elsewhere");
        assert_eq!(b.actor, 0x0106_D286);
        assert_eq!(src, EventBlockSource::SoleOwnerElsewhere);
    }

    // Zone 109's event DAT (ROM/20/46.DAT) scripts the outpost vendor event
    // only on Tahmasp (vendor/server/sql/npc_list.sql row 17224336; the LSB
    // vendor lua vendor/server/scripts/zones/Pashhow_Marshlands/npcs/Tahmasp.lua
    // vendorEvent) on both the horizonxi-2023 and retail-2026-09 KNOWN_CLIENTS
    // rows -- there is no block for PASHHOW_CONQUEST_BANNER, LSB's unique_no
    // for that entity. A HorizonXI
    // server (an LSB fork with a renumbered npc_list) was observed sending
    // OUTPOST_VENDOR_EVENT with unique_no PASHHOW_CONQUEST_BANNER instead, which
    // the DAT does not script. A caller that gates on block_for_actor before
    // this ladder loses the whole interaction.
    const PASHHOW_TAHMASP: u32 = 0x0106_D290;
    const PASHHOW_CONQUEST_BANNER: u32 = 0x0106_D291;
    const OUTPOST_VENDOR_EVENT: u16 = 32756;

    #[test]
    fn an_actor_with_no_block_of_its_own_still_resolves() {
        let dat = EventDat::parse(&dat_bytes(&[
            block_bytes(
                ZONE_PLAYER_ACTOR,
                &[(26, 0), (EVENT_ID_WILDCARD, 0)],
                &[],
                &[0],
            ),
            block_bytes(PASHHOW_TAHMASP, &[(OUTPOST_VENDOR_EVENT, 0)], &[], &[0]),
        ]))
        .expect("parse");
        assert_eq!(
            dat.block_for_actor(PASHHOW_CONQUEST_BANNER),
            None,
            "premise"
        );

        let (b, src) = dat
            .block_for_event(PASHHOW_CONQUEST_BANNER, OUTPOST_VENDOR_EVENT)
            .expect("sole owner");
        assert_eq!(b.actor, PASHHOW_TAHMASP);
        assert_eq!(src, EventBlockSource::SoleOwnerElsewhere);

        let (b, src) = dat
            .block_for_event(PASHHOW_CONQUEST_BANNER, 26)
            .expect("master");
        assert_eq!(b.actor, ZONE_PLAYER_ACTOR);
        assert_eq!(src, EventBlockSource::ZoneMasterBlock);
    }

    // 19% of (zone, event id) pairs are held by more than one NPC block, so the
    // sole-owner rung must refuse to pick when ownership is ambiguous.
    #[test]
    fn ambiguous_event_id_never_resolves_to_an_arbitrary_block() {
        let dat = EventDat::parse(&dat_bytes(&[
            block_bytes(0x0100_0001, &[(43, 0)], &[], &[0]),
            block_bytes(0x0100_0002, &[(43, 0)], &[], &[0]),
            block_bytes(0x0100_0003, &[(7, 0)], &[], &[0]),
        ]))
        .expect("parse");
        assert!(dat.block_for_event(0x0100_0003, 43).is_none());
    }

    // A real entry anywhere beats a catch-all: the master block's wildcard must
    // not shadow the block that actually owns the id.
    #[test]
    fn exact_match_elsewhere_outranks_the_master_wildcard() {
        let dat = EventDat::parse(&dat_bytes(&[
            block_bytes(ZONE_PLAYER_ACTOR, &[(EVENT_ID_WILDCARD, 0)], &[], &[0]),
            block_bytes(0x0100_0009, &[(500, 0)], &[], &[0]),
            block_bytes(0x0100_000A, &[(1, 0)], &[], &[0]),
        ]))
        .expect("parse");
        let (b, src) = dat.block_for_event(0x0100_000A, 500).expect("resolved");
        assert_eq!(b.actor, 0x0100_0009);
        assert_eq!(src, EventBlockSource::SoleOwnerElsewhere);

        // With no exact owner at all, the wildcard is still the last resort.
        let (_, src) = dat.block_for_event(0x0100_000A, 4242).expect("wildcard");
        assert_eq!(src, EventBlockSource::ZoneMasterBlock);
    }

    #[test]
    fn wildcard_entry_matches_any_id_and_placeholder_matches_none() {
        let dat = EventDat::parse(&dat_bytes(&[block_bytes(
            ZONE_PLAYER_ACTOR,
            &[(7, 0), (EVENT_ID_PLACEHOLDER, 2), (EVENT_ID_WILDCARD, 4)],
            &[],
            &[0, 1, 2, 3, 4, 5],
        )]))
        .expect("parse");
        let blk = dat.zone_block().expect("master block");

        assert_eq!(blk.event_entry(7), Some(0), "exact match wins");
        assert_eq!(
            blk.event_entry(999),
            Some(4),
            "unknown id falls to wildcard"
        );
        assert_eq!(
            blk.event_entry(EVENT_ID_PLACEHOLDER),
            None,
            "a placeholder cannot be requested by id"
        );

        // Without a wildcard present, an unknown id still resolves to nothing.
        let bare =
            EventDat::parse(&dat_bytes(&[block_bytes(0x01, &[(7, 0)], &[], &[0])])).expect("parse");
        assert_eq!(bare.blocks[0].event_entry(999), None);
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
