//! XIUI-style party/alliance frame — the single party/self HUD.
//!
//! Self is row 0 of Party A (XIUI behavior), so there is exactly ONE panel
//! drawing player/party state. This module owns the ROSTER column slot.
//!
//! Geometry:
//! - Party A = L1 "compact vertical": no name row — the name overlays the HP
//!   bar's top edge, bars start right of a fixed job-icon slot, MP bar is
//!   right-aligned under the HP bar.
//! - Alliance B/C = L2 "super compact": one text row [name … HP value] that
//!   dips into the HP bar's top; entry box wider than the bars with the bars
//!   right-aligned in the box; MP bar tucked up under the HP bar.
//!
//! Runtime knobs live in `PartyFrameSettings`, edited only from the Debug
//! menu "UI Settings" panel. No user-facing settings UI.
//!
//! Data: SceneSnapshot.party (GROUP_LIST 0x0DD / GROUP_ATTR 0x0DF; see
//! vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp), Res<Target>,
//! NameColorTable, ZoneNameResolver.

use bevy::prelude::*;

use crate::hud::panel_column::ColumnPanel;
use crate::hud::status_panel::job_abbrev;
use crate::hud::style::{self, theme};
use crate::hud::HudPanels;
use crate::nameplate_color::{ncol, NameColorTable};
use crate::scene::Target;
use crate::snapshot::SceneState;

/// XIUI PARTY_BAR_BASE_WIDTH_MULT: applied to every template width.
const BASE_MULT: f32 = 0.8;

// L1 — Party A "compact vertical" (display.lua layout 1).
const L1_HP_BASE_W: f32 = 150.0;
const L1_MP_BASE_W: f32 = 100.0;
const L1_BAR_H: f32 = 20.0;
const L1_ICON_SIZE: f32 = 28.0;
const L1_BAR_INSET: f32 = 4.0;
// display.lua HX_BAR_WIDTH_MULT: HP/MP bar length in layout 1 only.
const L1_HP_W_MULT: f32 = 0.82;
// display.lua: the MP bar takes an extra 10% trim on top of HX_BAR_WIDTH_MULT.
const L1_MP_EXTRA_W_MULT: f32 = 0.9;

// L2 — Alliance B/C "super compact" (display.lua layout 2).
const L2_HP_BASE_W: f32 = 135.0;
const L2_MP_BASE_W: f32 = 80.0;
const L2_BAR_H: f32 = 12.0;
/// Entry box width; wider than the bars, which sit right-aligned in it.
const L2_ENTRY_W: f32 = 160.0;
/// Text row dips this many px into the HP bar top.
const L2_NAME_BAR_OVERLAP: f32 = 3.0;
/// MP bar shifted up under the HP bar by this many px.
const L2_MP_OVERLAP: f32 = 2.0;

const NAME_PX: f32 = 12.0;
const JOB_PX: f32 = 10.0;
const TITLE_PX: f32 = 14.0;

const MP_COLOR: Color = Color::srgb(0.30, 0.50, 0.90);
const TP_FULL: Color = Color::srgb(1.00, 0.80, 0.20);
const TP_DIM: Color = Color::srgb(0.55, 0.55, 0.55);
const OUT_OF_ZONE_BLOCK: Color = Color::srgb(0.02, 0.02, 0.02);
const LEADER_DOT: Color = Color::srgb(1.00, 0.82, 0.25);
const TARGET_BG: Color = Color::srgba(0.35, 0.55, 0.95, 0.30);
const TARGET_BORDER: Color = Color::srgb(0.65, 0.80, 1.00);
/// Reserved for the subtarget highlight: Res<Target> carries one id until a
/// second slot lands.
#[allow(dead_code)]
const SUBTARGET_BG: Color = Color::srgba(0.95, 0.80, 0.25, 0.30);
#[allow(dead_code)]
const SUBTARGET_BORDER: Color = Color::srgb(1.00, 0.87, 0.40);
const BAND_COLOR: Color = Color::srgba(0.35, 0.06, 0.06, 0.55);
const TREASURE_FLAG: Color = Color::srgb(1.00, 0.84, 0.00);
// Activity-flag marker colors (XIUI display.lua DrawCurrentTarget block).
const FLAG_DC: Color = Color::srgb(0.60, 0.60, 0.60);
const FLAG_GM: Color = Color::srgb(1.00, 0.20, 0.20);
const FLAG_MENTOR: Color = Color::srgb(1.00, 0.85, 0.30);
const FLAG_NEW: Color = Color::srgb(0.40, 0.90, 0.40);
const FLAG_AWAY: Color = Color::srgb(0.55, 0.55, 0.55);
const FLAG_LFP: Color = Color::srgb(0.45, 0.70, 1.00);
const FLAG_BAZAAR: Color = Color::srgb(1.00, 0.80, 0.20);

#[derive(Resource, Clone)]
pub struct PartyFrameSettings {
    /// Per-window layout override: 0 = default (A=L1, B/C=L2), 1 = force L1,
    /// 2 = force L2. The ONLY layout switching that exists anywhere.
    pub layout_a: u8,
    pub layout_b: u8,
    pub layout_c: u8,
    /// Show Party A when solo. This is a client HUD, not XIUI: the frame
    /// draws regardless; the flag only exists so the Debug menu can hide it.
    pub show_when_solo: bool,
    /// Draw the MP bar for no-MP jobs too.
    pub always_show_mp_bar: bool,
    /// TP text on L1 rows / TP value on L2 rows.
    pub show_tp: bool,
    /// Per-member distance to self (L1 name line).
    pub show_member_distance: bool,
    /// Distance-to-target on the Party A title row (right side).
    pub show_target_distance: bool,
    /// Alternating dark-red band on odd L1 rows.
    pub alternating_bands: bool,
    /// Target/subtarget selection box around rows.
    pub selection_box: bool,
    /// HP value display mode: 0 number, 1 percent, 2 current/max.
    pub hp_display_mode: u8,
    /// Empty placeholder rows kept under the live members (retail-style).
    pub min_rows: u8,
    /// Uniform scale applied to all bar/entry dimensions.
    pub scale: f32,
}

impl Default for PartyFrameSettings {
    fn default() -> Self {
        Self {
            layout_a: 0,
            layout_b: 0,
            layout_c: 0,
            show_when_solo: true,
            always_show_mp_bar: true,
            show_tp: true,
            show_member_distance: true,
            show_target_distance: true,
            alternating_bands: true,
            selection_box: true,
            hp_display_mode: 0,
            min_rows: 1,
            scale: 1.0,
        }
    }
}

/// Single activity marker per member, retail priority order (XIUI mirrors
/// FFXI's player-icon rule: only one at a time):
/// link-dead > GM > mentor > new-adv > away > LFP/LFG > bazaar.
/// Sync is omitted: it needs buff data (0x076, vendor/server/src/map/packets/s2c/0x076_group_effects.cpp),
/// not entity flags.
fn activity_marker(flags: &kuluu_snapshot::CharFlags) -> Option<(&'static str, Color)> {
    if flags.linkdead {
        Some(("D/C", FLAG_DC))
    } else if flags.gm_level > 0 {
        Some(("GM", FLAG_GM))
    } else if flags.mentor {
        Some(("Mtr", FLAG_MENTOR))
    } else if flags.new_character {
        Some(("New", FLAG_NEW))
    } else if flags.away {
        Some(("Away", FLAG_AWAY))
    } else if flags.lfg {
        Some(("LFP", FLAG_LFP))
    } else if flags.bazaar {
        Some(("Baz", FLAG_BAZAAR))
    } else {
        None
    }
}

/// Returns true for L1, false for L2; window A defaults to L1, B/C to L2.
fn layout_for(party_no: u8, s: &PartyFrameSettings) -> bool {
    let forced = match party_no {
        0 => s.layout_a,
        1 => s.layout_b,
        _ => s.layout_c,
    };
    match forced {
        1 => true,
        2 => false,
        _ => party_no == 0,
    }
}

/// Root of one party window (A/B/C).
#[derive(Component)]
pub struct PartyFrameRoot {
    pub party_no: u8,
}

/// Title text ("Solo"/"Party"/"Party B"/"Party C").
#[derive(Component)]
pub struct PartyTitle {
    pub party_no: u8,
}

/// Distance-to-target readout on the Party A title row.
#[derive(Component)]
pub struct PartyTargetDist;

/// Per-member distance text on an L1 name line. Updated in place between row
/// rebuilds: a clear-and-respawn made the panel's measured size oscillate —
/// the "double box" ghost.
#[derive(Component)]
pub struct MemberDistText(pub u32);

/// "Treas." flag on the Party A title row (left of the title), lit while the
/// party treasure pool holds items — XIUI DrawWindow title flanks.
#[derive(Component)]
pub struct PartyTreasureFlag;

/// Container that holds the member rows for one window.
#[derive(Component)]
pub struct PartyRowsHost {
    pub party_no: u8,
}

/// Clickable member row: click sets Res<Target> to this entity
///.
#[derive(Component)]
pub struct PartyRowTarget(pub u32);

#[derive(Component)]
pub struct UiSettingsPanel;

/// One clickable settings row. `key` indexes the setting it toggles/cycles.
#[derive(Component, Clone, Copy, PartialEq)]
pub enum UiSettingKey {
    LayoutA,
    LayoutB,
    LayoutC,
    ShowWhenSolo,
    AlwaysShowMpBar,
    ShowTp,
    ShowMemberDistance,
    ShowTargetDistance,
    AlternatingBands,
    SelectionBox,
    HpDisplayMode,
    MinRows,
    Scale,
}

#[derive(Component)]
pub struct UiSettingsRow {
    pub key: UiSettingKey,
}

fn setting_label(key: UiSettingKey, s: &PartyFrameSettings) -> String {
    let layout = |v: u8| match v {
        0 => "default",
        1 => "L1 compact",
        _ => "L2 super",
    };
    let onoff = |b: bool| if b { "on" } else { "off" };
    match key {
        UiSettingKey::LayoutA => format!("Layout A [{}]", layout(s.layout_a)),
        UiSettingKey::LayoutB => format!("Layout B [{}]", layout(s.layout_b)),
        UiSettingKey::LayoutC => format!("Layout C [{}]", layout(s.layout_c)),
        UiSettingKey::ShowWhenSolo => format!("Solo window [{}]", onoff(s.show_when_solo)),
        UiSettingKey::AlwaysShowMpBar => format!("MP bar always [{}]", onoff(s.always_show_mp_bar)),
        UiSettingKey::ShowTp => format!("TP [{}]", onoff(s.show_tp)),
        UiSettingKey::ShowMemberDistance => {
            format!("Member distance [{}]", onoff(s.show_member_distance))
        }
        UiSettingKey::ShowTargetDistance => {
            format!("Target distance [{}]", onoff(s.show_target_distance))
        }
        UiSettingKey::AlternatingBands => format!("Alt bands [{}]", onoff(s.alternating_bands)),
        UiSettingKey::SelectionBox => format!("Sel box [{}]", onoff(s.selection_box)),
        UiSettingKey::HpDisplayMode => {
            let m = match s.hp_display_mode {
                1 => "percent",
                2 => "cur/max",
                _ => "number",
            };
            format!("HP mode [{}]", m)
        }
        UiSettingKey::MinRows => format!("Min rows [{}]", s.min_rows),
        UiSettingKey::Scale => format!("Scale [{:.2}]", s.scale),
    }
}

/// Cycles/toggles one setting. MinRows floors the placeholder row count at
/// 0..=6.
fn cycle_setting(key: UiSettingKey, s: &mut PartyFrameSettings) {
    match key {
        UiSettingKey::LayoutA => s.layout_a = (s.layout_a + 1) % 3,
        UiSettingKey::LayoutB => s.layout_b = (s.layout_b + 1) % 3,
        UiSettingKey::LayoutC => s.layout_c = (s.layout_c + 1) % 3,
        UiSettingKey::ShowWhenSolo => s.show_when_solo = !s.show_when_solo,
        UiSettingKey::AlwaysShowMpBar => s.always_show_mp_bar = !s.always_show_mp_bar,
        UiSettingKey::ShowTp => s.show_tp = !s.show_tp,
        UiSettingKey::ShowMemberDistance => s.show_member_distance = !s.show_member_distance,
        UiSettingKey::ShowTargetDistance => s.show_target_distance = !s.show_target_distance,
        UiSettingKey::AlternatingBands => s.alternating_bands = !s.alternating_bands,
        UiSettingKey::SelectionBox => s.selection_box = !s.selection_box,
        UiSettingKey::HpDisplayMode => s.hp_display_mode = (s.hp_display_mode + 1) % 3,
        UiSettingKey::MinRows => s.min_rows = (s.min_rows + 1) % 7,
        UiSettingKey::Scale => {
            let next = match s
                .scale
                .partial_cmp(&0.85)
                .unwrap_or(std::cmp::Ordering::Less)
            {
                std::cmp::Ordering::Less => 1.0,
                std::cmp::Ordering::Equal => 1.25,
                _ => 0.75,
            };
            s.scale = next;
        }
    }
}

pub fn hp_ramp(pct: u8) -> Color {
    let p = pct as f32;
    if p >= 70.0 {
        Color::srgb(0.25, 0.80, 0.30)
    } else if p >= 40.0 {
        lerp_rgb((0.85, 0.80, 0.20), (0.35, 0.75, 0.25), (p - 40.0) / 30.0)
    } else if p >= 20.0 {
        lerp_rgb((0.90, 0.45, 0.15), (0.85, 0.75, 0.20), (p - 20.0) / 20.0)
    } else {
        Color::srgb(0.85, 0.20, 0.20)
    }
}

fn lerp_rgb(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::srgb(
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

fn party_windows(
    party: &[kuluu_snapshot::PartyMember],
    self_id: Option<u32>,
) -> [Vec<&kuluu_snapshot::PartyMember>; 3] {
    // LSB's PartyNo is alliance-wide: vendor/server/src/map/packets/s2c/0x0dd_group_list.cpp GP_SERV_COMMAND_GROUP_LIST::GP_SERV_COMMAND_GROUP_LIST.
    let self_party = party
        .iter()
        .find(|m| Some(m.id) == self_id)
        .map(|m| m.party_no)
        .filter(|party_no| *party_no < ffxi_proto::decode::NO_PARTY)
        .unwrap_or(0);
    let mut windows = [vec![], vec![], vec![]];
    for member in party {
        let window = if Some(member.id) == self_id || member.party_no == self_party {
            0
        } else if member.party_no < ffxi_proto::decode::NO_PARTY {
            usize::from(member.party_no) + usize::from(member.party_no < self_party)
        } else {
            continue;
        };
        windows[window].push(member);
    }
    for members in &mut windows {
        members.sort_by_key(|m| (Some(m.id) != self_id, m.act_index));
    }
    windows
}

fn hp_value_text(m: &kuluu_snapshot::PartyMember, mode: u8) -> String {
    match mode {
        1 => format!("{}%", m.hp_pct),
        2 => format!("{}/{}", m.hp, max_from_pct(m)),
        _ => format!("{}", m.hp),
    }
}

/// Derived max, used only for the "current/max" display mode: PartyMember
/// carries no max HP/MP field, so the max is back-solved from the current
/// value and its percentage; the authoritative source is the 0x061 CLISTATUS
/// block (vendor/server/src/map/packets/s2c/0x061_clistatus.cpp).
fn max_from_pct(m: &kuluu_snapshot::PartyMember) -> u32 {
    if m.hp_pct > 0 {
        (m.hp as f32 / m.hp_pct as f32 * 100.0).round() as u32
    } else {
        m.hp
    }
}

/// Spawns the three window roots + the Debug-menu UI Settings panel. Window
/// padding per layout: L1 x=10, L2 x=3. The title straddles the top border,
/// centered; the treasure-pool flag sits left of the title (Party A only),
/// lit while the pool holds items; the distance-to-target readout sits right
/// (Party A only, owned by update_party_dist_text_system). The rows host
/// (re)builds member entries each dirty frame.
pub fn spawn_party_frames(mut commands: Commands) {
    for party_no in 0u8..3 {
        let is_l1_default = party_no == 0;
        let pad_x = if is_l1_default { 10.0 } else { 3.0 };
        let top_pad = if is_l1_default {
            TITLE_PX * 0.75 + 3.0
        } else {
            8.0
        };
        commands
            .spawn((
                crate::components::InGameEntity,
                PartyFrameRoot { party_no },
                ColumnPanel::ROSTER,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(style::PANEL_COLUMN_BOTTOM_PX),
                    right: Val::Px(style::PANEL_COLUMN_RIGHT_PX),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect {
                        left: Val::Px(pad_x),
                        right: Val::Px(pad_x),
                        top: Val::Px(top_pad),
                        bottom: Val::Px(6.0),
                    },
                    border: UiRect::all(Val::Px(1.0)),
                    row_gap: Val::Px(2.0),
                    overflow: Overflow::visible(),
                    display: if party_no == 0 {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    ..default()
                },
                BackgroundColor(theme::FRAME_BG),
                BorderColor::all(theme::FRAME_EDGE),
            ))
            .with_children(|root| {
                root.spawn((
                    PartyTitle { party_no },
                    Text::new(if party_no == 0 { "Solo" } else { "" }),
                    style::text_font(TITLE_PX),
                    TextColor(theme::TEXT),
                    Node {
                        position_type: PositionType::Absolute,
                        top: Val::Px(-TITLE_PX * 0.75),
                        left: Val::Px(4.0),
                        right: Val::Px(4.0),
                        ..default()
                    },
                    TextLayout::justify(Justify::Center),
                ));
                if party_no == 0 {
                    root.spawn((
                        PartyTreasureFlag,
                        Text::new(""),
                        style::text_font(JOB_PX),
                        TextColor(TREASURE_FLAG),
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(-TITLE_PX * 0.65),
                            left: Val::Px(4.0),
                            ..default()
                        },
                    ));
                }
                if party_no == 0 {
                    root.spawn((
                        PartyTargetDist,
                        Text::new(""),
                        style::text_font(JOB_PX),
                        TextColor(theme::MUTED),
                        Node {
                            position_type: PositionType::Absolute,
                            top: Val::Px(-TITLE_PX * 0.65),
                            right: Val::Px(4.0),
                            ..default()
                        },
                    ));
                }
                root.spawn((
                    PartyRowsHost { party_no },
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(3.0),
                        ..default()
                    },
                ));
            });
    }

    spawn_ui_settings_panel(commands);
}

fn spawn_ui_settings_panel(mut commands: Commands) {
    let rows = [
        UiSettingKey::LayoutA,
        UiSettingKey::LayoutB,
        UiSettingKey::LayoutC,
        UiSettingKey::ShowWhenSolo,
        UiSettingKey::AlwaysShowMpBar,
        UiSettingKey::ShowTp,
        UiSettingKey::ShowMemberDistance,
        UiSettingKey::ShowTargetDistance,
        UiSettingKey::AlternatingBands,
        UiSettingKey::SelectionBox,
        UiSettingKey::HpDisplayMode,
        UiSettingKey::MinRows,
        UiSettingKey::Scale,
    ];
    commands
        .spawn((
            crate::components::InGameEntity,
            UiSettingsPanel,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(48.0),
                left: Val::Px(style::PANEL_COLUMN_RIGHT_PX),
                width: Val::Px(230.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(1.0),
                padding: UiRect::all(Val::Px(6.0)),
                border: UiRect::all(Val::Px(1.0)),
                display: Display::None,
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::CURSOR),
        ))
        .with_children(|p| {
            p.spawn((
                Text::new("UI Settings (click rows)"),
                style::text_font(TITLE_PX - 2.0),
                TextColor(theme::CURSOR),
                Node {
                    margin: UiRect {
                        bottom: Val::Px(4.0),
                        ..default()
                    },
                    ..default()
                },
            ));
            for key in rows {
                p.spawn((
                    UiSettingsRow { key },
                    Interaction::None,
                    Text::new(setting_label(key, &PartyFrameSettings::default())),
                    style::text_font(12.0),
                    TextColor(theme::TEXT),
                    Node {
                        padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)),
                        ..default()
                    },
                ));
            }
        });
}

/// Rebuilds member rows + titles from the snapshot when anything relevant
/// changed: snapshot dirty, target changed (the highlight cannot wait for the
/// next packet), or settings changed.
///
/// Rebuilds only when rendered content actually changes: `state.dirty` fires
/// on every packet (position updates while moving), and a clear-and-respawn
/// per frame made the panel's measured size oscillate between frames — a
/// temporal double image ("double box"). `enhanced-job-display` is the
/// compile-time half of the Retail+ gate: without it a persisted
/// `job_display` from an enhanced build stays off in a plain one. A cheap
/// field gate runs before the expensive key build: party_content_key
/// deep-clones the party list and scans every entity per member, so it stays
/// out of the idle path; ingest_system (PreUpdate) sets `dirty` only when
/// fresh data lands, so with no new snapshot the cheap fields prove the whole
/// key would be unchanged. A fresh snapshot still pays for the full scan:
/// char_flags can move without touching any field the cheap gate reads (a
/// member toggling seeking-party). Titles + treasure flag share one merged
/// query: two separate `&mut Text` queries would conflict at runtime (B0001);
/// Bevy cannot prove the entities disjoint. Self prefers the session's own
/// char id over resolve_self's party.first() fallback: on a zone-in the group
/// list can land before our own entry, and pulling a stranger into window A as
/// "self" miscolours their row. Window show/hide is a client HUD, not XIUI:
/// missing party data keeps window A up; retail hides the frame only while a
/// map-server transition is in flight (Stage::Zoning), and its first draw
/// after load is the self row with name + 0/0 until group data lands. Rows
/// rebuild per window by clear-and-respawn (row counts are tiny); window A
/// with no group data yet shows a synthetic self row, name + 0/0, instead of
/// hiding, and min_rows keeps dimmed placeholder rows under the live members.
pub fn update_party_frame_system(
    mut commands: Commands,
    state: Res<SceneState>,
    target: Res<Target>,
    graphics: Res<crate::graphics_settings::GraphicsSettings>,
    settings: Res<PartyFrameSettings>,
    colors: Res<NameColorTable>,
    zone_names: Option<Res<crate::hud::zone_flash::ZoneNameResolver>>,
    mut root_q: Query<(&PartyFrameRoot, &mut Node), Without<PartyRowsHost>>,
    mut title_q: Query<(&mut Text, Option<&PartyTitle>, Option<&PartyTreasureFlag>)>,
    host_q: Query<(Entity, &PartyRowsHost, Option<&Children>)>,
    mut last_key: Local<Option<PartyContentKey>>,
) {
    let snap = &state.snapshot;
    #[cfg(feature = "enhanced-job-display")]
    let job_on = graphics.job_display;
    #[cfg(not(feature = "enhanced-job-display"))]
    let job_on: bool = {
        let _ = &graphics;
        false
    };
    if !target.is_changed() && !settings.is_changed() {
        if let Some(last) = last_key.as_ref() {
            let cheap_equal = snap.zone_id == last.zone_id
                && snap.stage == last.stage
                && snap.zone_generation == last.zone_generation
                && colors.generation() == last.name_color_generation
                && (!snap.treasure_pool.is_empty()) == last.treasure_nonempty
                && snap.party == last.party
                && snap.char_name.as_deref() == last.char_name.as_deref()
                && snap.self_char_id == last.self_char_id
                && job_on == last.job_display;
            if cheap_equal && !state.dirty {
                return;
            }
        }
    }
    let key = party_content_key(snap, colors.generation(), job_on);
    if !target.is_changed() && !settings.is_changed() && Some(&key) == last_key.as_ref() {
        return;
    }
    *last_key = Some(key);

    let self_member = crate::snapshot::resolve_self(&snap.party, snap.self_char_id);
    let self_zone = self_member.map(|m| m.zone_no);
    let self_id = snap.self_char_id.or(self_member.map(|m| m.id));
    let self_pos = snap.self_pos.pos;

    let windows = party_windows(&snap.party, self_id);

    let solo = windows[0].len() <= 1;
    let self_in_party = self_member
        .map(|m| m.party_no != ffxi_proto::decode::NO_PARTY)
        .unwrap_or(false);

    let zoning = snap.stage == kuluu_snapshot::Stage::Zoning;
    for (root, mut node) in root_q.iter_mut() {
        let show = if zoning {
            false
        } else if root.party_no == 0 {
            settings.show_when_solo
        } else {
            !windows[root.party_no as usize].is_empty()
        };
        node.display = if show { Display::Flex } else { Display::None };
    }

    for (mut text, title, treasure) in title_q.iter_mut() {
        let want = if let Some(title) = title {
            match title.party_no {
                0 => {
                    if self_in_party && !solo {
                        "Party".to_string()
                    } else {
                        "Solo".to_string()
                    }
                }
                1 => "Party B".to_string(),
                2 => "Party C".to_string(),
                _ => String::new(),
            }
        } else if treasure.is_some() {
            if snap.treasure_pool.is_empty() {
                String::new()
            } else {
                "Treas.".to_string()
            }
        } else {
            continue;
        };
        if **text != want {
            **text = want;
        }
    }

    for (host_entity, host, children) in host_q.iter() {
        if let Some(children) = children {
            for c in children.iter() {
                commands.entity(c).despawn();
            }
        }
        let is_l1 = layout_for(host.party_no, &settings);

        let synthetic_self;
        let members: Vec<&kuluu_snapshot::PartyMember> =
            if host.party_no == 0 && windows[0].is_empty() {
                synthetic_self = kuluu_snapshot::PartyMember {
                    id: snap.self_char_id.unwrap_or(0),
                    act_index: 0,
                    name: snap.char_name.clone(),
                    hp: 0,
                    mp: 0,
                    tp: 0,
                    hp_pct: 0,
                    mp_pct: 0,
                    zone_no: snap.zone_id.unwrap_or(0),
                    main_job: 0,
                    main_job_lv: 0,
                    sub_job: 0,
                    sub_job_lv: 0,
                    is_party_leader: false,
                    is_alliance_leader: false,
                    party_no: ffxi_proto::decode::NO_PARTY,
                    in_mog_house: false,
                };
                vec![&synthetic_self]
            } else {
                windows[host.party_no as usize].clone()
            };

        let total_rows = members.len().max(settings.min_rows as usize);
        commands.entity(host_entity).with_children(|host_cb| {
            for slot in 0..total_rows {
                match members.get(slot) {
                    Some(m) => {
                        spawn_member_row(
                            host_cb,
                            m,
                            is_l1,
                            &settings,
                            &colors,
                            zone_names.as_deref(),
                            self_zone,
                            self_id,
                            target.id,
                            snap,
                            self_pos,
                            job_on,
                        );
                    }
                    None => spawn_placeholder_row(host_cb, is_l1, &settings),
                }
            }
        });
    }
}

/// Everything that affects row/title rendering, cheaply comparable. Position
/// data is deliberately excluded: distance readouts are updated in place by
/// update_party_dist_text_system, so movement does not trigger a rebuild.
/// Zone + stage are included: a zone-in forces the first default draw even
/// when the party list matches the last zone's (or is empty), and
/// Zoning<->InZone flips re-run the show/hide logic.
#[derive(Debug, Clone, PartialEq)]
pub struct PartyContentKey {
    self_char_id: Option<u32>,
    party: Vec<kuluu_snapshot::PartyMember>,
    char_name: Option<String>,
    flags: Vec<(u32, kuluu_snapshot::CharFlags)>,
    treasure_nonempty: bool,
    zone_id: Option<u16>,
    stage: kuluu_snapshot::Stage,
    /// Monotonically increasing counter from the session, bumped on every zone
    /// change: after a zone transition the key differs even when the actual
    /// party data is byte-identical, forcing the UI to rebuild.
    zone_generation: u64,
    /// NameColorTable content version (0 until the retail table loads). The
    /// table can land after the first draw and its colours feed every row's
    /// name tint — without it in the key, late-loaded colours would sit stale
    /// until some unrelated field happened to change.
    name_color_generation: u64,
    /// Retail+ gate (GraphicsSettings.job_display): toggling it must rebuild
    /// the L1 rows with/without the job-icon column even when nothing else in
    /// the snapshot changed.
    job_display: bool,
}

fn party_content_key(
    snap: &kuluu_snapshot::SceneSnapshot,
    name_color_generation: u64,
    job_display: bool,
) -> PartyContentKey {
    let mut flags = snap
        .party
        .iter()
        .filter_map(|m| {
            snap.entities
                .iter()
                .find(|e| e.id == m.id)
                .map(|e| (m.id, e.char_flags))
        })
        .collect::<Vec<_>>();
    flags.sort_by_key(|(id, _)| *id);
    PartyContentKey {
        self_char_id: snap.self_char_id,
        party: snap.party.clone(),
        char_name: snap.char_name.clone(),
        flags,
        treasure_nonempty: !snap.treasure_pool.is_empty(),
        zone_id: snap.zone_id,
        stage: snap.stage,
        zone_generation: snap.zone_generation,
        name_color_generation,
        job_display,
    }
}

fn dist_str(a: kuluu_snapshot::Vec3, b: kuluu_snapshot::Vec3) -> String {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    format!("{:.1}", (dx * dx + dy * dy + dz * dz).sqrt())
}

fn target_dist_text(
    target_id: Option<u32>,
    snap: &kuluu_snapshot::SceneSnapshot,
    self_pos: kuluu_snapshot::Vec3,
) -> String {
    let Some(tid) = target_id else {
        return String::new();
    };
    let Some(ent) = snap.entities.iter().find(|e| e.id == tid) else {
        return String::new();
    };
    format!("d={}y", dist_str(self_pos, ent.pos))
}

/// Keeps distance readouts fresh between row rebuilds (in place, no churn):
/// per-member distances on L1 name lines + target distance on the Party A
/// title line. Runs every frame; writes only when a string actually changes.
/// Registered in its own add_systems call: the main HUD tuple above it is
/// already at Bevy's 20-element limit, and it must follow the row rebuild so
/// freshly spawned distance texts get their value same-frame.
pub fn update_party_dist_text_system(
    state: Res<SceneState>,
    target: Res<Target>,
    settings: Res<PartyFrameSettings>,
    mut q: Query<(&mut Text, Option<&MemberDistText>, Option<&PartyTargetDist>)>,
) {
    let snap = &state.snapshot;
    let self_pos = snap.self_pos.pos;
    for (mut text, member, target_dist) in q.iter_mut() {
        let want = if let Some(member) = member {
            snap.entities
                .iter()
                .find(|e| e.id == member.0)
                .map(|e| dist_str(self_pos, e.pos))
                .unwrap_or_default()
        } else if target_dist.is_some() {
            if settings.show_target_distance {
                target_dist_text(target.id, snap, self_pos)
            } else {
                String::new()
            }
        } else {
            continue;
        };
        if **text != want {
            **text = want;
        }
    }
}

/// Builds one member row's data. Self's PartyMember.name is often None (the
/// party packet does not carry the own name); fall back to the snapshot
/// char_name, and vice versa. Retail party-aware name color: self = PC row,
/// others = PARTY row (nameplate_color.rs ncol table). Member distance to
/// self appears on L1 name lines; self gets none, its distance to itself is
/// 0.0 (the "weird 0.0" behind the frame). The activity marker comes from
/// entity flags; only members present as visible entities carry one
/// (out-of-zone rows have no Entity).
fn spawn_member_row(
    parent: &mut ChildSpawnerCommands,
    m: &kuluu_snapshot::PartyMember,
    is_l1: bool,
    s: &PartyFrameSettings,
    colors: &NameColorTable,
    zone_names: Option<&crate::hud::zone_flash::ZoneNameResolver>,
    self_zone: Option<u16>,
    self_id: Option<u32>,
    target_id: Option<u32>,
    snap: &kuluu_snapshot::SceneSnapshot,
    self_pos: kuluu_snapshot::Vec3,
    job_display: bool,
) {
    let out_of_zone = matches!((self_zone, Some(m.zone_no)), (Some(sz), Some(mz)) if sz != mz);
    let is_target = target_id == Some(m.id);

    let name = if Some(m.id) == self_id {
        snap.char_name
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| m.name.clone())
            .unwrap_or_default()
    } else {
        m.name.clone().unwrap_or_default()
    };
    let name_label = if out_of_zone {
        format!("{} ({})", name, short_zone(m.zone_no, zone_names))
    } else {
        name.clone()
    };

    let is_self = Some(m.id) == self_id;
    let name_color = colors.color(if is_self { ncol::PC } else { ncol::PARTY });

    let member_dist: Option<String> = if s.show_member_distance && !out_of_zone && !is_self {
        snap.entities
            .iter()
            .find(|e| e.id == m.id)
            .map(|e| dist_str(self_pos, e.pos))
    } else {
        None
    };

    let flag = snap
        .entities
        .iter()
        .find(|e| e.id == m.id)
        .and_then(|e| activity_marker(&e.char_flags));

    if is_l1 {
        spawn_row_l1(
            parent,
            m,
            s,
            name_label,
            name_color,
            member_dist,
            flag,
            out_of_zone,
            is_target,
            job_display,
        );
    } else {
        spawn_row_l2(
            parent,
            m,
            s,
            name_label,
            name_color,
            flag,
            out_of_zone,
            is_target,
        );
    }
}

/// L1 compact vertical (Party A): [icon slot | HP bar / MP bar], with the
/// name line overlaid on the HP bar's top edge and distance right-aligned.
/// Retail+ gate: with the job column off the row is just the bars, no icon
/// slot, no gap (retail's party frame has no jobs). The row root is relative
/// so the absolute name line anchors to it; uniform padding on every row, the
/// selection box toggles colors, not geometry. The job icon slot is
/// placeholder text until icon textures land, with no background box: the
/// filled rectangle read as a leftover artifact. The bars column puts HP on
/// top, MP right-aligned under it (display.lua L1); [TP value] [MP bar] share
/// one line under the HP bar's right edge, TP a bare number (gold at 1000).
/// The name line straddles the HP bar's top edge: [leader dot(s)] [name]
/// ......... [distance].
fn spawn_row_l1(
    parent: &mut ChildSpawnerCommands,
    m: &kuluu_snapshot::PartyMember,
    s: &PartyFrameSettings,
    name_label: String,
    name_color: Color,
    member_dist: Option<String>,
    flag: Option<(&'static str, Color)>,
    out_of_zone: bool,
    is_target: bool,
    job_display: bool,
) {
    let sc = s.scale;
    let hp_w = L1_HP_BASE_W * BASE_MULT * L1_HP_W_MULT * sc;
    let mp_w = L1_MP_BASE_W * BASE_MULT * L1_HP_W_MULT * L1_MP_EXTRA_W_MULT * sc;
    let bar_h = L1_BAR_H * sc;
    let icon_size = if job_display { L1_ICON_SIZE * sc } else { 0.0 };
    let inset = if job_display { L1_BAR_INSET * sc } else { 0.0 };
    let entry_h = bar_h + 1.0 + bar_h;

    let mut row = parent.spawn((
        Node {
            position_type: PositionType::Relative,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(inset),
            width: Val::Px(icon_size + inset + hp_w),
            height: Val::Px(entry_h),
            padding: UiRect::all(Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        PartyRowTarget(m.id),
        Interaction::None,
    ));
    if s.selection_box && is_target {
        row.insert((BackgroundColor(TARGET_BG), BorderColor::all(TARGET_BORDER)));
    } else if s.alternating_bands && m.act_index % 2 == 1 {
        row.insert(BackgroundColor(BAND_COLOR));
    }

    row.with_children(|row| {
        if job_display {
            row.spawn((Node {
                width: Val::Px(icon_size),
                height: Val::Px(entry_h),
                flex_shrink: 0.0,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },))
                .with_children(|icon| {
                    icon.spawn((
                        Text::new(job_abbrev(m.main_job)),
                        style::text_font(JOB_PX * sc.max(0.75)),
                        TextColor(theme::MUTED),
                    ));
                });
        }

        row.spawn((Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(1.0),
            ..default()
        },))
            .with_children(|bars| {
                bars.spawn(bar_track(hp_w, bar_h)).with_children(|hp| {
                    fill_or_block(
                        hp,
                        out_of_zone,
                        m.hp_pct as f32,
                        hp_ramp(m.hp_pct),
                        &hp_value_text(m, s.hp_display_mode),
                        NAME_PX * sc.max(0.75),
                    );
                });

                let show_mp = s.always_show_mp_bar || m.mp > 0;
                if show_mp || s.show_tp {
                    bars.spawn(Node {
                        position_type: PositionType::Relative,
                        width: Val::Px(hp_w),
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::FlexEnd,
                        column_gap: Val::Px(4.0),
                        ..default()
                    })
                    .with_children(|line| {
                        if s.show_tp {
                            line.spawn((
                                Text::new(format!("{}", m.tp)),
                                style::text_font(JOB_PX * sc.max(0.75)),
                                TextColor(if m.tp >= 1000 { TP_FULL } else { TP_DIM }),
                            ));
                        }
                        if show_mp {
                            line.spawn(bar_track(mp_w, bar_h)).with_children(|mp| {
                                fill_or_block(
                                    mp,
                                    out_of_zone,
                                    m.mp_pct as f32,
                                    MP_COLOR,
                                    &format!("{}", m.mp),
                                    JOB_PX * sc.max(0.75),
                                );
                            });
                        }
                    });
                }
            });

        row.spawn((Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            top: Val::Px(-NAME_PX * 0.45),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(4.0),
            ..default()
        },))
            .with_children(|line| {
                if m.is_alliance_leader {
                    line.spawn(dot_node(LEADER_DOT));
                    line.spawn(dot_node(LEADER_DOT));
                } else if m.is_party_leader {
                    line.spawn(dot_node(LEADER_DOT));
                }
                line.spawn((
                    Text::new(name_label),
                    style::text_font(NAME_PX * s.scale.max(0.75)),
                    TextColor(name_color),
                ));
                if let Some((label, color)) = flag {
                    line.spawn((
                        Text::new(label.to_string()),
                        style::text_font(JOB_PX * s.scale.max(0.75)),
                        TextColor(color),
                    ));
                }
                if let Some(d) = member_dist {
                    line.spawn((Node {
                        flex_grow: 1.0,
                        ..default()
                    },));
                    line.spawn((
                        MemberDistText(m.id),
                        Text::new(d),
                        style::text_font(JOB_PX * s.scale.max(0.75)),
                        TextColor(theme::MUTED),
                    ));
                }
            });
    });
}

/// L2 super compact (Alliance B/C): text row [name … HP value] dipping into
/// the HP bar top; bars right-aligned in an entry box wider than they are.
/// The text row is [leader dot(s)] [name] ......... [HP value], dipping into
/// the HP bar top by `overlap` px (the bar starts at hp_top). The HP bar is
/// right-aligned in the entry box; the MP bar sits below it, shifted up so
/// the HP bar covers its top sliver. The TP value is text only in L2.
fn spawn_row_l2(
    parent: &mut ChildSpawnerCommands,
    m: &kuluu_snapshot::PartyMember,
    s: &PartyFrameSettings,
    name_label: String,
    name_color: Color,
    flag: Option<(&'static str, Color)>,
    out_of_zone: bool,
    is_target: bool,
) {
    let sc = s.scale;
    let hp_w = L2_HP_BASE_W * BASE_MULT * sc;
    let mp_w = L2_MP_BASE_W * BASE_MULT * sc;
    let bar_h = L2_BAR_H * sc;
    let entry_w = (L2_ENTRY_W * BASE_MULT).max(hp_w) * 1.0;
    let name_row_h = NAME_PX + 2.0;
    let overlap = L2_NAME_BAR_OVERLAP * sc;
    let mp_overlap = L2_MP_OVERLAP * sc;
    let hp_top = name_row_h - overlap;
    let entry_h = (name_row_h - overlap) + bar_h + bar_h - mp_overlap;

    let mut row = parent.spawn((
        Node {
            position_type: PositionType::Relative,
            width: Val::Px(entry_w),
            height: Val::Px(entry_h),
            padding: UiRect::all(Val::Px(1.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        PartyRowTarget(m.id),
        Interaction::None,
    ));
    if s.selection_box && is_target {
        row.insert((BackgroundColor(TARGET_BG), BorderColor::all(TARGET_BORDER)));
    } else if s.alternating_bands && m.act_index % 2 == 1 {
        row.insert(BackgroundColor(BAND_COLOR));
    }

    row.with_children(|row| {
        row.spawn((Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            right: Val::Px(0.0),
            top: Val::Px(0.0),
            height: Val::Px(name_row_h),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(3.0),
            padding: UiRect {
                left: Val::Px(4.0),
                ..default()
            },
            ..default()
        },))
            .with_children(|line| {
                if m.is_alliance_leader {
                    line.spawn(dot_node(LEADER_DOT));
                    line.spawn(dot_node(LEADER_DOT));
                } else if m.is_party_leader {
                    line.spawn(dot_node(LEADER_DOT));
                }
                line.spawn((
                    Text::new(name_label),
                    style::text_font(NAME_PX * sc.max(0.75)),
                    TextColor(name_color),
                ));
                if let Some((label, color)) = flag {
                    line.spawn((
                        Text::new(label.to_string()),
                        style::text_font(JOB_PX * sc.max(0.75)),
                        TextColor(color),
                    ));
                }
                line.spawn(Node {
                    flex_grow: 1.0,
                    ..default()
                });
                if !out_of_zone {
                    line.spawn((
                        Text::new(hp_value_text(m, s.hp_display_mode)),
                        style::text_font(JOB_PX * sc.max(0.75)),
                        TextColor(Color::WHITE),
                    ));
                }
            });

        row.spawn(Node {
            position_type: PositionType::Absolute,
            left: Val::Px(entry_w - hp_w),
            top: Val::Px(hp_top),
            ..bar_track(hp_w, bar_h)
        })
        .with_children(|hp| {
            fill_or_block(hp, out_of_zone, m.hp_pct as f32, hp_ramp(m.hp_pct), "", 0.0);
        });

        let show_mp = s.always_show_mp_bar || m.mp > 0;
        if show_mp {
            row.spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Px(entry_w - mp_w),
                top: Val::Px(hp_top + bar_h - mp_overlap),
                ..bar_track(mp_w, bar_h)
            })
            .with_children(|mp| {
                fill_or_block(
                    mp,
                    out_of_zone,
                    m.mp_pct as f32,
                    MP_COLOR,
                    &format!("{}", m.mp),
                    JOB_PX * sc.max(0.75),
                );
            });
        }

        if s.show_tp {
            row.spawn((
                Text::new(format!("TP {}", m.tp)),
                style::text_font(JOB_PX * 0.9 * sc.max(0.75)),
                TextColor(if m.tp >= 1000 { TP_FULL } else { TP_DIM }),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(4.0),
                    top: Val::Px(hp_top + bar_h - mp_overlap + bar_h * 0.25),
                    ..default()
                },
            ));
        }
    });
}

/// Dimmed empty row (min_rows floor). An empty track (no fill) reads as a
/// dimmed empty slot; Bevy's Node has no per-node opacity in 0.19, so the
/// dimming is the track's own colors, not an overlay.
fn spawn_placeholder_row(parent: &mut ChildSpawnerCommands, is_l1: bool, s: &PartyFrameSettings) {
    let sc = s.scale;
    let hp_w = if is_l1 {
        L1_HP_BASE_W * BASE_MULT * L1_HP_W_MULT * sc
    } else {
        L2_HP_BASE_W * BASE_MULT * sc
    };
    let bar_h = (if is_l1 { L1_BAR_H } else { L2_BAR_H }) * sc;
    parent
        .spawn((Node {
            position_type: PositionType::Relative,
            width: Val::Px(if is_l1 {
                L1_ICON_SIZE * sc + L1_BAR_INSET * sc + hp_w
            } else {
                (L2_ENTRY_W * BASE_MULT).max(hp_w)
            }),
            height: Val::Px(bar_h),
            padding: UiRect::all(Val::Px(if is_l1 { 2.0 } else { 1.0 })),
            ..default()
        },))
        .with_children(|row| {
            row.spawn(bar_track(hp_w, bar_h));
        });
}

fn bar_track(w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Relative,
        width: Val::Px(w),
        height: Val::Px(h),
        flex_shrink: 0.0,
        overflow: Overflow::clip(),
        ..default()
    }
}

/// Fills a bar track with the percent fill + right-aligned value text, or an
/// opaque black block when out of zone.
/// `value_font_px == 0`
/// suppresses the value text (L2 HP value lives in the text row instead).
fn fill_or_block(
    parent: &mut ChildSpawnerCommands,
    out_of_zone: bool,
    pct: f32,
    color: Color,
    value: &str,
    value_font_px: f32,
) {
    if out_of_zone {
        parent.spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            BackgroundColor(OUT_OF_ZONE_BLOCK),
        ));
        return;
    }
    parent.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            bottom: Val::Px(0.0),
            width: Val::Percent(pct.clamp(0.0, 100.0)),
            ..default()
        },
        BackgroundColor(color),
    ));
    if value_font_px > 0.0 && !value.is_empty() {
        parent
            .spawn((Node {
                position_type: PositionType::Absolute,
                right: Val::Px(4.0),
                top: Val::Px(0.0),
                bottom: Val::Px(0.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },))
            .with_children(|v| {
                v.spawn((
                    Text::new(value.to_string()),
                    style::text_font(value_font_px),
                    TextColor(Color::WHITE),
                ));
            });
    }
}

fn dot_node(color: Color) -> (Node, BackgroundColor) {
    (
        Node {
            width: Val::Px(5.0),
            height: Val::Px(5.0),
            border_radius: BorderRadius::MAX,
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(color),
    )
}

/// XIUI shortenZoneName over the resolved zone display name; falls back to a
/// stable "Z{id}" placeholder when no name is known. Word slicing is
/// char-based, not byte-sliced: a first word starting with a multi-byte char
/// (CJK/emoji zone names) would panic on a byte index that is not a char
/// boundary.
fn short_zone(zone_no: u16, resolver: Option<&crate::hud::zone_flash::ZoneNameResolver>) -> String {
    let Some(name) = resolver.and_then(|r| r.0(zone_no)) else {
        return format!("Z{zone_no}");
    };
    let cleaned: String = name.chars().filter(|c| *c != '\'').collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    match words.len() {
        0 => format!("Z{zone_no}"),
        1 => words[0].to_string(),
        _ if words.get(1) == Some(&"of") => words.last().copied().unwrap_or(words[0]).to_string(),
        2 => {
            let head: String = words[0].chars().take(2).collect();
            format!("{}{}", head, words[1])
        }
        _ => {
            let initials: String = words[..words.len() - 1]
                .iter()
                .filter_map(|w| w.chars().next())
                .collect();
            format!("{initials}{}", words.last().copied().unwrap_or(""))
        }
    }
}

/// Clicking a member row targets that entity — same path as world picking.
/// Settings rows carry no PartyRowTarget, so no filter is needed here.
pub fn party_row_click_system(
    mut q: Query<(&mut Interaction, &PartyRowTarget)>,
    mut target: ResMut<Target>,
) {
    for (mut interaction, row_target) in q.iter_mut() {
        if *interaction == Interaction::Pressed {
            *interaction = Interaction::None;
            target.id = Some(row_target.0);
        }
    }
}

/// Shows/hides the panel with the Debug menu flag and refreshes row labels
/// when settings change.
pub fn update_ui_settings_system(
    panels: Res<HudPanels>,
    settings: Res<PartyFrameSettings>,
    mut panel_q: Query<&mut Node, With<UiSettingsPanel>>,
    mut rows_q: Query<(&UiSettingsRow, &mut Text)>,
) {
    let Ok(mut node) = panel_q.single_mut() else {
        return;
    };
    let want = if panels.ui_settings {
        Display::Flex
    } else {
        Display::None
    };
    if node.display != want {
        node.display = want;
    }
    if settings.is_changed() || panels.ui_settings {
        for (row, mut text) in rows_q.iter_mut() {
            let label = setting_label(row.key, &settings);
            if **text != label {
                **text = label;
            }
        }
    }
}

/// Click a settings row: cycle/toggle the value.
pub fn ui_settings_click_system(
    mut q: Query<(&mut Interaction, &UiSettingsRow)>,
    mut settings: ResMut<PartyFrameSettings>,
) {
    for (mut interaction, row) in q.iter_mut() {
        if *interaction == Interaction::Pressed {
            *interaction = Interaction::None;
            cycle_setting(row.key, &mut settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retail colour table can land after the first party draw; its content
    /// version has to be part of the key or late-loaded colours sit stale until
    /// an unrelated field happens to change.
    #[test]
    fn content_key_tracks_name_color_table_generation() {
        let snap = kuluu_snapshot::SceneSnapshot::default();
        let a = party_content_key(&snap, 0, false);
        let b = party_content_key(&snap, 1, false);
        assert_eq!(a.party, b.party, "same scene, only the table moved");
        assert_ne!(a.name_color_generation, b.name_color_generation);
        assert_ne!(a, b, "late table load must force a rebuild");
    }

    #[test]
    fn content_key_tracks_self_identity_for_alliance_grouping() {
        let mut snap = kuluu_snapshot::SceneSnapshot::default();
        let unknown = party_content_key(&snap, 0, false);
        snap.self_char_id = Some(42);
        assert_ne!(unknown, party_content_key(&snap, 0, false));
    }

    #[test]
    fn content_key_tracks_job_display_gate() {
        let snap = kuluu_snapshot::SceneSnapshot::default();
        let a = party_content_key(&snap, 0, false);
        let b = party_content_key(&snap, 0, true);
        assert_eq!(a.name_color_generation, b.name_color_generation);
        assert_ne!(a.job_display, b.job_display);
        assert_ne!(a, b, "job gate toggle must force a rebuild");
    }

    #[test]
    fn hp_ramp_bands() {
        let hi = hp_ramp(100).to_srgba();
        let lo = hp_ramp(5).to_srgba();
        assert!(hi.green > hi.red, "high hp should be green-dominant");
        assert!(lo.red > lo.green, "low hp should be red-dominant");
    }

    #[test]
    fn layout_defaults_a_l1_bc_l2() {
        let s = PartyFrameSettings::default();
        assert!(layout_for(0, &s));
        assert!(!layout_for(1, &s));
        assert!(!layout_for(2, &s));
    }

    #[test]
    fn layout_override_cycles() {
        let mut s = PartyFrameSettings::default();
        cycle_setting(UiSettingKey::LayoutA, &mut s);
        assert!(layout_for(0, &s), "forced L1");
        cycle_setting(UiSettingKey::LayoutB, &mut s);
        assert!(layout_for(1, &s), "forced L1 on B");
    }

    #[test]
    fn min_rows_cycles_0_to_6() {
        let mut s = PartyFrameSettings::default();
        for _ in 0..7 {
            cycle_setting(UiSettingKey::MinRows, &mut s);
        }
        assert_eq!(s.min_rows, 1, "full cycle returns to default");
    }

    #[test]
    fn activity_marker_priority() {
        use kuluu_snapshot::CharFlags;
        let label = |f: &CharFlags| activity_marker(f).map(|(l, _)| l);
        assert!(label(&CharFlags::default()).is_none());
        let mut f = CharFlags {
            bazaar: true,
            ..Default::default()
        };
        assert_eq!(label(&f), Some("Baz"));
        f.lfg = true;
        assert_eq!(label(&f), Some("LFP"), "lfg outranks bazaar");
        f.away = true;
        assert_eq!(label(&f), Some("Away"), "away outranks lfp");
        f.linkdead = true;
        assert_eq!(label(&f), Some("D/C"), "link-dead is top priority");
    }

    /// A two-word zone whose first word starts with multi-byte chars must not
    /// hit a non-char-boundary byte slice (pre-fix this panicked).
    #[test]
    fn short_zone_two_word_multibyte_first_word_does_not_panic() {
        let resolver = crate::hud::zone_flash::ZoneNameResolver::new(|_: u16| Some("水城 広場"));
        assert_eq!(short_zone(1, Some(&resolver)), "水城広場");
    }

    #[test]
    fn short_zone_rules() {
        assert_eq!(
            short_zone(234, None),
            "Z234",
            "no resolver falls back to the placeholder"
        );
    }

    #[test]
    fn alliance_keeps_self_party_together_in_first_window() {
        for self_party in 0..3 {
            let mut party = Vec::new();
            for party_no in 0..3 {
                party.push(member(u32::from(party_no) * 10 + 1, party_no));
                party.push(member(u32::from(party_no) * 10 + 2, party_no));
            }
            let self_id = u32::from(self_party) * 10 + 2;
            let windows = party_windows(&party, Some(self_id));
            assert_eq!(windows[0][0].id, self_id);
            assert_eq!(windows.map(|window| window.len()), [2, 2, 2]);
            let windows = party_windows(&party, Some(self_id));
            assert!(windows[0].iter().all(|m| m.party_no == self_party));
            assert!(windows[1]
                .iter()
                .all(|m| m.party_no == windows[1][0].party_no));
            assert!(windows[2]
                .iter()
                .all(|m| m.party_no == windows[2][0].party_no));
            assert_ne!(windows[1][0].party_no, windows[2][0].party_no);
        }
    }

    #[test]
    fn solo_self_and_missing_self_group_safely() {
        let party = [member(1, ffxi_proto::decode::NO_PARTY)];
        assert_eq!(party_windows(&party, Some(1))[0][0].id, 1);
        let party = [member(10, 1), member(20, 2)];
        let windows = party_windows(&party, Some(99));
        assert!(windows[0].is_empty());
        assert_eq!(windows[1][0].id, 10);
        assert_eq!(windows[2][0].id, 20);
    }

    fn member(id: u32, party_no: u8) -> kuluu_snapshot::PartyMember {
        kuluu_snapshot::PartyMember {
            id,
            act_index: 0,
            name: Some("x".into()),
            hp: 800,
            mp: 400,
            tp: 500,
            hp_pct: 80,
            mp_pct: 50,
            zone_no: 1,
            main_job: 1,
            main_job_lv: 99,
            sub_job: 0,
            sub_job_lv: 0,
            is_party_leader: false,
            is_alliance_leader: false,
            party_no,
            in_mog_house: false,
        }
    }

    #[test]
    fn hp_value_modes() {
        let m = member(1, 0);
        assert_eq!(hp_value_text(&m, 0), "800");
        assert_eq!(hp_value_text(&m, 1), "80%");
        assert_eq!(hp_value_text(&m, 2), "800/1000");
    }
}
