//! Chrome widgets shared between the 2D ratatui TUI and the 3D Bevy
//! view. These render into ratatui `Rect`s so either renderer can compose
//! them into its own layout. Single source of truth for stage bar / chat
//! / diagnostics formatting — keeps the two views visually consistent.

use std::collections::VecDeque;

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::state::{
    BlowfishStatus, ChatChannel, LlmDecision, LlmDecisionKind, PartyMember, ReactorGoalSnapshot,
    ReconnectInfo, SessionState, Stage,
};

pub fn draw_stage_bar(f: &mut ratatui::Frame, area: Rect, state: &SessionState) {
    let (label, color) = match state.stage {
        Stage::Idle => ("idle", Color::DarkGray),
        Stage::Authenticating => ("auth", Color::Yellow),
        Stage::LobbyHandshake => ("lobby", Color::Yellow),
        Stage::MapBootstrap => ("map-bootstrap", Color::Yellow),
        Stage::Zoning => ("zoning", Color::Yellow),
        Stage::InZone => ("in-zone", Color::Green),
        Stage::Disconnected => ("disconnected", Color::Red),
    };
    let charname = state.character.as_deref().unwrap_or("(no char)");
    let zone = state
        .zone_id
        .map(|z| format!("zone {z}"))
        .unwrap_or_else(|| "—".into());
    let line = Line::from(vec![
        Span::styled(
            "▌ ffxi-client ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled("● ", Style::default().fg(color)),
        Span::styled(label, Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::raw(format!("  ▪  {charname}  ▪  {zone}")),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(line).block(block), area);
}

pub fn draw_chat(f: &mut ratatui::Frame, area: Rect, state: &SessionState) {
    let max_lines = (area.height as usize).saturating_sub(2);
    let lines: Vec<ListItem> = state
        .chat
        .iter()
        .rev()
        .take(max_lines)
        .rev()
        .map(|line| {
            let (tag, color) = match line.channel {
                ChatChannel::Say => ("[say]", Color::White),
                ChatChannel::Shout => ("[sho]", Color::Cyan),
                ChatChannel::Tell => ("[tll]", Color::Magenta),
                ChatChannel::Party => ("[pty]", Color::Blue),
                ChatChannel::Linkshell => ("[lin]", Color::Green),
                ChatChannel::Yell => ("[yel]", Color::Yellow),
                ChatChannel::System => ("[sys]", Color::Gray),
                ChatChannel::Other => ("[---]", Color::DarkGray),
            };
            let body = format!(" {}: {}", line.sender, line.text);
            ListItem::new(Line::from(vec![
                Span::styled(tag, Style::default().fg(color)),
                Span::raw(body),
            ]))
        })
        .collect();
    let list = List::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled("  chat  ", Style::default().fg(Color::Gray))),
    );
    f.render_widget(list, area);
}

/// JSON event/command log. Each line is prefixed with a direction glyph
/// emitted at the producer:
///   `→ ` agent-bound event (e.g. chat, zone change, low_hp)
///   `← ` outbound command (e.g. Move, Disconnect, Engage)
///   `✦ ` synthetic marker (filter toggle, etc.)
/// The prefix is stripped for color routing and the JSON body rendered
/// raw — operators get round-trippable JSON they can copy out for replay.
///
/// The title surfaces both the line count and the *next* state of the
/// `L` toggle (so "[L: all]" means pressing L will switch to all-events).
/// This is the inversion the ratatui chrome convention uses elsewhere
/// (the diagnostics hint shows the next action, not the current one).
pub fn draw_event_log(
    f: &mut ratatui::Frame,
    area: Rect,
    lines: &VecDeque<String>,
    show_all: bool,
) {
    let max_lines = (area.height as usize).saturating_sub(2);
    let items: Vec<ListItem> = lines
        .iter()
        .rev()
        .take(max_lines)
        .rev()
        .map(|line| {
            let (glyph, body, color) = if let Some(rest) = line.strip_prefix("→ ") {
                ("→", rest, Color::Green)
            } else if let Some(rest) = line.strip_prefix("← ") {
                ("←", rest, Color::Cyan)
            } else if let Some(rest) = line.strip_prefix("✦ ") {
                ("✦", rest, Color::Yellow)
            } else {
                (" ", line.as_str(), Color::DarkGray)
            };
            ListItem::new(Line::from(vec![
                Span::styled(glyph, Style::default().fg(color)),
                Span::raw(" "),
                Span::raw(body.to_string()),
            ]))
        })
        .collect();
    let title = if show_all {
        format!("  json log [{} lines · L: filter]  ", lines.len())
    } else {
        format!("  json log [{} lines · L: all]  ", lines.len())
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(Span::styled(title, Style::default().fg(Color::Gray))),
    );
    f.render_widget(list, area);
}

/// Top-left HUD: what the agent is doing right now.
///
/// Three lines:
///  - `goal: <kind> #<id> d=<dist>y` — pulled from `state.current_goal`
///    (set by the `ReactorGoalChanged` fold). "—" when idle.
///  - State pill: `[FOLLOWING]` / `[ENGAGED]` / `[PATHING]` / `[BANKING]` /
///    `[IDLE]`, color-coded so a glance tells you the reactor mode.
///  - `reconnect: 1.2s ago (520ms down)` from `state.last_reconnect`,
///    computed against current wall-clock. "—" when nothing's been seen.
///
/// Reads from `SessionState` only — no Bevy / view-side state. Pure
/// renderer; trivially testable with a fixture.
pub fn draw_agent_hud(f: &mut ratatui::Frame, area: Rect, state: &SessionState) {
    let goal_line = match &state.current_goal {
        Some(ReactorGoalSnapshot::Idle) | None => Line::from(Span::styled(
            "goal: —",
            Style::default().fg(Color::DarkGray),
        )),
        Some(ReactorGoalSnapshot::Following { target_id, distance }) => Line::from(vec![
            Span::styled("goal: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("follow #{target_id}"),
                Style::default().fg(Color::White),
            ),
            Span::styled(format!(" d={distance:.1}y"), Style::default().fg(Color::Gray)),
        ]),
        Some(ReactorGoalSnapshot::Engaged { target_id, attack_issued }) => {
            let suffix = if *attack_issued { " (atk sent)" } else { " (atk pending)" };
            Line::from(vec![
                Span::styled("goal: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("engage #{target_id}"),
                    Style::default().fg(Color::White),
                ),
                Span::styled(suffix, Style::default().fg(Color::DarkGray)),
            ])
        }
        Some(ReactorGoalSnapshot::Pathing {
            x,
            y,
            z,
            waypoints_remaining,
        }) => {
            let label = if *waypoints_remaining > 1 {
                format!("path → ({x:.0},{y:.0},{z:.0}) [{waypoints_remaining} wp]")
            } else {
                format!("path → ({x:.0},{y:.0},{z:.0})")
            };
            Line::from(vec![
                Span::styled("goal: ", Style::default().fg(Color::DarkGray)),
                Span::styled(label, Style::default().fg(Color::White)),
            ])
        }
        Some(ReactorGoalSnapshot::Banking {
            threshold,
            mog_house_zoneline,
        }) => Line::from(vec![
            Span::styled("goal: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("bank ≥{threshold} → zone {mog_house_zoneline}"),
                Style::default().fg(Color::White),
            ),
        ]),
    };

    let (pill_text, pill_color) = match &state.current_goal {
        None | Some(ReactorGoalSnapshot::Idle) => ("[IDLE]", Color::DarkGray),
        Some(ReactorGoalSnapshot::Following { .. }) => ("[FOLLOWING]", Color::Cyan),
        Some(ReactorGoalSnapshot::Engaged { .. }) => ("[ENGAGED]", Color::Red),
        Some(ReactorGoalSnapshot::Pathing { .. }) => ("[PATHING]", Color::Blue),
        Some(ReactorGoalSnapshot::Banking { .. }) => ("[BANKING]", Color::Yellow),
    };
    let pill_line = Line::from(Span::styled(
        pill_text,
        Style::default().fg(pill_color).add_modifier(Modifier::BOLD),
    ));

    let reconnect_line = match &state.last_reconnect {
        None => Line::from(Span::styled(
            "reconnect: —",
            Style::default().fg(Color::DarkGray),
        )),
        Some(ReconnectInfo {
            downtime_ms,
            at_unix_ms,
        }) => {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(*at_unix_ms);
            let ago_ms = now_ms.saturating_sub(*at_unix_ms);
            Line::from(vec![
                Span::styled("reconnect: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format_age_ms(ago_ms),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!(" ({downtime_ms}ms down)"),
                    Style::default().fg(Color::Gray),
                ),
            ])
        }
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled("  agent  ", Style::default().fg(Color::Gray)));
    f.render_widget(
        Paragraph::new(vec![goal_line, pill_line, reconnect_line]).block(block),
        area,
    );
}

/// Right-column party roster: one row per `PartyMember` with HP/MP/TP
/// bars and 2-char job codes. Color-codes HP% so a low-HP member is
/// visually unmissable.
pub fn draw_party_roster(f: &mut ratatui::Frame, area: Rect, members: &[PartyMember]) {
    let max_lines = (area.height as usize).saturating_sub(2);
    let rows: Vec<ListItem> = members
        .iter()
        .take(max_lines)
        .map(|m| {
            let leader_glyph_color = if m.is_alliance_leader {
                Color::Yellow
            } else if m.is_party_leader {
                Color::Cyan
            } else {
                Color::DarkGray
            };
            let name_display: String = m
                .name
                .as_deref()
                .unwrap_or("(?)")
                .chars()
                .take(8)
                .collect();
            let job_str = format!(
                "{}{}/{}{}",
                job_code(m.main_job),
                m.main_job_lv,
                job_code(m.sub_job),
                m.sub_job_lv
            );
            // Inclusive thresholds: 75% should already be "safe green",
            // 50% is "warning yellow" (the natural reading for the
            // operator), and below 25% is the urgent red. The `>=` form
            // also matches the operator's mental model — "is it at least
            // half full?" answers yes at exactly 50%.
            let hp_color = match m.hp_pct {
                p if p >= 75 => Color::Green,
                p if p >= 50 => Color::Yellow,
                p if p >= 25 => Color::White,
                _ => Color::Red,
            };
            let hp_bar = bar(m.hp_pct, 5);
            let mp_bar = bar(m.mp_pct, 4);
            let tp_color = if m.tp >= 1000 {
                Color::Green
            } else {
                Color::DarkGray
            };

            ListItem::new(Line::from(vec![
                Span::styled("●", Style::default().fg(leader_glyph_color)),
                Span::raw(" "),
                Span::styled(
                    format!("{name_display:<8}"),
                    Style::default().fg(Color::White),
                ),
                Span::raw(" "),
                Span::styled(job_str, Style::default().fg(Color::Gray)),
                Span::raw(" "),
                Span::styled(hp_bar, Style::default().fg(hp_color)),
                Span::styled(format!(" {}%", m.hp_pct), Style::default().fg(hp_color)),
                Span::raw(" "),
                Span::styled(mp_bar, Style::default().fg(Color::Blue)),
                Span::styled(format!(" {}%", m.mp_pct), Style::default().fg(Color::Blue)),
                Span::raw(" tp "),
                Span::styled(format!("{}", m.tp), Style::default().fg(tp_color)),
            ]))
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!("  party [{}]  ", members.len()),
            Style::default().fg(Color::Gray),
        ));
    f.render_widget(List::new(rows).block(block), area);
}

/// Bottom-left LLM-decision badge. Stage-V4 will animate the pulse and
/// surface a latency sparkline; for V0/V1/V2 this is a minimal renderer
/// that shows the most recent decision and a count. Empty state:
/// "agent: idle — no decisions". Avoids unwrap; safe on any input.
pub fn draw_llm_badge(f: &mut ratatui::Frame, area: Rect, decisions: &VecDeque<LlmDecision>) {
    let body_lines = match decisions.back() {
        None => vec![Line::from(Span::styled(
            "agent: idle — no decisions",
            Style::default().fg(Color::DarkGray),
        ))],
        Some(latest) => {
            let kind_summary = match &latest.kind {
                LlmDecisionKind::NotificationFired { uri } => {
                    format!("notify {uri}")
                }
                LlmDecisionKind::ToolDispatched { tool } => {
                    format!("tool {tool}")
                }
            };
            let header = Line::from(vec![
                Span::styled("●", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(" "),
                Span::styled(kind_summary, Style::default().fg(Color::White)),
            ]);
            let footer = Line::from(vec![
                Span::styled(
                    format!("{}μs ", latest.latency_us),
                    Style::default().fg(Color::Gray),
                ),
                Span::styled(
                    format!("· {} recent", decisions.len()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            vec![header, footer]
        }
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled("  llm  ", Style::default().fg(Color::Gray)));
    f.render_widget(Paragraph::new(body_lines).block(block), area);
}

/// Render an N-segment unicode block bar from a 0..=100 percent value.
/// Each segment lights when `pct >= (i+1) * (100/segments)`.
fn bar(pct: u8, segments: u8) -> String {
    let pct = pct.min(100);
    let lit = (pct as u32 * segments as u32 + 50) / 100; // round-half-up
    let mut s = String::with_capacity(segments as usize * 3);
    for i in 0..segments as u32 {
        if i < lit {
            s.push('█');
        } else {
            s.push('░');
        }
    }
    s
}

/// Format a millisecond age as the most-natural unit: "120ms" / "1.2s" /
/// "12s" / "3m" / "2h". Keeps the HUD compact at any session age.
fn format_age_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms ago")
    } else if ms < 10_000 {
        let secs = ms as f64 / 1000.0;
        format!("{secs:.1}s ago")
    } else if ms < 60_000 {
        format!("{}s ago", ms / 1000)
    } else if ms < 3_600_000 {
        format!("{}m ago", ms / 60_000)
    } else {
        format!("{}h ago", ms / 3_600_000)
    }
}

/// 2-character job code from FFXI MJob/SJob enum value. Mirrors
/// `Phoenix/src/map/utils/jobutils.cpp` shorthand. Unknown values render
/// as a numeric placeholder so the column stays aligned.
fn job_code(job: u8) -> &'static str {
    match job {
        0 => "—",
        1 => "WAR",
        2 => "MNK",
        3 => "WHM",
        4 => "BLM",
        5 => "RDM",
        6 => "THF",
        7 => "PLD",
        8 => "DRK",
        9 => "BST",
        10 => "BRD",
        11 => "RNG",
        12 => "SAM",
        13 => "NIN",
        14 => "DRG",
        15 => "SMN",
        16 => "BLU",
        17 => "COR",
        18 => "PUP",
        19 => "DNC",
        20 => "SCH",
        21 => "GEO",
        22 => "RUN",
        _ => "??",
    }
}

pub fn draw_diagnostics(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &SessionState,
    action_hint: Option<&str>,
    kitty_ok: bool,
) {
    let d = &state.diagnostics;
    let bf = match d.blowfish_status {
        Some(BlowfishStatus::Accepted) => "ok".green(),
        Some(s) => format!("{s:?}").yellow(),
        None => "-".dark_gray(),
    };
    let sync = match (d.sync_in, d.sync_out) {
        (Some(i), Some(o)) => format!("{i}/{o}"),
        _ => "-".into(),
    };
    let age = match d.last_server_packet_age_ms {
        Some(ms) if ms < 5_000 => format!("{ms}ms").into(),
        Some(ms) => format!("{ms}ms").red(),
        None => "-".into(),
    };
    let map = d.map_server_addr.clone().unwrap_or_else(|| "-".into());
    let default_hint = if kitty_ok {
        "[wasd] hold-to-move  [tab] target  [L]og filter  [q]uit"
    } else {
        "[wasd] step  [tab] target  [L]og filter  [q]uit  (kitty off — single-step)"
    };
    let hint = action_hint.unwrap_or(default_hint);
    let line = Line::from(vec![
        Span::styled("bf=", Style::default().fg(Color::DarkGray)),
        bf,
        Span::styled("  sync=", Style::default().fg(Color::DarkGray)),
        Span::raw(sync),
        Span::styled("  last=", Style::default().fg(Color::DarkGray)),
        age,
        Span::styled("  map=", Style::default().fg(Color::DarkGray)),
        Span::raw(map),
        Span::raw("    "),
        Span::styled(hint.to_string(), Style::default().fg(Color::Cyan)),
    ]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(
        Paragraph::new(line).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::{LlmDecision, LlmDecisionKind, PartyMember, ReconnectInfo, SessionState};
    use ratatui::{Terminal, backend::TestBackend};

    fn render<F>(width: u16, height: u16, draw: F) -> ratatui::buffer::Buffer
    where
        F: FnOnce(&mut ratatui::Frame, Rect),
    {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw(f, f.area());
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = buf.cell((x, y)).expect("in-bounds cell");
                out.push_str(cell.symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn agent_hud_idle_state_renders_dashes() {
        let state = SessionState::default();
        let buf = render(28, 5, |f, area| draw_agent_hud(f, area, &state));
        let text = buffer_text(&buf);
        assert!(text.contains("agent"), "title visible: {text}");
        assert!(text.contains("goal: —"), "idle goal placeholder: {text}");
        assert!(text.contains("[IDLE]"), "idle pill: {text}");
        assert!(text.contains("reconnect: —"), "no reconnect yet: {text}");
    }

    #[test]
    fn agent_hud_following_renders_target_and_pill() {
        let mut state = SessionState::default();
        state.current_goal = Some(ReactorGoalSnapshot::Following {
            target_id: 4242,
            distance: 3.5,
        });
        let buf = render(40, 5, |f, area| draw_agent_hud(f, area, &state));
        let text = buffer_text(&buf);
        assert!(text.contains("follow #4242"), "target id: {text}");
        assert!(text.contains("d=3.5y"), "distance with one decimal: {text}");
        assert!(text.contains("[FOLLOWING]"), "pill: {text}");
    }

    #[test]
    fn agent_hud_engaged_pill_visible() {
        let mut state = SessionState::default();
        state.current_goal = Some(ReactorGoalSnapshot::Engaged {
            target_id: 99,
            attack_issued: true,
        });
        let buf = render(40, 5, |f, area| draw_agent_hud(f, area, &state));
        let text = buffer_text(&buf);
        assert!(text.contains("engage #99"));
        assert!(text.contains("[ENGAGED]"));
        assert!(text.contains("atk sent"));
    }

    #[test]
    fn agent_hud_reconnect_renders_age() {
        let mut state = SessionState::default();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        state.last_reconnect = Some(ReconnectInfo {
            downtime_ms: 520,
            // Pretend the reconnect was 2 seconds ago.
            at_unix_ms: now_ms.saturating_sub(2000),
        });
        let buf = render(40, 5, |f, area| draw_agent_hud(f, area, &state));
        let text = buffer_text(&buf);
        assert!(text.contains("520ms down"), "downtime: {text}");
        // 2s ago lands in the "1.0s..10.0s" branch with one decimal.
        assert!(
            text.contains("2.0s ago") || text.contains("1.9s ago"),
            "age in seconds with one decimal: {text}"
        );
    }

    fn party_member(id: u32, name: &str, hp_pct: u8, mp_pct: u8, tp: u32) -> PartyMember {
        PartyMember {
            id,
            act_index: 1,
            name: Some(name.into()),
            hp: 1000,
            mp: 500,
            tp,
            hp_pct,
            mp_pct,
            zone_no: 230,
            main_job: 3,    // WHM
            main_job_lv: 75,
            sub_job: 4,     // BLM
            sub_job_lv: 37,
            is_party_leader: false,
            is_alliance_leader: false,
        }
    }

    #[test]
    fn party_roster_shows_names_and_jobs() {
        let members = vec![
            party_member(1, "Vanari", 100, 100, 0),
            party_member(2, "Tamora", 78, 40, 412),
        ];
        let buf = render(60, 5, |f, area| draw_party_roster(f, area, &members));
        let text = buffer_text(&buf);
        assert!(text.contains("Vanari"));
        assert!(text.contains("Tamora"));
        assert!(text.contains("WHM75/BLM37"), "job code shorthand: {text}");
        assert!(text.contains("party [2]"), "title shows count: {text}");
    }

    #[test]
    fn party_roster_color_codes_hp_thresholds() {
        // A 3-member fixture with HP 80% / 50% / 20% should produce
        // distinct foreground colors on the HP-bar spans.
        let members = vec![
            party_member(1, "Hi", 80, 100, 0),
            party_member(2, "Mid", 50, 100, 0),
            party_member(3, "Lo", 20, 100, 0),
        ];
        let buf = render(60, 6, |f, area| draw_party_roster(f, area, &members));
        // Find a cell on each row inside the HP bar (after the leader
        // glyph + space + 8-char name + space + job-string + space).
        // Simplest: scan each row for the first '█' and check its color.
        let mut colors: Vec<Color> = Vec::new();
        for y in 1..=3u16 {
            for x in 0..buf.area.width {
                let cell = buf.cell((x, y)).expect("cell");
                if cell.symbol() == "█" {
                    colors.push(cell.fg);
                    break;
                }
            }
        }
        assert_eq!(colors.len(), 3, "found one HP bar per row");
        assert_eq!(colors[0], Color::Green, "80% should be green");
        assert_eq!(colors[1], Color::Yellow, "50% should be yellow");
        assert_eq!(colors[2], Color::Red, "20% should be red");
    }

    #[test]
    fn llm_badge_idle_state_renders_placeholder() {
        let decisions: VecDeque<LlmDecision> = VecDeque::new();
        let buf = render(28, 4, |f, area| draw_llm_badge(f, area, &decisions));
        let text = buffer_text(&buf);
        assert!(text.contains("idle"), "idle placeholder: {text}");
    }

    #[test]
    fn llm_badge_shows_latest_decision_kind() {
        let mut decisions: VecDeque<LlmDecision> = VecDeque::new();
        decisions.push_back(LlmDecision {
            kind: LlmDecisionKind::ToolDispatched {
                tool: "engage".into(),
            },
            latency_us: 17_300,
            at_monotonic_ms: 0,
        });
        let buf = render(40, 4, |f, area| draw_llm_badge(f, area, &decisions));
        let text = buffer_text(&buf);
        assert!(text.contains("tool engage"), "latest tool name: {text}");
        assert!(text.contains("17300μs"), "latency rendered: {text}");
        assert!(text.contains("1 recent"), "decision count: {text}");
    }

    #[test]
    fn bar_segments_round_correctly() {
        // 5 segments, 100% → all 5 lit; 0% → none; 50% → 2 or 3 (round-half-up = 3).
        assert_eq!(bar(100, 5).matches('█').count(), 5);
        assert_eq!(bar(0, 5).matches('█').count(), 0);
        assert_eq!(bar(50, 5).matches('█').count(), 3);
        // 4 segments, 40% → 1.6 lit, round to 2.
        assert_eq!(bar(40, 4).matches('█').count(), 2);
    }

    #[test]
    fn format_age_ms_picks_sensible_units() {
        assert_eq!(format_age_ms(120), "120ms ago");
        assert_eq!(format_age_ms(1200), "1.2s ago");
        assert_eq!(format_age_ms(15_000), "15s ago");
        assert_eq!(format_age_ms(120_000), "2m ago");
        assert_eq!(format_age_ms(7_200_000), "2h ago");
    }

    #[test]
    fn job_code_lookup_covers_canonical_jobs() {
        assert_eq!(job_code(1), "WAR");
        assert_eq!(job_code(3), "WHM");
        assert_eq!(job_code(0), "—");
        assert_eq!(job_code(99), "??");
    }
}
