use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_world_key(
    key: &Key,
    bindings: &Bindings,
    current_target: Option<u32>,
    entities: &[kuluu_snapshot::Entity],
    self_pos: kuluu_snapshot::Vec3,
    self_id: Option<u32>,
    target_changed: bool,
    engaged: bool,
    usable_items_available: bool,
    can_fish: bool,
    mounted: bool,
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
    check_target: &mut kuluu_render::hud::check_view::CheckTarget,
    trade_state: &mut kuluu_render::hud::trade::TradeState,
    lock_on: &mut kuluu_render::LockOn,
) -> Option<InputMode> {
    if bindings.matches_logical(Action::OpenChat, key) {
        return Some(InputMode::Chat(ChatBuffer::empty()));
    }
    if bindings.matches_logical(Action::ConfirmAction, key) {
        return match current_target {
            Some(_) if target_changed => None,
            Some(id) => {
                let ent = entities.iter().find(|e| e.id == id);
                let is_npc = matches!(ent.map(|e| e.kind), Some(kuluu_snapshot::EntityKind::Npc));
                let in_range = ent.is_some_and(|e| {
                    let dx = e.pos.x - self_pos.x;
                    let dy = e.pos.y - self_pos.y;
                    let dz = e.pos.z - self_pos.z;
                    let r = kuluu_render::hud::action_model::NPC_INTERACT_YALMS;
                    dx * dx + dy * dy + dz * dz <= r * r
                });
                if is_npc {
                    if let (true, Some(ent)) = (in_range, ent) {
                        let _ = cmd_tx.try_send(AgentCommand::Action {
                            target_id: ent.id,
                            target_index: ent.act_index,
                            kind: ActionKind::Talk,
                        });
                    }
                    None
                } else {
                    open_target_action_menu(
                        current_target,
                        entities,
                        self_pos,
                        self_id,
                        engaged,
                        usable_items_available,
                        can_fish,
                        mounted,
                        cmd_tx,
                        scene_state,
                        check_target,
                        trade_state,
                        lock_on,
                    )
                }
            }
            // Retail opens the same menu with nothing selected — that is where
            // the untargeted commands (Magic / Abilities / Items / Fish) live
            // (research/xim UiState.kt `handleDefaultEnter`). The opener returns
            // None when the list would be empty, so this cannot pop a blank box.
            None => open_target_action_menu(
                current_target,
                entities,
                self_pos,
                self_id,
                engaged,
                usable_items_available,
                can_fish,
                mounted,
                cmd_tx,
                scene_state,
                check_target,
                trade_state,
                lock_on,
            ),
        };
    }
    None
}

/// Opens the target's Command Menu — or, when the menu holds a single
/// non-destructive entry, runs that entry instead of asking for a second
/// confirm ([`action_model::sole_auto_confirm_entry`]).
#[allow(clippy::too_many_arguments)]
fn open_target_action_menu(
    current_target: Option<u32>,
    entities: &[kuluu_snapshot::Entity],
    self_pos: kuluu_snapshot::Vec3,
    self_id: Option<u32>,
    engaged: bool,
    usable_items_available: bool,
    can_fish: bool,
    mounted: bool,
    cmd_tx: &Sender<AgentCommand>,
    scene_state: &mut SceneState,
    check_target: &mut kuluu_render::hud::check_view::CheckTarget,
    trade_state: &mut kuluu_render::hud::trade::TradeState,
    lock_on: &mut kuluu_render::LockOn,
) -> Option<InputMode> {
    use kuluu_render::hud::action_model;
    let ctx = action_model::context_for_target(
        current_target,
        entities,
        self_pos,
        self_id,
        engaged,
        usable_items_available,
        can_fish,
        mounted,
    );
    let entries = kuluu_render::hud::overlay::RETAIL.resolve_target_actions(&ctx);
    if entries.is_empty() {
        return None;
    }
    let mut state = kuluu_render::input_mode::TargetActionState::open(ctx);
    if action_model::sole_auto_confirm_entry(&entries).is_some() {
        return confirm_target_action_at_cursor(
            &mut state,
            &entries,
            scene_state,
            current_target,
            entities,
            cmd_tx,
            check_target,
            trade_state,
            lock_on,
        );
    }
    Some(InputMode::TargetAction(state))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_target_action_key(
    key: &Key,
    bindings: &Bindings,
    state: &mut kuluu_render::input_mode::TargetActionState,
    scene_state: &mut SceneState,
    current_target: Option<u32>,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
    check_target: &mut kuluu_render::hud::check_view::CheckTarget,
    trade_state: &mut kuluu_render::hud::trade::TradeState,
    trade_intent: &mut MessageWriter<kuluu_render::hud::trade::TradeIntent>,
    lock_on: &mut kuluu_render::LockOn,
) -> Option<InputMode> {
    use kuluu_render::hud::action_model::{ActionEntryKind, TargetActionId};
    use kuluu_render::input_mode::SubAction;

    if trade_state.open {
        return handle_trade_key(key, bindings, trade_state, trade_intent, scene_state);
    }

    if let Some(SubAction::AbilitiesGroup(group)) = state.sub.as_ref().and_then(|s| s.current()) {
        return handle_abilities_group_key(
            key,
            bindings,
            state,
            group,
            scene_state,
            current_target,
            entities,
            cmd_tx,
        );
    }

    let entries = kuluu_render::hud::overlay::RETAIL.resolve_target_actions(&state.ctx);
    let count = entries.len();
    if count == 0 {
        return Some(InputMode::World);
    }

    // A pending Dismount confirm owns the pane: Yes/No instead of the rows.
    if state.dismount_confirm {
        if bindings.matches_logical(Action::NavUp, key)
            || bindings.matches_logical(Action::NavDown, key)
        {
            state.cursor = 1 - state.cursor % 2;
            return None;
        }
        if bindings.matches_logical(Action::NavConfirm, key)
            || bindings.matches_logical(Action::NavCancel, key)
        {
            // Confirm takes the row under the cursor (0 = Yes); Cancel is No.
            return answer_dismount_confirm(
                state,
                &entries,
                bindings.matches_logical(Action::NavConfirm, key) && state.cursor == 0,
                scene_state,
                entities,
                cmd_tx,
            );
        }
        // Any other key drops the confirm and falls through to the rows.
        state.dismount_confirm = false;
    }

    if state.cursor >= count {
        state.cursor = count - 1;
    }

    if bindings.matches_logical(Action::NavUp, key) {
        state.cursor = if state.cursor == 0 {
            count - 1
        } else {
            state.cursor - 1
        };
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        let next = state.cursor + 1;
        state.cursor = if next >= count { 0 } else { next };
        return None;
    }
    if bindings.matches_logical(Action::NavRight, key) {
        if let Some(entry) = entries.get(state.cursor) {
            if let ActionEntryKind::Select { modes, .. } = &entry.kind {
                if !modes.is_empty() {
                    match entry.id {
                        TargetActionId::Chat => {
                            state.chat_mode_idx = (state.chat_mode_idx + 1) % modes.len();
                        }
                        TargetActionId::Abilities => {
                            state.abilities_group_idx =
                                (state.abilities_group_idx + 1) % modes.len();
                        }
                        _ => {}
                    }
                }
            }
        }
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        return confirm_target_action_at_cursor(
            state,
            &entries,
            scene_state,
            current_target,
            entities,
            cmd_tx,
            check_target,
            trade_state,
            lock_on,
        );
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        return Some(InputMode::World);
    }
    None
}

/// Confirm the target-action entry under the cursor. Disengage releases the
/// camera lock like the H toggle.
#[allow(clippy::too_many_arguments)]
pub(super) fn confirm_target_action_at_cursor(
    state: &mut kuluu_render::input_mode::TargetActionState,
    entries: &[kuluu_render::hud::action_model::ActionEntry],
    scene_state: &mut SceneState,
    current_target: Option<u32>,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
    check_target: &mut kuluu_render::hud::check_view::CheckTarget,
    trade_state: &mut kuluu_render::hud::trade::TradeState,
    lock_on: &mut kuluu_render::LockOn,
) -> Option<InputMode> {
    use kuluu_render::hud::action_model::TargetActionId;

    let Some(entry) = entries.get(state.cursor) else {
        return Some(InputMode::World);
    };
    if !entry.enabled {
        if let Some(hint) = &entry.hint {
            push_system_chat_line(scene_state, format!("[menu] {hint}"));
        }
        return None;
    }

    let target_ent = current_target.and_then(|id| entities.iter().find(|e| e.id == id));
    match entry.id {
        TargetActionId::Attack => {
            match target_ent {
                Some(e) => {
                    // The server's engage rejections, answered locally before the
                    // command goes out (the server's own 0x029 lines still land in
                    // the main log, vendor/server/src/map/packets/s2c/0x029_battle_message.cpp;
                    // these save the round trip).
                    if let Some(line) = crate::view_native::engage::rejection_line(
                        e,
                        scene_state.snapshot.self_pos.pos,
                        scene_state.snapshot.self_char_id,
                        &scene_state.snapshot.party,
                    ) {
                        push_system_chat_line(scene_state, line);
                    } else if let Err(err) =
                        cmd_tx.try_send(AgentCommand::Engage { target_id: e.id })
                    {
                        push_system_chat_line(
                            scene_state,
                            format!("[menu] Attack dispatch dropped: {err}"),
                        );
                    }
                }
                None => push_system_chat_line(scene_state, "[menu] Attack: no target".to_string()),
            }
            Some(InputMode::World)
        }
        TargetActionId::SwitchTarget => {
            // Retail's "Switch Target" opens the sub-target picker over the
            // other mobs; confirming asks the server to move the battle target,
            // and it becomes the main target when the 0x058 lands
            // (vendor/server/src/map/packets/s2c/0x058_assist.cpp).
            let sub_action = kuluu_render::input_mode::SubTargetAction::PickSub;
            let return_to = InputMode::TargetAction(state.clone());
            open_sub_target(sub_action, current_target, scene_state, return_to)
        }
        TargetActionId::Disengage => {
            lock_on.target_id = None;
            if let Err(err) = cmd_tx.try_send(AgentCommand::Cancel) {
                push_system_chat_line(
                    scene_state,
                    format!("[menu] Disengage dispatch dropped: {err}"),
                );
            }
            Some(InputMode::World)
        }
        TargetActionId::Chat => Some(InputMode::Chat(chat_buffer_for_mode(
            state.chat_mode_idx,
            target_ent,
        ))),
        TargetActionId::Magic => Some(open_submenu(MenuKind::Magic)),
        TargetActionId::Abilities => {
            use kuluu_render::hud::action_model::AbilityGroup;
            use kuluu_render::input_mode::{SubAction, SubActionStack};
            let group = AbilityGroup::ALL[state.abilities_group_idx % AbilityGroup::ALL.len()];
            state.sub = Some(SubActionStack::with(SubAction::AbilitiesGroup(group)));
            None
        }
        TargetActionId::Items => Some(open_submenu(MenuKind::UsableItems)),
        TargetActionId::Check => {
            use kuluu_render::hud::action_model::TargetKindLite;
            match target_ent {
                Some(e) => {
                    let cmd = AgentCommand::CheckTarget {
                        target_id: e.id,
                        target_index: e.act_index,
                        kind: CheckKind::Check,
                    };
                    if let Err(err) = cmd_tx.try_send(cmd) {
                        push_system_chat_line(
                            scene_state,
                            format!("[menu] Check dispatch dropped: {err}"),
                        );
                    }

                    let is_pc = matches!(
                        state.ctx.target_kind,
                        TargetKindLite::Pc | TargetKindLite::SelfPc
                    );
                    if is_pc {
                        check_target.open(
                            e.id,
                            kuluu_render::hud::check_view::wares_enabled(
                                &scene_state.snapshot,
                                e.id,
                            ),
                        );
                        Some(InputMode::Check)
                    } else {
                        Some(InputMode::World)
                    }
                }
                None => {
                    push_system_chat_line(scene_state, "[menu] Check: no target".into());
                    Some(InputMode::World)
                }
            }
        }
        TargetActionId::Trade => match target_ent {
            Some(e) => {
                *trade_state = kuluu_render::hud::trade::TradeState::open(e.id);
                None
            }
            None => {
                push_system_chat_line(scene_state, "[menu] Trade: no target".into());
                Some(InputMode::World)
            }
        },
        TargetActionId::Trust => {
            push_system_chat_line(scene_state, "[menu] Trust — not implemented yet".into());
            Some(InputMode::World)
        }
        TargetActionId::Open => {
            match target_ent {
                Some(e) => {
                    // Doors are TYPE_NPC server-side (look.size == 0x02) and
                    // trigger through the same Talk action_id as any other
                    // NPC — vendor/server/src/map/packets/c2s/0x01a_action.cpp GP_CLI_COMMAND_ACTION::process.
                    // The server's own door script drives the yes/no confirm
                    // and zone change; nothing door-specific is needed here.
                    let cmd = AgentCommand::Action {
                        target_id: e.id,
                        target_index: e.act_index,
                        kind: ActionKind::Talk,
                    };
                    if let Err(err) = cmd_tx.try_send(cmd) {
                        push_system_chat_line(
                            scene_state,
                            format!("[menu] Open dispatch dropped: {err}"),
                        );
                    }
                }
                None => push_system_chat_line(scene_state, "[menu] Open: no target".to_string()),
            }
            Some(InputMode::World)
        }
        TargetActionId::Fish => {
            if let Err(err) = cmd_tx.try_send(AgentCommand::Fish) {
                push_system_chat_line(scene_state, format!("[menu] Fish dispatch dropped: {err}"));
            }
            Some(InputMode::World)
        }
        TargetActionId::Dig => {
            send_self_action(
                ActionKind::ChocoboDig,
                &entry.label,
                scene_state,
                entities,
                cmd_tx,
            );
            Some(InputMode::World)
        }
        TargetActionId::Dismount => {
            // Retail asks before the mount comes off: the pane flips to a
            // Yes/No confirm; Yes sends the 0x01A, No returns to the rows.
            state.dismount_confirm = true;
            state.cursor = 0;
            None
        }
    }
}

/// The self-targeted 0x01A actions (Dig, Dismount): the vendor acts on the
/// sender and ignores the target fields
/// (vendor/server/src/map/packets/c2s/0x01a_action.cpp
/// GP_CLI_COMMAND_ACTION::process).
fn send_self_action(
    kind: ActionKind,
    label: &str,
    scene_state: &mut SceneState,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
) {
    let self_id = scene_state.snapshot.self_char_id.unwrap_or(0);
    let self_index = entities
        .iter()
        .find(|e| e.id == self_id)
        .map(|e| e.act_index)
        .unwrap_or(0);
    if let Err(err) = cmd_tx.try_send(AgentCommand::Action {
        target_id: self_id,
        target_index: self_index,
        kind,
    }) {
        push_system_chat_line(
            scene_state,
            format!("[menu] {label} dispatch dropped: {err}"),
        );
    }
}

/// The Dismount confirm's answer: Yes sends the self-targeted 0x01A and closes
/// the menu; No (or a cancel) returns to the rows with the cursor back on
/// Dismount.
#[allow(clippy::too_many_arguments)]
pub(super) fn answer_dismount_confirm(
    state: &mut kuluu_render::input_mode::TargetActionState,
    entries: &[kuluu_render::hud::action_model::ActionEntry],
    yes: bool,
    scene_state: &mut SceneState,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    use kuluu_render::hud::action_model::TargetActionId;
    state.dismount_confirm = false;
    if !yes {
        if let Some(row) = entries
            .iter()
            .position(|e| e.id == TargetActionId::Dismount)
        {
            state.cursor = row;
        }
        return None;
    }
    send_self_action(
        ActionKind::Dismount,
        "dismount",
        scene_state,
        entities,
        cmd_tx,
    );
    Some(InputMode::World)
}

#[allow(clippy::too_many_arguments)]
fn handle_abilities_group_key(
    key: &Key,
    bindings: &Bindings,
    state: &mut kuluu_render::input_mode::TargetActionState,
    group: kuluu_render::hud::action_model::AbilityGroup,
    scene_state: &mut SceneState,
    current_target: Option<u32>,
    entities: &[kuluu_snapshot::Entity],
    cmd_tx: &Sender<AgentCommand>,
) -> Option<InputMode> {
    let rows = kuluu_render::hud::menu::ability_group_rows(&scene_state.snapshot, group);
    let count = rows.len();

    let sub = state.sub.as_mut()?;
    if count > 0 && sub.cursor >= count {
        sub.cursor = count - 1;
    }

    if bindings.matches_logical(Action::NavUp, key) {
        if count > 0 {
            sub.cursor = if sub.cursor == 0 {
                count - 1
            } else {
                sub.cursor - 1
            };
        }
        return None;
    }
    if bindings.matches_logical(Action::NavDown, key) {
        if count > 0 {
            let next = sub.cursor + 1;
            sub.cursor = if next >= count { 0 } else { next };
        }
        return None;
    }
    if bindings.matches_logical(Action::NavConfirm, key) {
        if let Some(row) = rows.get(sub.cursor) {
            let action = row.action;
            let sub_action = sub_target_action_for(action);
            if let Some(sub_action) = sub_action {
                if !selected_target_valid(sub_action, current_target, scene_state) {
                    // No valid target selected: retail's flashing sub-target
                    // cursor asks "on whom?" first. Esc returns here with the
                    // menu cursor preserved.
                    let return_to = InputMode::TargetAction(state.clone());
                    return open_sub_target(sub_action, current_target, scene_state, return_to);
                }
            }
            let self_pos = scene_state.snapshot.self_pos.pos;
            dispatch_dynamic_menu_action(
                action,
                current_target,
                self_pos,
                entities,
                cmd_tx,
                scene_state,
            );
            return Some(InputMode::World);
        }

        return None;
    }
    if bindings.matches_logical(Action::NavCancel, key) {
        if !sub.pop() {
            state.sub = None;
        }
        return None;
    }
    None
}

fn open_submenu(kind: MenuKind) -> InputMode {
    let mut stack = MenuStack::root();
    stack.push(kind);
    InputMode::Menu(stack)
}
