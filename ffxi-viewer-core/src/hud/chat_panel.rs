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
pub const VISIBLE_ROWS: usize = 8;

/// Rows per unit of `MouseWheel.y`. Tuned so a normal trackpad two-finger
/// swipe scrolls at a comfortable rate — values much above ~0.4 feel
/// "skippy" on macOS. Independent of frame rate: the accumulator in
/// [`ChatScrollAccum`] / [`BattleScrollAccum`] sums sub-row fractions
/// across frames and only spends integer rows when |accum| ≥ 1.0.
const WHEEL_ROWS_PER_UNIT: f32 = 0.25;

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

/// Which side of the FFXI-style split a chat panel renders.
///
/// `Social` (Chat 1): Say/Shout/Tell/Party/Linkshell/Yell/Other +
/// local toasts. The panel the operator types into.
///
/// `Battle` (Chat 2): combat log + server System messages — the
/// noisy stream we want isolated from typed conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    Social,
    Battle,
}

impl ChatKind {
    /// Does a given channel render in this panel? See [`ChatKind`] docs
    /// for the split rules.
    pub fn accepts(self, c: ChatChannel) -> bool {
        match self {
            ChatKind::Battle => matches!(c, ChatChannel::Battle | ChatChannel::System),
            ChatKind::Social => !matches!(c, ChatChannel::Battle | ChatChannel::System),
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

pub fn spawn_chat_panel(mut commands: Commands) {
    // Social panel: bottom-left, 50% width.
    spawn_panel(&mut commands, ChatKind::Social, Val::Px(0.0), None);
    // Battle panel: bottom-right, 48% width with a 2% gap.
    spawn_panel(
        &mut commands,
        ChatKind::Battle,
        Val::Percent(52.0),
        Some(Val::Px(0.0)),
    );
}

fn spawn_panel(commands: &mut Commands, kind: ChatKind, left: Val, right: Option<Val>) {
    let width = if right.is_some() {
        Val::Percent(48.0)
    } else {
        Val::Percent(50.0)
    };
    commands
        .spawn((
            ChatPanel { kind },
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
                right: right.unwrap_or(Val::Auto),
                width,
                height: Val::Px(160.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                row_gap: Val::Px(2.0),
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
        ))
        .with_children(|p| {
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
                    ));
                });
            }
        });
}

pub fn update_chat_panel(
    state: Res<SceneState>,
    mode: Res<InputMode>,
    scroll: Res<ChatScroll>,
    battle_scroll: Res<BattleScroll>,
    mut panel_q: Query<(&ChatPanel, &mut BorderColor, &Children)>,
    rows: Query<(&ChatRow, &Children), Without<ChatPanel>>,
    mut body_q: Query<(&mut Text, &mut TextColor), With<ChatRowBody>>,
) {
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
    for (panel, mut border, panel_children) in &mut panel_q {
        let filtered: Vec<&ChatLine> = all
            .iter()
            .copied()
            .filter(|l| panel.kind.accepts(l.channel))
            .collect();

        let scroll_offset = match panel.kind {
            ChatKind::Social => scroll.rows,
            ChatKind::Battle => battle_scroll.rows,
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
            ChatKind::Battle => scroll_offset != 0,
        };
        let want_border = if focused {
            palette::ACCENT
        } else {
            palette::BORDER
        };
        if border.left != want_border {
            *border = BorderColor::all(want_border);
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
                if let Ok((mut text, mut tc)) = body_q.get_mut(body_child) {
                    match line {
                        Some(l) => {
                            let want = format_chat_line(l.channel, &l.sender, &l.text);
                            if **text != want {
                                **text = want;
                            }
                            let want_color = channel_color(l.channel);
                            if tc.0 != want_color {
                                tc.0 = want_color;
                            }
                        }
                        None => {
                            if !text.is_empty() {
                                **text = String::new();
                            }
                        }
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
    }
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
    mut accum: ResMut<ChatScrollAccum>,
    mut battle_accum: ResMut<BattleScrollAccum>,
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
    }
    // Suppress camera zoom on the same wheel event.
    pointer.wheel = 0.0;
}

#[cfg(test)]
mod tests {
    use super::*;

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
