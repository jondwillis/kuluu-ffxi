//! Bottom-left chat panel — retail-FFXI-style formatting.
//!
//! Each line is rendered as a single colored row. The format is
//! channel-specific (matching SE's default client):
//!   - Say / Shout / Other: `Sender : text`
//!   - Tell:                `>>Sender : text`
//!   - Party:               `(Sender) text`
//!   - Linkshell:           `<Sender> text`
//!   - Yell:                `[Sender] : text`
//!   - System / Battle:     `text` (no sender — server already formatted)
//!
//! Color encodes channel: the whole line picks up [`channel_color`].
//! There is no separate `[say]`-style tag prefix any more — color is the
//! channel cue.
//!
//! Strategy: spawn a fixed-size pool of empty rows once. Each frame, fill
//! visible rows with the most recent N chat lines (newest at the bottom).
//! Avoids spawn-despawn churn at 60 Hz.

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::ui::RelativeCursorPosition;
use ffxi_viewer_wire::{ChatChannel, ChatLine};

use crate::hud::palette;
use crate::input_mode::{InputMode, PassiveCursorFocus};
use crate::mouse::MousePointer;
use crate::snapshot::{rendered_chat, SceneState};

/// Number of chat rows visible at once. Matches what fits in the panel
/// height at the default font size.
pub const VISIBLE_ROWS: usize = 12;

/// Auto-shrink geometry — mirrors retail FFXI's chat-pane fade. The
/// panel sits at MAX while there's recent activity (new message,
/// hover, scroll, or PassiveCursor focus); after `FULL_HOLD_SECS` of
/// idle it linearly shrinks over `FADE_SECS` toward `MIN`. `Overflow::clip`
/// keeps the row pool from spilling above the panel rect when it shrinks.
pub const PANEL_MAX_HEIGHT_PX: f32 = 220.0;
pub const PANEL_MIN_HEIGHT_PX: f32 = 60.0;
pub const FULL_HOLD_SECS: f32 = 4.0;
pub const FADE_SECS: f32 = 1.5;

/// Per-panel decay state: when the panel last saw "activity" (any of:
/// a new chat line in its filter, cursor hover, scroll != 0, passive
/// cursor focus). Read each frame by `update_chat_panel` to interpolate
/// panel height between MAX and MIN.
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct ChatPanelDecay {
    /// `Time::elapsed_secs()` value at the most recent activity. Stored
    /// as `f32` so the initial-zero state reads as "infinitely idle" —
    /// the panel starts collapsed and grows when the first message lands.
    pub last_active_secs: f32,
    /// Most recently observed filtered-chat length for this panel. A
    /// growth in this count is the "new message arrived" signal.
    pub prev_filtered_len: usize,
}

/// Rows per unit of `MouseWheel.y`. Tuned so a normal trackpad two-finger
/// swipe scrolls at a comfortable rate — values much above ~0.4 feel
/// "skippy" on macOS. Independent of frame rate: the accumulator in
/// [`ChatScrollAccum`] / [`BattleScrollAccum`] sums sub-row fractions
/// across frames and only spends integer rows when |accum| ≥ 1.0.
const WHEEL_ROWS_PER_UNIT: f32 = 0.12;

/// Chat-panel scroll offset in row units (one [`ChatLine`] per unit).
/// `0` = newest message pinned to the bottom (default tailing behavior);
/// higher values walk older messages into the visible window.
///
/// Lives as a free-standing resource (not inside `PassiveCursorState`)
/// so mouse-wheel can drive it in any [`InputMode`]. Keyboard
/// PassiveCursor handlers and the wheel system both mutate it; the
/// chat panel render reads it unconditionally.
///
/// Drives the *social* panel (Chat 1). Keyboard PassiveCursor scroll keys
/// target this one — that's the panel the user types into. The battle
/// panel (Chat 2) has its own [`BattleScroll`] driven by the mouse wheel
/// only.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ChatScroll {
    pub rows: usize,
}

/// Per-panel scroll offset for the combat/system panel (Chat 2). Wheel
/// over Chat 2 drives this; wheel over Chat 1 drives [`ChatScroll`].
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct BattleScroll {
    pub rows: usize,
}

/// Per-panel scroll offset for the client-internal debug panel (Chat 3).
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct DebugScroll {
    pub rows: usize,
}

/// Fractional-row accumulator for the social panel's wheel scroll. The
/// wheel system adds `delta * WHEEL_ROWS_PER_UNIT` here every frame; when
/// `|frac| >= 1.0` it spends whole rows into [`ChatScroll`]. Frame-rate
/// independent: total scroll distance depends only on total wheel delta,
/// not on how many frames it spans.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct ChatScrollAccum {
    pub frac: f32,
}

/// Fractional-row accumulator for the battle panel. See [`ChatScrollAccum`].
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct BattleScrollAccum {
    pub frac: f32,
}

/// Fractional-row accumulator for the debug panel. See [`ChatScrollAccum`].
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct DebugScrollAccum {
    pub frac: f32,
}

/// Which side of the FFXI-style split a chat panel renders.
///
/// `Social` (Chat 1): Say/Shout/Tell/Party/Linkshell/Yell/Other.
/// The panel the operator types into.
///
/// `Battle` (Chat 2): combat log + server System messages — the
/// noisy stream we want isolated from typed conversation.
///
/// `Debug` (Chat 3): client-internal toasts (auto-load, zone-change
/// drops, slash-command errors). Kept out of Battle so the operator
/// can read combat without our diagnostic chatter mixing in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatKind {
    #[default]
    Social,
    Battle,
    Debug,
}

/// Which chat tab is currently active. Drives `Display` toggles on the
/// stacked `ChatPanel` entities and the tab-bar button styling.
/// Default: `Social` (`Chat 1`), matching retail's "chat-window-1
/// pre-selected on connect" behavior.
#[derive(Resource, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActiveChatTab(pub ChatKind);

/// Marker on the tab-bar root.
#[derive(Component)]
pub struct ChatTabBar;

/// Per-tab button. Click → mutate [`ActiveChatTab`].
#[derive(Component, Debug, Clone, Copy)]
pub struct ChatTabButton {
    pub kind: ChatKind,
}

/// Marker on the text-label child of a [`ChatTabButton`]. Lets the
/// visuals-update system find the label without re-querying the
/// button's `Children`.
#[derive(Component)]
pub struct ChatTabButtonLabel;

impl ChatKind {
    /// Does a given channel render in this panel?
    ///
    /// Three-tab routing:
    /// - `Social` ("Chat"): everything that's neither combat nor system
    ///   noise — Say/Shout/Tell/Party/Linkshell/Yell/Other.
    /// - `Battle`: combat log only. Retail folds server System messages
    ///   in here too, but the operator wants combat isolated from the
    ///   chatter that goes with auto-load notices etc., so System now
    ///   routes to the dedicated `Debug` tab instead.
    /// - `Debug` ("System"): server System messages + client-internal
    ///   toasts (slash-command feedback, auto-load notices, dev
    ///   diagnostics). The "[dbg] " prefix on `ChatChannel::Debug`
    ///   visually distinguishes client toasts from server System
    ///   messages within this tab.
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

/// Marker on the panel root. Carries which side of the split this panel
/// is rendering so the update systems can filter chat lines and pick the
/// correct scroll resource.
#[derive(Component)]
pub struct ChatPanel {
    pub kind: ChatKind,
}

/// Marker on each row container; `slot` is its position 0..VISIBLE_ROWS-1.
#[derive(Component)]
pub struct ChatRow {
    pub slot: usize,
}

/// Marker on the line text within a row.
#[derive(Component)]
pub struct ChatRowBody;

/// Marker on each `TextSpan` child of [`ChatRowBody`]. The pool of these
/// gives us inline-mixed-color rendering (FFXI auto-translate phrases
/// painted in sky-blue against the channel-color background text).
#[derive(Component)]
pub struct ChatRowSpan;

/// Pre-allocated span slots per row. Sized for the worst real-world
/// shout we've observed (Hishiamazon: 7 auto-translate blocks → 15
/// alternating text/AT segments). One extra for safety.
const SPANS_PER_ROW: usize = 16;

/// Classic FFXI sky-blue used by SE's client for auto-translate phrases.
/// Brighter than any of the social-channel colors so AT blocks always
/// pop visually against their containing line, regardless of the
/// channel's base color.
const AUTOTRANSLATE_COLOR: Color = Color::srgb(0.50, 0.78, 1.00);


pub fn spawn_chat_panel(mut commands: Commands) {
    // Tabbed layout: three panel entities stacked at the SAME bottom-
    // left slot (0..50% width), only one visible at a time. The tab
    // bar sitting just above them switches `ActiveChatTab`, which a
    // system reacts to by toggling `Display` on the panels.
    //
    // - Social ("Chat"): say/shout/tell/party/linkshell/yell/other
    // - Battle ("Battle"): combat log only
    // - Debug  ("System"): server System messages + client toasts
    spawn_panel(
        &mut commands,
        ChatKind::Social,
        Val::Percent(0.0),
        Val::Percent(50.0),
        Display::Flex,
    );
    spawn_panel(
        &mut commands,
        ChatKind::Battle,
        Val::Percent(0.0),
        Val::Percent(50.0),
        Display::None,
    );
    spawn_panel(
        &mut commands,
        ChatKind::Debug,
        Val::Percent(0.0),
        Val::Percent(50.0),
        Display::None,
    );
    spawn_chat_tab_bar(&mut commands);
}

/// Spawn the tab bar that sits above the chat panels. Two buttons —
/// "Chat 1" / "Chat 2" — each carrying a [`ChatTabButton`] marker so
/// [`chat_tab_click_system`] knows which tab to switch to.
fn spawn_chat_tab_bar(commands: &mut Commands) {
    commands
        .spawn((
            crate::components::InGameEntity,
            ChatTabBar,
            Node {
                position_type: PositionType::Absolute,
                // `bottom` is rewritten every frame by
                // [`position_bottom_left_stack_system`] to track the
                // chat panel's current (auto-decaying) height. The
                // initial value (matches chat MIN_HEIGHT + gap) is
                // just to avoid a one-frame flash at the wrong
                // location before the stack system first runs.
                bottom: Val::Px(54.0 + PANEL_MIN_HEIGHT_PX + 4.0),
                left: Val::Px(0.0),
                height: Val::Px(20.0),
                padding: UiRect::axes(Val::Px(2.0), Val::Px(0.0)),
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(2.0),
                ..default()
            },
        ))
        .with_children(|p| {
            // Descriptive labels, retail-faithful where possible:
            // "Chat" = Chat 1, "Battle" = Chat 2's combat half,
            // "System" = server System + client Debug toasts.
            spawn_tab_button(p, ChatKind::Social, "Chat", true);
            spawn_tab_button(p, ChatKind::Battle, "Battle", false);
            spawn_tab_button(p, ChatKind::Debug, "System", false);
        });
}

fn spawn_tab_button(
    p: &mut ChildSpawnerCommands,
    kind: ChatKind,
    label: &str,
    is_active: bool,
) {
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
                font_size: 12.0,
                ..default()
            },
            TextColor(fg),
        ));
    });
}

fn spawn_panel(
    commands: &mut Commands,
    kind: ChatKind,
    left: Val,
    width: Val,
    initial_display: Display,
) {
    let mut e = commands.spawn((
        crate::components::InGameEntity,
        ChatPanel { kind },
        ChatPanelDecay::default(),
        // `RelativeCursorPosition::cursor_over` is what
        // `chat_wheel_scroll_system` reads to decide whether to
        // consume the wheel. No `Pickable` needed — Bevy UI
        // updates the field automatically each frame.
        RelativeCursorPosition::default(),
        Node {
            position_type: PositionType::Absolute,
            // Stack: 28px diagnostics bar + 24px chat-input slot + 2px
            // gap = 54. The chat input bar at `bottom: 28` (height 24)
            // slots into the gap below this panel rather than overlaying
            // its bottommost row. When the input is hidden the gap is
            // just empty breathing room above the diagnostics strip.
            bottom: Val::Px(54.0),
            left,
            right: Val::Auto,
            width,
            height: Val::Px(PANEL_MIN_HEIGHT_PX),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
            border: UiRect::all(Val::Px(1.0)),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::FlexEnd,
            row_gap: Val::Px(2.0),
            display: initial_display,
            // Clip anything that overflows the panel rect. Without
            // this, a chat row that wraps to taller than the
            // remaining panel space spills upward over the 3D
            // viewport (visible bug: `/help`'s multi-line output
            // overflowed the panel before this was set).
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(palette::BACKGROUND),
        BorderColor::all(palette::BORDER),
    ));
    e.with_children(|p| {
            for slot in 0..VISIBLE_ROWS {
                p.spawn((
                    ChatRow { slot },
                    Node {
                        flex_direction: FlexDirection::Row,
                        // `width: 100%` + `min_width: 0` are the two
                        // halves of "let me actually shrink to fit my
                        // parent so wrapping has something to wrap
                        // against." Without `width: 100%` the row would
                        // size to its content; without `min_width: 0`
                        // flex would honor an implicit min-content
                        // width and the text could still overflow.
                        width: Val::Percent(100.0),
                        min_width: Val::Px(0.0),
                        ..default()
                    },
                ))
                .with_children(|row| {
                    row.spawn((
                        ChatRowBody,
                        Text::new(""),
                        // `WordOrCharacter`: wrap at word boundaries
                        // when there are spaces, but break mid-token if
                        // a single token (e.g. a long URL or hex
                        // string) is wider than the line. Bevy 0.17's
                        // default is `WordBoundary`, which leaves long
                        // unbroken tokens to overflow off-screen.
                        TextLayout {
                            linebreak: LineBreak::WordOrCharacter,
                            ..default()
                        },
                        // The Text node itself needs a `max_width`
                        // constraint — Bevy UI does NOT propagate the
                        // parent Node's width down to a Text child the
                        // way HTML/CSS does. Without this, the Text
                        // grows past the row's 100% bound and overflows
                        // even with WordOrCharacter set.
                        Node {
                            max_width: Val::Percent(100.0),
                            ..default()
                        },
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(palette::TEXT),
                    ))
                    .with_children(|body| {
                        // Pre-spawn a fixed pool of `TextSpan` children
                        // for inline-colored runs (e.g. auto-translate
                        // phrases). The parent `Text` stays empty; each
                        // span carries one (text, color) segment. Pool
                        // size is overhead-cheap and avoids per-frame
                        // spawn/despawn churn — `update_chat_panel`
                        // refills the spans in place via change-detect.
                        for _ in 0..SPANS_PER_ROW {
                            body.spawn((
                                ChatRowSpan,
                                TextSpan::new(""),
                                TextFont {
                                    font_size: 13.0,
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
    // Intentionally NOT gated on `state.dirty` — the dirty flag is reset
    // by `ingest_system` in `PreUpdate`, but `text_input_system` /
    // `dialog_mode_sync_system` push local toasts mid-`Update` and there
    // is no enforced ordering with this system. Without a strict chain a
    // toast set after this system ran would never paint (race: ingest
    // resets dirty next frame before this gets a second chance). The
    // body's per-row `if **text != want` change-detection guard keeps
    // the per-frame cost trivial when nothing actually changed.
    let all = rendered_chat(&state);

    // Per-panel: filter the full stream by channel, then render the
    // tail with that panel's own scroll offset. Both panels read the
    // same source-of-truth `rendered_chat` so a new line lands in
    // exactly one panel (the one whose `accepts()` is true).
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

        // The accent border is only meaningful for the social panel —
        // that's the one keyboard PassiveCursor focuses on. The battle
        // panel still highlights when *its own* scroll is non-zero so
        // the operator can see they're not pinned to newest.
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

        // Auto-shrink decay. Any of these counts as activity and resets
        // the idle timer to "now": (1) filtered length grew (new chat
        // line landed in this panel), (2) cursor is over the panel, (3)
        // user has scrolled away from newest, (4) (Social only)
        // PassiveCursor is focused on Chat. While active, panel height
        // sits at MAX; after FULL_HOLD_SECS of idle it linearly fades to
        // MIN over FADE_SECS.
        let new_msg = filtered.len() > decay.prev_filtered_len;
        decay.prev_filtered_len = filtered.len();
        let interacted = rel_cursor.cursor_over() || scroll_offset != 0 || focused;
        if new_msg || interacted {
            decay.last_active_secs = now;
        }
        let idle = (now - decay.last_active_secs).max(0.0);
        let t = ((idle - FULL_HOLD_SECS) / FADE_SECS).clamp(0.0, 1.0);
        let target_h =
            PANEL_MAX_HEIGHT_PX + (PANEL_MIN_HEIGHT_PX - PANEL_MAX_HEIGHT_PX) * t;
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
            // The row has one ChatRowBody child; that body has a pool of
            // SPANS_PER_ROW TextSpan children. Walk panel → row → body →
            // spans, filling segments in order.
            for body_child in row_children.iter() {
                let Ok(span_children) = body_q.get(body_child) else {
                    continue;
                };
                // Build the segment list once for this row (or an empty
                // marker for the no-line case), then fill spans in order
                // and clear the tail.
                let segments: Vec<(String, Color)> = match line {
                    Some(l) => {
                        let base = channel_color(l.channel);
                        let formatted =
                            format_chat_line(l.channel, &l.sender, &l.text);
                        segment_chat_line(&formatted, base)
                    }
                    None => Vec::new(),
                };
                for (i, span_child) in span_children.iter().enumerate() {
                    let Ok((mut span_text, mut span_color)) =
                        span_q.get_mut(span_child)
                    else {
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

/// Pure formatter for a single chat row, matching SE's default-client
/// per-channel layout. Pulled out so it can be unit-tested without a
/// Bevy app.
pub fn format_chat_line(channel: ChatChannel, sender: &str, text: &str) -> String {
    match channel {
        // Say / Shout / Other: "Sender : text". Same shape as the
        // canonical FFXI default-client display; channel is conveyed by
        // color, not by tag.
        ChatChannel::Say | ChatChannel::Shout | ChatChannel::Other => {
            format!("{sender} : {text}")
        }
        // Tell: ">>Sender : text". The double-greater-than is FFXI's
        // tell sigil. For *outbound* tells the sender field carries the
        // recipient (the operator's local-echo path puts the recipient
        // there); the layout still reads correctly: ">>Daisy : msg" is
        // "I told Daisy" or "Daisy told me" depending on direction.
        ChatChannel::Tell => format!(">>{sender} : {text}"),
        // Party: "(Sender) text". Parens around name, no colon.
        ChatChannel::Party => format!("({sender}) {text}"),
        // Linkshell: "<Sender> text". Angle brackets, no colon.
        ChatChannel::Linkshell => format!("<{sender}> {text}"),
        // Yell: "[Sender] : text". Square brackets around name.
        ChatChannel::Yell => format!("[{sender}] : {text}"),
        // System and Battle: server already formatted these. Print the
        // text bare — no sender prefix, no decoration.
        ChatChannel::System | ChatChannel::Battle => text.to_string(),
        // Debug: client-internal toasts. Prefix with a faint marker so
        // the operator can tell a debug line from a server System line
        // if they ever bleed across panels (e.g., in a postcard log).
        ChatChannel::Debug => format!("[dbg] {text}"),
    }
}

/// Segment a fully-formatted chat line into colored runs for inline
/// rendering. Splits at every `{...}` block, painting the contents in
/// [`AUTOTRANSLATE_COLOR`] and the surrounding braces in
/// [`AUTOTRANSLATE_BRACE_COLOR`]; everything else picks up `base`. The
/// result is always non-empty (an empty input yields a single empty
/// segment) — callers can rely on indexing into it.
///
/// This is intentionally lossy with respect to escaped braces: a literal
/// `{` in chat text is exceedingly rare on the wire (FFXI's input UI
/// doesn't even let you type one), and the upstream auto-translate
/// decoder is the only producer of brace pairs. If someone genuinely
/// types `{`, it falls into the AT-styled bucket — acceptable.
pub fn segment_chat_line(line: &str, base: Color) -> Vec<(String, Color)> {
    let mut out: Vec<(String, Color)> = Vec::new();
    let mut buf = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            // Flush the base-color run before this AT block.
            if !buf.is_empty() {
                out.push((std::mem::take(&mut buf), base));
            }
            // Collect `{...}` inclusive of braces into one span.
            let mut at = String::from('{');
            let mut closed = false;
            for ic in chars.by_ref() {
                at.push(ic);
                if ic == '}' {
                    closed = true;
                    break;
                }
            }
            if closed {
                out.push((at, AUTOTRANSLATE_COLOR));
            } else {
                // Unterminated — emit defensively as AT so the bug is
                // visible upstream instead of silently lost.
                out.push((at, AUTOTRANSLATE_COLOR));
            }
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

/// Per-channel line color — the whole row picks this up.
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
        // Orange — matches classic FFXI's combat log color so the
        // operator's at-a-glance read picks up battle lines apart from
        // social channels.
        ChatChannel::Battle => Color::srgb(1.00, 0.55, 0.10),
        // Debug toasts: dim teal — clearly client-internal, not server.
        ChatChannel::Debug => Color::srgb(0.55, 0.75, 0.80),
    }
}

/// Apply a wheel delta and return the new (rows, fractional-accum) pair.
/// Pure — no Bevy types — so the math is unit-testable.
///
/// `delta` is summed `MouseWheel.y` for the frame; positive = scroll up
/// (older content), negative = scroll down (newer). The accumulator
/// soaks up sub-row movement so total scroll distance is independent of
/// frame rate: at 30 Hz vs 144 Hz, the same total `delta` produces the
/// same total row movement.
///
/// Clamps the result to `0..buffer_len-1` and resets the accumulator
/// when clamped — otherwise a long swipe past the oldest line would
/// "bank" rows the user has to swipe back through.
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
    // Take integer rows out of the accumulator; trunc handles both
    // signs (positive trunc → floor, negative trunc → ceil toward 0).
    let whole = frac.trunc() as i32;
    frac -= whole as f32;
    let max_rows = buffer_len.saturating_sub(1) as i32;
    let next = (current as i32 + whole).clamp(0, max_rows);
    // Reset the accumulator at the bounds — otherwise the user has to
    // "spend" all the over-scroll before motion resumes the other way.
    let frac = if next == 0 && whole < 0 {
        0.0
    } else if next == max_rows && whole > 0 {
        0.0
    } else {
        frac
    };
    (next as usize, frac)
}

/// Mouse-wheel-over-chat-panel → scroll the chat log. Runs in
/// `PreUpdate` after `collect_mouse_system` so we can zero
/// `MousePointer.wheel` when we consume a notch, suppressing the
/// camera-zoom double-fire that would otherwise happen on the same
/// physical wheel event.
///
/// Hover detection uses [`RelativeCursorPosition::cursor_over`], the
/// Bevy 0.17 idiom for "is the mouse inside this UI node right now?".
/// Bevy populates the field every frame; outside the panel the wheel
/// passes through to camera zoom unchanged.
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
    // Find which panel the cursor is over. If neither, the wheel falls
    // through to camera zoom unchanged.
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
    // Per-panel buffer length so the clamp matches what we actually
    // render in that panel.
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
    // Suppress camera zoom on the same wheel event.
    pointer.wheel = 0.0;
}

/// Responsive bottom-left stack. Reads the visible chat panel's
/// current height (which auto-decays between [`PANEL_MIN_HEIGHT_PX`]
/// and [`PANEL_MAX_HEIGHT_PX`]) and slides the chat tab bar + minimap
/// up/down so they sit *above* the chat without ever overlapping it.
///
/// Stack (anchored bottom-left, growing up):
/// ```text
/// ┌─────────────┐
/// │  minimap    │  ← bottom = chat_top + tab_h + 2 gaps
/// ├─────────────┤
/// │ [1] [2]     │  ← bottom = chat_top + 1 gap
/// ├─────────────┤
/// │   chat      │  bottom: 54  (chat-input strip below)
/// │             │
/// └─────────────┘
/// ```
///
/// Without this system the tab bar (fixed `bottom: 218`) and the
/// minimap (fixed `bottom: 220`) overlapped each other AND the
/// chat panel at its full auto-decay height (`bottom: 274`).
///
/// Runs every frame because chat height interpolates continuously
/// during auto-decay fade-out; the change-detection guard
/// (`if node.bottom != want`) keeps the per-frame cost trivial.
pub fn position_bottom_left_stack_system(
    chat_q: Query<&Node, With<ChatPanel>>,
    mut tab_bar_q: Query<
        &mut Node,
        (
            With<ChatTabBar>,
            Without<ChatPanel>,
            Without<crate::minimap::MinimapRoot>,
        ),
    >,
    mut minimap_q: Query<
        &mut Node,
        (
            With<crate::minimap::MinimapRoot>,
            Without<ChatPanel>,
            Without<ChatTabBar>,
        ),
    >,
) {
    // Use the maximum visible-panel height so tab+minimap don't dip
    // into a hidden panel's space when ActiveChatTab swaps. Each
    // panel's height interpolates between MIN and MAX via auto-decay,
    // so this captures the live size of whatever tab is showing.
    let chat_h = chat_q
        .iter()
        .filter(|n| n.display != Display::None)
        .filter_map(|n| match n.height {
            Val::Px(h) => Some(h),
            _ => None,
        })
        .fold(0.0_f32, f32::max);

    const CHAT_BOTTOM_PX: f32 = 54.0;
    const TAB_BAR_HEIGHT_PX: f32 = 20.0;
    const GAP_PX: f32 = 4.0;

    let chat_top = CHAT_BOTTOM_PX + chat_h;
    let tab_bottom = chat_top + GAP_PX;
    let minimap_bottom = tab_bottom + TAB_BAR_HEIGHT_PX + GAP_PX;

    if let Ok(mut node) = tab_bar_q.single_mut() {
        let want = Val::Px(tab_bottom);
        if node.bottom != want {
            node.bottom = want;
        }
    }
    if let Ok(mut node) = minimap_q.single_mut() {
        let want = Val::Px(minimap_bottom);
        if node.bottom != want {
            node.bottom = want;
        }
    }
}

/// React to clicks on a [`ChatTabButton`] — set [`ActiveChatTab`] to
/// that tab's kind. `Changed<Interaction>` keeps the system cost
/// per-frame O(buttons-that-just-changed), not O(all buttons).
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

/// Apply [`ActiveChatTab`] to the UI: toggle `Display` on the stacked
/// [`ChatPanel`]s and recolor the [`ChatTabButton`] labels + borders
/// so the active tab pops in `palette::ACCENT`.
pub fn update_chat_tab_visuals_system(
    active: Res<ActiveChatTab>,
    mut panel_q: Query<(&ChatPanel, &mut Node), Without<ChatTabButton>>,
    mut tab_q: Query<
        (&ChatTabButton, &mut BorderColor, &Children),
        (Without<ChatPanel>, Without<ChatTabButtonLabel>),
    >,
    mut label_q: Query<&mut TextColor, With<ChatTabButtonLabel>>,
) {
    if !active.is_changed() {
        return;
    }
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
    for (button, mut border, children) in &mut tab_q {
        let is_active = button.kind == active.0;
        let (border_c, label_c) = if is_active {
            (palette::ACCENT, palette::ACCENT)
        } else {
            (palette::BORDER, palette::MUTED)
        };
        if border.left != border_c {
            *border = BorderColor::all(border_c);
        }
        for child in children.iter() {
            if let Ok(mut tc) = label_q.get_mut(child) {
                if tc.0 != label_c {
                    tc.0 = label_c;
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
        // AT spans (1 and 3) pick up the sky-blue; surrounding text stays base.
        assert_eq!(segs[1].1, AUTOTRANSLATE_COLOR);
        assert_eq!(segs[3].1, AUTOTRANSLATE_COLOR);
        assert_eq!(segs[0].1, palette::TEXT);
        assert_eq!(segs[2].1, palette::TEXT);
        assert_eq!(segs[4].1, palette::TEXT);
    }

    #[test]
    fn segment_empty_input_yields_single_empty_segment() {
        // Render path indexes into segments[..]; an empty result would
        // skip clearing trailing spans. Guarantee non-empty.
        let segs = segment_chat_line("", palette::TEXT);
        assert_eq!(segs.len(), 1);
        assert!(segs[0].0.is_empty());
    }

    #[test]
    fn segment_unclosed_brace_does_not_lose_tail() {
        // Pathological input — the autotranslate decoder shouldn't ever
        // emit this, but be defensive: the tail must still surface so a
        // bug upstream is visible to the operator instead of silently
        // eaten.
        let segs = segment_chat_line("foo {open and never close", palette::TEXT);
        let joined: String = segs.iter().map(|(t, _)| t.as_str()).collect();
        assert!(joined.contains("open and never close"));
        assert!(joined.contains('{'));
    }

    #[test]
    fn segment_count_stays_under_pool_for_worst_case_shout() {
        // Hishiamazon's screenshot — 7 AT blocks interleaved with text.
        // Each block contributes 3 spans (open brace, phrase, close
        // brace). Plus the gaps. The pool must comfortably absorb this.
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
        // Color differentiates them, not the layout.
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

    // --- chat-scroll wheel math --------------------------------------

    #[test]
    fn small_delta_accumulates_before_stepping() {
        // A single small delta below the sensitivity threshold should
        // not move the cursor yet — it banks into the accumulator. The
        // very next equivalent delta crosses 1.0 and steps.
        // WHEEL_ROWS_PER_UNIT = 0.25, so a delta of 1.0 banks 0.25
        // rows, well under 1.
        let (rows, accum) = apply_wheel_delta(0, 0.0, 1.0, 100);
        assert_eq!(rows, 0);
        assert!(accum > 0.0 && accum < 1.0);
    }

    #[test]
    fn accumulator_eventually_spends_a_row() {
        // Repeated small deltas accumulate to a whole row.
        // With WHEEL_ROWS_PER_UNIT = 0.25, 4 ticks of delta=1.0 sum
        // to a full row.
        let (rows, accum) = apply_wheel_delta(0, 0.0, 1.0, 100);
        let (rows, accum) = apply_wheel_delta(rows, accum, 1.0, 100);
        let (rows, accum) = apply_wheel_delta(rows, accum, 1.0, 100);
        let (rows, _accum) = apply_wheel_delta(rows, accum, 1.0, 100);
        assert_eq!(rows, 1);
    }

    #[test]
    fn equal_total_delta_produces_equal_rows_regardless_of_frame_count() {
        // The whole point of the accumulator: same total wheel delta,
        // any partition across frames, gives the same row count.
        // High-frame-rate path: 12 frames of delta=1.0 (total 12).
        let mut rows = 0usize;
        let mut accum = 0.0f32;
        for _ in 0..12 {
            let (r, a) = apply_wheel_delta(rows, accum, 1.0, 1000);
            rows = r;
            accum = a;
        }
        let high_fps_rows = rows;
        // Low-frame-rate path: 1 frame of delta=12.0.
        let (low_fps_rows, _) = apply_wheel_delta(0, 0.0, 12.0, 1000);
        assert_eq!(high_fps_rows, low_fps_rows);
    }

    #[test]
    fn wheel_down_at_bottom_stays_at_bottom() {
        // Already at newest (rows = 0); a down-direction delta saturates.
        let (rows, accum) = apply_wheel_delta(0, 0.0, -100.0, 100);
        assert_eq!(rows, 0);
        // Accumulator resets at the clamp boundary so the user doesn't
        // have to "spend" the over-scroll before motion resumes.
        assert_eq!(accum, 0.0);
    }

    #[test]
    fn wheel_up_clamps_at_oldest() {
        // Buffer of 5 lines → max meaningful offset is 4. A huge delta
        // should pin at 4 and zero the accumulator.
        let (rows, accum) = apply_wheel_delta(4, 0.0, 100.0, 5);
        assert_eq!(rows, 4);
        assert_eq!(accum, 0.0);
    }

    #[test]
    fn empty_buffer_is_noop() {
        // No lines yet → wheel does nothing regardless of direction.
        assert_eq!(apply_wheel_delta(0, 0.0, 1.0, 0), (0, 0.0));
        assert_eq!(apply_wheel_delta(0, 0.0, -1.0, 0), (0, 0.0));
    }
}
