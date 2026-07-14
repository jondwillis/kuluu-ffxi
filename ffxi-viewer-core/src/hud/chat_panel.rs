use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use ffxi_viewer_wire::{ChatChannel, ChatLine};

use crate::hud::palette;
use crate::input_mode::{InputMode, PassiveCursorFocus};
use crate::mouse::MousePointer;
use crate::snapshot::{rendered_chat, SceneState};

pub const VISIBLE_ROWS: usize = 12;

pub const PANEL_MAX_HEIGHT_PX: f32 = 220.0;
pub const PANEL_MIN_HEIGHT_PX: f32 = 60.0;
pub const FULL_HOLD_SECS: f32 = 10.0;
pub const FADE_SECS: f32 = 10.0;

#[derive(Component, Debug, Default, Clone, Copy)]
pub struct ChatPanelDecay {
    pub last_active_secs: f32,

    pub prev_filtered_len: usize,
}

const WHEEL_ROWS_PER_UNIT: f32 = 0.12;

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

#[derive(Component, Debug, Clone, Copy)]
pub struct ChatTabButton {
    pub kind: ChatKind,
}

#[derive(Component)]
pub struct ChatTabButtonLabel;

#[derive(Resource, Debug, Clone, Copy)]
pub struct ChatAutoSwitch(pub bool);

impl Default for ChatAutoSwitch {
    fn default() -> Self {
        Self(true)
    }
}

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

#[derive(Component)]
pub struct ChatAutoSwitchToggle;

#[derive(Component)]
pub struct ChatAutoSwitchLabel;

impl ChatKind {
    pub fn accepts(self, c: ChatChannel) -> bool {
        match self {
            ChatKind::Battle => matches!(c, ChatChannel::Battle),
            ChatKind::Debug => matches!(c, ChatChannel::System | ChatChannel::Debug),
            ChatKind::Social => !matches!(
                c,
                ChatChannel::Battle | ChatChannel::System | ChatChannel::Debug
            ),
        }
    }
}

#[derive(Component)]
pub struct ChatPanel {
    pub kind: ChatKind,
}

#[derive(Component)]
pub struct ChatRow {
    pub slot: usize,
}

#[derive(Component)]
pub struct ChatRowBody;

#[derive(Component)]
pub struct ChatRowSpan;

const SPANS_PER_ROW: usize = 16;

const AUTOTRANSLATE_COLOR: Color = Color::srgb(0.50, 0.78, 1.00);

pub fn spawn_chat_panels_as_children(p: &mut ChildSpawnerCommands) {
    spawn_panel(p, ChatKind::Social, Display::Flex);
    spawn_panel(p, ChatKind::Battle, Display::None);
    spawn_panel(p, ChatKind::Debug, Display::None);
}

pub fn spawn_chat_tab_bar_as_child(p: &mut ChildSpawnerCommands) {
    p.spawn((
        ChatTabBar,
        Node {
            height: Val::Px(20.0),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(2.0),
            ..default()
        },
    ))
    .with_children(|p| {
        spawn_tab_button(p, ChatKind::Social, "Chat", true);
        spawn_tab_button(p, ChatKind::Battle, "Battle", false);
        spawn_tab_button(p, ChatKind::Debug, "System", false);
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
        BackgroundColor(palette::BACKGROUND),
        BorderColor::all(palette::BORDER),
    ))
    .with_children(|btn| {
        btn.spawn((
            ChatAutoSwitchLabel,
            Text::new("auto \u{2713}"),
            TextFont {
                font_size: 12.0.into(),
                ..default()
            },
            TextColor(palette::ACCENT),
        ));
    });
}

fn spawn_tab_button(p: &mut ChildSpawnerCommands, kind: ChatKind, label: &str, is_active: bool) {
    let (bg, fg, border) = if is_active {
        (palette::BACKGROUND, palette::ACCENT, palette::ACCENT)
    } else {
        (palette::BACKGROUND, palette::MUTED, palette::BORDER)
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
            TextFont {
                font_size: 12.0.into(),
                ..default()
            },
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
            BackgroundColor(palette::BACKGROUND),
            BorderColor::all(palette::BORDER),
        ))
        .with_children(|p| {
            for slot in 0..VISIBLE_ROWS {
                p.spawn((
                    ChatRow { slot },
                    Node {
                        flex_direction: FlexDirection::Row,

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
                        TextFont {
                            font_size: 13.0.into(),
                            ..default()
                        },
                        TextColor(palette::TEXT),
                    ))
                    .with_children(|body| {
                        for _ in 0..SPANS_PER_ROW {
                            body.spawn((
                                ChatRowSpan,
                                TextSpan::new(""),
                                TextFont {
                                    font_size: 13.0.into(),
                                    ..default()
                                },
                                TextColor(palette::TEXT),
                            ));
                        }
                    });
                });
            }
        });
}

pub fn update_chat_panel(
    time: Res<Time>,
    state: Res<SceneState>,
    mode: Res<InputMode>,
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
    body_q: Query<&Children, With<ChatRowBody>>,
    mut span_q: Query<(&mut TextSpan, &mut TextColor), With<ChatRowSpan>>,
) {
    let now = time.elapsed_secs();

    let all = rendered_chat(&state);

    for (panel, mut border, mut node, mut decay, rel_cursor, panel_children) in &mut panel_q {
        let filtered: Vec<&ChatLine> = all
            .iter()
            .copied()
            .filter(|l| panel.kind.accepts(l.channel))
            .collect();

        let scroll_offset = match panel.kind {
            ChatKind::Social => scroll.rows,
            ChatKind::Battle => battle_scroll.rows,
            ChatKind::Debug => debug_scroll.rows,
        };

        let focused = match panel.kind {
            ChatKind::Social => {
                scroll_offset != 0
                    || matches!(
                        &*mode,
                        InputMode::PassiveCursor(s) if matches!(s.focus, PassiveCursorFocus::Chat)
                    )
            }
            ChatKind::Battle | ChatKind::Debug => scroll_offset != 0,
        };
        let want_border = if focused {
            palette::ACCENT
        } else {
            palette::BORDER
        };
        if border.left != want_border {
            *border = BorderColor::all(want_border);
        }

        let new_msg = filtered.len() > decay.prev_filtered_len;
        decay.prev_filtered_len = filtered.len();
        let interacted = rel_cursor.cursor_over() || scroll_offset != 0 || focused;
        if new_msg || interacted {
            decay.last_active_secs = now;
        }
        let idle = (now - decay.last_active_secs).max(0.0);
        let t = ((idle - FULL_HOLD_SECS) / FADE_SECS).clamp(0.0, 1.0);
        let target_h = PANEL_MAX_HEIGHT_PX + (PANEL_MIN_HEIGHT_PX - PANEL_MAX_HEIGHT_PX) * t;
        let want_h = Val::Px(target_h);
        if node.height != want_h {
            node.height = want_h;
        }

        let visible: Vec<Option<&ChatLine>> = (0..VISIBLE_ROWS)
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
                    Some(l) => {
                        let base = channel_color(l.channel);
                        let formatted = format_chat_line(l.channel, &l.sender, &l.text);
                        segment_chat_line(&formatted, base)
                    }
                    None => Vec::new(),
                };
                for (i, span_child) in span_children.iter().enumerate() {
                    let Ok((mut span_text, mut span_color)) = span_q.get_mut(span_child) else {
                        continue;
                    };
                    let (want_text, want_color): (&str, Color) = segments
                        .get(i)
                        .map(|(t, c)| (t.as_str(), *c))
                        .unwrap_or(("", palette::TEXT));
                    if span_text.as_str() != want_text {
                        **span_text = want_text.to_string();
                    }
                    if span_color.0 != want_color {
                        span_color.0 = want_color;
                    }
                }
            }
        }
    }
}

pub fn format_chat_line(channel: ChatChannel, sender: &str, text: &str) -> String {
    match channel {
        ChatChannel::Say | ChatChannel::Shout | ChatChannel::Other => {
            format!("{sender} : {text}")
        }

        ChatChannel::Tell => format!(">>{sender} : {text}"),

        ChatChannel::Party => format!("({sender}) {text}"),

        ChatChannel::Linkshell => format!("<{sender}> {text}"),

        ChatChannel::Yell => format!("[{sender}] : {text}"),

        ChatChannel::System | ChatChannel::Battle => text.to_string(),

        ChatChannel::Debug => format!("[dbg] {text}"),
    }
}

pub fn segment_chat_line(line: &str, base: Color) -> Vec<(String, Color)> {
    let mut out: Vec<(String, Color)> = Vec::new();
    let mut buf = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if !buf.is_empty() {
                out.push((std::mem::take(&mut buf), base));
            }

            let mut at = String::from('{');
            for ic in chars.by_ref() {
                at.push(ic);
                if ic == '}' {
                    break;
                }
            }
            out.push((at, AUTOTRANSLATE_COLOR));
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        out.push((buf, base));
    }
    if out.is_empty() {
        out.push((String::new(), base));
    }
    out
}

pub fn channel_color(c: ChatChannel) -> Color {
    match c {
        ChatChannel::Say => palette::TEXT,
        ChatChannel::Shout => palette::ACCENT,
        ChatChannel::Tell => Color::srgb(0.95, 0.40, 0.95),
        ChatChannel::Party => Color::srgb(0.50, 0.65, 1.00),
        ChatChannel::Linkshell => Color::srgb(0.40, 0.95, 0.50),
        ChatChannel::Yell => Color::srgb(1.00, 0.85, 0.20),
        ChatChannel::System => palette::MUTED,
        ChatChannel::Other => palette::DARK,

        ChatChannel::Battle => Color::srgb(1.00, 0.55, 0.10),

        ChatChannel::Debug => Color::srgb(0.55, 0.75, 0.80),
    }
}

pub fn apply_wheel_delta(
    current: usize,
    accum: f32,
    delta: f32,
    buffer_len: usize,
) -> (usize, f32) {
    if buffer_len == 0 {
        return (current, 0.0);
    }
    let mut frac = accum + delta * WHEEL_ROWS_PER_UNIT;

    let whole = frac.trunc() as i32;
    frac -= whole as f32;
    let max_rows = buffer_len.saturating_sub(1) as i32;
    let next = (current as i32 + whole).clamp(0, max_rows);

    let frac = if (next == 0 && whole < 0) || (next == max_rows && whole > 0) {
        0.0
    } else {
        frac
    };
    (next as usize, frac)
}

pub fn chat_wheel_scroll_system(
    mut wheel: MessageReader<MouseWheel>,
    panel_q: Query<(&ChatPanel, &RelativeCursorPosition)>,
    state: Res<SceneState>,
    mut scroll: ResMut<ChatScroll>,
    mut battle_scroll: ResMut<BattleScroll>,
    mut debug_scroll: ResMut<DebugScroll>,
    mut accum: ResMut<ChatScrollAccum>,
    mut battle_accum: ResMut<BattleScrollAccum>,
    mut debug_accum: ResMut<DebugScrollAccum>,
    mut pointer: ResMut<MousePointer>,
) {
    let mut delta: f32 = 0.0;
    for ev in wheel.read() {
        delta += ev.y;
    }
    if delta == 0.0 {
        return;
    }

    let mut hovered: Option<ChatKind> = None;
    for (panel, rel) in &panel_q {
        if rel.cursor_over() {
            hovered = Some(panel.kind);
            break;
        }
    }
    let Some(kind) = hovered else {
        return;
    };

    let all = rendered_chat(&state);
    let buffer_len = all.iter().filter(|l| kind.accepts(l.channel)).count();
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
) {
    let all = rendered_chat(&state);
    let count = |kind: ChatKind| all.iter().filter(|l| kind.accepts(l.channel)).count();
    let kinds = [
        (ChatKind::Social, count(ChatKind::Social), tracker.social),
        (ChatKind::Battle, count(ChatKind::Battle), tracker.battle),
        (ChatKind::Debug, count(ChatKind::Debug), tracker.debug),
    ];
    let mut to_switch: Option<ChatKind> = None;
    for (kind, now_count, prev_count) in kinds {
        if now_count > prev_count && kind != active.0 {
            if !unread.get(kind) {
                unread.set(kind, true);
            }
            to_switch = Some(kind);
        }
    }
    tracker.social = kinds[0].1;
    tracker.battle = kinds[1].1;
    tracker.debug = kinds[2].1;
    if auto.0 {
        if let Some(kind) = to_switch {
            if active.0 != kind {
                active.0 = kind;
            }
        }
    }

    if unread.get(active.0) {
        unread.set(active.0, false);
    }
}

pub fn update_chat_tab_visuals_system(
    active: Res<ActiveChatTab>,
    unread: Res<ChatUnread>,
    auto: Res<ChatAutoSwitch>,
    mut panel_q: Query<(&ChatPanel, &mut Node), Without<ChatTabButton>>,
    mut tab_q: Query<
        (&ChatTabButton, &mut BorderColor, &Children),
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
        let want = if panel.kind == active.0 {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != want {
            node.display = want;
        }
    }

    let unread_color = Color::srgb(1.00, 0.85, 0.20);
    for (button, mut border, children) in &mut tab_q {
        let is_active = button.kind == active.0;
        let is_unread = !is_active && unread.get(button.kind);
        let (border_c, label_c) = if is_active {
            (palette::ACCENT, palette::ACCENT)
        } else if is_unread {
            (unread_color, unread_color)
        } else {
            (palette::BORDER, palette::MUTED)
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
        ("auto \u{2713}", palette::ACCENT, palette::ACCENT)
    } else {
        ("auto \u{2717}", palette::MUTED, palette::BORDER)
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

    #[test]
    fn segment_plain_text_is_single_base_span() {
        let segs = segment_chat_line("hello world", palette::TEXT);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].0, "hello world");
        assert_eq!(segs[0].1, palette::TEXT);
    }

    #[test]
    fn segment_splits_braces_and_colors_them() {
        let segs = segment_chat_line(
            "[Skaine] : {Looking for Party} {Experience points} : THF 59",
            palette::TEXT,
        );
        let texts: Vec<&str> = segs.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "[Skaine] : ",
                "{Looking for Party}",
                " ",
                "{Experience points}",
                " : THF 59",
            ]
        );

        assert_eq!(segs[1].1, AUTOTRANSLATE_COLOR);
        assert_eq!(segs[3].1, AUTOTRANSLATE_COLOR);
        assert_eq!(segs[0].1, palette::TEXT);
        assert_eq!(segs[2].1, palette::TEXT);
        assert_eq!(segs[4].1, palette::TEXT);
    }

    #[test]
    fn segment_empty_input_yields_single_empty_segment() {
        let segs = segment_chat_line("", palette::TEXT);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].0.is_empty());
    }

    #[test]
    fn segment_unclosed_brace_does_not_lose_tail() {
        let segs = segment_chat_line("foo {open and never close", palette::TEXT);
        let joined: String = segs.iter().map(|(t, _)| t.as_str()).collect();
        assert!(joined.contains("open and never close"));
        assert!(joined.contains('{'));
    }

    #[test]
    fn segment_count_stays_under_pool_for_worst_case_shout() {
        let line = "{a}{b}{c}{d}{e}{f}{g}";
        let segs = segment_chat_line(line, palette::TEXT);
        assert!(
            segs.len() <= SPANS_PER_ROW,
            "{} segments overflows pool of {}",
            segs.len(),
            SPANS_PER_ROW
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
    fn small_delta_accumulates_before_stepping() {
        let (rows, accum) = apply_wheel_delta(0, 0.0, 1.0, 100);
        assert_eq!(rows, 0);
        assert!(accum > 0.0 && accum < 1.0);
    }

    #[test]
    fn accumulator_eventually_spends_a_row() {
        let ticks = (1.0 / WHEEL_ROWS_PER_UNIT).ceil() as usize;
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
