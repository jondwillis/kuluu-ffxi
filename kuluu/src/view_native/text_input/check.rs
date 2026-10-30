use super::*;

use kuluu_render::hud::bazaar_view::{BazaarScreenState, PRICE_CAP};
use kuluu_render::hud::check_view::{self, CheckTarget};
use kuluu_render::hud::digit_spinner::DigitSpinner;
use kuluu_render::hud::menu::DynamicMenu;

/// Keeps the Check/Bazaar windows in step with the server:
/// - leaving `Bazaar` when the browsed bazaar disappears (s2c 0x107, or its last
///   row selling out). Only the `Some → None` edge closes it, so the mode
///   survives the gap between our c2s 0x105 and the first row arriving.
/// - keeping the row cursor inside a list the server shrinks under us.
/// - closing both windows on a zone change, since the checked PC and their
///   bazaar are entities of the zone we just left. A zone change is not an
///   `AppPhase` transition, so no state-exit cleanup fires for us (kuluu-oe8y).
pub fn bazaar_mode_sync_system(
    state: Res<SceneState>,
    mut mode: ResMut<InputMode>,
    mut screen: ResMut<BazaarScreenState>,
    mut check: ResMut<CheckTarget>,
    mut was_open: Local<bool>,
    mut last_zone: Local<Option<u16>>,
) {
    let zone = state.snapshot.zone_id;
    if zone.is_some() && *last_zone != zone {
        let zoned = last_zone.is_some();
        *last_zone = zone;
        if zoned {
            screen.reset();
            check.close();
            if matches!(*mode, InputMode::Check | InputMode::Bazaar) {
                *mode = InputMode::World;
            }
        }
    }

    let open = state.snapshot.bazaar.is_some();
    // Our own `Set Prices` menu lives in the same mode with no browsed stall behind
    // it, so none of this applies while we are pricing.
    if screen.prices.is_none() && matches!(*mode, InputMode::Bazaar) {
        if *was_open && !open {
            screen.reset();
            *mode = InputMode::Check;
        } else if let Some(view) = state.snapshot.bazaar.as_ref() {
            screen.clamp(view.items.len());
        }
    }
    *was_open = open;
}

/// Keyboard handling for the /check window: the equipment grid cursor plus the
/// View Wares entry.
pub(super) fn handle_check_key(
    key: &Key,
    bindings: &Bindings,
    check: &mut CheckTarget,
    screen: &mut BazaarScreenState,
    scene_state: &SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let Some(target_id) = check.target_id else {
        check.close();
        return Some(InputMode::World);
    };
    let snap = &scene_state.snapshot;
    let wares_enabled = check_view::wares_enabled(snap, target_id);

    for (action, dx, dy) in [
        (Action::NavUp, 0, -1),
        (Action::NavDown, 0, 1),
        (Action::NavLeft, -1, 0),
        (Action::NavRight, 1, 0),
    ] {
        if bindings.matches_logical(action, key) {
            check.move_focus(dx, dy, wares_enabled);
            return None;
        }
    }

    if bindings.matches_logical(Action::NavCancel, key) {
        check.close();
        return Some(InputMode::World);
    }

    if !bindings.matches_logical(Action::NavConfirm, key) {
        return None;
    }
    if !check.on_wares || !wares_enabled {
        return None;
    }
    let target_index = snap
        .entities
        .iter()
        .find(|e| e.id == target_id)
        .map(|e| e.act_index)?;
    let _ = cmd_tx.try_send(AgentCommand::OpenBazaar {
        target_id,
        target_index,
    });
    screen.reset();
    Some(InputMode::Bazaar)
}

/// A confirmed `Set Prices` row hands the frame over to the price menu: the shared
/// bazaar surface fills with our sellable inventory and takes the keys, so no menu
/// window is involved any more (retail leaves the Command menu behind for it too).
pub fn bazaar_price_mode_sync_system(
    state: Res<SceneState>,
    mut mode: ResMut<InputMode>,
    mut screen: ResMut<BazaarScreenState>,
    mut dynamic: ResMut<DynamicMenu>,
) {
    // The help band reads this mirror rather than the price menu's own state, so the
    // caption follows the surface whatever closed it.
    let pricing = screen.prices.is_some();
    if dynamic.bazaar_prices_active != pricing {
        dynamic.bazaar_prices_active = pricing;
    }
    if !std::mem::take(&mut dynamic.bazaar_prices_request) {
        return;
    }
    screen.open_prices(&state.snapshot);
    *mode = InputMode::Bazaar;
}

/// The `Set Prices` menu of our own stall, on the same surface and mode as a
/// browsed bazaar: rows are sellable inventory, Enter opens the gil spinner where
/// retail does (over the Current Gil box), a second Enter commits c2s 0x10A, and
/// Esc leaves the price menu with c2s 0x109 so the stall republishes
/// (vendor/server/src/map/packets/c2s/0x10a_bazaar_itemset.cpp,
/// 0x109_bazaar_open.cpp).
pub(super) fn handle_bazaar_price_key(
    key: &Key,
    bindings: &Bindings,
    screen: &mut BazaarScreenState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let len = screen
        .prices
        .as_ref()
        .map(|mode| mode.rows.len())
        .unwrap_or_default();

    // A price being typed: the shared spinner law (Up/Down step a digit, Left/Right
    // move the column), Enter commits it.
    if screen
        .prices
        .as_ref()
        .and_then(|mode| mode.edit_row)
        .is_some()
    {
        if bindings.matches_logical(Action::NavConfirm, key) {
            let price = screen
                .quantity
                .as_ref()
                .map(|spin| spin.confirm())
                .unwrap_or(0);
            let edited = screen.prices.as_ref().and_then(|menu| {
                menu.edit_row
                    .and_then(|row| menu.rows.get(row).map(|item| (row, item.slot)))
            });
            let Some((row, slot)) = edited else {
                screen.quantity = None;
                if let Some(mode) = screen.prices.as_mut() {
                    mode.edit_row = None;
                }
                return None;
            };
            if let Some(mode) = screen.prices.as_mut() {
                mode.rows[row].price = price;
                mode.edit_row = None;
            }
            screen.quantity = None;
            let _ = cmd_tx.try_send(AgentCommand::BazaarPriceSet { slot, price });
            return None;
        }
        if bindings.matches_logical(Action::NavCancel, key) {
            screen.quantity = None;
            if let Some(mode) = screen.prices.as_mut() {
                mode.edit_row = None;
            }
            return None;
        }
        // A price is typed as a number rather than walked column by column: digits build the
        // amount, and Backspace takes one digit back off.
        if let Some(digit) = typed_digit(key) {
            if let Some(spin) = screen.quantity.as_mut() {
                spin.type_digit(digit);
            }
            return None;
        }
        if bindings.matches_logical(Action::ChatBackspace, key) {
            if let Some(spin) = screen.quantity.as_mut() {
                spin.untype_digit();
            }
            return None;
        }
        if let Some(spin) = screen.quantity.as_mut() {
            spinner_nav(spin, key, bindings);
        }
        return None;
    }

    if bindings.matches_logical(Action::NavUp, key) {
        screen.move_cursor(-1, len);
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        screen.move_cursor(1, len);
        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        let _ = cmd_tx.try_send(AgentCommand::BazaarPricesDone);
        screen.reset();
        return Some(InputMode::World);
    }
    if !bindings.matches_logical(Action::NavConfirm, key) {
        return None;
    }
    // Retail replaces the description panel with the price entry for that item.
    let current = screen
        .prices
        .as_ref()
        .and_then(|mode| mode.rows.get(screen.cursor))
        .map(|row| row.price)
        .unwrap_or(0);
    if let Some(mode) = screen.prices.as_mut() {
        mode.edit_row = Some(screen.cursor.min(mode.rows.len().saturating_sub(1)));
    }
    screen.quantity = Some(DigitSpinner::with_value(PRICE_CAP, current));
    None
}

/// One typed decimal digit: the top row and the numpad both arrive as this logical character, and
/// a composed multi-character input is deliberately not one.
fn typed_digit(key: &Key) -> Option<u8> {
    let Key::Character(text) = key else {
        return None;
    };
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() || !c.is_ascii_digit() {
        return None;
    }
    c.to_digit(10).map(|d| d as u8)
}

/// Keyboard handling for the browsed bazaar's wares list.
pub(super) fn handle_bazaar_key(
    key: &Key,
    bindings: &Bindings,
    screen: &mut BazaarScreenState,
    scene_state: &mut SceneState,
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    if screen.prices.is_some() {
        return handle_bazaar_price_key(key, bindings, screen, cmd_tx);
    }
    let Some(view) = scene_state.snapshot.bazaar.clone() else {
        return Some(InputMode::Check);
    };

    // Retail asks "Purchase N x for Y gil?" in chat before sending the buy.
    if let Some(buy) = screen.pending {
        if bindings.matches_logical(Action::NavConfirm, key) {
            screen.pending = None;
            let _ = cmd_tx.try_send(AgentCommand::BuyBazaarItem {
                index: buy.index,
                quantity: buy.quantity,
            });
            return None;
        }
        if bindings.matches_logical(Action::NavCancel, key) {
            screen.pending = None;
        }
        return None;
    }

    // Quantity picker for the focused stack.
    if let Some(spinner) = screen.quantity.as_mut() {
        if bindings.matches_logical(Action::NavConfirm, key) {
            let quantity = spinner.confirm();
            if let Some(entry) = view.items.get(screen.cursor).copied() {
                let buy = screen.stage_purchase(&entry, quantity);
                push_purchase_prompt(scene_state, buy);
            } else {
                screen.quantity = None;
            }
            return None;
        }
        if bindings.matches_logical(Action::NavCancel, key) {
            screen.quantity = None;
            return None;
        }
        spinner_nav(spinner, key, bindings);
        return None;
    }

    if bindings.matches_logical(Action::NavUp, key) {
        screen.move_cursor(-1, view.items.len());
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        screen.move_cursor(1, view.items.len());
        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        let _ = cmd_tx.try_send(AgentCommand::CloseBazaar);
        screen.reset();
        return Some(InputMode::Check);
    }
    if !bindings.matches_logical(Action::NavConfirm, key) {
        return None;
    }
    let entry = *view.items.get(screen.cursor)?;
    match BazaarScreenState::begin_quantity(&entry) {
        Some(spinner) => screen.quantity = Some(spinner),
        None => {
            let buy = screen.stage_purchase(&entry, 1);
            push_purchase_prompt(scene_state, buy);
        }
    }
    None
}

/// The retail prompt line; confirm sends the buy, cancel drops it.
fn push_purchase_prompt(
    scene_state: &mut SceneState,
    buy: kuluu_render::hud::bazaar_view::PendingBuy,
) {
    let name = kuluu_render::hud::bazaar_view::item_name(buy.item_no, None);
    push_system_chat_line(
        scene_state,
        kuluu_render::hud::bazaar_view::purchase_prompt(&name, buy.quantity, buy.total_gil),
    );
}
