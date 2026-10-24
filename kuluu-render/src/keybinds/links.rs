//! Retail re-binds whole **link groups**, not single actions: the Controls page's apply path (FFXiMain.dll
//! retail-2026-09 `.text 0x19CFA0`) writes the freshly pressed key to the row's own action *and* to every
//! further action in its `u16 linked[4]` column (`.rdata 0x37A290`; row law and the measured groups live in
//! [`crate::hud::controls_law`] — Confirm row `0x0E` links `0x42, 0x8F, 0x91` plus menu-mode confirm
//! `0x93`). So one visible row owns several actions, and a remake that re-binds them independently drifts:
//! the menu would keep answering an old key.
//!
//! The same law is expressed here over kuluu's own action names so every preset can be checked against it.

use super::Action;

/// One group of kuluu actions that retail keeps on a single key, as `[primary, linked…]`.
pub type LinkGroup = &'static [Action];

/// Groups verified against the row table's link column:
/// * confirm — the game-wide accept action and its menu-mode twin (retail `0x0E` + `0x93`).
///
/// The cancel family is deliberately absent: retail links four ids (`0x90`, `0x92`, `0x95` plus menu-cancel
/// `0x94`) and which kuluu actions those are has not been verified — the chat-exit line in particular could
/// be dialog-dismiss rather than a cancel sibling. Guessing a group here would let an unverified mapping
/// become load-bearing, so it stays unwritten until each id is read out of its handler.
pub const LINK_GROUPS: [LinkGroup; 1] = [&[Action::ConfirmAction, Action::NavConfirm]];

/// Every action `action` must share a key with, itself included — empty when it is not in any verified group.
pub fn link_group_of(action: Action) -> &'static [Action] {
    LINK_GROUPS
        .iter()
        .find(|group| group.contains(&action))
        .copied()
        .unwrap_or(&[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybinds::presets::Preset;

    /// Retail writes one key across the whole group, so no preset may put a group's members on different
    /// keys — or bind only some of them.
    #[test]
    fn every_preset_keeps_each_link_group_on_one_key() {
        for preset in [Preset::Standard, Preset::Compact1, Preset::Compact2] {
            let bindings = preset.bindings();
            for group in LINK_GROUPS.iter() {
                let labels: Vec<(&Action, Option<String>)> = group
                    .iter()
                    .map(|action| (action, bindings.get(*action).map(|b| format!("{b:?}"))))
                    .collect();
                let first = labels[0].1.clone();
                for (action, label) in &labels {
                    assert_eq!(
                        label, &first,
                        "{preset:?} splits the link group member {action:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_group_member_answers_with_the_whole_group() {
        assert_eq!(
            link_group_of(Action::NavConfirm),
            &[Action::ConfirmAction, Action::NavConfirm][..]
        );
        // Unverified families stay out of the law rather than guessing at them.
        assert!(link_group_of(Action::NavCancel).is_empty());
    }
}
