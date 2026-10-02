use super::*;

/// Drive `InputMode` in lock-step with the server-owned delivery box: enter
/// `DeliveryBox` once the box is open (and no dialog is mid-transition), leave
/// on PostClose ack. Resets the transient screen state on each edge. A
/// Receive/Send switch keeps the mode, so the focus reset keys off the box as
/// well: the panels do not draw the same regions.
pub fn delivery_mode_sync_system(
    state: Res<SceneState>,
    mut mode: ResMut<InputMode>,
    mut screen: ResMut<kuluu_render::hud::delivery::DeliveryScreenState>,
) {
    let box_no = state.snapshot.delivery_box.as_ref().map(|d| d.box_no);
    let open = box_no.is_some();
    match (&*mode, open) {
        (InputMode::DeliveryBox, false) => {
            *mode = InputMode::World;
            screen.close();
        }
        // Enter the modal delivery mode whenever the server has a box open,
        // regardless of any dialog still settling — the box owns the screen and
        // suppresses world movement. The dialog panel hides itself when the box
        // is open (see `update_dialog_panel_system`).
        (m, true) if !matches!(m, InputMode::DeliveryBox) => {
            *mode = InputMode::DeliveryBox;
            screen.open(box_no.expect("open"));
        }
        _ => {}
    }
    if let (InputMode::DeliveryBox, Some(box_no)) = (&*mode, box_no) {
        if screen.box_no != Some(box_no) {
            screen.open(box_no);
        }
    }
}

fn wire_to_client_box(
    box_no: kuluu_snapshot::DeliveryBoxNo,
) -> kuluu_session::state::DeliveryBoxNo {
    match box_no {
        kuluu_snapshot::DeliveryBoxNo::Incoming => kuluu_session::state::DeliveryBoxNo::Incoming,
        kuluu_snapshot::DeliveryBoxNo::Outgoing => kuluu_session::state::DeliveryBoxNo::Outgoing,
    }
}

const RECIPIENT_NAME_MAX: usize = 15;

/// Keyboard handling for the dedicated delivery screen. Reads the snapshot +
/// `DeliveryScreenState` + deliverable inventory, drives focus/spinner/recipient
/// entry, and emits delivery `AgentCommand`s. Mode transitions are owned by
/// `delivery_mode_sync_system`, so this never changes `InputMode` directly.
/// Moving the cursor disarms a pending dispatch confirmation so it can't be
/// answered by accident later. Dispatch is irreversible, so it takes a second
/// press: the first arms the confirmation, the second sends.
pub(super) fn handle_delivery_key(
    key: &Key,
    bindings: &Bindings,
    screen: &mut kuluu_render::hud::delivery::DeliveryScreenState,
    scene_state: &mut SceneState,
    inv: &kuluu_render::hud::delivery::DeliveryInventory,
    cmd_tx: &Sender<AgentCommand>,
) {
    use kuluu_render::hud::delivery::{self, DeliveryCtx, DeliveryFocus};
    use kuluu_snapshot::{DeliveryBoxNo as WireBox, RecipientStatus};

    let Some(d) = scene_state.snapshot.delivery_box.clone() else {
        return;
    };
    let gil = delivery::current_gil(&scene_state.snapshot);
    let outgoing = d.box_no == WireBox::Outgoing;
    let recipient_ok = matches!(d.recipient_status, RecipientStatus::Ok { .. });
    screen.reclamp(inv.rows.len());
    let ctx = DeliveryCtx {
        box_no: d.box_no,
        inv_len: inv.rows.len(),
        recipient_ok,
    };
    let sent = ffxi_proto::map::pbx::stat::SENT;
    let send = |op: kuluu_session::state::DeliveryBoxOp| {
        let _ = cmd_tx.try_send(AgentCommand::DeliveryBox { op });
    };
    let close = || {
        let _ = cmd_tx.try_send(AgentCommand::DeliveryBox {
            op: kuluu_session::state::DeliveryBoxOp::PostClose {
                box_no: wire_to_client_box(d.box_no),
            },
        });
    };

    // 1. Active quantity/gil spinner.
    if screen.selector.is_some() {
        if bindings.matches_logical(Action::NavConfirm, key) {
            let binding = screen.selector.take().expect("selector present");
            let qty = binding.spinner.confirm();
            let inv_slot = binding.target.inventory_slot();
            let out_slot = binding.target.out_slot();
            screen.leave_item_list();
            screen.focus = DeliveryFocus::Slot(out_slot as usize);
            if qty > 0 {
                send(kuluu_session::state::DeliveryBoxOp::Set {
                    slot: out_slot,
                    inventory_slot: inv_slot,
                    quantity: qty,
                    recipient: String::new(),
                });
            }
            return;
        }
        if bindings.matches_logical(Action::NavCancel, key) {
            screen.selector = None;
            return;
        }
        if let Some(b) = screen.selector.as_mut() {
            spinner_nav(&mut b.spinner, key, bindings);
        }
        return;
    }

    // 2. Recipient text entry.
    if screen.recipient_buf.is_some() {
        if bindings.matches_logical(Action::NavConfirm, key) {
            let name = screen
                .recipient_buf
                .take()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !name.is_empty() {
                send(kuluu_session::state::DeliveryBoxOp::Query { recipient: name });
            }
            return;
        }
        if bindings.matches_logical(Action::NavCancel, key) {
            screen.recipient_buf = None;
            return;
        }
        if let Some(buf) = screen.recipient_buf.as_mut() {
            if matches!(key, Key::Backspace) {
                buf.pop();
            } else if matches!(key, Key::Space) {
                if buf.len() < RECIPIENT_NAME_MAX {
                    buf.push(' ');
                }
            } else if let Key::Character(s) = key {
                for c in s.chars() {
                    if !c.is_control() && buf.len() < RECIPIENT_NAME_MAX {
                        buf.push(c);
                    }
                }
            }
        }
        return;
    }

    // 2b. Toggle Received/Send box (retail's in-window switch; same key as the
    // item-window bag tabs). Closes the current box and opens the other.
    if bindings.matches_logical(Action::SelectActiveWindow, key) {
        let _ = cmd_tx.try_send(AgentCommand::DeliveryBox {
            op: kuluu_session::state::DeliveryBoxOp::PostClose {
                box_no: wire_to_client_box(d.box_no),
            },
        });
        let other = if outgoing {
            kuluu_session::state::DeliveryBoxOp::PostOpen
        } else {
            kuluu_session::state::DeliveryBoxOp::DeliOpen
        };
        let _ = cmd_tx.try_send(AgentCommand::DeliveryBox { op: other });
        return;
    }

    if bindings.matches_logical(Action::NavUp, key) {
        screen.confirm_send = false;
        delivery::focus_up(screen, &ctx);
        return;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        screen.confirm_send = false;
        delivery::focus_down(screen, &ctx);
        return;
    }
    if bindings.matches_logical(Action::NavLeft, key) {
        screen.confirm_send = false;
        delivery::focus_left(screen, &ctx);
        return;
    }
    if bindings.matches_logical(Action::NavRight, key) {
        screen.confirm_send = false;
        delivery::focus_right(screen, &ctx);
        return;
    }
    // Esc unwinds one level: the item list to the slot it was entered from,
    // Take/Return to the grid, an armed dispatch to unarmed. From the panel
    // itself it closes the box, which lands back on Receive/Send
    // (.agents/skills/retail-observe/references/2026-07-17-moghouse-menu.md
    // "How the menu opens", "Send flow" step 7).
    if bindings.matches_logical(Action::NavCancel, key) {
        match screen.focus {
            DeliveryFocus::TakeBtn | DeliveryFocus::RejectBtn => screen.leave_parcel_actions(),
            DeliveryFocus::InvRow(_) => screen.leave_item_list(),
            _ if screen.confirm_send => screen.confirm_send = false,
            _ => close(),
        }
        return;
    }
    if !bindings.matches_logical(Action::NavConfirm, key) {
        return;
    }

    // NavConfirm dispatch by focused region.
    let notice = |scene: &mut SceneState, msg: &str| {
        push_system_chat_line(scene, format!("[delivery] {msg}"));
    };
    match screen.focus {
        DeliveryFocus::Recipient => {
            screen.recipient_buf = Some(d.recipient.clone().unwrap_or_default());
        }
        DeliveryFocus::Slot(i) if outgoing => match d.slots.get(i).and_then(|c| c.as_ref()) {
            None => {
                if !recipient_ok {
                    notice(scene_state, "Specify a recipient first.");
                } else if inv.rows.is_empty() {
                    notice(scene_state, "No deliverable items.");
                } else {
                    screen.enter_item_list(i, inv.rows.len());
                }
            }
            Some(item) if item.stat == sent => {
                send(kuluu_session::state::DeliveryBoxOp::Cancel { slot: i as u8 });
            }
            Some(_) => {
                send(kuluu_session::state::DeliveryBoxOp::Get {
                    box_no: kuluu_session::state::DeliveryBoxNo::Outgoing,
                    slot: i as u8,
                });
            }
        },
        DeliveryFocus::Slot(i) => {
            if d.slots.get(i).and_then(|c| c.as_ref()).is_some() {
                screen.last_in_slot = i;
                screen.focus = DeliveryFocus::TakeBtn;
            }
        }
        DeliveryFocus::Gil => {
            if !recipient_ok {
                notice(scene_state, "Specify a recipient first.");
            } else {
                match delivery::first_free_slot(&d) {
                    Some(free) => screen.selector = delivery::begin_gil_stage(gil, Some(free)),
                    None => notice(scene_state, "The delivery box is full."),
                }
            }
        }
        DeliveryFocus::InvRow(i) => {
            // The slot the list was entered from may have filled from a server
            // update while it was open, and LSB drops a Set into an occupied
            // cell with no packet back.
            let target = screen
                .pick_slot
                .filter(|slot| d.slots.get(*slot).is_some_and(Option::is_none))
                .or_else(|| delivery::first_free_slot(&d));
            if !recipient_ok {
                notice(scene_state, "Specify a recipient first.");
            } else if let Some(row) = inv.rows.get(i).cloned() {
                if !row.deliverable {
                    notice(scene_state, "That item cannot be delivered.");
                } else {
                    match target {
                        None => notice(scene_state, "The delivery box is full."),
                        Some(slot) if row.quantity <= 1 => {
                            screen.leave_item_list();
                            send(kuluu_session::state::DeliveryBoxOp::Set {
                                slot: slot as u8,
                                inventory_slot: row.inv_slot,
                                quantity: 1,
                                recipient: String::new(),
                            });
                        }
                        Some(slot) => {
                            screen.selector = delivery::begin_item_stage(&row, Some(slot));
                        }
                    }
                }
            }
        }
        DeliveryFocus::SendOk => {
            let staged: Vec<u8> = d
                .slots
                .iter()
                .enumerate()
                .filter_map(|(i, cell)| cell.as_ref().filter(|it| it.stat != sent).map(|_| i as u8))
                .collect();
            if !recipient_ok {
                notice(scene_state, "Specify a recipient first.");
            } else if staged.is_empty() {
                notice(scene_state, "Place an item in the delivery box first.");
            } else if !screen.confirm_send {
                screen.confirm_send = true;
            } else {
                screen.confirm_send = false;
                for slot in staged {
                    send(kuluu_session::state::DeliveryBoxOp::Send { slot });
                }
            }
        }
        DeliveryFocus::Exit => close(),
        DeliveryFocus::TakeBtn => {
            let _ = cmd_tx.try_send(AgentCommand::DeliveryTake {
                slot: screen.last_in_slot as u8,
            });
            screen.leave_parcel_actions();
        }
        DeliveryFocus::RejectBtn => {
            send(kuluu_session::state::DeliveryBoxOp::Reject {
                slot: screen.last_in_slot as u8,
            });
            screen.leave_parcel_actions();
        }
    }
}
