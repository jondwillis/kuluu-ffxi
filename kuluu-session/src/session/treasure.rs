//! Treasure-pool handling: s2c 0x0D2 TROPHY_LIST / 0x0D3 TROPHY_SOLUTION into
//! pool state plus the chat lines retail composes locally.
//!
//! The wording is not written here — it comes out of the client's
//! system-message table (`ffxi_dat::sysmes`), which is also what makes the item
//! name its own coloured span.

use ffxi_proto::decode::{TrophyEntryKind, TrophyJudge, TrophyList, TrophySolution};
use tokio::sync::broadcast;

use crate::state::{
    AgentEvent, ChatChannel, ChatLine, ChatSpan, ChatSpanKind, TreasureEntry, TreasurePoolSlot,
};

use ffxi_dat::sysmes::{self, SpanKind, SysMesDat, SysMesLine, SysMesParams};

/// Lazily opens the system-message table, once, and remembers a miss so a
/// missing install costs one attempt rather than one per drop.
pub(super) struct SysMesResolver {
    root: Option<std::sync::Arc<ffxi_dat::DatRoot>>,
    table: Option<Option<SysMesDat>>,
    items: Option<Option<ffxi_dat::item_dat::ItemTable>>,
}

impl SysMesResolver {
    pub(super) fn new(root: Option<std::sync::Arc<ffxi_dat::DatRoot>>) -> Self {
        Self {
            root,
            table: None,
            items: None,
        }
    }

    fn table(&mut self) -> Option<&SysMesDat> {
        let root = self.root.as_ref();
        self.table
            .get_or_insert_with(|| {
                let loaded = root.and_then(|r| SysMesDat::open(r));
                if loaded.is_none() {
                    tracing::info!(
                        "system-message DAT (ROM/27/76) unavailable — treasure chat lines degrade to plain text"
                    );
                }
                loaded
            })
            .as_ref()
    }

    /// Retail's chat-log item name ("fire crystal", not "Fire Crystal") from
    /// the item DAT, falling back to the display name without an install.
    fn log_name(&mut self, item_id: u16) -> String {
        let root = self.root.as_ref();
        self.items
            .get_or_insert_with(|| root.map(|r| ffxi_dat::item_dat::ItemTable::open(r.root())))
            .as_ref()
            .and_then(|t| t.lookup(item_id))
            .map(|i| i.log_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| item_name(item_id))
    }
}

/// The session task's own view of the pool. s2c 0x0D3 names only a slot index,
/// so the item it refers to has to be remembered from the 0x0D2 that filled it.
#[derive(Debug, Default)]
pub(super) struct TreasurePool {
    slots: [Option<TreasurePoolSlot>; ffxi_proto::decode::TREASURE_POOL_SIZE],
}

impl TreasurePool {
    fn get(&self, slot: u8) -> Option<&TreasurePoolSlot> {
        self.slots.get(slot as usize)?.as_ref()
    }

    fn item_id(&self, slot: u8) -> Option<u16> {
        self.get(slot).map(|s| s.item_id)
    }

    pub(super) fn clear(&mut self) {
        self.slots = Default::default();
    }
}

pub(super) fn handle_trophy_list(
    data: &[u8],
    event_tx: &broadcast::Sender<AgentEvent>,
    sysmes: &mut SysMesResolver,
    pool: &mut TreasurePool,
    name_cache: &std::collections::HashMap<u32, String>,
    self_char_name: &str,
) {
    let Ok(t) = TrophyList::decode(data)
        .inspect_err(|e| tracing::warn!(error = %e, "0x0D2 TROPHY_LIST decode failed"))
    else {
        return;
    };

    if t.gold != 0 {
        // Retail's found-gil line names the player receiving the split share.
        // LSB never populates Gold (0x0d2_trophy_list.cpp leaves it 0 with a
        // TODO) and announces gil through battle message 565 instead, so this
        // arm is retail-only today.
        let mut params = SysMesParams::default();
        params.strings[0] = Some(self_char_name);
        params.numbers[0] = t.gold as i64;
        emit(event_tx, sysmes, sysmes::treasure::OBTAINS_GIL, &params);
    }

    if t.is_gil_only() {
        return;
    }

    let item_name = item_name(t.item_no);
    let log_name = sysmes.log_name(t.item_no);
    let dropper = name_cache
        .get(&t.target_unique_no)
        .cloned()
        .unwrap_or_default();

    // LSB overloads Entry as an "old item" flag (0x0d2_trophy_list.cpp passes
    // isOldItem) where the client reads it as the local player's lot/pass
    // state. Both readings agree that a non-zero Entry is not a fresh drop, so
    // gate the announcement on it and the pool replay a party member gets on
    // zone-in stays silent.
    if t.entry == TrophyEntryKind::None {
        let mut params = SysMesParams::default();
        params.items[0] = Some(&log_name);
        let index = if t.is_container {
            params.strings[1] = Some(&dropper);
            sysmes::treasure::FIND_IN
        } else {
            params.target_name = Some(&dropper);
            params.target_article = !t.named;
            sysmes::treasure::FIND_ON
        };
        emit(event_tx, sysmes, index, &params);
    }

    let slot = TreasurePoolSlot {
        slot: t.slot,
        item_id: t.item_no,
        item_name,
        count: t.item_count,
        dropper,
        start_time: t.start_time,
        own_entry: entry_to_state(t.entry),
        own_lot: t.own_lot,
        winner: t.loot_act_name.clone(),
        winner_lot: t.loot_point,
    };
    if let Some(dest) = pool.slots.get_mut(t.slot as usize) {
        *dest = Some(slot.clone());
    }
    let _ = event_tx.send(AgentEvent::TreasurePoolUpdated {
        slot: Box::new(slot),
    });
}

pub(super) fn handle_trophy_solution(
    data: &[u8],
    event_tx: &broadcast::Sender<AgentEvent>,
    sysmes: &mut SysMesResolver,
    pool: &mut TreasurePool,
) {
    let Ok(t) = TrophySolution::decode(data)
        .inspect_err(|e| tracing::warn!(error = %e, "0x0D3 TROPHY_SOLUTION decode failed"))
    else {
        return;
    };
    let log_name = pool
        .item_id(t.slot)
        .map(|id| sysmes.log_name(id))
        .unwrap_or_default();

    let mut params = SysMesParams::default();
    params.items[0] = Some(&log_name);

    match t.judge {
        TrophyJudge::Pending => {
            announce_lot(&t, event_tx, sysmes, &log_name);
            if let Some(Some(s)) = pool.slots.get_mut(t.slot as usize) {
                s.winner = t.loot_name.clone();
                s.winner_lot = t.loot_point.max(0) as u16;
                if t.entry_lotted {
                    s.own_entry = TreasureEntry::Lotted;
                }
                let updated = s.clone();
                let _ = event_tx.send(AgentEvent::TreasurePoolUpdated {
                    slot: Box::new(updated),
                });
            }
            return;
        }
        TrophyJudge::Won => {
            announce_lot(&t, event_tx, sysmes, &log_name);
            // Under a Won verdict the client reads LootUniqueNo as a message
            // selector, not an entity id: 0 means the local player won
            // (research/XiPackets/world/server/0x00D3).
            if t.loot_unique_no == 0 {
                emit(event_tx, sysmes, sysmes::treasure::YOU_OBTAIN, &params);
            } else {
                params.strings[2] = t.loot_name.as_deref();
                emit(event_tx, sysmes, sysmes::treasure::OBTAINS_ITEM, &params);
            }
        }
        TrophyJudge::WinnerIneligible => {
            if t.loot_unique_no == 0 {
                emit(event_tx, sysmes, sysmes::treasure::YOU_INELIGIBLE, &params);
            } else {
                params.strings[2] = t.loot_name.as_deref();
                emit(
                    event_tx,
                    sysmes,
                    sysmes::treasure::OTHER_INELIGIBLE,
                    &params,
                );
            }
        }
        TrophyJudge::SilentClear => {}
    }

    if let Some(dest) = pool.slots.get_mut(t.slot as usize) {
        *dest = None;
    }
    let _ = event_tx.send(AgentEvent::TreasurePoolCleared { slot: t.slot });
}

/// Retail prints the roll a player cast; passes are silent. Under a Won verdict
/// EntryUniqueNo is a message selector too, with 0 meaning the local player.
fn announce_lot(
    t: &TrophySolution,
    event_tx: &broadcast::Sender<AgentEvent>,
    sysmes: &mut SysMesResolver,
    log_name: &str,
) {
    if !t.announces_lot() {
        return;
    }
    let mut params = SysMesParams::default();
    params.items[0] = Some(log_name);
    if t.judge == TrophyJudge::Won && t.entry_unique_no == 0 {
        emit(event_tx, sysmes, sysmes::treasure::YOU_CAST_LOTS, &params);
        return;
    }
    let points = t.entry_point.to_string();
    params.strings[3] = t.entry_name.as_deref();
    params.strings[2] = Some(&points);
    emit(event_tx, sysmes, sysmes::treasure::LOT, &params);
}

fn emit(
    event_tx: &broadcast::Sender<AgentEvent>,
    sysmes: &mut SysMesResolver,
    index: usize,
    params: &SysMesParams,
) {
    let Some(line) = sysmes.table().and_then(|d| d.message(index, params)) else {
        return;
    };
    for chat in chat_lines(&line) {
        let _ = event_tx.send(AgentEvent::ChatLine { line: chat });
    }
}

/// One retail log line per composed line, each keeping its span colouring.
pub(super) fn chat_lines(line: &SysMesLine) -> Vec<ChatLine> {
    line.lines
        .iter()
        .map(|spans| {
            ChatLine::spanned(
                ChatChannel::System,
                spans
                    .iter()
                    .map(|s| ChatSpan {
                        text: s.text.clone(),
                        kind: match s.kind {
                            SpanKind::Text => ChatSpanKind::Text,
                            SpanKind::Item => ChatSpanKind::Item,
                            SpanKind::KeyItem => ChatSpanKind::KeyItem,
                        },
                    })
                    .collect(),
            )
        })
        .collect()
}

fn item_name(item_id: u16) -> String {
    ffxi_vocab::item_names::lookup(item_id)
        .map(str::to_string)
        .unwrap_or_else(|| format!("item #{item_id}"))
}

fn entry_to_state(e: TrophyEntryKind) -> TreasureEntry {
    match e {
        TrophyEntryKind::None => TreasureEntry::None,
        TrophyEntryKind::Passed => TreasureEntry::Passed,
        TrophyEntryKind::Lotted => TreasureEntry::Lotted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_with(text: &str, item: &str) -> SysMesLine {
        SysMesLine {
            log_mode: Some(0x79),
            lines: vec![vec![
                sysmes::Span {
                    text: text.to_string(),
                    kind: SpanKind::Text,
                },
                sysmes::Span {
                    text: item.to_string(),
                    kind: SpanKind::Item,
                },
            ]],
        }
    }

    #[test]
    fn composed_spans_survive_into_the_chat_line() {
        let lines = chat_lines(&line_with("You find a ", "Lizard Tail"));
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert_eq!(l.text, "You find a Lizard Tail", "text is the span join");
        assert_eq!(l.spans.len(), 2);
        assert_eq!(l.spans[1].kind, ChatSpanKind::Item);
    }

    #[test]
    fn a_two_line_message_becomes_two_chat_lines() {
        let composed = SysMesLine {
            log_mode: Some(0x7b),
            lines: vec![
                vec![sysmes::Span {
                    text: "first".into(),
                    kind: SpanKind::Text,
                }],
                vec![sysmes::Span {
                    text: "second".into(),
                    kind: SpanKind::Text,
                }],
            ],
        };
        let lines = chat_lines(&composed);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[1].text, "second");
    }

    #[test]
    fn an_unknown_item_id_still_names_something() {
        /// The value coincides with the event-id wildcard const; the point is
        /// the display fallback for an id with no name.
        const UNKNOWN_ITEM_NO_PINNED: u16 = 0xFFFE;
        assert_eq!(
            item_name(UNKNOWN_ITEM_NO_PINNED),
            format!("item #{UNKNOWN_ITEM_NO_PINNED}")
        );
    }
}
