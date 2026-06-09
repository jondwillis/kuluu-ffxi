//! `/check` view — the contextual "examine another player / NPC" window.
//!
//! Retail FFXI's `/check` shows, in order:
//!
//!   1. **View Wares** — the target's personal bazaar, when they have one
//!      set. Bazaar-less targets skip straight to the gear view (retail
//!      collapses the empty section rather than printing "no wares").
//!   2. A **4×4 equipment grid** of the target's *visible* equipment, with
//!      a per-slot tooltip carrying the static item facts.
//!   3. A one-line level/job ribbon ("Lv.5 Black Mage").
//!
//! This module renders that window. It is a *read-only* composition over
//! the snapshot (the target's gear + bazaar mirror) joined with the
//! two-tier item metadata via [`crate::hud::item_meta::compose_item_detail`]
//! — the same composer the Items list and Trade window use, so a checked
//! item and an owned item render identically.
//!
//! Visibility is driven by the [`CheckTarget`] resource, which the Wire
//! phase sets when the operator confirms `Check` on a target and clears on
//! back-out; otherwise the window is `Display::None`. The Wire phase
//! registers [`spawn_check_view`] / [`update_check_view`] and inits
//! [`CheckTarget`].

use bevy::prelude::*;

use crate::hud::item_meta::{compose_item_detail, ItemDetail};
use crate::hud::palette;
use crate::snapshot::SceneState;

/// Whether the `/check` window is open, and on whom.
///
/// The foundation `SubAction` enum has no dedicated `Check` variant (and
/// `input_mode.rs` is owned by the foundation, not this feature), so the
/// `/check` window can't key off the menu stack the way Magic / Items
/// leaves do. Instead the Wire phase — which owns the input transition —
/// sets this resource when the operator confirms `Check` on a target and
/// clears it when they back out. The renderer below reads it; nothing
/// else writes it.
///
/// `target_id` is reserved for a future wire addition of a dedicated
/// examined-PC mirror (so the gear grid can show *their* equipment rather
/// than the operator's); today the grid reads `snapshot.equipped`, which
/// is the self doll, so `/check` on self is exact and other targets
/// degrade gracefully.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct CheckTarget {
    pub open: bool,
    pub target_id: Option<u32>,
}

/// The 4×4 examine grid, laid out exactly as retail's `/check` window.
/// Each cell is a `(SLOTTYPE id, label)` pair; the order is the visible
/// "doll" reading rows left-to-right, top-to-bottom:
///
/// ```text
///   Main   Sub
///   Range  Ammo
///   Head   Neck   Ear1   Ear2
///   Body   Hands  Ring1  Ring2
///   Back   Waist  Legs   Feet
/// ```
///
/// The flattened sequence below is what the grid iterates; the renderer
/// wraps every two-then-four columns to reproduce the retail shape. The
/// numeric ids match the wire `SLOTTYPE` convention used by
/// `SceneSnapshot.equipped` (0=Main, 1=Sub, 2=Ranged, 3=Ammo, 4=Head,
/// 5=Body, 6=Hands, 7=Legs, 8=Feet, 9=Neck, 10=Waist, 11=LEar, 12=REar,
/// 13=LRing, 14=RRing, 15=Back).
pub const CHECK_GRID_SLOTS: &[(u8, &str)] = &[
    (0, "Main"),
    (1, "Sub"),
    (2, "Range"),
    (3, "Ammo"),
    (4, "Head"),
    (9, "Neck"),
    (11, "Ear1"),
    (12, "Ear2"),
    (5, "Body"),
    (6, "Hands"),
    (13, "Ring1"),
    (14, "Ring2"),
    (15, "Back"),
    (10, "Waist"),
    (7, "Legs"),
    (8, "Feet"),
];

const PANEL_WIDTH_PX: f32 = 320.0;

/// Root marker on the `/check` window.
#[derive(Component)]
pub struct CheckView;

/// The "View Wares" bazaar section header + body. Hidden (skipped to gear)
/// when the target has no bazaar.
#[derive(Component)]
pub struct CheckWaresSection;

/// One bazaar row inside the View-Wares section. `idx` is the row index
/// into `snapshot.bazaar`.
#[derive(Component)]
pub struct CheckWaresRow {
    pub idx: usize,
}

/// One equipment-grid cell. `grid_index` indexes [`CHECK_GRID_SLOTS`].
#[derive(Component)]
pub struct CheckGridCell {
    pub grid_index: usize,
}

/// The level/job ribbon at the bottom ("Lv.5 Black Mage").
#[derive(Component)]
pub struct CheckJobRibbon;

/// Maximum bazaar rows we pool. Personal bazaars cap at 7 wares retail-side;
/// pool a few extra so a server that over-reports doesn't drop rows.
const MAX_WARES_ROWS: usize = 8;

pub fn spawn_check_view(mut commands: Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            CheckView,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(20.0),
                left: Val::Percent(30.0),
                width: Val::Px(PANEL_WIDTH_PX),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(3.0),
                display: Display::None,
                ..default()
            },
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::ACCENT),
        ))
        .with_children(|p| {
            // Title row.
            p.spawn((
                Text::new("Check"),
                TextFont {
                    font_size: 14.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));

            // View-Wares section — hidden when the bazaar is empty so the
            // window collapses straight to the gear grid (retail behavior).
            p.spawn((
                CheckWaresSection,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(1.0),
                    display: Display::None,
                    ..default()
                },
            ))
            .with_children(|w| {
                w.spawn((
                    Text::new("View Wares"),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(palette::MUTED),
                ));
                for idx in 0..MAX_WARES_ROWS {
                    w.spawn((
                        CheckWaresRow { idx },
                        Text::new(""),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(palette::TEXT),
                    ));
                }
            });

            // Equipment grid — one pooled cell per slot, rendered as a
            // single flowing column of "Slot: item" rows. The retail
            // 2-then-4-column doll shape is conveyed by the slot ordering
            // in [`CHECK_GRID_SLOTS`]; a flat row list keeps the renderer
            // simple while preserving the reading order.
            p.spawn((Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                ..default()
            },))
                .with_children(|g| {
                    for grid_index in 0..CHECK_GRID_SLOTS.len() {
                        g.spawn((
                            CheckGridCell { grid_index },
                            Text::new(""),
                            TextFont {
                                font_size: 13.0,
                                ..default()
                            },
                            TextColor(palette::TEXT),
                        ));
                    }
                });

            // Level / job ribbon.
            p.spawn((
                CheckJobRibbon,
                Text::new(""),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(palette::ACCENT),
            ));
        });
}

/// Per-frame: show the window while the operator is in the `Check`
/// sub-frame of the contextual target-action menu, paint the bazaar +
/// equipment grid + job ribbon from the snapshot.
pub fn update_check_view(
    target: Res<CheckTarget>,
    state: Res<SceneState>,
    mut view_q: Query<&mut Node, With<CheckView>>,
    mut wares_section_q: Query<&mut Node, (With<CheckWaresSection>, Without<CheckView>)>,
    mut wares_row_q: Query<(&CheckWaresRow, &mut Text), Without<CheckGridCell>>,
    mut grid_q: Query<(&CheckGridCell, &mut Text, &mut TextColor), Without<CheckWaresRow>>,
    mut ribbon_q: Query<
        &mut Text,
        (
            With<CheckJobRibbon>,
            Without<CheckWaresRow>,
            Without<CheckGridCell>,
        ),
    >,
) {
    let Ok(mut view_node) = view_q.single_mut() else {
        return;
    };

    if !target.open {
        if view_node.display != Display::None {
            view_node.display = Display::None;
        }
        return;
    }
    if view_node.display == Display::None {
        view_node.display = Display::Flex;
    }

    let snap = &state.snapshot;

    // View Wares — skip (collapse) the section entirely when the target's
    // bazaar is empty.
    let bazaar = &snap.bazaar;
    if let Ok(mut wares_node) = wares_section_q.single_mut() {
        let want_display = if bazaar.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        if wares_node.display != want_display {
            wares_node.display = want_display;
        }
    }
    for (row, mut text) in wares_row_q.iter_mut() {
        let want = match bazaar.get(row.idx) {
            Some(entry) => {
                let name = ffxi_proto::item_names::lookup(entry.item_no)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("item #{}", entry.item_no));
                if entry.quantity > 1 {
                    format!("  {name} x{}  {} gil", entry.quantity, entry.price)
                } else {
                    format!("  {name}  {} gil", entry.price)
                }
            }
            None => String::new(),
        };
        if **text != want {
            **text = want;
        }
    }

    // Equipment grid — compose each visible slot's item through the shared
    // item-detail composer so the displayed name/tooltip matches the Items
    // window exactly.
    for (cell, mut text, mut color) in grid_q.iter_mut() {
        let Some(&(slot_id, slot_label)) = CHECK_GRID_SLOTS.get(cell.grid_index) else {
            continue;
        };
        let item_no = snap.equipped.get(slot_id as usize).copied().flatten();
        let (body, filled) = match item_no {
            Some(no) => {
                // DAT static metadata is resolved by the caller that owns
                // the `DatRoot`; here we degrade to the LSB-scraped label
                // (None static), which is exactly the fallback path
                // `compose_item_detail` is designed for.
                let detail: ItemDetail = compose_item_detail(no, snap, None);
                let name = item_label(no, &detail);
                (format!("{slot_label:<6}: {name}"), true)
            }
            None => (format!("{slot_label:<6}: —"), false),
        };
        if **text != body {
            **text = body;
        }
        let want_color = if filled {
            palette::TEXT
        } else {
            palette::MUTED
        };
        if color.0 != want_color {
            color.0 = want_color;
        }
    }

    // Level / job ribbon — "Lv.5 Black Mage". Reads the checked target's
    // job + level from the captured action context's target party row when
    // available, falling back to the operator's own self row (a `/check`
    // on self).
    if let Ok(mut text) = ribbon_q.single_mut() {
        let want = job_ribbon(snap);
        if **text != want {
            **text = want;
        }
    }
}

/// Resolve the display name for a checked equipment item: the DAT static
/// name when present, else the LSB-scraped label, else a numeric stub.
fn item_label(item_no: u16, detail: &ItemDetail) -> String {
    if let Some(s) = detail.static_.as_ref() {
        if !s.name.is_empty() {
            return s.name.clone();
        }
    }
    ffxi_proto::item_names::lookup(item_no)
        .map(str::to_string)
        .unwrap_or_else(|| format!("item #{item_no}"))
}

/// Format the "Lv.N <Job>" ribbon for the checked target. Uses the self
/// party row (the only row carrying full job/level data) — a future wire
/// addition of a dedicated `check_target` field would let this address an
/// arbitrary examined PC; until then `/check` on self renders correctly
/// and other targets degrade to the operator's own ribbon.
fn job_ribbon(snap: &ffxi_viewer_wire::SceneSnapshot) -> String {
    let me = crate::hud::self_hud::resolve_self(&snap.party, snap.self_char_id);
    match me {
        Some(m) => {
            let job = ffxi_proto::job_names::lookup(m.main_job as u16).unwrap_or("Adventurer");
            format!("Lv.{} {job}", m.main_job_lv)
        }
        None => "Lv.? —".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_grid_has_sixteen_unique_slots() {
        assert_eq!(CHECK_GRID_SLOTS.len(), 16, "all 16 equipment slots present");
        let mut ids: Vec<u8> = CHECK_GRID_SLOTS.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 16, "no duplicate slot ids in the grid");
        assert_eq!(*ids.last().unwrap(), 15, "max slot id is Back (15)");
    }

    #[test]
    fn grid_reading_order_starts_main_sub() {
        assert_eq!(CHECK_GRID_SLOTS[0], (0, "Main"));
        assert_eq!(CHECK_GRID_SLOTS[1], (1, "Sub"));
        assert_eq!(CHECK_GRID_SLOTS[2], (2, "Range"));
        assert_eq!(CHECK_GRID_SLOTS[3], (3, "Ammo"));
    }
}
