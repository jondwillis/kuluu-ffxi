//! Debug "Entity List" overlay (Debug menu row, default off): a scrollable
//! dump of every live wire entity from the [`EntityTable`] — id, name, kind,
//! position, STATUS_TYPE name, hp%, claim state (ClaimedSelf / ClaimedOther),
//! the nameplate status markers the plate would draw for that entity, and the
//! invis/name-hidden/dead flags. The point is to
//! eyeball what the packet system actually believes about each entity (worm-
//! blink class: does `status` flip INVISIBLE when it dives?; icon class: do
//! the char-flag bits that drive a nameplate marker arrive at all?) without
//! touching any game state — read-only over the table.
//!
//! The current target's row carries a rectangle while one is set, so the
//! targeted entity can be found in the list without scanning names.
//!
//! Rows are fixed slots paged by a row offset; the mouse wheel scrolls while
//! the panel is on (in-game the wheel drives nothing else, and this matches
//! the launcher's global-while-visible scroll precedent). Refreshed at 5 Hz,
//! not per frame — it is a debug readout, not a live tracker.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use kuluu_render::hud::{style::theme, HudPanels};
use kuluu_render::{nameplate_marker, EntityTable, InGameEntity, Target};
use kuluu_snapshot::EntityKind;

/// How many row slots the panel shows at once (the scroll window).
const VISIBLE_ROWS: usize = 16;
/// Refresh cadence for the text content.
const REFRESH_INTERVAL: f32 = 0.2;
const ROW_FONT: f32 = 12.0;
/// Approximate rendered row height in px, used to convert pixel-unit wheel
/// deltas into rows.
const ROW_H_PX: f32 = 15.0;
const NAME_WIDTH: usize = 12;

#[derive(Component)]
pub struct EntityListPanel;

#[derive(Component)]
pub struct EntityListHeader;

#[derive(Component)]
pub struct EntityListRow(pub usize);

/// Scroll offset in rows for the open panel. Reset to 0 when the panel is
/// toggled on; clamped against the live entity count by the update system.
#[derive(Resource, Default)]
pub struct EntityListScroll {
    pub rows: i32,
}

pub fn spawn_entity_list_hud(mut commands: Commands) {
    commands
        .spawn((
            InGameEntity,
            EntityListPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(300.0),
                left: Val::Px(8.0),
                // Wide enough for the claim + status-name columns at FiraMono
                // 12px without clipping FLAGS (the old 600px clipped it).
                width: Val::Px(750.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(2.0),
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
            GlobalZIndex(10),
            Visibility::Hidden,
        ))
        .with_children(|p| {
            p.spawn((
                EntityListHeader,
                Text::new(""),
                TextFont {
                    font_size: ROW_FONT.into(),
                    ..default()
                },
                TextColor(theme::TITLE),
                TextLayout::no_wrap(),
            ));

            // Clipped scroll window: fixed row slots inside; the update system
            // pages them by offset instead of spawning/despawning per entity.
            p.spawn(Node {
                width: Val::Percent(100.0),
                height: Val::Px(VISIBLE_ROWS as f32 * ROW_H_PX + 4.0),
                overflow: Overflow::clip(),
                flex_direction: FlexDirection::Column,
                ..default()
            })
            .with_children(|box_| {
                // Column header (static; aligned with the row format below).
                box_.spawn((
                    Text::new(column_header()),
                    TextFont {
                        font_size: ROW_FONT.into(),
                        ..default()
                    },
                    TextColor(theme::MUTED),
                    TextLayout::no_wrap(),
                ));
                for i in 0..VISIBLE_ROWS {
                    box_.spawn((
                        EntityListRow(i),
                        Text::new(""),
                        TextFont {
                            font_size: ROW_FONT.into(),
                            ..default()
                        },
                        TextColor(theme::TEXT),
                        TextLayout::no_wrap(),
                        // The target rectangle toggles border width per row;
                        // a 0-width transparent border is the resting state.
                        Node {
                            display: Display::None,
                            border: UiRect::all(Val::Px(0.0)),
                            ..default()
                        },
                        // Color::NONE (not a transparent srgba): the resting
                        // state draws no border at all.
                        BorderColor::all(Color::NONE),
                    ));
                }
            });
        });
}

pub fn apply_entity_list_visibility(
    panels: Res<HudPanels>,
    mut scroll: ResMut<EntityListScroll>,
    mut q: Query<&mut Visibility, With<EntityListPanel>>,
) {
    if !panels.is_changed() {
        return;
    }
    let want = if panels.entity_list {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    // A fresh open starts at the top of the list.
    if panels.entity_list && scroll.rows != 0 {
        scroll.rows = 0;
    }
    for mut v in q.iter_mut() {
        if *v != want {
            *v = want;
        }
    }
}

/// Mouse-wheel scrolling while the panel is on. No hover gate: in-game the
/// wheel drives nothing else (zoom is PgUp/PgDown), and the launcher's scroll
/// precedent scrolls visible regions globally too. The upper clamp happens in
/// the update system, which knows the live entity count.
pub fn entity_list_wheel_system(
    panels: Res<HudPanels>,
    mut wheel: MessageReader<MouseWheel>,
    mut scroll: ResMut<EntityListScroll>,
) {
    if !panels.entity_list {
        return;
    }
    let mut delta_rows = 0.0;
    for ev in wheel.read() {
        delta_rows += match ev.unit {
            MouseScrollUnit::Line => ev.y,
            MouseScrollUnit::Pixel => ev.y / ROW_H_PX,
        };
    }
    if delta_rows == 0.0 {
        return;
    }
    scroll.rows = (scroll.rows - delta_rows.round() as i32).max(0);
}

pub fn update_entity_list_hud(
    panels: Res<HudPanels>,
    time: Res<Time>,
    table: Res<EntityTable>,
    target: Res<Target>,
    mut scroll: ResMut<EntityListScroll>,
    mut refresh: Local<f32>,
    // Without keeps the two `&mut Text` queries provably disjoint (B0001):
    // the header row never carries EntityListRow.
    mut header_q: Query<&mut Text, (With<EntityListHeader>, Without<EntityListRow>)>,
    mut row_q: Query<(
        &EntityListRow,
        &mut Text,
        &mut TextColor,
        &mut Node,
        &mut BorderColor,
    )>,
) {
    if !panels.entity_list {
        return;
    }
    *refresh += time.delta_secs();
    if *refresh < REFRESH_INTERVAL {
        return;
    }
    *refresh = 0.0;

    // Id-sorted: deterministic order so a given entity stays on the same row
    // while it is alive, which makes watching its status byte flip readable.
    let mut ents: Vec<_> = table.iter().collect();
    ents.sort_by_key(|r| r.entity.id);
    let n = ents.len();

    scroll.rows = scroll
        .rows
        .clamp(0, (n.saturating_sub(VISIBLE_ROWS)) as i32);
    let start = scroll.rows as usize;
    let end = (start + VISIBLE_ROWS).min(n);

    for mut header in header_q.iter_mut() {
        let text = format!(
            "ENTITY LIST  n={n}   rows {}-{} of {n}   wheel scrolls",
            start + 1,
            end.max(start + 1)
        );
        if **header != text {
            **header = text;
        }
    }

    let self_id = table.self_id();
    // The target rectangle: the row slot holding the targeted entity, if it is
    // on the visible page. A stale id (target despawned) matches nothing.
    let target_row = target
        .id
        .and_then(|id| ents.iter().position(|r| r.entity.id == id));
    for (row, mut text, mut color, mut node, mut border_color) in row_q.iter_mut() {
        match ents.get(start + row.0) {
            Some(rec) => {
                let e = &rec.entity;
                let is_self = self_id == Some(e.id);
                let markers = marker_labels(e);
                // Claim state, live off the table record: it flips on every
                // UPDATE_STATUS 0x0E and this panel re-reads at 5 Hz, so a flip
                // shows up within one refresh — no extra tracking needed.
                let claim_label = if e.claim_id == 0 {
                    "-"
                } else if self_id.is_some_and(|s| s == e.claim_id) {
                    "ClaimedSelf"
                } else {
                    "ClaimedOther"
                };
                let line = format_row(
                    is_self,
                    e.id,
                    e.name.as_deref(),
                    e.kind,
                    e.pos.x,
                    e.pos.y,
                    e.pos.z,
                    e.status,
                    e.hp_pct,
                    claim_label,
                    &markers,
                    rec.is_invisible(),
                    rec.name_hidden(),
                    rec.is_dead(),
                );
                if **text != line {
                    **text = line;
                }
                color.0 = row_color(is_self, rec.is_invisible(), rec.is_dead());
                node.display = Display::Flex;
                let is_target = target_row == Some(row.0);
                node.border = UiRect::all(Val::Px(if is_target { 1.0 } else { 0.0 }));
                *border_color = BorderColor::all(if is_target {
                    theme::DANGER
                } else {
                    Color::NONE
                });
            }
            None => {
                if node.display != Display::None {
                    node.display = Display::None;
                    **text = String::new();
                    node.border = UiRect::all(Val::Px(0.0));
                    *border_color = BorderColor::all(Color::NONE);
                }
            }
        }
    }
}

fn column_header() -> String {
    // Plain padding (no `08X`): the row format's hex spec is integer-only, and
    // this header carries labels, not values.
    format!(
        "  {id:>8}  {name:<12} {kind:<4} {pos:<12}  {st_name:<13} {hp:>3}  {claim:<13} {status:<7} FLAGS",
        id = "ID",
        name = "NAME",
        kind = "KIND",
        pos = "X,Y,Z",
        st_name = "STATUS",
        hp = "HP",
        claim = "CLAIM",
        status = "MARKERS"
    )
}

/// The STATUS_TYPE byte as a name (LSB `STATUS_TYPE`, baseentity.h). A raw 1
/// reads as "missing" at a glance; an unknown byte reports itself so a new
/// server state is visible instead of hidden.
fn status_name(status: u8) -> String {
    use kuluu_snapshot::status_type as st;
    match status {
        0 => "NORMAL".into(),
        1 => "UPDATE".into(),
        st::DISAPPEAR => "DISAPPEAR".into(),
        st::INVISIBLE => "INVISIBLE".into(),
        st::STATUS_4 => "S4".into(),
        st::CUTSCENE_ONLY => "CUTSCENE_ONLY".into(),
        st::STATUS_18 => "S18".into(),
        st::SHUTDOWN => "SHUTDOWN".into(),
        _ => format!("?{status}"),
    }
}

fn format_row(
    is_self: bool,
    id: u32,
    name: Option<&str>,
    kind: EntityKind,
    x: f32,
    y: f32,
    z: f32,
    status: u8,
    hp_pct: Option<u8>,
    claim_label: &str,
    markers: &str,
    invisible: bool,
    name_hidden: bool,
    dead: bool,
) -> String {
    let flags = [
        invisible.then_some("INV"),
        name_hidden.then_some("HNAME"),
        dead.then_some("DEAD"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ");
    // X,Y,Z flattened into one column to leave room for ST/HP/STATUS/FLAGS on
    // the right (the three separate columns clipped FLAGS at 500px panel
    // width). Whole yalms: this is a where-is-it readout, not a survey.
    let pos = format!("{x:.0},{y:.0},{z:.0}");
    let st_name = status_name(status);
    format!(
        "{} {id:08X}  {} {:<4} {pos:<12}  {st_name:<13} {}  {claim_label:<13} {markers:<7} {}",
        if is_self { "*" } else { " " },
        truncate_pad(name.unwrap_or("?"), NAME_WIDTH),
        kind_label(kind),
        hp_str(hp_pct),
        if flags.is_empty() { "-" } else { &flags }
    )
}

/// The nameplate status markers this entity's plate would draw, as short
/// labels — the same priority-resolved list `nameplate_markers` feeds the
/// billboard raster. Empty for kinds that never carry icons (mobs/pets) and
/// for flag-less players; a `-` there means "no marker-driving state arrived",
/// which is exactly what the icon debugging needs to see.
fn marker_labels(e: &kuluu_snapshot::Entity) -> String {
    let labels = nameplate_marker::nameplate_markers(e)
        .into_iter()
        .filter_map(|code| label_for_glyph(code))
        .collect::<Vec<_>>()
        .join(" ");
    if labels.is_empty() {
        "-".to_string()
    } else {
        labels
    }
}

fn label_for_glyph(code: u8) -> Option<&'static str> {
    use nameplate_marker::glyph as g;
    match code {
        g::PLAY_ONLINE => Some("POL"),
        g::LINKDEAD => Some("LD"),
        g::AWAY => Some("AWY"),
        g::SEEKING => Some("LFG"),
        g::LINKSHELL => Some("LS"),
        g::GM_1 | g::GM_4 | g::GM_5 | g::GM_6 | g::GM_7 | g::GM_8 => Some("GM"),
        g::BAZAAR => Some("BAZ"),
        g::AUTO_PARTY => Some("AP"),
        g::NATION_SAN_DORIA
        | g::NATION_BASTOK
        | g::NATION_WINDURST
        | g::NATION_BEAUFORT
        | g::NATION_RABANASTRE => Some("NAT"),
        g::NEW_PLAYER => Some("?"),
        g::BESIEGED_EVEN | g::BESIEGED_ODD => Some("BSG"),
        g::MONSTROSITY => Some("MON"),
        g::JOB_MASTER => Some("JM"),
        // The half-scale companion of JOB_MASTER: no label of its own.
        g::JOB_MASTER_TAIL => None,
        _ => None,
    }
}

fn row_color(is_self: bool, invisible: bool, dead: bool) -> Color {
    if is_self {
        theme::CURSOR
    } else if invisible {
        theme::DANGER
    } else if dead {
        theme::MUTED
    } else {
        theme::TEXT
    }
}

fn hp_str(hp_pct: Option<u8>) -> String {
    match hp_pct {
        Some(p) => format!("{p:>3}"),
        None => "--".to_string(),
    }
}

fn kind_label(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Pc => "PC",
        EntityKind::Npc => "NPC",
        EntityKind::Mob => "MOB",
        EntityKind::Pet => "PET",
        EntityKind::Other => "OTH",
    }
}

fn truncate_pad(s: &str, width: usize) -> String {
    // ASCII ellipsis: the rows rasterize with Bevy's bundled FiraMono subset,
    // which turns any non-ASCII glyph into a tofu box.
    let count = s.chars().count();
    if count > width {
        let mut out: String = s.chars().take(width.saturating_sub(3)).collect();
        out.push_str("...");
        out
    } else {
        format!("{s:<width$}")
    }
}
