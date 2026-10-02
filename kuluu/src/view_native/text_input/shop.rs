use super::*;

use kuluu_render::hud::digit_spinner::DigitSpinner;
use kuluu_render::hud::shop::{
    self, ShopFocus, ShopMode, ShopRegion, ShopRow, ShopRowActivated, ShopScreenState, SHOP_NO,
    VENDOR_RANGE_YALMS,
};

/// Keeps the shop window in step with the world around it. The shop has no
/// close packet (research/XiPackets client 0x0082 is deprecated and GM-only),
/// so every exit is decided here:
/// - the window opens when the stock arrives and closes when it goes away;
/// - a zone change takes the vendor with it (the retail shop table lives in
///   GC_ZONE — research/XIClient GC_ZONE::gcShop), and a zone change is not an
///   `AppPhase` transition, so no state-exit cleanup fires for us (kuluu-oe8y);
/// - walking out of the vendor's trigger range ends the conversation, at the
///   same distance LSB requires to start one
///   (vendor/server/src/map/packets/c2s/0x01a_action.cpp Trigger).
///
/// A shop that opens while another window owns the cursor waits behind it;
/// the window takes focus only from the world.
pub fn shop_mode_sync_system(
    state: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mut mode: ResMut<InputMode>,
    mut screen: ResMut<ShopScreenState>,
    mut last_zone: Local<Option<u16>>,
) {
    let zone = state.snapshot.zone_id;
    let zoned = zone.is_some() && last_zone.is_some() && *last_zone != zone;
    if zone.is_some() {
        *last_zone = zone;
    }

    let Some(shop) = state.snapshot.shop.as_ref() else {
        if screen.dismissed || matches!(*mode, InputMode::Shop) {
            screen.reset();
        }
        if matches!(*mode, InputMode::Shop) {
            *mode = InputMode::World;
        }
        return;
    };

    if screen.dismissed {
        if matches!(*mode, InputMode::Shop) {
            *mode = InputMode::World;
        }
        return;
    }

    if zoned {
        close_shop(&cmd_tx, &mut mode, &mut screen);
        return;
    }

    if !matches!(*mode, InputMode::Shop) {
        if matches!(*mode, InputMode::World) {
            screen.reset();
            *mode = InputMode::Shop;
        }
        return;
    }

    if vendor_out_of_range(&state.snapshot, shop.vendor_id) {
        close_shop(&cmd_tx, &mut mode, &mut screen);
        return;
    }

    let len = shop::rows_for(screen.mode, &state.snapshot).len();
    screen.clamp(len);
}

fn close_shop(cmd_tx: &CommandTx, mode: &mut InputMode, screen: &mut ShopScreenState) {
    let _ = cmd_tx.0.try_send(AgentCommand::CloseShop);
    screen.dismiss();
    *mode = InputMode::World;
}

/// Whether the vendor has left the player's reach. A shop with no resolved
/// vendor (`vendor_id == 0`, e.g. one opened by a server-driven menu rather
/// than a Talk) has no anchor to measure against and stays open; a vendor
/// that despawned or fell out of the entity table counts as out of reach.
fn vendor_out_of_range(snap: &kuluu_snapshot::SceneSnapshot, vendor_id: u32) -> bool {
    if vendor_id == 0 {
        return false;
    }
    match snap.entities.iter().find(|e| e.id == vendor_id) {
        Some(vendor) => out_of_reach(snap.self_pos.pos, vendor.pos),
        None => true,
    }
}

fn out_of_reach(me: kuluu_snapshot::Vec3, vendor: kuluu_snapshot::Vec3) -> bool {
    let (dx, dy, dz) = (vendor.x - me.x, vendor.y - me.y, vendor.z - me.z);
    (dx * dx + dy * dy + dz * dz).sqrt() > VENDOR_RANGE_YALMS
}

/// A clicked or tapped row answers exactly as Enter would have on that row:
/// [`shop_mouse_hover_system`](kuluu_render::hud::shop::shop_mouse_hover_system)
/// has already put the cursor there.
pub fn shop_mouse_activate_system(
    mut activated: MessageReader<ShopRowActivated>,
    mut screen: ResMut<ShopScreenState>,
    mut scene_state: ResMut<SceneState>,
    cmd_tx: Res<CommandTx>,
) {
    for event in activated.read() {
        if scene_state.snapshot.shop.is_none() || screen.dismissed {
            continue;
        }
        let rows = shop::rows_for(screen.mode, &scene_state.snapshot);
        match (event.region, screen.focus) {
            (ShopRegion::Menu, ShopFocus::Menu) => screen.enter_list(),
            (ShopRegion::List, ShopFocus::List) => {
                activate_row(&mut screen, &mut scene_state, &cmd_tx.0, &rows)
            }
            (ShopRegion::Confirm, ShopFocus::Confirm) => {
                answer_confirm(&mut screen, &mut scene_state, &cmd_tx.0)
            }
            _ => {}
        }
    }
}

/// Keyboard handling for the shop window. Cancel unwinds exactly one level per
/// press — confirm box -> list -> Buy/Sell picker -> closed — matching how the
/// retail primitives nest (`shopmain` over `shopbuy`/`shopsell`).
pub(super) fn handle_shop_key(
    key: &Key,
    bindings: &Bindings,
    screen: &mut ShopScreenState,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    if scene_state.snapshot.shop.is_none() || screen.dismissed {
        return Some(InputMode::World);
    }
    let rows = shop::rows_for(screen.mode, &scene_state.snapshot);

    match screen.focus {
        ShopFocus::Menu => handle_menu_key(key, bindings, screen, cmd_tx),
        ShopFocus::List => {
            handle_list_key(key, bindings, screen, scene_state, cmd_tx, &rows);
            None
        }
        ShopFocus::Quantity => {
            handle_quantity_key(key, bindings, screen, scene_state, cmd_tx, &rows);
            None
        }
        ShopFocus::Confirm => {
            handle_confirm_key(key, bindings, screen, scene_state, cmd_tx);
            None
        }
    }
}

fn handle_menu_key(
    key: &Key,
    bindings: &Bindings,
    screen: &mut ShopScreenState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    if bindings.matches_logical(Action::NavUp, key) {
        screen.move_menu_cursor(-1);
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        screen.move_menu_cursor(1);
        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        let _ = cmd_tx.try_send(AgentCommand::CloseShop);
        screen.dismiss();
        return Some(InputMode::World);
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        screen.enter_list();
    }
    None
}

fn handle_list_key(
    key: &Key,
    bindings: &Bindings,
    screen: &mut ShopScreenState,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    rows: &[ShopRow],
) {
    if bindings.matches_logical(Action::NavUp, key) {
        screen.move_cursor(-1, rows.len());
        return;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        screen.move_cursor(1, rows.len());
        return;
    }
    // Retail's item lists page ten rows on Left/Right
    // (.agents/skills/retail-observe/references/2026-09-11-items-window.md).
    if bindings.matches_logical(Action::NavLeft, key) {
        screen.page(-1, rows.len());
        return;
    }
    if bindings.matches_logical(Action::NavRight, key) {
        screen.page(1, rows.len());
        return;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        screen.focus = ShopFocus::Menu;
        return;
    }
    if !bindings.matches_logical(Action::NavConfirm, key) {
        return;
    }
    activate_row(screen, scene_state, cmd_tx, rows);
}

/// Answer the row the cursor is on: size the stack, or move a lone item
/// outright. A sell row carries no price of its own, so the first confirm
/// buys the quote: appraise a single unit and let the picker show what each
/// one is worth before the player commits to a count.
fn activate_row(
    screen: &mut ShopScreenState,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    rows: &[ShopRow],
) {
    let Some(row) = rows.get(screen.cursor).copied() else {
        return;
    };
    match begin_quantity(screen.mode, &row) {
        Some(spinner) => {
            if matches!(screen.mode, ShopMode::Sell) {
                clear_appraisal(scene_state);
                request_appraisal(cmd_tx, &row, 1);
            }
            screen.quantity = Some(spinner);
            screen.focus = ShopFocus::Quantity;
        }
        None => commit_quantity(screen, scene_state, cmd_tx, &row, 1),
    }
}

fn clear_appraisal(scene_state: &mut SceneState) {
    if let Some(shop) = scene_state.snapshot.shop.as_mut() {
        shop.pending_sale = None;
    }
}

fn request_appraisal(cmd_tx: &Sender<AgentCommand>, row: &ShopRow, qty: u32) {
    let _ = cmd_tx.try_send(AgentCommand::ShopSellReq {
        qty,
        item_no: row.item_no,
        item_index: row.index,
    });
}

fn handle_quantity_key(
    key: &Key,
    bindings: &Bindings,
    screen: &mut ShopScreenState,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    rows: &[ShopRow],
) {
    let Some(spinner) = screen.quantity.as_mut() else {
        screen.focus = ShopFocus::List;
        return;
    };
    if bindings.matches_logical(Action::NavConfirm, key) {
        let quantity = spinner.value;
        match rows.get(screen.cursor).copied() {
            Some(row) => commit_quantity(screen, scene_state, cmd_tx, &row, quantity),
            None => {
                screen.quantity = None;
                screen.focus = ShopFocus::List;
            }
        }
        return;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        if matches!(screen.mode, ShopMode::Sell) {
            let _ = cmd_tx.try_send(AgentCommand::ShopSellCancel);
        }
        screen.quantity = None;
        screen.focus = ShopFocus::List;
        return;
    }
    spinner_nav(spinner, key, bindings);
}

/// Walk the confirm box. It is two rows, so either axis walks them — retail's
/// Log Out prompt answers to Left, the stacked Yes/No boxes to Up/Down.
fn handle_confirm_key(
    key: &Key,
    bindings: &Bindings,
    screen: &mut ShopScreenState,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) {
    if bindings.matches_logical(Action::NavCancel, key) {
        decline(screen, cmd_tx);
        return;
    }
    for action in [
        Action::NavUp,
        Action::NavDown,
        Action::NavLeft,
        Action::NavRight,
    ] {
        if bindings.matches_logical(action, key) {
            screen.confirm_yes = !screen.confirm_yes;
            return;
        }
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        answer_confirm(screen, scene_state, cmd_tx);
    }
}

/// Send the confirm box's answer. Confirming with the cursor on No is the same
/// answer as cancelling. In Sell mode the appraisal is what the player is
/// answering; until it lands there is no price to say yes to.
fn answer_confirm(
    screen: &mut ShopScreenState,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) {
    if !screen.confirm_yes {
        decline(screen, cmd_tx);
        return;
    }
    match screen.mode {
        ShopMode::Buy => {
            let Some(buy) = screen.pending_buy.take() else {
                screen.focus = ShopFocus::List;
                return;
            };
            let _ = cmd_tx.try_send(AgentCommand::ShopBuy {
                shop_no: SHOP_NO,
                shop_index: buy.shop_index,
                qty: buy.quantity,
            });
            screen.focus = ShopFocus::List;
        }
        ShopMode::Sell => {
            if shop::confirmed_sale(screen, &scene_state.snapshot).is_none() {
                return;
            }
            let _ = cmd_tx.try_send(AgentCommand::ShopSellConfirm);
            screen.focus = ShopFocus::List;
        }
    }
}

/// Answer the confirm box with No: drop the quote the server parked for a sale
/// and fall back to the list.
fn decline(screen: &mut ShopScreenState, cmd_tx: &Sender<AgentCommand>) {
    if matches!(screen.mode, ShopMode::Sell) {
        let _ = cmd_tx.try_send(AgentCommand::ShopSellCancel);
    }
    screen.pending_buy = None;
    screen.focus = ShopFocus::List;
}

/// The quantity picker for a row, or `None` when there is only one to move —
/// retail commits a lone item outright rather than asking for a count. Buy
/// stacks are bounded by the item's stack size (LSB clamps anything larger:
/// vendor/server/src/map/packets/c2s/0x083_shop_buy.cpp process); sell stacks
/// by what the player is holding.
fn begin_quantity(mode: ShopMode, row: &ShopRow) -> Option<DigitSpinner> {
    let max = match mode {
        ShopMode::Buy => ffxi_vocab::item_flags::stack_size(row.item_no) as u32,
        ShopMode::Sell => row.quantity,
    };
    (max > 1).then(|| DigitSpinner::item(max))
}

/// Take the sized amount into the priced step. A buy prices itself from the
/// listed unit price; a sell has to ask the server, so it sends the 0x084
/// appraisal (vendor/server/src/map/packets/c2s/0x084_shop_sell_req.cpp) and
/// waits for the 0x03D answer
/// (vendor/server/src/map/packets/s2c/0x03d_shop_sell.cpp) to fill the prompt
/// in. A sell re-prices at the chosen count: the quote shown while sizing was
/// for one unit, and the confirm prompt states the whole sale.
fn commit_quantity(
    screen: &mut ShopScreenState,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
    row: &ShopRow,
    quantity: u32,
) {
    match screen.mode {
        ShopMode::Buy => {
            let buy = screen.stage_buy(row, quantity);
            let name = kuluu_render::hud::bazaar_view::item_name(buy.item_no, None);
            push_system_chat_line(
                scene_state,
                shop::purchase_prompt(&name, buy.quantity, buy.total_gil),
            );
        }
        ShopMode::Sell => {
            screen.quantity = None;
            screen.pending_sell = Some((row.index, row.item_no, quantity));
            screen.enter_confirm();
            clear_appraisal(scene_state);
            request_appraisal(cmd_tx, row, quantity);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kuluu_snapshot::{SceneSnapshot, Vec3};

    fn at(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    #[test]
    fn second_stack_quantity_and_confirmation_use_its_slot() {
        let bindings = Bindings::default();
        let rows = [
            ShopRow {
                index: 1,
                item_no: 4096,
                quantity: 12,
                price: 0,
            },
            ShopRow {
                index: 2,
                item_no: 4096,
                quantity: 10,
                price: 0,
            },
        ];
        let mut screen = ShopScreenState {
            mode: ShopMode::Sell,
            focus: ShopFocus::Quantity,
            cursor: 1,
            quantity: begin_quantity(ShopMode::Sell, &rows[1]),
            ..Default::default()
        };
        let mut scene = SceneState {
            snapshot: SceneSnapshot {
                shop: Some(kuluu_snapshot::ShopState::default()),
                ..Default::default()
            },
            ..Default::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        for key in [Key::ArrowDown, Key::ArrowLeft, Key::ArrowLeft, Key::ArrowUp] {
            handle_quantity_key(&key, &bindings, &mut screen, &mut scene, &tx, &rows);
        }
        assert_eq!(screen.quantity.as_ref().unwrap().value, rows[1].quantity);
        handle_quantity_key(&Key::Enter, &bindings, &mut screen, &mut scene, &tx, &rows);
        assert!(matches!(
            rx.try_recv(),
            Ok(AgentCommand::ShopSellReq {
                item_index: 2,
                qty: 10,
                ..
            })
        ));
        handle_confirm_key(&Key::Enter, &bindings, &mut screen, &mut scene, &tx);
        assert!(rx.try_recv().is_err());
        scene.snapshot.shop.as_mut().unwrap().pending_sale = Some(kuluu_snapshot::ShopSale {
            item_index: 1,
            item_no: rows[0].item_no,
            count: rows[0].quantity,
            unit_price: 20,
        });
        handle_confirm_key(&Key::Enter, &bindings, &mut screen, &mut scene, &tx);
        assert!(rx.try_recv().is_err());
        scene.snapshot.shop.as_mut().unwrap().pending_sale = Some(kuluu_snapshot::ShopSale {
            item_index: rows[1].index,
            item_no: rows[1].item_no,
            count: rows[1].quantity,
            unit_price: 20,
        });
        handle_confirm_key(&Key::Enter, &bindings, &mut screen, &mut scene, &tx);
        assert!(matches!(rx.try_recv(), Ok(AgentCommand::ShopSellConfirm)));
    }

    #[test]
    fn the_vendor_leaving_reach_ends_the_conversation() {
        let me = at(0.0, 0.0, 0.0);
        assert!(!out_of_reach(me, at(VENDOR_RANGE_YALMS - 0.5, 0.0, 0.0)));
        assert!(out_of_reach(me, at(VENDOR_RANGE_YALMS + 0.5, 0.0, 0.0)));
    }

    #[test]
    fn range_is_measured_in_three_dimensions() {
        let me = at(0.0, 0.0, 0.0);
        assert!(
            out_of_reach(me, at(0.0, VENDOR_RANGE_YALMS + 1.0, 0.0)),
            "a vendor a floor away is out of reach"
        );
        assert!(
            out_of_reach(me, at(4.0, 0.0, 5.0)),
            "the diagonal is what counts, not either axis"
        );
    }

    #[test]
    fn a_vendor_missing_from_the_zone_closes_the_window() {
        let snap = SceneSnapshot::default();
        assert!(vendor_out_of_range(&snap, 7), "vendor is no longer spawned");
    }

    #[test]
    fn a_shop_with_no_resolved_vendor_is_never_range_closed() {
        let snap = SceneSnapshot::default();
        assert!(!vendor_out_of_range(&snap, 0));
    }

    /// Closing is a client decision that takes a round trip to reach the
    /// snapshot. Until it lands, the sync system must leave the still-present
    /// stock alone instead of reopening the window on top of the player — that
    /// oscillation strobed the window and its help bar every frame.
    #[test]
    fn a_dismissed_shop_is_not_reopened_while_the_stock_lingers() {
        let mut app = App::new();
        app.init_resource::<InputMode>()
            .init_resource::<ShopScreenState>()
            .insert_resource(CommandTx(tokio::sync::mpsc::channel(8).0))
            .insert_resource(SceneState {
                snapshot: SceneSnapshot {
                    zone_id: Some(245),
                    shop: Some(kuluu_snapshot::ShopState {
                        opened: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            });
        app.add_systems(Update, shop_mode_sync_system);

        app.update();
        assert!(
            matches!(*app.world().resource::<InputMode>(), InputMode::Shop),
            "a live shop takes the cursor"
        );

        app.world_mut().resource_mut::<ShopScreenState>().dismiss();
        *app.world_mut().resource_mut::<InputMode>() = InputMode::World;

        for _ in 0..5 {
            app.update();
            assert!(
                matches!(*app.world().resource::<InputMode>(), InputMode::World),
                "the window must stay closed until the snapshot catches up"
            );
        }

        app.world_mut().resource_mut::<SceneState>().snapshot.shop = None;
        app.update();
        assert!(!app.world().resource::<ShopScreenState>().dismissed);
    }
}
