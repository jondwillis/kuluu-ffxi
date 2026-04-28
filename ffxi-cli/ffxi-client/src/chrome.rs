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

use crate::state::{BlowfishStatus, ChatChannel, SessionState, Stage};

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
