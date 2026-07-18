//! Client-side delivery box driver. Mirrors the retail client's sequencing on
//! top of the c2s 0x04D / s2c 0x04B sub-protocol (vendor/server/src/map/
//! packets/c2s/0x04d_pbx.cpp, utils/dboxutils.cpp): opening a box triggers
//! Work (list all 8 slots) then Check; a positive Check auto-drains the queue
//! (Recv into free inbox slots / Confirm delivered outbox items); taking an
//! item chains Accept → Get. The session owns the sends — this machine only
//! turns decoded [`PbxResult`]s into follow-up ops and state updates.

use ffxi_proto::decode::PbxResult;
use ffxi_proto::map::pbx;

use crate::state::{DeliveryBoxNo, DeliveryBoxOp, DeliveryBoxUpdate, DeliveryItem};

/// What a [`PbxResult`] asks of the session: `sends` go out as 0x04D requests,
/// `updates` fan out as [`AgentEvent::DeliveryBoxUpdated`], `notices` as system
/// chat lines. `settled` marks the end of an auto-sequence (menu-driven flows
/// re-render the box menu then).
///
/// [`AgentEvent::DeliveryBoxUpdated`]: crate::state::AgentEvent::DeliveryBoxUpdated
#[derive(Debug, Default, PartialEq)]
pub struct PbxOutcome {
    pub sends: Vec<DeliveryBoxOp>,
    pub updates: Vec<(DeliveryBoxNo, DeliveryBoxUpdate)>,
    pub notices: Vec<String>,
    pub settled: bool,
}

#[derive(Debug, Default)]
pub struct DeliveryBoxSession {
    open: Option<DeliveryBoxNo>,
    menu_driven: bool,
    slots: [Option<DeliveryItem>; pbx::SLOT_COUNT],
    /// Inbox slot awaiting the Accept → Get chain.
    pending_take: Option<u8>,
}

impl DeliveryBoxSession {
    /// The open request for `box_no`. `menu_driven` flags flows started from
    /// the Mog Menu, which re-render a dialog menu on [`PbxOutcome::settled`].
    pub fn request_open(&mut self, box_no: DeliveryBoxNo, menu_driven: bool) -> DeliveryBoxOp {
        self.menu_driven = menu_driven;
        self.pending_take = None;
        match box_no {
            DeliveryBoxNo::Incoming => DeliveryBoxOp::PostOpen,
            DeliveryBoxNo::Outgoing => DeliveryBoxOp::DeliOpen,
        }
    }

    /// Retail's take: Accept first, Get on its ack (dboxutils.cpp
    /// UpdateDeliveryCellBeforeRemoving → TakeItemFromCell).
    pub fn request_take(&mut self, slot: u8) -> DeliveryBoxOp {
        self.pending_take = Some(slot);
        DeliveryBoxOp::Accept { slot }
    }

    pub fn open(&self) -> Option<DeliveryBoxNo> {
        self.open
    }

    pub fn menu_driven(&self) -> bool {
        self.menu_driven
    }

    pub fn slots(&self) -> &[Option<DeliveryItem>; pbx::SLOT_COUNT] {
        &self.slots
    }

    pub fn on_result(&mut self, r: &PbxResult) -> PbxOutcome {
        let mut out = PbxOutcome::default();
        let Some(box_no) = DeliveryBoxNo::from_wire(r.box_no).or(self.open) else {
            return out;
        };

        // Most commands answer twice (Result=PENDING then OK/error); the
        // PENDING push carries no state change worth acting on.
        if r.result == pbx::result::PENDING {
            return out;
        }

        if r.result != pbx::result::OK {
            self.on_error(r, box_no, &mut out);
            return out;
        }

        match r.command {
            pbx::command::DELI_OPEN | pbx::command::POST_OPEN => {
                self.open = Some(box_no);
                self.slots = Default::default();
                out.updates.push((box_no, DeliveryBoxUpdate::Opened));
                out.sends.push(DeliveryBoxOp::Work { box_no });
            }
            pbx::command::WORK => {
                let slot = r.post_work_no.max(0) as u8;
                self.set_slot(box_no, slot, item_of(r), &mut out);
                // The server answers Work with exactly one push per slot, in
                // order (dboxutils.cpp:105-108); the last slot ends the batch,
                // so a repeated Work still triggers exactly one Check.
                if slot as usize == pbx::SLOT_COUNT - 1 {
                    out.sends.push(DeliveryBoxOp::Check { box_no });
                }
            }
            pbx::command::CHECK => {
                // Count lands in ResParam2 (Incoming) / ResParam3 (Outgoing)
                // (0x04b_pbx_result.cpp:44-54).
                let count = match box_no {
                    DeliveryBoxNo::Incoming => r.res_param2.max(0) as u8,
                    DeliveryBoxNo::Outgoing => r.res_param3.max(0) as u8,
                };
                out.updates
                    .push((box_no, DeliveryBoxUpdate::PendingCount { count }));
                if count == 0 {
                    out.settled = true;
                } else {
                    match box_no {
                        DeliveryBoxNo::Incoming => match self.first_free_slot() {
                            Some(slot) => out.sends.push(DeliveryBoxOp::Recv { slot }),
                            None => {
                                out.notices.push(format!(
                                    "{count} more item(s) are waiting — free a delivery box \
                                     slot to receive them."
                                ));
                                out.settled = true;
                            }
                        },
                        DeliveryBoxNo::Outgoing => out.sends.push(DeliveryBoxOp::Confirm),
                    }
                }
            }
            pbx::command::RECV => {
                let slot = r.post_work_no.max(0) as u8;
                self.set_slot(box_no, slot, item_of(r), &mut out);
                out.sends.push(DeliveryBoxOp::Check { box_no });
            }
            pbx::command::CONFIRM => {
                let slot = r.post_work_no.max(0) as u8;
                if let Some(item) = item_of(r) {
                    out.notices.push(format!(
                        "The {} you sent to {} was delivered.",
                        item_name(item.item_no),
                        item.counterpart.as_deref().unwrap_or("someone"),
                    ));
                }
                self.set_slot(box_no, slot, None, &mut out);
                out.sends.push(DeliveryBoxOp::Check { box_no });
            }
            pbx::command::ACCEPT => match self.pending_take.take() {
                Some(slot) => out.sends.push(DeliveryBoxOp::Get { box_no, slot }),
                None => out.settled = true,
            },
            pbx::command::GET => {
                let slot = r.post_work_no.max(0) as u8;
                if let Some(item) = item_of(r).or_else(|| self.slots[slot as usize].clone()) {
                    out.notices
                        .push(format!("You take the {}.", item_name(item.item_no)));
                }
                self.set_slot(box_no, slot, None, &mut out);
                out.settled = true;
            }
            pbx::command::REJECT => {
                let slot = r.post_work_no.max(0) as u8;
                if let Some(item) = item_of(r).or_else(|| self.slots[slot as usize].clone()) {
                    out.notices.push(format!(
                        "The {} was returned to {}.",
                        item_name(item.item_no),
                        item.counterpart.as_deref().unwrap_or("its sender"),
                    ));
                }
                self.set_slot(box_no, slot, None, &mut out);
                out.settled = true;
            }
            pbx::command::CLEAR => {
                let slot = r.post_work_no.max(0) as u8;
                self.set_slot(box_no, slot, None, &mut out);
                out.settled = true;
            }
            pbx::command::SET | pbx::command::SEND | pbx::command::CANCEL => {
                let slot = r.post_work_no.max(0) as u8;
                self.set_slot(box_no, slot, item_of(r), &mut out);
                out.settled = true;
            }
            pbx::command::QUERY => {
                // An OK Query means the name resolved; a nonexistent name
                // answers Result 0xFB instead. ResParam1 is 1 only for a
                // recipient on the sender's own account (dboxutils.cpp
                // ConfirmNameBeforeSending) — it is NOT an existence flag.
                out.updates.push((
                    box_no,
                    DeliveryBoxUpdate::RecipientCheck {
                        ok: true,
                        same_account: r.res_param1 == 1,
                    },
                ));
                out.settled = true;
            }
            pbx::command::POST_CLOSE => {
                self.open = None;
                self.slots = Default::default();
                self.pending_take = None;
                out.updates.push((box_no, DeliveryBoxUpdate::Closed));
                out.settled = true;
            }
            _ => {}
        }
        out
    }

    fn on_error(&mut self, r: &PbxResult, box_no: DeliveryBoxNo, out: &mut PbxOutcome) {
        self.pending_take = None;
        if r.command == pbx::command::QUERY {
            out.updates.push((
                box_no,
                DeliveryBoxUpdate::RecipientCheck {
                    ok: false,
                    same_account: false,
                },
            ));
        }
        out.updates.push((
            box_no,
            DeliveryBoxUpdate::Failed {
                command: r.command,
                result: r.result,
            },
        ));
        let text = match r.result {
            pbx::result::INVENTORY_FULL => "Your inventory is full.".to_string(),
            pbx::result::NO_SUCH_CHAR => "That character does not exist.".to_string(),
            pbx::result::RECIPIENT_FULL => {
                "The recipient's delivery box is at capacity.".to_string()
            }
            pbx::result::BACKLOGGED => "Delivery orders are currently backlogged.".to_string(),
            code => format!(
                "Delivery box request failed (command 0x{:02X}, result 0x{code:02X}).",
                r.command
            ),
        };
        out.notices.push(text);
        out.settled = true;
    }

    fn set_slot(
        &mut self,
        box_no: DeliveryBoxNo,
        slot: u8,
        item: Option<DeliveryItem>,
        out: &mut PbxOutcome,
    ) {
        let Some(cell) = self.slots.get_mut(slot as usize) else {
            return;
        };
        if *cell != item {
            *cell = item.clone();
            out.updates
                .push((box_no, DeliveryBoxUpdate::SlotChanged { slot, item }));
        }
    }

    fn first_free_slot(&self) -> Option<u8> {
        self.slots.iter().position(|s| s.is_none()).map(|i| i as u8)
    }
}

fn item_of(r: &PbxResult) -> Option<DeliveryItem> {
    let s = r.state.as_ref()?;
    (s.item_no != 0).then(|| DeliveryItem {
        item_no: s.item_no,
        quantity: s.stack,
        counterpart: s.counterpart.clone(),
        stat: s.stat,
    })
}

pub fn item_name(item_no: u16) -> String {
    ffxi_proto::item_names::lookup(item_no)
        .map(str::to_string)
        .unwrap_or_else(|| format!("item #{item_no}"))
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use ffxi_proto::decode::PbxBoxState;

    fn result(command: u8, box_no: DeliveryBoxNo, result: u8) -> PbxResult {
        PbxResult {
            command,
            box_no: box_no.wire(),
            post_work_no: -1,
            item_work_no: -1,
            item_stacks: -1,
            result,
            res_param1: -1,
            res_param2: -1,
            res_param3: -1,
            state: None,
        }
    }

    fn with_item(mut r: PbxResult, slot: u8, item_no: u16, stack: u32, from: &str) -> PbxResult {
        r.post_work_no = slot as i8;
        r.state = Some(PbxBoxState {
            stat: pbx::stat::INCOMING,
            counterpart: Some(from.to_string()),
            item_sub_no: 0,
            item_no,
            kind: 0,
            stack,
            extra: [0; 28],
        });
        r
    }

    /// Open → Work — the retail sequence LSB expects (dboxutils.cpp
    /// OpenRecvBox then SendOldItems while the box is open).
    #[test]
    fn open_lists_then_checks() {
        let mut s = DeliveryBoxSession::default();
        assert_eq!(
            s.request_open(DeliveryBoxNo::Incoming, true),
            DeliveryBoxOp::PostOpen
        );

        let out = s.on_result(&result(
            pbx::command::POST_OPEN,
            DeliveryBoxNo::Incoming,
            pbx::result::OK,
        ));
        assert_eq!(
            out.sends,
            vec![DeliveryBoxOp::Work {
                box_no: DeliveryBoxNo::Incoming
            }]
        );
        assert_eq!(s.open(), Some(DeliveryBoxNo::Incoming));

        // 8 Work replies (one per slot, LSB pushes all 8); the last triggers Check.
        for slot in 0..8u8 {
            let mut r = result(pbx::command::WORK, DeliveryBoxNo::Incoming, pbx::result::OK);
            r.post_work_no = slot as i8;
            let r = if slot == 2 {
                with_item(r, slot, 5075, 1, "Atti")
            } else {
                r
            };
            let out = s.on_result(&r);
            if slot < 7 {
                assert!(out.sends.is_empty(), "no Check before slot 7");
            } else {
                assert_eq!(
                    out.sends,
                    vec![DeliveryBoxOp::Check {
                        box_no: DeliveryBoxNo::Incoming
                    }]
                );
            }
        }
        assert_eq!(s.slots()[2].as_ref().map(|i| i.item_no), Some(5075));
    }

    /// A positive inbox Check drains the queue: Recv into the first free slot,
    /// then re-Check until it reports zero.
    #[test]
    fn check_recv_loop_until_empty() {
        let mut s = DeliveryBoxSession::default();
        s.open = Some(DeliveryBoxNo::Incoming);

        let mut check = result(
            pbx::command::CHECK,
            DeliveryBoxNo::Incoming,
            pbx::result::OK,
        );
        check.res_param2 = 1;
        let out = s.on_result(&check);
        assert_eq!(out.sends, vec![DeliveryBoxOp::Recv { slot: 0 }]);
        assert!(!out.settled);

        let recv = with_item(
            result(pbx::command::RECV, DeliveryBoxNo::Incoming, pbx::result::OK),
            0,
            4869,
            2,
            "Atti",
        );
        let out = s.on_result(&recv);
        assert_eq!(
            out.sends,
            vec![DeliveryBoxOp::Check {
                box_no: DeliveryBoxNo::Incoming
            }]
        );
        assert_eq!(s.slots()[0].as_ref().map(|i| i.quantity), Some(2));

        let mut done = result(
            pbx::command::CHECK,
            DeliveryBoxNo::Incoming,
            pbx::result::OK,
        );
        done.res_param2 = 0;
        let out = s.on_result(&done);
        assert!(out.sends.is_empty());
        assert!(out.settled, "zero Check settles the flow");
    }

    /// Take chains Accept → Get; Get success clears the slot.
    #[test]
    fn take_chains_accept_then_get() {
        let mut s = DeliveryBoxSession::default();
        s.open = Some(DeliveryBoxNo::Incoming);
        s.slots[3] = Some(DeliveryItem {
            item_no: 4869,
            quantity: 1,
            counterpart: Some("Atti".into()),
            stat: pbx::stat::INCOMING,
        });

        assert_eq!(s.request_take(3), DeliveryBoxOp::Accept { slot: 3 });

        let mut accept = result(
            pbx::command::ACCEPT,
            DeliveryBoxNo::Incoming,
            pbx::result::OK,
        );
        accept.post_work_no = 3;
        let out = s.on_result(&accept);
        assert_eq!(
            out.sends,
            vec![DeliveryBoxOp::Get {
                box_no: DeliveryBoxNo::Incoming,
                slot: 3
            }]
        );

        let mut get = result(pbx::command::GET, DeliveryBoxNo::Incoming, pbx::result::OK);
        get.post_work_no = 3;
        let out = s.on_result(&get);
        assert!(s.slots()[3].is_none(), "Get success empties the slot");
        assert!(out.settled);
        assert!(out.notices.iter().any(|n| n.contains("You take")));
    }

    /// Inventory-full Get (0xB9) keeps the slot and surfaces the retail error.
    #[test]
    fn get_inventory_full_keeps_slot() {
        let mut s = DeliveryBoxSession::default();
        s.open = Some(DeliveryBoxNo::Incoming);
        s.slots[0] = Some(DeliveryItem {
            item_no: 4869,
            quantity: 1,
            counterpart: None,
            stat: pbx::stat::INCOMING,
        });

        let mut get = result(
            pbx::command::GET,
            DeliveryBoxNo::Incoming,
            pbx::result::INVENTORY_FULL,
        );
        get.post_work_no = 0;
        let out = s.on_result(&get);
        assert!(s.slots()[0].is_some(), "error keeps the item in the box");
        assert!(out.notices.iter().any(|n| n.contains("inventory is full")));
        assert!(out.settled);
    }

    /// A positive outbox Check auto-Confirms delivered items (retail clears
    /// them with the "was delivered" message).
    #[test]
    fn outbox_check_confirms_delivered() {
        let mut s = DeliveryBoxSession::default();
        s.open = Some(DeliveryBoxNo::Outgoing);
        s.slots[1] = Some(DeliveryItem {
            item_no: 4869,
            quantity: 1,
            counterpart: Some("Atti".into()),
            stat: pbx::stat::SENT,
        });

        let mut check = result(
            pbx::command::CHECK,
            DeliveryBoxNo::Outgoing,
            pbx::result::OK,
        );
        check.res_param3 = 1;
        let out = s.on_result(&check);
        assert_eq!(out.sends, vec![DeliveryBoxOp::Confirm]);

        let mut confirm = with_item(
            result(
                pbx::command::CONFIRM,
                DeliveryBoxNo::Outgoing,
                pbx::result::OK,
            ),
            1,
            4869,
            1,
            "Atti",
        );
        confirm.state.as_mut().unwrap().stat = pbx::stat::SENT;
        let out = s.on_result(&confirm);
        assert!(s.slots()[1].is_none(), "delivered item leaves the outbox");
        assert!(out.notices.iter().any(|n| n.contains("was delivered")));
        assert_eq!(
            out.sends,
            vec![DeliveryBoxOp::Check {
                box_no: DeliveryBoxNo::Outgoing
            }]
        );
    }

    /// Pins LSB's Query contract (dboxutils.cpp ConfirmNameBeforeSending): an
    /// OK result means the name resolved — ResParam1 is a same-account flag,
    /// not existence — and only Result 0xFB means "no such character".
    #[test]
    fn query_ok_means_name_exists_regardless_of_res_param1() {
        let mut s = DeliveryBoxSession::default();
        s.open = Some(DeliveryBoxNo::Outgoing);

        // Cross-account recipient: OK with ResParam1 = 0.
        let mut q = result(
            pbx::command::QUERY,
            DeliveryBoxNo::Outgoing,
            pbx::result::OK,
        );
        q.res_param1 = 0;
        let out = s.on_result(&q);
        assert!(out.updates.iter().any(|(_, u)| matches!(
            u,
            DeliveryBoxUpdate::RecipientCheck {
                ok: true,
                same_account: false
            }
        )));
        assert!(out.notices.is_empty(), "existing recipient gets no error");

        // Nonexistent name: 0xFB.
        let out = s.on_result(&result(
            pbx::command::QUERY,
            DeliveryBoxNo::Outgoing,
            pbx::result::NO_SUCH_CHAR,
        ));
        assert!(out.updates.iter().any(|(_, u)| matches!(
            u,
            DeliveryBoxUpdate::RecipientCheck {
                ok: false,
                same_account: false
            }
        )));
        assert!(out.notices.iter().any(|n| n.contains("does not exist")));
    }

    /// A second Work batch (e.g. an agent re-listing) must trigger exactly one
    /// Check again — the trigger is the final slot, not a lifetime counter.
    #[test]
    fn repeated_work_batches_each_trigger_one_check() {
        let mut s = DeliveryBoxSession::default();
        s.open = Some(DeliveryBoxNo::Incoming);
        for batch in 0..2 {
            let mut checks = 0;
            for slot in 0..8u8 {
                let mut r = result(pbx::command::WORK, DeliveryBoxNo::Incoming, pbx::result::OK);
                r.post_work_no = slot as i8;
                checks += s
                    .on_result(&r)
                    .sends
                    .iter()
                    .filter(|op| matches!(op, DeliveryBoxOp::Check { .. }))
                    .count();
            }
            assert_eq!(checks, 1, "batch {batch}");
        }
    }

    /// The PENDING half of the dual push must not double-drive the sequence.
    #[test]
    fn pending_results_are_inert() {
        let mut s = DeliveryBoxSession::default();
        s.open = Some(DeliveryBoxNo::Incoming);
        let out = s.on_result(&result(
            pbx::command::CHECK,
            DeliveryBoxNo::Incoming,
            pbx::result::PENDING,
        ));
        assert_eq!(out, PbxOutcome::default());
    }

    #[test]
    fn post_close_resets() {
        let mut s = DeliveryBoxSession::default();
        s.open = Some(DeliveryBoxNo::Incoming);
        s.slots[0] = Some(DeliveryItem {
            item_no: 1,
            quantity: 1,
            counterpart: None,
            stat: pbx::stat::INCOMING,
        });
        let out = s.on_result(&result(
            pbx::command::POST_CLOSE,
            DeliveryBoxNo::Incoming,
            pbx::result::OK,
        ));
        assert_eq!(s.open(), None);
        assert!(s.slots().iter().all(Option::is_none));
        assert!(out
            .updates
            .iter()
            .any(|(_, u)| matches!(u, DeliveryBoxUpdate::Closed)));
    }
}
