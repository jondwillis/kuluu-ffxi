use bevy::prelude::*;

use crate::hud::action_model::{ActionEntry, ActionEntryKind, TargetActionId};
use crate::hud::overlay::ActiveOverlay;
use crate::hud::palette;
use crate::input_mode::{InputMode, SubAction, TargetLevel};

const MAX_ROWS: usize = 7;

const SUBMENU_ARROW: &str = "▶";

#[derive(Component)]
pub struct TargetActionMenu;

#[derive(Component)]
pub struct TargetActionRow {
    pub slot: usize,
}

#[derive(Component)]
pub struct TargetActionBreadcrumb;

#[derive(Message, Debug, Clone, Copy)]
pub struct TargetActionActivated {
    pub slot: usize,
}

pub fn entries_for_mode(mode: &InputMode, overlay: &ActiveOverlay) -> Option<Vec<ActionEntry>> {
    match mode {
        InputMode::TargetAction(state) => Some(overlay.0.resolve_target_actions(&state.ctx)),
        _ => None,
    }
}

pub fn entry_count(mode: &InputMode, overlay: &ActiveOverlay) -> usize {
    entries_for_mode(mode, overlay)
        .map(|e| e.len())
        .unwrap_or(0)
}

fn entry_text(entry: &ActionEntry) -> String {
    let mut s = match &entry.kind {
        ActionEntryKind::Plain => entry.label.clone(),
        ActionEntryKind::Select { .. } => format!("{} {SUBMENU_ARROW}", entry.label),
    };
    if let Some(hint) = &entry.hint {
        s.push_str("  (");
        s.push_str(hint);
        s.push(')');
    }
    s
}

fn breadcrumb_text(sub: Option<SubAction>) -> Option<String> {
    sub.map(|frame| match frame {
        SubAction::MagicCategory(cat) => format!("Magic / {}", cat.label()),
        SubAction::AbilitiesGroup(group) => format!("Abilities / {}", group.label()),
        SubAction::Items => "Items".to_string(),
        SubAction::ChatCompose => "Chat".to_string(),
    })
    .map(|s| format!("» {s}"))
}

pub fn spawn_target_action_menu_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        TargetActionMenu,
        Node {
            align_self: AlignSelf::FlexStart,
            flex_shrink: 0.0,
            width: Val::Auto,
            padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
            border: UiRect::all(Val::Px(1.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            display: Display::None,
            ..default()
        },
        ZIndex(20),
        BackgroundColor(palette::BACKGROUND),
        BorderColor::all(palette::ACCENT),
    ))
    .with_children(spawn_target_action_rows);
}

fn spawn_target_action_rows(p: &mut ChildSpawnerCommands) {
    p.spawn((
        TargetActionBreadcrumb,
        Node {
            display: Display::None,
            ..default()
        },
        Text::new(""),
        TextFont {
            font_size: 11.0,
            ..default()
        },
        TextColor(palette::MUTED),
    ));
    for slot in 0..MAX_ROWS {
        p.spawn((
            TargetActionRow { slot },
            Button,
            Node::default(),
            Text::new(""),
            TextFont {
                font_size: 13.0,
                ..default()
            },
            TextColor(palette::MUTED),
        ));
    }
}

pub fn update_target_action_menu(
    mode: Res<InputMode>,
    overlay: Res<ActiveOverlay>,

    scene: Res<crate::snapshot::SceneState>,
    vana_clock: Res<crate::vana_time::VanaClock>,
    mut panel_q: Query<&mut Node, With<TargetActionMenu>>,
    mut row_q: Query<
        (&TargetActionRow, &mut Node, &mut Text, &mut TextColor),
        (Without<TargetActionMenu>, Without<TargetActionBreadcrumb>),
    >,
    mut crumb_q: Query<
        (&mut Node, &mut Text, &mut TextColor),
        (With<TargetActionBreadcrumb>, Without<TargetActionMenu>),
    >,
) {
    let Ok(mut panel) = panel_q.single_mut() else {
        return;
    };

    let InputMode::TargetAction(state) = &*mode else {
        if panel.display != Display::None {
            panel.display = Display::None;
        }
        return;
    };

    panel.display = Display::Flex;

    let sub_active = match state.stack.current().map(|l| l.kind) {
        Some(TargetLevel::Sub(action)) => Some(action),
        _ => None,
    };
    if let Ok((mut node, mut text, mut color)) = crumb_q.single_mut() {
        match breadcrumb_text(sub_active) {
            Some(crumb) => {
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                if **text != crumb {
                    **text = crumb;
                }
                let want = palette::ACCENT;
                if color.0 != want {
                    color.0 = want;
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }

    let mut entries = overlay.0.resolve_target_actions(&state.ctx);

    for entry in entries.iter_mut() {
        let cycled = match entry.id {
            TargetActionId::Chat => Some(state.chat_mode_idx),
            TargetActionId::Abilities => Some(state.abilities_group_idx),
            _ => None,
        };
        if let Some(idx) = cycled {
            if let ActionEntryKind::Select { modes, mode_idx } = &mut entry.kind {
                if !modes.is_empty() {
                    *mode_idx = idx % modes.len();
                }
            }
        }
    }
    let cursor = state.stack.current().map(|l| l.cursor).unwrap_or(0);

    if let Some(SubAction::AbilitiesGroup(group)) = sub_active {
        let rows = crate::hud::menu::ability_group_rows(&scene.snapshot, group);
        let sub_cursor = cursor;
        for (row, mut node, mut text, mut color) in row_q.iter_mut() {
            let (want, want_color) = if rows.is_empty() {
                if row.slot == 0 {
                    (
                        format!("  {}", crate::hud::menu::ability_group_empty_hint(group)),
                        palette::MUTED,
                    )
                } else {
                    if node.display != Display::None {
                        node.display = Display::None;
                    }
                    continue;
                }
            } else if let Some(leaf) = rows.get(row.slot) {
                let is_cursor = row.slot == sub_cursor;
                let caret = if is_cursor { "> " } else { "  " };
                let now = vana_clock.earth_unix_secs_now() as u32;
                match recast_remaining(&scene.snapshot.ability_recasts, &leaf.action, now) {
                    Some(remaining) => (
                        format!(
                            "{caret}{} ({})",
                            leaf.label,
                            crate::hud::format_timer(remaining)
                        ),
                        palette::DARK,
                    ),
                    None => {
                        let color = if is_cursor {
                            palette::ACCENT
                        } else {
                            palette::TEXT
                        };
                        (format!("{caret}{}", leaf.label), color)
                    }
                }
            } else {
                if node.display != Display::None {
                    node.display = Display::None;
                }
                continue;
            };
            if node.display != Display::Flex {
                node.display = Display::Flex;
            }
            if **text != want {
                **text = want;
            }
            if color.0 != want_color {
                color.0 = want_color;
            }
        }
        return;
    }

    for (row, mut node, mut text, mut color) in row_q.iter_mut() {
        match entries.get(row.slot) {
            Some(entry) => {
                if node.display != Display::Flex {
                    node.display = Display::Flex;
                }
                let is_cursor = row.slot == cursor && sub_active.is_none();
                let caret = if is_cursor { "> " } else { "  " };
                let want = format!("{caret}{}", entry_text(entry));
                if **text != want {
                    **text = want;
                }

                let want_color = if !entry.enabled {
                    palette::DARK
                } else if sub_active.is_some() {
                    palette::MUTED
                } else if is_cursor {
                    palette::ACCENT
                } else {
                    palette::TEXT
                };
                if color.0 != want_color {
                    color.0 = want_color;
                }
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                }
            }
        }
    }
}

fn recast_remaining(
    recasts: &[(u16, u32)],
    action: &crate::hud::menu::DynamicMenuAction,
    now: u32,
) -> Option<u32> {
    use crate::hud::menu::DynamicMenuAction as A;
    let ability_id = match action {
        A::JobAbility { ability_id } | A::PetAbility { ability_id } => *ability_id,
        _ => return None,
    };
    let recast_id = ffxi_proto::recast::ability_recast_id(ability_id)?;
    let expiry = recasts
        .iter()
        .find(|(rid, _)| *rid == recast_id)
        .map(|(_, e)| *e)?;
    let remaining = expiry.saturating_sub(now);
    (remaining > 0).then_some(remaining)
}

pub fn target_action_mouse_hover_system(
    mut mode: ResMut<InputMode>,
    overlay: Res<ActiveOverlay>,
    rows: Query<(&Interaction, &TargetActionRow), Changed<Interaction>>,
) {
    let limit = entry_count(&mode, &overlay);
    let InputMode::TargetAction(state) = &mut *mode else {
        return;
    };

    if matches!(
        state.stack.current().map(|l| l.kind),
        Some(TargetLevel::Sub(_))
    ) {
        return;
    }
    let Some(level) = state.stack.current_mut() else {
        return;
    };
    for (interaction, row) in &rows {
        if matches!(interaction, Interaction::Hovered | Interaction::Pressed)
            && row.slot < limit
            && level.cursor != row.slot
        {
            level.cursor = row.slot;
        }
    }
}

pub fn target_action_mouse_click_system(
    mode: Res<InputMode>,
    overlay: Res<ActiveOverlay>,
    rows: Query<(&Interaction, &TargetActionRow), Changed<Interaction>>,
    mut out: MessageWriter<TargetActionActivated>,
) {
    let limit = entry_count(&mode, &overlay);
    let InputMode::TargetAction(state) = &*mode else {
        return;
    };
    if matches!(
        state.stack.current().map(|l| l.kind),
        Some(TargetLevel::Sub(_))
    ) {
        return;
    }
    for (interaction, row) in &rows {
        if *interaction == Interaction::Pressed && row.slot < limit {
            out.write(TargetActionActivated { slot: row.slot });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::action_model::{
        build_target_action_entries, TargetActionContext, TargetActionId, TargetKindLite,
    };
    use crate::hud::overlay::RETAIL;
    use crate::input_mode::TargetActionState;

    fn pc_ctx(in_range: bool) -> TargetActionContext {
        TargetActionContext {
            has_target: true,
            target_kind: TargetKindLite::Pc,
            in_range,
            trusts_available: false,
            engaged: false,
        }
    }

    #[test]
    fn closed_mode_yields_no_entries() {
        let overlay = ActiveOverlay(&RETAIL);
        assert!(entries_for_mode(&InputMode::World, &overlay).is_none());
        assert_eq!(entry_count(&InputMode::World, &overlay), 0);
    }

    #[test]
    fn open_mode_yields_contextual_entries() {
        let overlay = ActiveOverlay(&RETAIL);
        let mode = InputMode::TargetAction(TargetActionState::open(pc_ctx(true)));
        let entries = entries_for_mode(&mode, &overlay).expect("menu open");
        assert!(!entries.is_empty());

        assert!(entries
            .iter()
            .any(|e| e.id == TargetActionId::Chat && e.enabled));
    }

    #[test]
    fn out_of_range_pc_disables_chat_with_hint() {
        let overlay = ActiveOverlay(&RETAIL);
        let mode = InputMode::TargetAction(TargetActionState::open(pc_ctx(false)));
        let entries = entries_for_mode(&mode, &overlay).expect("menu open");
        let chat = entries
            .iter()
            .find(|e| e.id == TargetActionId::Chat)
            .expect("chat present");
        assert!(!chat.enabled);
        assert!(chat.hint.is_some());
    }

    #[test]
    fn select_entry_renders_label_with_submenu_arrow() {
        let overlay = ActiveOverlay(&RETAIL);
        let entries = build_target_action_entries(&pc_ctx(true), overlay.0);
        let chat = entries
            .iter()
            .find(|e| e.id == TargetActionId::Chat)
            .expect("chat present");
        let text = entry_text(chat);
        assert_eq!(text, format!("Chat {SUBMENU_ARROW}"));
        assert!(!text.contains(':'), "mode must not be inlined: {text}");
    }

    #[test]
    fn plain_entry_renders_bare_label() {
        let overlay = ActiveOverlay(&RETAIL);
        let entries = build_target_action_entries(&pc_ctx(true), overlay.0);
        let items = entries
            .iter()
            .find(|e| e.id == TargetActionId::Items)
            .expect("items present");
        assert_eq!(entry_text(items), "Items");
    }

    #[test]
    fn hint_renders_as_suffix() {
        let overlay = ActiveOverlay(&RETAIL);
        let entries = build_target_action_entries(&pc_ctx(false), overlay.0);
        let chat = entries
            .iter()
            .find(|e| e.id == TargetActionId::Chat)
            .expect("chat present");
        let text = entry_text(chat);
        assert!(text.contains("(Target out of range.)"));
    }

    #[test]
    fn breadcrumb_only_for_active_sub_frame() {
        assert!(breadcrumb_text(None).is_none());
        let crumb = breadcrumb_text(Some(SubAction::Items)).expect("crumb");
        assert!(crumb.contains("Items"));
    }
}
