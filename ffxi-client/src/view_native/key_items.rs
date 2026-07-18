//! Key Items menu mark-seen tracking: rows the cursor visits while the menu is
//! open accumulate here, and closing the menu flushes them as one
//! [`AgentCommand::MarkKeyItemsSeen`] per 512-id table (c2s 0x064; the exact
//! retail send moment is unverified, bead kuluu-h7x retail_unknowns).

use bevy::prelude::*;
use ffxi_viewer_core::hud::menu::{entry_action, DynamicMenu, DynamicMenuAction};
use ffxi_viewer_core::input_mode::{InputMode, MenuKind};
use ffxi_viewer_core::snapshot::SceneState;

use super::input::CommandTx;
use crate::state::{AgentCommand, KEY_ITEMS_PER_TABLE};

#[derive(Resource, Default)]
pub struct KeyItemsViewed(pub std::collections::BTreeSet<u16>);

pub fn key_items_mark_seen_system(
    mode: Res<InputMode>,
    dynamic: Res<DynamicMenu>,
    scene: Res<SceneState>,
    cmd_tx: Res<CommandTx>,
    mut viewed: ResMut<KeyItemsViewed>,
) {
    let cursor_in_key_items = match &*mode {
        InputMode::Menu(stack) => stack
            .current()
            .filter(|l| l.kind == MenuKind::KeyItems)
            .map(|l| l.cursor),
        _ => None,
    };
    match cursor_in_key_items {
        Some(cursor) => {
            if let Some(DynamicMenuAction::KeyItem { id }) =
                entry_action(MenuKind::KeyItems, cursor, &dynamic)
            {
                if scene.snapshot.key_items_seen.binary_search(&id).is_err() {
                    viewed.0.insert(id);
                }
            }
        }
        None => {
            if viewed.0.is_empty() {
                return;
            }
            let ids = std::mem::take(&mut viewed.0);
            let mut by_table: std::collections::BTreeMap<u16, Vec<u16>> = Default::default();
            for id in ids {
                if scene.snapshot.key_items_seen.binary_search(&id).is_ok() {
                    continue;
                }
                let table_index = (id as usize / KEY_ITEMS_PER_TABLE) as u16;
                by_table.entry(table_index).or_default().push(id);
            }
            for (table_index, ids) in by_table {
                let _ = cmd_tx
                    .0
                    .try_send(AgentCommand::MarkKeyItemsSeen { table_index, ids });
            }
        }
    }
}

/// A menu left open across logout must not flush into the next session's
/// channel (the ids belong to the previous character).
pub fn drain_key_items_viewed(mut viewed: ResMut<KeyItemsViewed>) {
    viewed.0.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_partition_at_512_id_boundaries() {
        let ids = [5u16, 511, 512, 1024];
        let tables: Vec<u16> = ids
            .iter()
            .map(|&id| (id as usize / KEY_ITEMS_PER_TABLE) as u16)
            .collect();
        assert_eq!(tables, [0, 0, 1, 2]);
    }
}
