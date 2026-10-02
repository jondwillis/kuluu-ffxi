use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use kuluu_snapshot::{ChatChannel, ChatLine, ChatSpanKind};

use crate::graphics_settings::{ChatLayout, GraphicsSettings};
use crate::hud::list_view::apply_wheel_delta;
use crate::hud::style::{self, theme};
use crate::input_mode::{InputMode, PassiveCursorFocus};
use crate::mouse::MousePointer;
use crate::snapshot::{rendered_chat, SceneState};

pub const VISIBLE_ROWS: usize = 12;

/// Rows shown when the focused log is expanded to the full-screen window
/// (retail: confirm on the focused Log Window). Also the number of rows
/// pre-spawned per panel, since the extra rows above the compact viewport are
/// simply clipped until an expand reveals them.
pub const EXPANDED_ROWS: usize = 24;

pub const PANEL_MAX_HEIGHT_PX: f32 = 220.0;
pub const PANEL_EXPANDED_HEIGHT_PX: f32 = 440.0;
pub const PANEL_MIN_HEIGHT_PX: f32 = 60.0;
pub const FULL_HOLD_SECS: f32 = 10.0;
pub const FADE_SECS: f32 = 10.0;

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct ChatPanelDecay {
    pub last_active_secs: f32,

    pub prev_filtered_len: usize,
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ChatScroll {
    pub rows: usize,
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct BattleScroll {
    pub rows: usize,
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct DebugScroll {
    pub rows: usize,
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ChatScrollAccum {
    pub frac: f32,
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct BattleScrollAccum {
    pub frac: f32,
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct DebugScrollAccum {
    pub frac: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatKind {
    #[default]
    Social,
    Battle,
    Debug,
}

#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActiveChatTab(pub ChatKind);

#[derive(Component)]
pub struct ChatTabBar;

#[derive(Component)]
pub struct ChatPanelGroup;

#[derive(Component, Default)]
pub struct ChatArea {
    pub horizontal: bool,
    pub panel_height: f32,
}

const CHAT_SPLIT_MIN_WIDTH_PX: f32 = 960.0;
const CHAT_WINDOW_GAP_PX: f32 = 4.0;
const CHAT_TAB_HEIGHT_PX: f32 = 20.0;
/// Leave breathing room above the log controls at large text scales.
const CHAT_TOP_CLEARANCE_PX: f32 = 16.0;

#[derive(Component, Debug, Clone, Copy)]
pub struct ChatTabButton {
    pub kind: ChatKind,
}

#[derive(Component)]
pub struct ChatTabButtonLabel;

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ChatAutoSwitch(pub bool);

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ChatUnread {
    pub social: bool,
    pub battle: bool,
    pub debug: bool,
}

impl ChatUnread {
    pub fn get(&self, kind: ChatKind) -> bool {
        match kind {
            ChatKind::Social => self.social,
            ChatKind::Battle => self.battle,
            ChatKind::Debug => self.debug,
        }
    }
    pub fn set(&mut self, kind: ChatKind, value: bool) {
        match kind {
            ChatKind::Social => self.social = value,
            ChatKind::Battle => self.battle = value,
            ChatKind::Debug => self.debug = value,
        }
    }
}

#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ChatActivityTracker {
    pub social: usize,
    pub battle: usize,
    pub debug: usize,
}

pub fn reset_chat_session(
    mut active: ResMut<ActiveChatTab>,
    mut unread: ResMut<ChatUnread>,
    mut tracker: ResMut<ChatActivityTracker>,
    mut social: ResMut<ChatScroll>,
    mut battle: ResMut<BattleScroll>,
    mut debug: ResMut<DebugScroll>,
    mut social_accum: ResMut<ChatScrollAccum>,
    mut battle_accum: ResMut<BattleScrollAccum>,
    mut debug_accum: ResMut<DebugScrollAccum>,
) {
    *active = default();
    *unread = default();
    *tracker = default();
    *social = default();
    *battle = default();
    *debug = default();
    *social_accum = default();
    *battle_accum = default();
    *debug_accum = default();
}

#[derive(Component)]
pub struct ChatAutoSwitchToggle;

#[derive(Component)]
pub struct ChatAutoSwitchLabel;

impl ChatKind {
    pub const TAB_ORDER: [ChatKind; 3] = [ChatKind::Social, ChatKind::Battle, ChatKind::Debug];

    pub fn cycle_next(self) -> ChatKind {
        let pos = Self::TAB_ORDER.iter().position(|&k| k == self).unwrap_or(0);
        Self::TAB_ORDER[(pos + 1) % Self::TAB_ORDER.len()]
    }

    pub fn cycle_prev(self) -> ChatKind {
        let n = Self::TAB_ORDER.len();
        let pos = Self::TAB_ORDER.iter().position(|&k| k == self).unwrap_or(0);
        Self::TAB_ORDER[(pos + n - 1) % n]
    }

    pub fn tab_label(self) -> &'static str {
        match self {
            ChatKind::Social => "Chat",
            ChatKind::Battle => "Log",
            ChatKind::Debug => "Debug",
        }
    }

    pub fn accepts(self, c: ChatChannel) -> bool {
        match self {
            ChatKind::Battle => matches!(c, ChatChannel::Battle | ChatChannel::System),
            ChatKind::Debug => matches!(c, ChatChannel::Debug),
            ChatKind::Social => !matches!(
                c,
                ChatChannel::Battle | ChatChannel::System | ChatChannel::Debug
            ),
        }
    }

    pub fn accepts_in_layout(self, channel: ChatChannel, layout: ChatLayout) -> bool {
        if layout == ChatLayout::Unified {
            self == Self::Social
        } else {
            self.accepts(channel)
        }
    }

    pub fn available_in_layout(layout: ChatLayout, debug_chat: bool) -> &'static [Self] {
        if layout == ChatLayout::Unified {
            &[Self::Social]
        } else {
            Self::available(debug_chat)
        }
    }

    pub fn available(debug_chat: bool) -> &'static [Self] {
        if debug_chat {
            &Self::TAB_ORDER
        } else {
            &Self::TAB_ORDER[..2]
        }
    }

    pub fn step(self, forward: bool, debug_chat: bool) -> Self {
        let kinds = Self::available(debug_chat);
        let pos = kinds.iter().position(|&kind| kind == self).unwrap_or(0);
        let offset = if forward { 1 } else { kinds.len() - 1 };
        kinds[(pos + offset) % kinds.len()]
    }
}

#[derive(Component)]
pub struct ChatPanel {
    pub kind: ChatKind,
}

pub fn advance_split_focus(active: &mut ChatKind, layout: ChatLayout, debug_chat: bool) -> bool {
    if matches!(layout, ChatLayout::Unified | ChatLayout::Tabbed) {
        return false;
    }
    let kinds = ChatKind::available(debug_chat);
    if let Some(next) = kinds
        .iter()
        .position(|kind| kind == active)
        .and_then(|pos| kinds.get(pos + 1))
    {
        *active = *next;
        true
    } else {
        false
    }
}

#[derive(Component)]
pub struct ChatRow {
    pub slot: usize,
}

#[derive(Component)]
pub struct ChatRowBody;

#[derive(Component)]
pub struct ChatRowSpan;

// .agents/skills/retail-observe/references/2026-09-14-chat-windows.md Observed
const AUTOTRANSLATE_OPEN_COLOR: Color = Color::srgb(0.35, 0.90, 0.35);
const AUTOTRANSLATE_CLOSE_COLOR: Color = Color::srgb(1.00, 0.35, 0.35);
const LOG_TEXT_COLOR: Color = Color::srgb(0.95, 0.95, 0.55);
const ACTION_TEXT_COLOR: Color = Color::srgb(1.00, 1.00, 0.20);
const YELL_TEXT_COLOR: Color = Color::srgb(1.00, 0.50, 0.50);

pub fn spawn_chat_panels_as_children(p: &mut ChildSpawnerCommands) {
    p.spawn((
        ChatPanelGroup,
        ChatArea::default(),
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            column_gap: Val::Px(CHAT_WINDOW_GAP_PX),
            row_gap: Val::Px(CHAT_WINDOW_GAP_PX),
            ..default()
        },
    ))
    .with_children(|p| {
        spawn_panel(p, ChatKind::Social, Display::Flex);
        spawn_panel(p, ChatKind::Battle, Display::None);
        spawn_panel(p, ChatKind::Debug, Display::None);
    });
}

pub fn spawn_chat_tab_bar_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        ChatTabBar,
        Node {
            height: Val::Px(CHAT_TAB_HEIGHT_PX),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(2.0),
            ..default()
        },
    ))
    .with_children(|p| {
        for kind in ChatKind::TAB_ORDER {
            spawn_tab_button(p, kind, kind.tab_label(), kind == ChatKind::default());
        }
        spawn_auto_switch_toggle(p);
    });
}

fn spawn_auto_switch_toggle(p: &mut ChildSpawnerCommands) {
    p.spawn((
        Button,
        ChatAutoSwitchToggle,
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),

            margin: UiRect::left(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(theme::FRAME_BG),
        BorderColor::all(theme::FRAME_EDGE),
    ))
    .with_children(|btn| {
        btn.spawn((
            ChatAutoSwitchLabel,
            Text::new("Auto: off"),
            style::text_font(12.0),
            TextColor(theme::CURSOR),
        ));
    });
}

fn spawn_tab_button(p: &mut ChildSpawnerCommands, kind: ChatKind, label: &str, is_active: bool) {
    let (bg, fg, border) = if is_active {
        (theme::FRAME_BG, theme::CURSOR, theme::CURSOR)
    } else {
        (theme::FRAME_BG, theme::MUTED, theme::FRAME_EDGE)
    };
    p.spawn((
        Button,
        ChatTabButton { kind },
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(bg),
        BorderColor::all(border),
    ))
    .with_children(|btn| {
        btn.spawn((
            ChatTabButtonLabel,
            Text::new(label.to_string()),
            style::text_font(12.0),
            TextColor(fg),
        ));
    });
}

fn spawn_panel(parent: &mut ChildSpawnerCommands, kind: ChatKind, initial_display: Display) {
    parent
        .spawn((
            ChatPanel { kind },
            ChatPanelDecay::default(),
            RelativeCursorPosition::default(),
            Node {
                width: Val::Percent(100.0),
                min_width: Val::Px(0.0),
                min_height: Val::Px(0.0),
                flex_basis: Val::Auto,
                flex_grow: 0.0,
                flex_shrink: 1.0,
                height: Val::Px(PANEL_MIN_HEIGHT_PX),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                row_gap: Val::Px(2.0),
                display: initial_display,

                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(theme::FRAME_BG),
            BorderColor::all(theme::FRAME_EDGE),
        ))
        .with_children(|p| {
            for slot in 0..EXPANDED_ROWS {
                p.spawn((
                    ChatRow { slot },
                    Node {
                        flex_direction: FlexDirection::Row,
                        flex_shrink: 0.0,
                        width: Val::Percent(100.0),
                        min_width: Val::Px(0.0),
                        ..default()
                    },
                ))
                .with_children(|row| {
                    row.spawn((
                        ChatRowBody,
                        Text::new(""),
                        TextLayout {
                            linebreak: LineBreak::WordOrCharacter,
                            ..default()
                        },
                        Node {
                            max_width: Val::Percent(100.0),
                            ..default()
                        },
                        style::text_font(13.0),
                        TextColor(theme::TEXT),
                    ));
                });
            }
        });
}

pub fn update_chat_panel(
    mut commands: Commands,
    time: Res<Time>,
    state: Res<SceneState>,
    mode: Res<InputMode>,
    active: Res<ActiveChatTab>,
    scroll: Res<ChatScroll>,
    battle_scroll: Res<BattleScroll>,
    debug_scroll: Res<DebugScroll>,
    mut panel_q: Query<(
        &ChatPanel,
        &mut BorderColor,
        &mut Node,
        &mut ChatPanelDecay,
        &RelativeCursorPosition,
        &Children,
    )>,
    rows: Query<(&ChatRow, &Children), Without<ChatPanel>>,
    body_q: Query<Option<&Children>, With<ChatRowBody>>,
    mut span_q: Query<(&mut TextSpan, &mut TextColor), With<ChatRowSpan>>,
    graphics: Res<GraphicsSettings>,
    area: Query<&ChatArea>,
) {
    let now = time.elapsed_secs();
    let height_limit = area
        .single()
        .map(|area| area.panel_height)
        .unwrap_or(PANEL_MAX_HEIGHT_PX);

    let refill = state.is_changed()
        || mode.is_changed()
        || scroll.is_changed()
        || battle_scroll.is_changed()
        || debug_scroll.is_changed()
        || graphics.is_changed();

    let all = if refill {
        rendered_chat(&state)
    } else {
        Vec::new()
    };

    for (panel, mut border, mut node, mut decay, rel_cursor, panel_children) in &mut panel_q {
        let filtered: Option<Vec<&ChatLine>> = refill.then(|| {
            all.iter()
                .copied()
                .filter(|l| {
                    panel
                        .kind
                        .accepts_in_layout(l.channel, graphics.chat_layout)
                        && crate::snapshot::chat_line_visible(l.channel, graphics.debug_chat)
                })
                .collect()
        });

        let scroll_offset = match panel.kind {
            ChatKind::Social => scroll.rows,
            ChatKind::Battle => battle_scroll.rows,
            ChatKind::Debug => debug_scroll.rows,
        };

        let chat_focused = panel.kind == active.0
            && matches!(
                &*mode,
                InputMode::PassiveCursor(s) if matches!(s.focus, PassiveCursorFocus::Chat)
            );
        let chat_expanded = panel.kind == active.0
            && matches!(
                &*mode,
                InputMode::PassiveCursor(s) if s.chat_expanded && matches!(s.focus, PassiveCursorFocus::Chat)
            );
        let focused = scroll_offset != 0 || chat_focused;
        let want_border = if chat_focused {
            theme::CURSOR
        } else {
            theme::FRAME_EDGE
        };
        if border.left != want_border {
            *border = BorderColor::all(want_border);
        }

        if let Some(filtered) = &filtered {
            let new_msg = filtered.len() > decay.prev_filtered_len;
            decay.prev_filtered_len = filtered.len();
            if new_msg {
                decay.last_active_secs = now;
            }
        }
        let interacted = rel_cursor.cursor_over() || scroll_offset != 0 || focused;
        if interacted {
            decay.last_active_secs = now;
        }
        let target_h = if chat_expanded {
            PANEL_EXPANDED_HEIGHT_PX
        } else {
            let idle = (now - decay.last_active_secs).max(0.0);
            let t = ((idle - FULL_HOLD_SECS) / FADE_SECS).clamp(0.0, 1.0);
            PANEL_MAX_HEIGHT_PX + (PANEL_MIN_HEIGHT_PX - PANEL_MAX_HEIGHT_PX) * t
        };
        let want_h = Val::Px(target_h.min(height_limit));
        if node.height != want_h {
            node.height = want_h;
        }

        let Some(filtered) = filtered else {
            continue;
        };

        // Fill every pre-spawned row (newest at the bottom); the panel height +
        // overflow clip windows how many actually show, so compact reveals the
        // newest ~VISIBLE_ROWS and expand grows the height to reveal the rest.
        let visible: Vec<Option<&ChatLine>> = (0..EXPANDED_ROWS)
            .rev()
            .map(|i| {
                let n = filtered.len();
                let newest_visible = n.checked_sub(1 + scroll_offset);
                match newest_visible {
                    Some(top) => {
                        if i <= top {
                            Some(filtered[top - i])
                        } else {
                            None
                        }
                    }
                    None => None,
                }
            })
            .collect();

        for child in panel_children.iter() {
            let Ok((row, row_children)) = rows.get(child) else {
                continue;
            };
            let line = visible.get(row.slot).copied().flatten();

            for body_child in row_children.iter() {
                let Ok(span_children) = body_q.get(body_child) else {
                    continue;
                };

                let segments: Vec<(String, Color)> = match line {
                    Some(l) => segment_line(l),
                    None => Vec::new(),
                };
                for (i, span_child) in span_children
                    .into_iter()
                    .flat_map(|children| children.iter())
                    .enumerate()
                {
                    let Ok((mut span_text, mut span_color)) = span_q.get_mut(span_child) else {
                        continue;
                    };
                    let (want_text, want_color): (&str, Color) = segments
                        .get(i)
                        .map(|(t, c)| (t.as_str(), *c))
                        .unwrap_or(("", theme::TEXT));
                    if span_text.as_str() != want_text {
                        **span_text = want_text.to_string();
                    }
                    if span_color.0 != want_color {
                        span_color.0 = want_color;
                    }
                }
                let existing = span_children.map_or(0, |children| children.len());
                if segments.len() > existing {
                    commands.entity(body_child).with_children(|body| {
                        for (text, color) in segments.iter().skip(existing) {
                            body.spawn((
                                ChatRowSpan,
                                TextSpan::new(text.clone()),
                                style::text_font(13.0),
                                TextColor(*color),
                            ));
                        }
                    });
                }
            }
        }
    }
}

pub fn format_chat_line(channel: ChatChannel, sender: &str, text: &str) -> String {
    // A blank sender is the "No speaker object displayed" (NS_*) case: retail
    // renders the text with no name prefix, keeping the channel's color.
    if sender.is_empty()
        && matches!(
            channel,
            ChatChannel::Say
                | ChatChannel::Shout
                | ChatChannel::Party
                | ChatChannel::Linkshell
                | ChatChannel::Yell
                | ChatChannel::Other
        )
    {
        return text.to_string();
    }
    match channel {
        ChatChannel::Say | ChatChannel::Shout | ChatChannel::Other => {
            format!("{sender} : {text}")
        }

        ChatChannel::Tell => format!(">>{sender} : {text}"),

        ChatChannel::Party => format!("({sender}) {text}"),

        ChatChannel::Linkshell => format!("<{sender}> {text}"),

        ChatChannel::Yell => format!("[{sender}] : {text}"),

        ChatChannel::System | ChatChannel::Battle => text.to_string(),

        // Canned emotes arrive fully composed (empty sender); /em free text
        // arrives as (sender, body) and retail renders "Name body".
        ChatChannel::Emote => {
            if sender.is_empty() {
                text.to_string()
            } else {
                format!("{sender} {text}")
            }
        }

        ChatChannel::Debug => format!("[dbg] {text}"),
    }
}

/// Split one chat line into coloured runs. A line that carries retail's
/// per-substitution spans keeps them (the item name in a treasure line renders
/// green against the channel colour); anything else takes the channel colour
/// with autotranslate phrases picked out.
pub fn segment_line(l: &ChatLine) -> Vec<(String, Color)> {
    let base = channel_color(l.channel);
    if l.spans.is_empty() {
        let formatted = format_chat_line(l.channel, &l.sender, &l.text);
        return segment_chat_line(&formatted, base);
    }
    // The channel's name prefix still belongs in front of the spans; it is
    // empty for the unattributed channels the spanned lines actually use.
    let prefix = format_chat_line(l.channel, &l.sender, "");
    std::iter::once((prefix, base))
        .chain(
            l.spans
                .iter()
                .flat_map(|s| segment_chat_line(&s.text, span_color(s.kind, base))),
        )
        .filter(|(t, _)| !t.is_empty())
        .collect()
}

// .agents/skills/retail-observe/references/2026-09-14-chat-windows.md Observed
const ITEM_NAME_COLOR: Color = Color::srgb(0.65, 1.00, 0.15);

pub fn span_color(kind: ChatSpanKind, base: Color) -> Color {
    match kind {
        ChatSpanKind::Text => base,
        ChatSpanKind::Item | ChatSpanKind::KeyItem => ITEM_NAME_COLOR,
    }
}

pub fn segment_chat_line(line: &str, base: Color) -> Vec<(String, Color)> {
    use ffxi_proto::autotranslate::{PHRASE_CLOSE, PHRASE_OPEN};
    let mut out: Vec<(String, Color)> = ffxi_proto::autotranslate::split_phrases(line)
        .into_iter()
        .flat_map(|span| {
            if !span.is_phrase {
                return vec![(span.text, base)];
            }
            let body = span.text.strip_prefix(PHRASE_OPEN).unwrap_or(&span.text);
            let closed = body.ends_with(PHRASE_CLOSE);
            let body = body.strip_suffix(PHRASE_CLOSE).unwrap_or(body);
            let mut segments = vec![
                (PHRASE_OPEN.to_string(), AUTOTRANSLATE_OPEN_COLOR),
                (body.to_string(), base),
            ];
            if closed {
                segments.push((PHRASE_CLOSE.to_string(), AUTOTRANSLATE_CLOSE_COLOR));
            }
            segments
        })
        .collect();
    if out.is_empty() {
        out.push((String::new(), base));
    }
    out
}

pub fn channel_color(c: ChatChannel) -> Color {
    match c {
        ChatChannel::Say => theme::TEXT,
        ChatChannel::Shout => Color::srgb(1.00, 0.65, 0.45),
        ChatChannel::Tell => Color::srgb(0.95, 0.40, 0.95),
        ChatChannel::Party => Color::srgb(0.50, 0.65, 1.00),
        ChatChannel::Linkshell => Color::srgb(0.40, 0.95, 0.50),
        ChatChannel::Yell => YELL_TEXT_COLOR,
        ChatChannel::System => LOG_TEXT_COLOR,
        ChatChannel::Other => theme::FAINT,

        ChatChannel::Battle => ACTION_TEXT_COLOR,

        ChatChannel::Debug => Color::srgb(0.55, 0.75, 0.80),

        // Retail's channel-8 emote color is unverified (bead kuluu-d4u
        // retail_unknowns); reuse the say color until captured.
        ChatChannel::Emote => theme::TEXT,
    }
}

pub fn chat_wheel_scroll_system(
    mut wheel: MessageReader<MouseWheel>,
    panel_q: Query<(&ChatPanel, &Node, &RelativeCursorPosition)>,
    state: Res<SceneState>,
    mut scroll: ResMut<ChatScroll>,
    mut battle_scroll: ResMut<BattleScroll>,
    mut debug_scroll: ResMut<DebugScroll>,
    mut accum: ResMut<ChatScrollAccum>,
    mut battle_accum: ResMut<BattleScrollAccum>,
    mut debug_accum: ResMut<DebugScrollAccum>,
    mut pointer: ResMut<MousePointer>,
    graphics: Res<GraphicsSettings>,
) {
    let mut delta: f32 = 0.0;
    for ev in wheel.read() {
        delta += ev.y;
    }
    if delta == 0.0 {
        return;
    }

    let mut hovered: Option<ChatKind> = None;
    for (panel, node, rel) in &panel_q {
        if node.display != Display::None && rel.cursor_over() {
            hovered = Some(panel.kind);
            break;
        }
    }
    let Some(kind) = hovered else {
        return;
    };

    let all = rendered_chat(&state);
    let buffer_len = all
        .iter()
        .filter(|l| {
            kind.accepts_in_layout(l.channel, graphics.chat_layout)
                && crate::snapshot::chat_line_visible(l.channel, graphics.debug_chat)
        })
        .count();
    match kind {
        ChatKind::Social => {
            let (rows, frac) = apply_wheel_delta(scroll.rows, accum.frac, delta, buffer_len);
            scroll.rows = rows;
            accum.frac = frac;
        }
        ChatKind::Battle => {
            let (rows, frac) =
                apply_wheel_delta(battle_scroll.rows, battle_accum.frac, delta, buffer_len);
            battle_scroll.rows = rows;
            battle_accum.frac = frac;
        }
        ChatKind::Debug => {
            let (rows, frac) =
                apply_wheel_delta(debug_scroll.rows, debug_accum.frac, delta, buffer_len);
            debug_scroll.rows = rows;
            debug_accum.frac = frac;
        }
    }

    pointer.wheel = 0.0;
}

pub fn chat_tab_click_system(
    interactions: Query<(&Interaction, &ChatTabButton), Changed<Interaction>>,
    mut active: ResMut<ActiveChatTab>,
) {
    for (interaction, button) in &interactions {
        if *interaction == Interaction::Pressed && active.0 != button.kind {
            active.0 = button.kind;
        }
    }
}

pub fn apply_chat_layout(
    graphics: Res<GraphicsSettings>,
    ui_scale: Res<UiScale>,
    mode: Res<InputMode>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    obstacles: Query<
        (
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
            Option<&InheritedVisibility>,
        ),
        Or<(
            With<super::panel_column::ColumnPanel>,
            With<super::menu::MainMenu>,
        )>,
    >,
    tools: Query<&ComputedNode, With<super::ChatTools>>,
    mut nodes: Query<
        (&mut Node, Option<&mut ChatArea>, Has<ChatTabBar>),
        (
            Or<(
                With<super::BottomLeftStack>,
                With<ChatPanelGroup>,
                With<ChatTabBar>,
            )>,
            Without<super::panel_column::ColumnPanel>,
            Without<super::menu::MainMenu>,
        ),
    >,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let viewport = Vec2::new(window.width(), window.height()) / ui_scale.0;
    let obstacles: Vec<Rect> = obstacles
        .iter()
        .filter_map(|(node, computed, transform, visibility)| {
            if node.display == Display::None
                || visibility.is_some_and(|v| !v.get())
                || computed.size().min_element() <= 0.0
            {
                return None;
            }
            let scale = computed.inverse_scale_factor();
            Some(Rect::from_center_size(
                transform.translation * scale,
                computed.size() * scale,
            ))
        })
        .collect();
    let tools_height = tools
        .iter()
        .map(|node| node.size().y * node.inverse_scale_factor())
        .fold(0.0_f32, f32::max);
    let tabbed = graphics.chat_layout == ChatLayout::Tabbed;
    let window_count =
        ChatKind::available_in_layout(graphics.chat_layout, graphics.debug_chat).len();
    let expanded = matches!(&*mode, InputMode::PassiveCursor(state) if state.chat_expanded);
    let preferred_height = if expanded {
        PANEL_EXPANDED_HEIGHT_PX
    } else {
        PANEL_MAX_HEIGHT_PX
    };
    let mut horizontal = horizontal_chat_layout(graphics.chat_layout, viewport.x);
    let layout_region = |horizontal: bool| {
        let rows = if tabbed || horizontal {
            1
        } else {
            window_count
        };
        let reserved = tools_height
            + if tabbed {
                CHAT_TAB_HEIGHT_PX + CHAT_WINDOW_GAP_PX
            } else {
                0.0
            }
            + CHAT_WINDOW_GAP_PX * rows as f32;
        let wanted = preferred_height * rows as f32 + reserved;
        let region = available_chat_region(viewport, wanted, reserved, &obstacles);
        (
            region,
            ((region.height() - reserved) / rows as f32).max(0.0),
        )
    };
    let (mut region, mut panel_height) = layout_region(horizontal);
    if horizontal && !horizontal_chat_layout(graphics.chat_layout, region.width()) {
        horizontal = false;
        (region, panel_height) = layout_region(horizontal);
    }
    for (mut node, area, tab_bar) in &mut nodes {
        if let Some(mut area) = area {
            area.horizontal = horizontal;
            area.panel_height = panel_height;
            node.flex_direction = if horizontal {
                FlexDirection::Row
            } else {
                FlexDirection::Column
            };
        } else if tab_bar {
            node.display = if tabbed { Display::Flex } else { Display::None };
        } else {
            node.width = Val::Px(region.width());
            node.max_width = Val::Auto;
        }
    }
}

fn available_chat_region(
    viewport: Vec2,
    wanted_height: f32,
    reserved: f32,
    obstacles: &[Rect],
) -> Rect {
    let bottom = (viewport.y - super::BOTTOM_LEFT_INSET_PX).max(0.0);
    let top = (bottom - wanted_height)
        .max(CHAT_TOP_CLEARANCE_PX)
        .min(bottom);
    let widths = std::iter::once(viewport.x).chain(
        obstacles
            .iter()
            .map(|rect| (rect.min.x - CHAT_WINDOW_GAP_PX).clamp(0.0, viewport.x)),
    );
    widths
        .map(|width| {
            let mut candidate_top = top;
            for obstacle in obstacles {
                if obstacle.min.x < width
                    && obstacle.max.x > 0.0
                    && obstacle.min.y < bottom
                    && obstacle.max.y > candidate_top
                {
                    candidate_top = candidate_top
                        .max(obstacle.max.y + CHAT_WINDOW_GAP_PX)
                        .min(bottom);
                }
            }
            Rect::from_corners(Vec2::new(0.0, candidate_top), Vec2::new(width, bottom))
        })
        .max_by(|a, b| {
            let usable = |rect: &Rect| rect.width() * (rect.height() - reserved).max(0.0);
            usable(a)
                .total_cmp(&usable(b))
                .then_with(|| a.width().total_cmp(&b.width()))
        })
        .unwrap_or_default()
}

fn horizontal_chat_layout(layout: ChatLayout, width: f32) -> bool {
    layout == ChatLayout::SideBySide && width >= CHAT_SPLIT_MIN_WIDTH_PX
}

pub fn chat_auto_switch_click_system(
    interactions: Query<&Interaction, (Changed<Interaction>, With<ChatAutoSwitchToggle>)>,
    mut auto: ResMut<ChatAutoSwitch>,
) {
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            auto.0 = !auto.0;
        }
    }
}

pub fn chat_auto_switch_and_unread_system(
    state: Res<SceneState>,
    auto: Res<ChatAutoSwitch>,
    mut active: ResMut<ActiveChatTab>,
    mut unread: ResMut<ChatUnread>,
    mut tracker: ResMut<ChatActivityTracker>,
    graphics: Res<GraphicsSettings>,
    mode: Res<InputMode>,
) {
    if !ChatKind::available_in_layout(graphics.chat_layout, graphics.debug_chat).contains(&active.0)
    {
        active.0 = ChatKind::Social;
    }
    let all = rendered_chat(&state);
    let count = |kind: ChatKind| {
        all.iter()
            .filter(|l| {
                kind.accepts(l.channel)
                    && crate::snapshot::chat_line_visible(l.channel, graphics.debug_chat)
            })
            .count()
    };
    let kinds = [
        (ChatKind::Social, count(ChatKind::Social), tracker.social),
        (ChatKind::Battle, count(ChatKind::Battle), tracker.battle),
        (ChatKind::Debug, count(ChatKind::Debug), tracker.debug),
    ];
    let mut to_switch: Option<ChatKind> = None;
    for (kind, now_count, prev_count) in kinds {
        if now_count > prev_count
            && kind != active.0
            && ChatKind::available_in_layout(graphics.chat_layout, graphics.debug_chat)
                .contains(&kind)
        {
            if !unread.get(kind) {
                unread.set(kind, true);
            }
            to_switch = Some(kind);
        }
    }
    tracker.social = kinds[0].1;
    tracker.battle = kinds[1].1;
    tracker.debug = kinds[2].1;
    if auto.0 && graphics.chat_layout == ChatLayout::Tabbed && matches!(*mode, InputMode::World) {
        if let Some(kind) = to_switch {
            if active.0 != kind {
                active.0 = kind;
            }
        }
    }

    for &kind in ChatKind::available(graphics.debug_chat) {
        if (graphics.chat_layout != ChatLayout::Tabbed || kind == active.0) && unread.get(kind) {
            unread.set(kind, false);
        }
    }
}

pub fn update_chat_tab_visuals_system(
    active: Res<ActiveChatTab>,
    unread: Res<ChatUnread>,
    auto: Res<ChatAutoSwitch>,
    graphics: Res<GraphicsSettings>,
    mut panel_q: Query<(&ChatPanel, &mut Node), Without<ChatTabButton>>,
    mut tab_q: Query<
        (&ChatTabButton, &mut BorderColor, &mut Node, &Children),
        (
            Without<ChatPanel>,
            Without<ChatTabButtonLabel>,
            Without<ChatAutoSwitchToggle>,
        ),
    >,
    mut tab_label_q: Query<
        &mut TextColor,
        (With<ChatTabButtonLabel>, Without<ChatAutoSwitchLabel>),
    >,
    mut toggle_label_q: Query<
        (&mut Text, &mut TextColor),
        (With<ChatAutoSwitchLabel>, Without<ChatTabButtonLabel>),
    >,
    mut toggle_q: Query<
        (&mut BorderColor, &Children),
        (
            With<ChatAutoSwitchToggle>,
            Without<ChatTabButton>,
            Without<ChatPanel>,
        ),
    >,
) {
    for (panel, mut node) in &mut panel_q {
        let want = if ChatKind::available_in_layout(graphics.chat_layout, graphics.debug_chat)
            .contains(&panel.kind)
            && (graphics.chat_layout != ChatLayout::Tabbed || panel.kind == active.0)
        {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }

    for (button, mut border, mut node, children) in &mut tab_q {
        node.display = if ChatKind::available_in_layout(graphics.chat_layout, graphics.debug_chat)
            .contains(&button.kind)
        {
            Display::Flex
        } else {
            Display::None
        };
        let is_active = button.kind == active.0;
        let is_unread = !is_active && unread.get(button.kind);
        let (border_c, label_c) = if is_active {
            (theme::CURSOR, theme::CURSOR)
        } else if is_unread {
            (theme::FRAME_EDGE, theme::TEXT)
        } else {
            (theme::FRAME_EDGE, theme::MUTED)
        };
        if border.left != border_c {
            *border = BorderColor::all(border_c);
        }
        for child in children.iter() {
            if let Ok(mut tc) = tab_label_q.get_mut(child) {
                if tc.0 != label_c {
                    tc.0 = label_c;
                }
            }
        }
    }

    let (want_text, want_color, want_border) = if auto.0 {
        ("Auto: on", theme::CURSOR, theme::CURSOR)
    } else {
        ("Auto: off", theme::MUTED, theme::FRAME_EDGE)
    };
    for (mut border, children) in &mut toggle_q {
        if border.left != want_border {
            *border = BorderColor::all(want_border);
        }
        for child in children.iter() {
            if let Ok((mut text, mut color)) = toggle_label_q.get_mut(child) {
                if text.as_str() != want_text {
                    **text = want_text.to_string();
                }
                if color.0 != want_color {
                    color.0 = want_color;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use kuluu_snapshot::ChatSpan;

    #[test]
    fn unified_log_clears_hidden_selection_and_reads_combined_history() {
        let mut app = App::new();
        app.init_resource::<SceneState>()
            .init_resource::<ChatAutoSwitch>()
            .init_resource::<ChatUnread>()
            .init_resource::<ChatActivityTracker>()
            .init_resource::<GraphicsSettings>()
            .init_resource::<InputMode>()
            .insert_resource(ActiveChatTab(ChatKind::Battle))
            .add_systems(Update, chat_auto_switch_and_unread_system);
        app.world_mut().resource_mut::<ChatUnread>().battle = true;
        app.world_mut()
            .resource_mut::<SceneState>()
            .snapshot
            .chat
            .push(drop_line());
        app.update();
        assert_eq!(app.world().resource::<ActiveChatTab>().0, ChatKind::Social);
        assert!(!app.world().resource::<ChatUnread>().battle);
        app.world_mut()
            .resource_mut::<GraphicsSettings>()
            .chat_layout = ChatLayout::Tabbed;
        app.update();
        assert!(!app.world().resource::<ChatUnread>().battle);
    }

    #[test]
    fn unified_log_combines_channels_without_hidden_window_focus() {
        let layout = ChatLayout::default();
        assert_eq!(layout, ChatLayout::Unified);
        for debug_chat in [false, true] {
            assert_eq!(
                ChatKind::available_in_layout(layout, debug_chat),
                &[ChatKind::Social]
            );
            let mut active = ChatKind::Social;
            assert!(!advance_split_focus(&mut active, layout, debug_chat));
            assert_eq!(active, ChatKind::Social);
        }
        for channel in [
            ChatChannel::Say,
            ChatChannel::Battle,
            ChatChannel::System,
            ChatChannel::Debug,
        ] {
            assert!(ChatKind::Social.accepts_in_layout(channel, layout));
            assert!(!ChatKind::Battle.accepts_in_layout(channel, layout));
            assert!(!ChatKind::Debug.accepts_in_layout(channel, layout));
        }
    }

    #[test]
    fn chat_fills_the_viewport_until_a_visible_hud_obstructs_it() {
        let viewport = Vec2::new(1920.0, 1080.0);
        let clear = available_chat_region(viewport, 220.0, 0.0, &[]);
        assert_eq!(clear.width(), viewport.x);
        let party = Rect::from_corners(Vec2::new(1600.0, 800.0), Vec2::new(1912.0, 1072.0));
        let region = available_chat_region(viewport, 220.0, 0.0, &[party]);
        assert!(region.max.x < party.min.x);
        assert_eq!(region.height(), clear.height());
        assert_eq!(region.min.x, 0.0);
    }

    #[test]
    fn menus_only_reserve_space_when_they_reach_the_chat_stack() {
        let viewport = Vec2::new(1920.0, 1080.0);
        let menu = Rect::from_corners(Vec2::new(1200.0, 48.0), Vec2::new(1912.0, 700.0));
        let compact = available_chat_region(viewport, 220.0, 0.0, &[menu]);
        assert_eq!(compact.width(), viewport.x);
        let expanded = available_chat_region(viewport, 700.0, 100.0, &[menu]);
        assert!(expanded.max.x <= menu.min.x || expanded.min.y >= menu.max.y);
        assert!(expanded.height() > 100.0);
    }

    #[test]
    fn toolbar_space_is_preserved_when_choosing_between_free_regions() {
        let viewport = Vec2::new(800.0, 450.0);
        let menu = Rect::from_corners(Vec2::new(500.0, 48.0), Vec2::new(792.0, 350.0));
        let region = available_chat_region(viewport, 400.0, 180.0, &[menu]);
        assert!(region.max.x < menu.min.x);
        assert!(region.height() > 180.0);
        assert!(region.min.y >= CHAT_TOP_CLEARANCE_PX);
    }

    fn drop_line() -> ChatLine {
        // What kuluu's treasure handler emits for s2c 0x0D2, composed
        // from the retail system-message table.
        ChatLine {
            channel: ChatChannel::System,
            sender: String::new(),
            text: "You find a Lizard Tail on the Rock Lizard.".into(),
            server_ts: 0,
            local_seq: 0,
            spans: vec![
                ChatSpan {
                    text: "You find a ".into(),
                    kind: ChatSpanKind::Text,
                },
                ChatSpan {
                    text: "Lizard Tail".into(),
                    kind: ChatSpanKind::Item,
                },
                ChatSpan {
                    text: " on the Rock Lizard.".into(),
                    kind: ChatSpanKind::Text,
                },
            ],
        }
    }

    #[test]
    fn a_drop_line_colours_only_the_item_name() {
        let segs = segment_line(&drop_line());
        let texts: Vec<&str> = segs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            texts,
            vec!["You find a ", "Lizard Tail", " on the Rock Lizard."]
        );
        assert_eq!(segs[1].1, ITEM_NAME_COLOR);
        assert_eq!(segs[0].1, segs[2].1, "the text around it shares one colour");
        assert_ne!(segs[0].1, ITEM_NAME_COLOR);
    }

    #[test]
    fn joined_spans_reproduce_the_plain_text() {
        // The flat `text` is what headless/agent consumers read, so it must
        // stay the concatenation of what the panel draws.
        let l = drop_line();
        let joined: String = segment_line(&l).iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(joined, l.text);
    }

    #[test]
    fn autotranslate_still_splits_inside_a_span() {
        let mut l = drop_line();
        l.spans[2].text = " on the {Rock Lizard}.".into();
        l.text = "You find a Lizard Tail on the {Rock Lizard}.".into();
        let segs = segment_line(&l);
        assert!(
            segs.iter()
                .any(|(t, c)| t == "Rock Lizard" && *c == channel_color(l.channel)),
            "{segs:?}"
        );
    }

    #[test]
    fn a_line_without_spans_takes_the_old_path() {
        let l = ChatLine {
            channel: ChatChannel::Say,
            sender: "Daisy".into(),
            text: "hi".into(),
            server_ts: 0,
            local_seq: 0,
            spans: Vec::new(),
        };
        let segs = segment_line(&l);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "Daisy : hi");
    }

    #[test]
    fn a_spanned_line_on_an_attributed_channel_keeps_its_name_prefix() {
        let mut l = drop_line();
        l.channel = ChatChannel::Party;
        l.sender = "Daisy".into();
        let segs = segment_line(&l);
        assert_eq!(segs[0].0, "(Daisy) ");
        assert!(segs.iter().any(|(_, c)| *c == ITEM_NAME_COLOR));
    }

    #[test]
    fn cycle_next_steps_all_tabs_and_wraps() {
        assert_eq!(ChatKind::Social.cycle_next(), ChatKind::Battle);
        assert_eq!(ChatKind::Battle.cycle_next(), ChatKind::Debug);
        assert_eq!(ChatKind::Debug.cycle_next(), ChatKind::Social);
    }

    #[test]
    fn segment_plain_text_is_single_base_span() {
        let segs = segment_chat_line("hello world", theme::TEXT);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "hello world");
        assert_eq!(segs[0].1, theme::TEXT);
    }

    #[test]
    fn auto_translate_colors_only_the_markers_and_preserves_text() {
        let line = "hello {Looking for Party} {Experience points}";
        let segments = segment_chat_line(line, theme::TEXT);
        assert_eq!(
            segments
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<String>(),
            line
        );
        for (text, color) in segments {
            let expected = match text.as_str() {
                "{" => AUTOTRANSLATE_OPEN_COLOR,
                "}" => AUTOTRANSLATE_CLOSE_COLOR,
                _ => theme::TEXT,
            };
            assert_eq!(color, expected);
        }
    }

    #[test]
    fn segment_empty_input_yields_single_empty_segment() {
        let segs = segment_chat_line("", theme::TEXT);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].0.is_empty());
    }

    #[test]
    fn segment_unclosed_brace_does_not_lose_tail() {
        let segs = segment_chat_line("foo {open and never close", theme::TEXT);
        let joined: String = segs.iter().map(|(t, _)| t.as_str()).collect();
        assert!(joined.contains("open and never close"));
        assert!(joined.contains('{'));
    }

    #[test]
    fn heavily_formatted_shout_keeps_every_phrase_and_tail() {
        let line = "{a}{b}{c}{d}{e}{f}{g} tail";
        let segments = segment_chat_line(line, theme::TEXT);
        assert_eq!(
            segments
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<String>(),
            line
        );
    }

    #[test]
    fn say_format_is_name_colon_text() {
        assert_eq!(
            format_chat_line(ChatChannel::Say, "Daisy", "hi"),
            "Daisy : hi"
        );
    }

    #[test]
    fn shout_uses_same_format_as_say() {
        assert_eq!(
            format_chat_line(ChatChannel::Shout, "Daisy", "hi"),
            "Daisy : hi"
        );
    }

    #[test]
    fn tell_prepends_double_arrow() {
        assert_eq!(
            format_chat_line(ChatChannel::Tell, "Daisy", "hi"),
            ">>Daisy : hi"
        );
    }

    #[test]
    fn party_uses_parens_no_colon() {
        assert_eq!(
            format_chat_line(ChatChannel::Party, "Daisy", "hi"),
            "(Daisy) hi"
        );
    }

    #[test]
    fn linkshell_uses_angle_brackets_no_colon() {
        assert_eq!(
            format_chat_line(ChatChannel::Linkshell, "Daisy", "hi"),
            "<Daisy> hi"
        );
    }

    #[test]
    fn yell_uses_square_brackets() {
        assert_eq!(
            format_chat_line(ChatChannel::Yell, "Daisy", "hi"),
            "[Daisy] : hi"
        );
    }

    #[test]
    fn system_and_battle_omit_sender() {
        assert_eq!(
            format_chat_line(ChatChannel::System, "ignored", "Welcome to Vana'diel."),
            "Welcome to Vana'diel."
        );
        assert_eq!(
            format_chat_line(
                ChatChannel::Battle,
                "ignored",
                "Daisy hits the Mandragora for 12 points of damage."
            ),
            "Daisy hits the Mandragora for 12 points of damage."
        );
    }

    #[test]
    fn empty_text_still_renders_sender_layout() {
        assert_eq!(format_chat_line(ChatChannel::Say, "Daisy", ""), "Daisy : ");
    }

    #[test]
    fn log_groups_system_and_actions_separately_from_social() {
        for channel in [ChatChannel::System, ChatChannel::Battle] {
            assert!(ChatKind::Battle.accepts(channel));
            assert!(!ChatKind::Social.accepts(channel));
            assert!(!ChatKind::Debug.accepts(channel));
        }
        for channel in [
            ChatChannel::Say,
            ChatChannel::Yell,
            ChatChannel::Tell,
            ChatChannel::Party,
        ] {
            assert!(ChatKind::Social.accepts(channel));
            assert!(!ChatKind::Battle.accepts(channel));
        }
    }

    #[test]
    fn split_focus_visits_visible_windows_before_releasing() {
        for layout in [ChatLayout::Vertical, ChatLayout::SideBySide] {
            let mut active = ChatKind::Social;
            assert!(advance_split_focus(&mut active, layout, false));
            assert_eq!(active, ChatKind::Battle);
            assert!(!advance_split_focus(&mut active, layout, false));
            assert!(advance_split_focus(&mut active, layout, true));
            assert_eq!(active, ChatKind::Debug);
            assert!(!advance_split_focus(&mut active, layout, true));
        }
        let mut active = ChatKind::Social;
        assert!(!advance_split_focus(&mut active, ChatLayout::Tabbed, false));
        assert_eq!(ChatKind::Battle.step(true, false), ChatKind::Social);
    }

    #[test]
    fn blank_sender_renders_unattributed() {
        // NS_* ("No speaker object displayed") chat: text only, no prefix.
        assert_eq!(
            format_chat_line(
                ChatChannel::Say,
                "",
                "You can set this as your current home point."
            ),
            "You can set this as your current home point."
        );
        assert_eq!(format_chat_line(ChatChannel::Party, "", "hi"), "hi");
    }

    #[test]
    fn small_delta_accumulates_before_stepping() {
        let (rows, accum) = apply_wheel_delta(0, 0.0, 1.0, 100);
        assert_eq!(rows, 0);
        assert!(accum > 0.0 && accum < 1.0);
    }

    #[test]
    fn accumulator_eventually_spends_a_row() {
        let ticks = (1.0 / crate::hud::list_view::WHEEL_ROWS_PER_UNIT).ceil() as usize;
        let mut rows = 0;
        let mut accum = 0.0;
        for _ in 0..ticks {
            (rows, accum) = apply_wheel_delta(rows, accum, 1.0, 100);
        }
        let _ = accum;
        assert_eq!(rows, 1);
    }

    #[test]
    fn equal_total_delta_produces_equal_rows_regardless_of_frame_count() {
        let mut rows = 0usize;
        let mut accum = 0.0f32;
        for _ in 0..12 {
            let (r, a) = apply_wheel_delta(rows, accum, 1.0, 1000);
            rows = r;
            accum = a;
        }
        let high_fps_rows = rows;

        let (low_fps_rows, _) = apply_wheel_delta(0, 0.0, 12.0, 1000);
        assert_eq!(high_fps_rows, low_fps_rows);
    }

    #[test]
    fn wheel_down_at_bottom_stays_at_bottom() {
        let (rows, accum) = apply_wheel_delta(0, 0.0, -100.0, 100);
        assert_eq!(rows, 0);

        assert_eq!(accum, 0.0);
    }

    #[test]
    fn wheel_up_clamps_at_oldest() {
        let (rows, accum) = apply_wheel_delta(4, 0.0, 100.0, 5);
        assert_eq!(rows, 4);
        assert_eq!(accum, 0.0);
    }

    #[test]
    fn empty_buffer_is_noop() {
        assert_eq!(apply_wheel_delta(0, 0.0, 1.0, 0), (0, 0.0));
        assert_eq!(apply_wheel_delta(0, 0.0, -1.0, 0), (0, 0.0));
    }
}
